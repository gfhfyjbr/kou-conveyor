//! The protocol: JSON-RPC 2.0, one message per line, in both eras of MCP.
//!
//! Revisions up to 2025-11-25 open with an `initialize` handshake that fixes
//! the version for the whole connection. Revision 2026-07-28 has none: every
//! request names its version in `_meta`, and `server/discover` tells a client
//! what the server speaks. This server answers both on the same stream, as the
//! specification allows a dual-era server to: a request that names its
//! version is served in that version, and one that does not is served in the
//! version its `initialize` settled.

use crate::Env;
use crate::app::Cancel;
use crate::guidance::Flavour;
use crate::tools;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

/// The revisions served without a handshake, newest first.
pub const MODERN_VERSIONS: &[&str] = &["2026-07-28"];

/// The revisions served after `initialize`, newest first. A client that asks
/// for another one is offered the first.
pub const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// How long a client may keep the tool list and the discovery result. Both
/// are fixed for the life of the process.
const CACHE_TTL_MS: u64 = 60 * 60 * 1000;

/// The clocks of the server, shorter in tests.
#[derive(Clone, Debug)]
pub(crate) struct Timing {
    /// The longest single wait asked of the app. A long wait is a series of
    /// these, so a pane that gets blocked is noticed within one.
    pub slice: Duration,
    /// How often a long wait reports progress, when the client asked for it.
    pub progress_every: Duration,
    /// How long an ordinary request may take to be answered.
    pub call: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            slice: Duration::from_secs(5),
            progress_every: Duration::from_secs(15),
            call: Duration::from_secs(30),
        }
    }
}

/// A protocol revision, and what its messages may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Version(&'static str);

impl Version {
    fn known(asked: &str) -> Option<Version> {
        MODERN_VERSIONS
            .iter()
            .chain(LEGACY_VERSIONS)
            .find(|known| **known == asked)
            .map(|known| Version(known))
    }

    /// Stateless, with `resultType` and per-result `_meta`.
    pub(crate) fn modern(self) -> bool {
        self.0 >= MODERN_VERSIONS[MODERN_VERSIONS.len() - 1]
    }

    /// Tool annotations (`readOnlyHint` and the rest) exist.
    pub(crate) fn annotations(self) -> bool {
        self.0 >= "2025-03-26"
    }

    /// Tool titles, `outputSchema` and `structuredContent` exist.
    pub(crate) fn structured(self) -> bool {
        self.0 >= "2025-06-18"
    }
}

/// An error answered in place of a result.
#[derive(Debug)]
struct Fault {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl Fault {
    fn new(code: i64, message: impl Into<String>) -> Fault {
        Fault {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn unsupported(asked: &str) -> Fault {
        let supported: Vec<&str> = MODERN_VERSIONS
            .iter()
            .chain(LEGACY_VERSIONS)
            .copied()
            .collect();
        Fault {
            code: UNSUPPORTED_PROTOCOL_VERSION,
            message: "Unsupported protocol version".into(),
            data: Some(json!({ "supported": supported, "requested": asked })),
        }
    }
}

/// Where messages go: one writer shared by every call's thread, a whole line
/// at a time.
struct Out<W> {
    writer: Mutex<W>,
    broken: AtomicBool,
}

impl<W: Write> Out<W> {
    fn send(&self, message: &Value) {
        // A `Value` always serialises, and never with a raw newline in it.
        let Ok(mut line) = serde_json::to_vec(message) else {
            return;
        };
        line.push(b'\n');
        let mut writer = lock(&self.writer);
        if writer
            .write_all(&line)
            .and_then(|()| writer.flush())
            .is_err()
        {
            self.broken.store(true, Ordering::SeqCst);
        }
    }

    fn broken(&self) -> bool {
        self.broken.load(Ordering::SeqCst)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Server<'a, W> {
    env: &'a Env,
    timing: &'a Timing,
    out: Out<W>,
    /// The revision `initialize` settled, for requests that name none.
    handshake: Mutex<Option<Version>>,
    /// Tool calls in flight, by request id as JSON text.
    calls: Mutex<HashMap<String, Arc<Cancel>>>,
}

pub(crate) fn serve(
    input: impl Read,
    output: impl Write + Send,
    env: &Env,
    timing: &Timing,
) -> i32 {
    let server = Server {
        env,
        timing,
        out: Out {
            writer: Mutex::new(output),
            broken: AtomicBool::new(false),
        },
        handshake: Mutex::new(None),
        calls: Mutex::new(HashMap::new()),
    };
    let mut input = BufReader::new(input);
    thread::scope(|scope| {
        let mut line = Vec::new();
        let status = loop {
            line.clear();
            match input.read_until(b'\n', &mut line) {
                Ok(0) => break 0,
                Ok(_) => server.line(&line, scope),
                Err(_) => break 1,
            }
            if server.out.broken() {
                break 1;
            }
        };
        // No one is left to answer: stop whatever still waits on the app.
        for cancel in lock(&server.calls).values() {
            cancel.cancel();
        }
        status
    })
}

impl<'env, W: Write + Send> Server<'env, W> {
    fn line<'scope>(&'env self, line: &[u8], scope: &'scope thread::Scope<'scope, 'env>) {
        let line = line.trim_ascii();
        if line.is_empty() {
            return;
        }
        let message = match serde_json::from_slice::<Value>(line) {
            Ok(Value::Object(message)) => message,
            Ok(Value::Array(_)) => {
                return self.fail(
                    None,
                    Fault::new(
                        INVALID_REQUEST,
                        "Invalid request: batches are not supported, send one message per line",
                    ),
                );
            }
            Ok(_) => {
                return self.fail(
                    None,
                    Fault::new(
                        INVALID_REQUEST,
                        "Invalid request: a message is a JSON object",
                    ),
                );
            }
            Err(error) => {
                return self.fail(
                    None,
                    Fault::new(PARSE_ERROR, format!("Parse error: {error}")),
                );
            }
        };
        let id = message
            .get("id")
            .filter(|id| id.is_string() || id.is_number())
            .cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response: this server sends no requests, so there is nothing
            // to match it with.
            if message.contains_key("result") || message.contains_key("error") {
                return;
            }
            return self.fail(
                id,
                Fault::new(INVALID_REQUEST, "Invalid request: no method"),
            );
        };
        let params = match message.get("params") {
            None | Some(Value::Null) => Some(Map::new()),
            Some(Value::Object(params)) => Some(params.clone()),
            Some(_) => None,
        };
        // A notification is never answered, not even when it is malformed.
        if !message.contains_key("id") {
            if let Some(params) = params {
                self.notification(method, &params);
            }
            return;
        }
        let Some(params) = params else {
            return self.fail(
                id,
                Fault::new(INVALID_PARAMS, "Invalid params: params must be an object"),
            );
        };
        let Some(id) = id else {
            return self.fail(
                None,
                Fault::new(
                    INVALID_REQUEST,
                    "Invalid request: the id must be a string or a number",
                ),
            );
        };
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return self.fail(
                Some(id),
                Fault::new(INVALID_REQUEST, "Invalid request: jsonrpc must be \"2.0\""),
            );
        }
        self.request(id, method, params, scope);
    }

    fn notification(&self, method: &str, params: &Map<String, Value>) {
        // `notifications/initialized` and anything else need nothing from us.
        if method == "notifications/cancelled"
            && let Some(id) = params.get("requestId")
            && let Some(cancel) = lock(&self.calls).get(&id.to_string())
        {
            cancel.cancel();
        }
    }

    fn request<'scope>(
        &'env self,
        id: Value,
        method: &str,
        params: Map<String, Value>,
        scope: &'scope thread::Scope<'scope, 'env>,
    ) {
        let answer = match method {
            "initialize" => self.initialize(&params),
            "ping" => self.ping(&params),
            "server/discover" => self.discover(&params),
            "tools/list" => self.list_tools(&params),
            "tools/call" => return self.call_tool(id, params, scope),
            _ => Err(Fault::new(
                METHOD_NOT_FOUND,
                format!("Method not found: {method}"),
            )),
        };
        match answer {
            Ok(result) => self.answer(id, result),
            Err(fault) => self.fail(Some(id), fault),
        }
    }

    /// The revision a request is served in: the one it names, else the one
    /// the handshake settled.
    fn version(&self, params: &Map<String, Value>) -> Result<Version, Fault> {
        let meta = params.get("_meta").and_then(Value::as_object);
        match meta.and_then(|meta| meta.get(PROTOCOL_VERSION)) {
            Some(Value::String(asked)) => {
                let version = Version::known(asked).ok_or_else(|| Fault::unsupported(asked))?;
                let declared = meta
                    .and_then(|meta| meta.get(CLIENT_CAPABILITIES))
                    .is_some_and(Value::is_object);
                if version.modern() && !declared {
                    return Err(Fault::new(
                        INVALID_PARAMS,
                        format!("Invalid params: _meta must carry {CLIENT_CAPABILITIES}"),
                    ));
                }
                Ok(version)
            }
            Some(_) => Err(Fault::new(
                INVALID_PARAMS,
                format!("Invalid params: {PROTOCOL_VERSION} must be a string"),
            )),
            None => (*lock(&self.handshake)).ok_or_else(|| {
                Fault::new(
                    INVALID_PARAMS,
                    format!(
                        "Invalid params: no protocol version. Send initialize first (revisions up to {}), \
                         or name the version in _meta[\"{PROTOCOL_VERSION}\"] ({})",
                        LEGACY_VERSIONS[0], MODERN_VERSIONS[0]
                    ),
                )
            }),
        }
    }

    fn initialize(&self, params: &Map<String, Value>) -> Result<Value, Fault> {
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Fault::new(
                    INVALID_PARAMS,
                    "Invalid params: initialize needs protocolVersion",
                )
            })?;
        let version = LEGACY_VERSIONS
            .iter()
            .find(|known| **known == asked)
            .unwrap_or(&LEGACY_VERSIONS[0]);
        *lock(&self.handshake) = Some(Version(version));
        Ok(json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": server_info(),
            "instructions": Flavour::Tools.instructions(),
        }))
    }

    fn ping(&self, params: &Map<String, Value>) -> Result<Value, Fault> {
        // Gone from the newest revision, kept for every client that still asks.
        let version = self.version(params).ok();
        Ok(finish(Map::new(), version))
    }

    fn discover(&self, params: &Map<String, Value>) -> Result<Value, Fault> {
        // A probe: any client may send it, with or without a version.
        let asked = params
            .get("_meta")
            .and_then(|meta| meta.get(PROTOCOL_VERSION))
            .and_then(Value::as_str);
        if let Some(asked) = asked
            && Version::known(asked).is_none()
        {
            return Err(Fault::unsupported(asked));
        }
        let supported: Vec<&str> = MODERN_VERSIONS
            .iter()
            .chain(LEGACY_VERSIONS)
            .copied()
            .collect();
        let mut result = Map::new();
        result.insert("supportedVersions".into(), json!(supported));
        result.insert("capabilities".into(), json!({ "tools": {} }));
        result.insert("instructions".into(), json!(Flavour::Tools.instructions()));
        cacheable(&mut result);
        Ok(finish(result, Some(Version(MODERN_VERSIONS[0]))))
    }

    fn list_tools(&self, params: &Map<String, Value>) -> Result<Value, Fault> {
        let version = self.version(params)?;
        let tools: Vec<Value> = tools::catalog()
            .iter()
            .map(|tool| tool.describe(version))
            .collect();
        let mut result = Map::new();
        result.insert("tools".into(), Value::Array(tools));
        if version.modern() {
            cacheable(&mut result);
        }
        Ok(finish(result, Some(version)))
    }

    fn call_tool<'scope>(
        &'env self,
        id: Value,
        params: Map<String, Value>,
        scope: &'scope thread::Scope<'scope, 'env>,
    ) {
        let version = match self.version(&params) {
            Ok(version) => version,
            Err(fault) => return self.fail(Some(id), fault),
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return self.fail(
                Some(id),
                Fault::new(
                    INVALID_PARAMS,
                    "Invalid params: tools/call needs the tool's name",
                ),
            );
        };
        let Some(tool) = tools::find(name) else {
            return self.fail(
                Some(id),
                Fault::new(INVALID_PARAMS, format!("Unknown tool: {name}")),
            );
        };
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(arguments)) => arguments.clone(),
            Some(_) => {
                return self.fail(
                    Some(id),
                    Fault::new(
                        INVALID_PARAMS,
                        "Invalid params: arguments must be an object",
                    ),
                );
            }
        };
        let token = params
            .get("_meta")
            .and_then(|meta| meta.get("progressToken"))
            .filter(|token| token.is_string() || token.is_number())
            .cloned();

        let key = id.to_string();
        let cancel = Arc::new(Cancel::default());
        lock(&self.calls).insert(key.clone(), cancel.clone());
        let spawned = thread::Builder::new()
            .name(format!("kou-mcp {name}"))
            .spawn_scoped(scope, {
                let id = id.clone();
                let key = key.clone();
                move || {
                    let report = |progress: f64, total: f64, message: String| {
                        if let Some(token) = &token
                            && !cancel.is_cancelled()
                        {
                            self.out.send(&json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/progress",
                                "params": {
                                    "progressToken": token,
                                    "progress": progress,
                                    "total": total,
                                    "message": message,
                                },
                            }));
                        }
                    };
                    let context = tools::Context {
                        env: self.env,
                        timing: self.timing,
                        cancel: &cancel,
                        progress: &report,
                    };
                    let result = tools::call(tool, &arguments, &context, version);
                    lock(&self.calls).remove(&key);
                    // A cancelled request gets no answer at all.
                    if let Some(result) = result
                        && !cancel.is_cancelled()
                    {
                        self.answer(id, finish(result, Some(version)));
                    }
                }
            });
        if let Err(error) = spawned {
            lock(&self.calls).remove(&key);
            self.fail(
                Some(id),
                Fault::new(INTERNAL_ERROR, format!("could not start the call: {error}")),
            );
        }
    }

    fn answer(&self, id: Value, result: Value) {
        self.out
            .send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn fail(&self, id: Option<Value>, fault: Fault) {
        let mut error = Map::new();
        error.insert("code".into(), json!(fault.code));
        error.insert("message".into(), json!(fault.message));
        if let Some(data) = fault.data {
            error.insert("data".into(), data);
        }
        let mut message = Map::new();
        message.insert("jsonrpc".into(), json!("2.0"));
        // MCP ids are never null: an id that could not be read is left out.
        if let Some(id) = id {
            message.insert("id".into(), id);
        }
        message.insert("error".into(), Value::Object(error));
        self.out.send(&Value::Object(message));
    }
}

/// A result as `version` shapes it: the newest revision marks every result
/// complete and names the server in it.
fn finish(result: impl Into<Value>, version: Option<Version>) -> Value {
    let mut result = match result.into() {
        Value::Object(result) => result,
        other => return other,
    };
    if version.is_some_and(Version::modern) {
        result.insert("resultType".into(), json!("complete"));
        let meta = result
            .entry("_meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(meta) = meta {
            meta.insert(SERVER_INFO.into(), server_info());
        }
    }
    Value::Object(result)
}

fn cacheable(result: &mut Map<String, Value>) {
    result.insert("ttlMs".into(), json!(CACHE_TTL_MS));
    // The same for every caller: nothing in it depends on who asks.
    result.insert("cacheScope".into(), json!("public"));
}

fn server_info() -> Value {
    json!({
        "name": "kou-desktop",
        "title": "kou desktop",
        "version": env!("CARGO_PKG_VERSION"),
    })
}
