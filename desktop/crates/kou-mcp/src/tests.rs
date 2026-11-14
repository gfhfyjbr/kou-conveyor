//! The server against a fake app on a temporary socket, driven by a client on
//! the other end of a socket pair.

#![cfg(unix)]

use crate::guidance::{self, Flavour, Piece, Pointer};
use crate::rpc::{self, Timing};
use crate::tools::{self, MAX_WAIT_SECONDS};
use crate::{Env, PLUGIN_NAME, SKILL_NAME};
use kou_ipc::{
    Activity, Harness, Incoming, Layout, Pane, PaneKind, Placement, Request, Response, Session,
    Space, Spawn, Tab, Until,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// A directory for a socket: under /tmp, because a socket's path must stay
/// under about a hundred bytes.
fn socket_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("kou-mcp-")
        .tempdir_in("/tmp")
        .unwrap()
}

/// How the fake app answers one request.
enum Answer {
    Now(Response),
    Later(Duration, Response),
    /// Never: the reply is kept until the app goes away.
    Hold,
}

/// The app's socket with a scripted answer for every request, and the
/// requests it was sent.
struct FakeApp {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    seen: Arc<Mutex<Vec<Request>>>,
    server: Option<kou_ipc::Server>,
    thread: Option<JoinHandle<()>>,
}

impl FakeApp {
    fn start(answer: impl Fn(&Request) -> Answer + Send + 'static) -> FakeApp {
        let dir = socket_dir();
        let socket = dir.path().join("kou.sock");
        let (server, incoming) = kou_ipc::Server::bind(&socket).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let thread = thread::spawn({
            let seen = seen.clone();
            move || {
                let mut held = Vec::new();
                while let Ok(Incoming { request, reply }) = incoming.recv_blocking() {
                    seen.lock().unwrap().push(request.clone());
                    match answer(&request) {
                        Answer::Now(response) => reply.send(response),
                        Answer::Later(delay, response) => {
                            thread::spawn(move || {
                                thread::sleep(delay);
                                reply.send(response);
                            });
                        }
                        Answer::Hold => held.push(reply),
                    }
                }
            }
        });
        FakeApp {
            _dir: dir,
            socket,
            seen,
            server: Some(server),
            thread: Some(thread),
        }
    }

    fn seen(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }

    fn env(&self) -> Env {
        env_at(self.socket.clone())
    }
}

impl Drop for FakeApp {
    fn drop(&mut self) {
        drop(self.server.take());
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

/// The environment of pane p1 in session s1, working in /work.
fn env_at(socket: PathBuf) -> Env {
    Env {
        socket,
        pane: Some("p1".into()),
        session: Some("s1".into()),
        cwd: "/work".into(),
    }
}

fn quick() -> Timing {
    Timing {
        slice: Duration::from_millis(100),
        progress_every: Duration::from_millis(50),
        call: Duration::from_secs(5),
    }
}

/// An MCP client on one end of a socket pair, the server on the other.
struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    pending: VecDeque<Value>,
    next: u64,
    server: Option<JoinHandle<i32>>,
}

impl Client {
    fn start(env: Env) -> Client {
        Client::with_timing(env, quick())
    }

    fn with_timing(env: Env, timing: Timing) -> Client {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let input = theirs.try_clone().unwrap();
        let server = thread::spawn(move || rpc::serve(input, theirs, &env, &timing));
        ours.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        Client {
            reader: BufReader::new(ours.try_clone().unwrap()),
            stream: ours,
            pending: VecDeque::new(),
            next: 1,
            server: Some(server),
        }
    }

    /// A client that has done the handshake of `version`.
    fn initialized(env: Env, version: &str) -> Client {
        let mut client = Client::start(env);
        client.request(
            "initialize",
            json!({
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            }),
        );
        client.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        client
    }

    fn send_raw(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).unwrap();
    }

    fn send(&mut self, message: Value) {
        let mut line = serde_json::to_vec(&message).unwrap();
        line.push(b'\n');
        self.send_raw(&line);
    }

    fn read(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                Some(serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line}")))
            }
        }
    }

    /// The next message, waiting at most `within`.
    fn next_within(&mut self, within: Duration) -> Option<Value> {
        if let Some(message) = self.pending.pop_front() {
            return Some(message);
        }
        self.stream.set_read_timeout(Some(within)).unwrap();
        let message = self.read();
        self.stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        message
    }

    fn recv(&mut self) -> Value {
        self.next_within(Duration::from_secs(10))
            .expect("the server sent nothing")
    }

    fn send_request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    /// The answer to request `id`; other messages are kept for later.
    fn answer(&mut self, id: u64) -> Value {
        if let Some(at) = self
            .pending
            .iter()
            .position(|message| message["id"] == json!(id))
        {
            return self.pending.remove(at).unwrap();
        }
        loop {
            let message = self
                .read()
                .unwrap_or_else(|| panic!("no answer to request {id}"));
            if message["id"] == json!(id) {
                return message;
            }
            self.pending.push_back(message);
        }
    }

    /// Send a request and return its `result`, which must be there.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, params);
        let answer = self.answer(id);
        assert!(answer.get("error").is_none(), "{method}: {answer}");
        answer["result"].clone()
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        )
    }

    /// End the input and return the server's exit status.
    fn finish(mut self) -> i32 {
        self.stream.shutdown(std::net::Shutdown::Write).unwrap();
        self.server.take().unwrap().join().unwrap()
    }
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn pane(id: &str, activity: Activity) -> Pane {
    Pane {
        id: id.into(),
        session: "s1".into(),
        harness: "codex".into(),
        title: "Review auth".into(),
        cwd: "/work".into(),
        kind: PaneKind::Terminal,
        activity,
        parent: (id != "p1").then(|| "p1".into()),
        since_ms: 20_000,
    }
}

fn tab(id: &str, panes: &[&str], front: bool) -> Tab {
    Tab {
        id: id.into(),
        session: "s1".into(),
        name: format!("tab {id}"),
        named: false,
        icon: None,
        layout: Layout::Panes,
        panes: panes.iter().map(|pane| (*pane).into()).collect(),
        active: panes[0].into(),
        front,
    }
}

fn session(id: &str, selected: bool, archived: bool) -> Session {
    Session {
        id: id.into(),
        space: "sp1".into(),
        name: format!("session {id}"),
        cwd: "/work".into(),
        archived,
        tabs: 2,
        selected,
    }
}

/// An app with a little of everything: two panes and a chat in session s1,
/// an archived s2, and pane p9 that does not exist.
fn app_answer(request: &Request) -> Answer {
    let missing = |pane: &str| Response::Error {
        message: format!("no pane {pane}"),
    };
    Answer::Now(match request {
        Request::Harnesses => Response::Harnesses {
            harnesses: vec![
                Harness {
                    id: "claude".into(),
                    name: "Claude Code".into(),
                    version: Some("2.1.322".into()),
                    ready: true,
                    message: None,
                    native: true,
                },
                Harness {
                    id: "gemini".into(),
                    name: "Gemini CLI".into(),
                    version: None,
                    ready: false,
                    message: Some("not installed".into()),
                    native: false,
                },
            ],
        },
        Request::PaneSpawn(spawn) => Response::Pane(Pane {
            id: "p7".into(),
            harness: spawn.harness.clone(),
            title: spawn.title.clone().unwrap_or_default(),
            cwd: spawn.cwd.clone().unwrap_or_else(|| "/work".into()),
            parent: spawn.from.clone(),
            ..pane("p7", Activity::Working)
        }),
        Request::PaneInfo { pane: id } if id == "p9" => missing(id),
        Request::PaneInfo { pane: id } if id == "p5" => Response::Pane(Pane {
            kind: PaneKind::Thread,
            harness: "claude".into(),
            ..pane(id, Activity::Idle)
        }),
        Request::PaneInfo { pane: id } => Response::Pane(pane(id, Activity::Idle)),
        Request::PaneRead { pane: id, .. } if id == "p5" => Response::Text {
            text: "The fix is in src/auth.rs.\n".into(),
        },
        Request::PaneRead { .. } => Response::Text {
            text: "\n\n  > done: 3 files changed   \n  tests pass\n\n\n".into(),
        },
        Request::PaneWait { panes, .. } => Response::Waited {
            panes: panes.iter().map(|id| pane(id, Activity::Idle)).collect(),
            timed_out: false,
        },
        Request::PaneList { .. } => Response::Panes {
            panes: vec![
                pane("p1", Activity::Working),
                pane("p2", Activity::Idle),
                Pane {
                    kind: PaneKind::Thread,
                    cwd: "/work/docs".into(),
                    parent: None,
                    ..pane("p5", Activity::Blocked)
                },
            ],
        },
        Request::TabList { .. } => Response::Tabs {
            tabs: vec![tab("t1", &["p1", "p2"], true), tab("t2", &["p5"], false)],
        },
        Request::SessionList { .. } => Response::Sessions {
            sessions: vec![session("s1", true, false), session("s2", false, true)],
        },
        Request::SpaceList => Response::Spaces {
            spaces: vec![Space {
                id: "sp1".into(),
                name: "kou".into(),
                path: "/work".into(),
                sessions: 1,
            }],
        },
        Request::PaneSend { pane: id, .. }
        | Request::PaneClose { pane: id }
        | Request::PaneFocus { pane: id }
            if id == "p9" =>
        {
            missing(id)
        }
        Request::PaneSend { .. } | Request::PaneClose { .. } | Request::PaneFocus { .. } => {
            Response::Done
        }
        Request::TabSet { name, icon, .. } => Response::Tab(Tab {
            name: name.clone().unwrap_or_else(|| "tab t1".into()),
            named: true,
            icon: icon.clone(),
            ..tab("t1", &["p1", "p2"], true)
        }),
        Request::SessionRename { name, .. } => Response::Session(Session {
            name: name.clone(),
            ..session("s1", true, false)
        }),
        Request::SpaceRename { name, .. } => Response::Space(Space {
            id: "sp1".into(),
            name: name.clone(),
            path: "/work".into(),
            sessions: 1,
        }),
        _ => Response::Error {
            message: "not scripted".into(),
        },
    })
}

// A small JSON Schema validator: the keywords this crate's schemas use.

const KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "default",
    "description",
    "minimum",
    "maximum",
    "minItems",
];

fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_u64() || value.is_i64(),
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// Whether `schema` is a valid schema in the subset, with the reason if not.
fn check_schema(schema: &Value, at: &str) -> Result<(), String> {
    let object = schema.as_object().ok_or(format!("{at}: not an object"))?;
    for key in object.keys() {
        if !KEYWORDS.contains(&key.as_str()) {
            return Err(format!("{at}: unexpected keyword {key}"));
        }
    }
    if let Some(kind) = object.get("type") {
        let kind = kind.as_str().ok_or(format!("{at}: type is not a string"))?;
        if ![
            "object", "array", "string", "boolean", "integer", "number", "null",
        ]
        .contains(&kind)
        {
            return Err(format!("{at}: unknown type {kind}"));
        }
    }
    let properties = object.get("properties").and_then(Value::as_object);
    if let Some(properties) = properties {
        for (name, property) in properties {
            check_schema(property, &format!("{at}.{name}"))?;
        }
    }
    if let Some(required) = object.get("required") {
        for name in required
            .as_array()
            .ok_or(format!("{at}: required is not a list"))?
        {
            let name = name
                .as_str()
                .ok_or(format!("{at}: a required name is not a string"))?;
            if !properties.is_some_and(|properties| properties.contains_key(name)) {
                return Err(format!("{at}: {name} is required but not a property"));
            }
        }
    }
    if let Some(items) = object.get("items") {
        check_schema(items, &format!("{at}[]"))?;
    }
    if let Some(choices) = object.get("enum") {
        let choices = choices.as_array().filter(|choices| !choices.is_empty());
        for choice in choices.ok_or(format!("{at}: enum is empty"))? {
            let mut plain = schema.clone();
            plain.as_object_mut().unwrap().remove("enum");
            validate(&plain, choice, at)?;
        }
    }
    if let Some(default) = object.get("default") {
        let mut plain = schema.clone();
        plain.as_object_mut().unwrap().remove("default");
        validate(&plain, default, &format!("{at} (default)"))?;
    }
    Ok(())
}

/// Whether `value` conforms to `schema`, with the reason if not.
fn validate(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    if let Some(kind) = schema.get("type").and_then(Value::as_str)
        && !type_matches(kind, value)
    {
        return Err(format!("{at}: {value} is not of type {kind}"));
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array)
        && !choices.contains(value)
    {
        return Err(format!("{at}: {value} is not one of {choices:?}"));
    }
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && value.as_f64().is_some_and(|number| number < minimum)
    {
        return Err(format!("{at}: {value} is below {minimum}"));
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        for name in schema["required"].as_array().into_iter().flatten() {
            if !object.contains_key(name.as_str().unwrap()) {
                return Err(format!("{at}: missing {name}"));
            }
        }
        for (name, field) in object {
            match properties.and_then(|properties| properties.get(name)) {
                Some(property) => validate(property, field, &format!("{at}.{name}"))?,
                None if schema.get("additionalProperties") == Some(&json!(false)) => {
                    return Err(format!("{at}: unexpected {name}"));
                }
                None => {}
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (index, item) in array.iter().enumerate() {
            validate(items, item, &format!("{at}[{index}]"))?;
        }
    }
    Ok(())
}

/// The tool's structured result conforms to its output schema, and its text
/// is there.
fn conforms(tool: &str, result: &Value) {
    assert!(!text(result).is_empty(), "{tool}: no text in {result}");
    let schema = &tools::find(tool).unwrap().output;
    let structured = result
        .get("structuredContent")
        .unwrap_or_else(|| panic!("{tool}: no structured content in {result}"));
    if let Err(error) = validate(schema, structured, tool) {
        panic!("{tool}: {error}\n{structured:#}");
    }
}

// The handshake and the protocol.

#[test]
fn the_handshake_answers_the_clients_version_or_the_latest() {
    for (asked, answered) in [
        ("2025-06-18", "2025-06-18"),
        ("2024-11-05", "2024-11-05"),
        ("2025-11-25", "2025-11-25"),
        ("1999-01-01", "2025-11-25"),
        ("2026-07-28", "2025-11-25"),
    ] {
        let mut client = Client::start(env_at("/nowhere".into()));
        let result = client.request(
            "initialize",
            json!({ "protocolVersion": asked, "capabilities": {} }),
        );
        assert_eq!(result["protocolVersion"], answered, "{asked}");
        assert_eq!(result["capabilities"], json!({ "tools": {} }));
        assert_eq!(result["serverInfo"]["name"], "kou-desktop");
        assert!(result["serverInfo"]["version"].is_string());
        assert_eq!(result["instructions"], Flavour::Tools.instructions());
        assert!(
            result.get("resultType").is_none(),
            "a legacy result has no resultType"
        );

        client.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        assert_eq!(client.request("ping", json!({})), json!({}));
        assert_eq!(client.finish(), 0);
    }
}

#[test]
fn a_modern_client_discovers_and_calls_without_a_handshake() {
    let app = FakeApp::start(app_answer);
    let mut client = Client::start(app.env());
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
    });

    let discovered = client.request("server/discover", json!({ "_meta": meta }));
    assert_eq!(discovered["resultType"], "complete");
    let versions = discovered["supportedVersions"].as_array().unwrap();
    assert_eq!(versions[0], "2026-07-28");
    assert!(versions.contains(&json!("2025-11-25")));
    assert_eq!(discovered["capabilities"], json!({ "tools": {} }));
    assert_eq!(discovered["instructions"], Flavour::Tools.instructions());
    assert_eq!(
        discovered["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "kou-desktop"
    );
    assert!(discovered["ttlMs"].as_u64().is_some());
    assert_eq!(discovered["cacheScope"], "public");

    let listed = client.request("tools/list", json!({ "_meta": meta }));
    assert_eq!(listed["resultType"], "complete");
    assert!(listed["ttlMs"].as_u64().is_some());
    assert_eq!(
        listed["tools"].as_array().unwrap().len(),
        tools::catalog().len()
    );

    let called = client.request(
        "tools/call",
        json!({ "_meta": meta, "name": "describe_pane", "arguments": {} }),
    );
    assert_eq!(called["resultType"], "complete");
    assert_eq!(
        called["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "kou-desktop"
    );
    conforms("describe_pane", &called);

    // A version the server does not know names the ones it does.
    let id = client.send_request(
        "tools/list",
        json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "2099-01-01", "io.modelcontextprotocol/clientCapabilities": {} } }),
    );
    let refused = client.answer(id);
    assert_eq!(refused["error"]["code"], -32022);
    assert_eq!(refused["error"]["data"]["requested"], "2099-01-01");
    assert!(
        refused["error"]["data"]["supported"]
            .as_array()
            .unwrap()
            .contains(&json!("2026-07-28"))
    );

    // The capabilities are required in the newest revision.
    let id = client.send_request(
        "tools/list",
        json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" } }),
    );
    assert_eq!(client.answer(id)["error"]["code"], -32602);

    // Without a handshake, a request must name its version.
    let id = client.send_request("tools/list", json!({}));
    let refused = client.answer(id);
    assert_eq!(refused["error"]["code"], -32602);
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("initialize")
    );
    assert_eq!(client.finish(), 0);
}

#[test]
fn every_tool_has_a_description_and_a_valid_schema() {
    let mut client = Client::initialized(env_at("/nowhere".into()), "2025-11-25");
    let listed = client.request("tools/list", json!({}));
    let listed = listed["tools"].as_array().unwrap();
    assert_eq!(listed.len(), tools::catalog().len());
    let mut names = Vec::new();
    for tool in listed {
        let name = tool["name"].as_str().unwrap();
        assert!(
            !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{name}"
        );
        assert!(!names.contains(&name), "{name} twice");
        names.push(name);
        let description = tool["description"].as_str().unwrap();
        // Long enough to say what it is for and what comes back; short
        // enough that no client cuts it.
        assert!(
            (150..2000).contains(&description.len()),
            "{name}: {} characters",
            description.len()
        );
        assert!(
            tool["title"]
                .as_str()
                .is_some_and(|title| !title.is_empty()),
            "{name}"
        );
        let input = &tool["inputSchema"];
        check_schema(input, name).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(input["type"], "object", "{name}");
        assert_eq!(input["additionalProperties"], false, "{name}");
        for (argument, property) in input["properties"].as_object().unwrap() {
            assert!(
                property["description"].is_string(),
                "{name}.{argument} has no description"
            );
            assert!(
                property["type"].is_string(),
                "{name}.{argument} has no type"
            );
        }
        let output = &tool["outputSchema"];
        check_schema(output, &format!("{name} output")).unwrap_or_else(|error| panic!("{error}"));
        // Revisions before 2026-07-28 accept only an object here.
        assert_eq!(output["type"], "object", "{name}");
        let annotations = &tool["annotations"];
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(annotations[hint].is_boolean(), "{name}: {hint}");
        }
    }
    let read_only: Vec<&str> = listed
        .iter()
        .filter(|tool| tool["annotations"]["readOnlyHint"] == true)
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        read_only,
        [
            "list_harnesses",
            "wait_for_panes",
            "read_pane",
            "list_panes",
            "describe_pane"
        ]
    );
    let destructive: Vec<&str> = listed
        .iter()
        .filter(|tool| tool["annotations"]["destructiveHint"] == true)
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(destructive, ["close_pane"]);
    assert_eq!(client.finish(), 0);
}

#[test]
fn older_revisions_get_only_the_fields_they_know() {
    let app = FakeApp::start(app_answer);
    for (version, annotations, structured) in [
        ("2024-11-05", false, false),
        ("2025-03-26", true, false),
        ("2025-06-18", true, true),
    ] {
        let mut client = Client::initialized(app.env(), version);
        let listed = client.request("tools/list", json!({}));
        let tool = &listed["tools"][0];
        assert_eq!(tool.get("annotations").is_some(), annotations, "{version}");
        assert_eq!(tool.get("outputSchema").is_some(), structured, "{version}");
        assert_eq!(tool.get("title").is_some(), structured, "{version}");
        let result = client.call("list_harnesses", json!({}));
        assert_eq!(
            result.get("structuredContent").is_some(),
            structured,
            "{version}"
        );
        assert!(
            text(&result).starts_with("claude: Claude Code 2.1.322, ready"),
            "{version}"
        );
        assert_eq!(client.finish(), 0);
    }
}

#[test]
fn malformed_input_is_answered_and_the_server_carries_on() {
    let mut client = Client::initialized(env_at("/nowhere".into()), "2025-11-25");

    client.send_raw(b"this is not json\n");
    let answer = client.recv();
    assert_eq!(answer["error"]["code"], -32700);
    assert!(answer.get("id").is_none(), "{answer}");

    client.send_raw(b"{\"jsonrpc\":\"2.0\",\"id\":2,\xff\xfe}\n");
    assert_eq!(client.recv()["error"]["code"], -32700);

    client.send_raw(b"[{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}]\n");
    assert_eq!(client.recv()["error"]["code"], -32600);

    client.send_raw(b"42\n");
    assert_eq!(client.recv()["error"]["code"], -32600);

    client.send(json!({ "jsonrpc": "2.0", "id": 5 }));
    let answer = client.recv();
    assert_eq!(
        (answer["error"]["code"].clone(), answer["id"].clone()),
        (json!(-32600), json!(5))
    );

    client.send(json!({ "jsonrpc": "2.0", "id": null, "method": "ping" }));
    assert_eq!(client.recv()["error"]["code"], -32600);

    client.send(json!({ "jsonrpc": "1.0", "id": 6, "method": "ping" }));
    assert_eq!(client.recv()["error"]["code"], -32600);

    client.send(json!({ "jsonrpc": "2.0", "id": 7, "method": "ping", "params": [1] }));
    assert_eq!(client.recv()["error"]["code"], -32602);

    client.send(json!({ "jsonrpc": "2.0", "id": "eight", "method": "resources/list" }));
    let answer = client.recv();
    assert_eq!(
        (answer["error"]["code"].clone(), answer["id"].clone()),
        (json!(-32601), json!("eight"))
    );

    // Blank lines, notifications nobody handles and stray responses get no answer.
    client.send_raw(b"\n\r\n   \n");
    client.send(json!({ "jsonrpc": "2.0", "method": "notifications/roots/list_changed" }));
    client.send(json!({ "jsonrpc": "2.0", "id": 99, "result": {} }));
    client.send(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 12345 } }));
    client.send(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": "now" }));

    let id = client.send_request(
        "tools/call",
        json!({ "name": "no_such_tool", "arguments": {} }),
    );
    let answer = client.answer(id);
    assert_eq!(answer["error"]["code"], -32602);
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no_such_tool")
    );

    let id = client.send_request(
        "tools/call",
        json!({ "name": "list_panes", "arguments": "all" }),
    );
    assert_eq!(client.answer(id)["error"]["code"], -32602);

    let id = client.send_request("ping", json!({}));
    assert_eq!(client.answer(id)["result"], json!({}));
    assert!(client.pending.is_empty(), "{:?}", client.pending);
    assert_eq!(client.finish(), 0);
}

// Each tool on the wire.

#[test]
fn each_tool_sends_its_request_and_reads_the_answer() {
    let app = FakeApp::start(app_answer);
    let mut client = Client::initialized(app.env(), "2025-11-25");

    let result = client.call("list_harnesses", json!({}));
    conforms("list_harnesses", &result);
    assert_eq!(
        text(&result),
        "claude: Claude Code 2.1.322, ready, can run as a native chat\n\
         gemini: Gemini CLI, not available: not installed"
    );

    let result = client.call(
        "start_agent",
        json!({
            "harness": "codex",
            "prompt": "Review the diff.",
            "title": "Review auth",
            "cwd": "sub",
            "model": "gpt-5.5",
            "placement": "split",
            "native": true,
        }),
    );
    conforms("start_agent", &result);
    let said = text(&result);
    assert!(
        said.starts_with("Started pane p7: codex, in a terminal, \"Review auth\", in /work/sub."),
        "{said}"
    );
    assert!(said.contains("cannot run as a native chat"), "{said}");
    assert!(
        said.contains("wait_for_panes with panes [\"p7\"]"),
        "{said}"
    );
    assert_eq!(result["structuredContent"]["pane"]["started_by_you"], true);

    let result = client.call("start_agent", json!({ "harness": "shell" }));
    assert!(
        text(&result).contains("It has no task yet"),
        "{}",
        text(&result)
    );

    let result = client.call(
        "send_to_pane",
        json!({ "pane": "p7", "text": "Also check the tests." }),
    );
    conforms("send_to_pane", &result);
    let result = client.call(
        "send_to_pane",
        json!({ "pane": "p7", "text": "draft", "submit": false }),
    );
    assert_eq!(result["structuredContent"]["submitted"], false);

    let result = client.call("read_pane", json!({ "pane": "p2" }));
    conforms("read_pane", &result);
    assert_eq!(
        text(&result),
        "p2 (codex, terminal) is idle for 20s. Its screen:\n  > done: 3 files changed\n  tests pass"
    );
    let result = client.call("read_pane", json!({ "pane": "p2", "scrollback": true }));
    assert!(text(&result).contains("scrollback included"));
    let result = client.call("read_pane", json!({ "pane": "p5" }));
    assert_eq!(
        text(&result),
        "p5 (claude, native chat) is idle for 20s. Its latest answer:\nThe fix is in src/auth.rs."
    );

    let result = client.call(
        "wait_for_panes",
        json!({ "panes": ["p2", "p7"], "timeout_seconds": 60 }),
    );
    conforms("wait_for_panes", &result);
    assert_eq!(result["structuredContent"]["outcome"], "done");
    assert!(
        text(&result).starts_with("Done: every pane finished.\np2: codex, terminal, idle for 20s"),
        "{}",
        text(&result)
    );
    // One id on its own is taken as a list.
    let result = client.call(
        "wait_for_panes",
        json!({ "panes": "p2", "timeout_seconds": 5, "until": "exit", "any": true }),
    );
    assert_eq!(result["structuredContent"]["outcome"], "done");

    let result = client.call("list_panes", json!({}));
    conforms("list_panes", &result);
    assert_eq!(
        text(&result),
        "Space \"kou\" (sp1, /work)\n\
         \x20 Session \"session s1\" (s1, on screen, yours, /work)\n\
         \x20   Tab \"tab t1\" (t1, in front)\n\
         \x20     p1: codex, terminal, working for 20s, \"Review auth\" <- you\n\
         \x20     p2: codex, terminal, idle for 20s, \"Review auth\", started by you\n\
         \x20   Tab \"tab t2\" (t2)\n\
         \x20     p5: codex, chat, blocked for 20s, waiting for an answer, \"Review auth\", in /work/docs"
    );
    assert_eq!(result["structuredContent"]["you"], "p1");
    let result = client.call("list_panes", json!({ "all": true }));
    conforms("list_panes", &result);
    assert!(
        text(&result).contains("Session \"session s2\" (s2, archived, /work)"),
        "{}",
        text(&result)
    );

    let result = client.call("describe_pane", json!({}));
    conforms("describe_pane", &result);
    assert!(
        text(&result).starts_with("p1 (this is you): codex in a terminal, idle for 20s."),
        "{}",
        text(&result)
    );
    let result = client.call("describe_pane", json!({ "pane": "p2" }));
    assert!(
        text(&result).ends_with("Started by you."),
        "{}",
        text(&result)
    );

    let result = client.call("close_pane", json!({ "pane": "p7" }));
    conforms("close_pane", &result);
    let result = client.call("focus_pane", json!({ "pane": "p7" }));
    conforms("focus_pane", &result);

    let result = client.call("name_tab", json!({ "name": "Fix login", "icon": "bug" }));
    conforms("name_tab", &result);
    assert_eq!(
        text(&result),
        "Tab t1 is now called \"Fix login\", with the bug icon."
    );
    let result = client.call("rename", json!({ "target": "session", "name": "Auth" }));
    conforms("rename", &result);
    let result = client.call("rename", json!({ "target": "space", "name": "Kou" }));
    assert_eq!(text(&result), "The space sp1 is now called \"Kou\".");

    let seen = app.seen();
    let expected = [
        Request::Harnesses,
        Request::PaneSpawn(Spawn {
            from: Some("p1".into()),
            harness: "codex".into(),
            cwd: Some("/work/sub".into()),
            title: Some("Review auth".into()),
            prompt: Some("Review the diff.".into()),
            model: Some("gpt-5.5".into()),
            placement: Placement::Split,
            below: false,
            share: None,
            native: true,
        }),
        Request::PaneSpawn(Spawn {
            from: Some("p1".into()),
            harness: "shell".into(),
            ..Spawn::default()
        }),
        Request::PaneSend {
            pane: "p7".into(),
            text: "Also check the tests.".into(),
            submit: true,
            from: Some("p1".into()),
        },
        Request::PaneSend {
            pane: "p7".into(),
            text: "draft".into(),
            submit: false,
            from: Some("p1".into()),
        },
        Request::PaneInfo { pane: "p2".into() },
        Request::PaneRead {
            pane: "p2".into(),
            lines: None,
            scrollback: false,
        },
        Request::PaneInfo { pane: "p2".into() },
        Request::PaneRead {
            pane: "p2".into(),
            lines: Some(200),
            scrollback: true,
        },
        Request::PaneInfo { pane: "p5".into() },
        Request::PaneRead {
            pane: "p5".into(),
            lines: None,
            scrollback: false,
        },
        Request::PaneWait {
            panes: vec!["p2".into(), "p7".into()],
            until: Until::Idle,
            any: false,
            timeout_ms: Some(100),
        },
        Request::PaneWait {
            panes: vec!["p2".into()],
            until: Until::Exit,
            any: true,
            timeout_ms: Some(100),
        },
        Request::SpaceList,
        Request::SessionList { space: None },
        Request::TabList {
            session: Some("s1".into()),
        },
        Request::PaneList {
            session: Some("s1".into()),
        },
        Request::SpaceList,
        Request::SessionList { space: None },
        Request::TabList { session: None },
        Request::PaneList { session: None },
        Request::PaneInfo { pane: "p1".into() },
        Request::PaneInfo { pane: "p2".into() },
        Request::PaneClose { pane: "p7".into() },
        Request::PaneFocus { pane: "p7".into() },
        Request::TabSet {
            tab: None,
            pane: Some("p1".into()),
            name: Some("Fix login".into()),
            icon: Some("bug".into()),
        },
        Request::SessionRename {
            session: Some("s1".into()),
            pane: Some("p1".into()),
            name: "Auth".into(),
        },
        Request::SpaceRename {
            space: None,
            pane: Some("p1".into()),
            name: "Kou".into(),
        },
    ];
    assert_eq!(seen.len(), expected.len(), "{seen:#?}");
    for (seen, expected) in seen.iter().zip(expected) {
        assert_eq!(seen, &expected);
    }
    assert_eq!(client.finish(), 0);
}

#[test]
fn mistakes_a_model_makes_are_tool_errors_it_can_act_on() {
    let app = FakeApp::start(app_answer);
    let mut client = Client::initialized(app.env(), "2025-11-25");
    let refused = |client: &mut Client, tool: &str, arguments: Value, says: &str| {
        let result = client.call(tool, arguments);
        assert_eq!(result["isError"], true, "{tool}: {result}");
        assert!(result.get("structuredContent").is_none(), "{tool}");
        assert!(text(&result).contains(says), "{tool}: {}", text(&result));
    };
    refused(
        &mut client,
        "wait_for_panes",
        json!({ "panes": ["p2"] }),
        "`timeout_seconds` is required",
    );
    refused(
        &mut client,
        "wait_for_panes",
        json!({ "panes": ["p2"], "timeout_seconds": MAX_WAIT_SECONDS + 1 }),
        "from 1 to 3600",
    );
    refused(
        &mut client,
        "wait_for_panes",
        json!({ "panes": [], "timeout_seconds": 5 }),
        "`panes` is required",
    );
    refused(
        &mut client,
        "wait_for_panes",
        json!({ "panes": ["p1"], "timeout_seconds": 5 }),
        "your own pane",
    );
    refused(
        &mut client,
        "wait_for_panes",
        json!({ "panes": ["p2"], "timeout": 5 }),
        "no argument `timeout`",
    );
    refused(
        &mut client,
        "start_agent",
        json!({ "prompt": "do it" }),
        "`harness` is required",
    );
    refused(
        &mut client,
        "start_agent",
        json!({ "harness": "claude", "placement": "left" }),
        "auto, split, tab",
    );
    refused(
        &mut client,
        "send_to_pane",
        json!({ "pane": "p1", "text": "hi" }),
        "your own pane",
    );
    refused(
        &mut client,
        "send_to_pane",
        json!({ "pane": "p2", "text": "  " }),
        "`text` is required",
    );
    refused(
        &mut client,
        "close_pane",
        json!({ "pane": "p1" }),
        "closing it would end you",
    );
    refused(
        &mut client,
        "name_tab",
        json!({}),
        "a `name`, an `icon`, or both",
    );
    refused(
        &mut client,
        "rename",
        json!({ "name": "x" }),
        "`target` is required",
    );
    refused(
        &mut client,
        "read_pane",
        json!({ "pane": "p2", "lines": "many" }),
        "whole number",
    );

    // A null for an optional argument is no argument.
    let result = client.call(
        "read_pane",
        json!({ "pane": "p2", "lines": null, "scrollback": null }),
    );
    assert!(result.get("isError").is_none(), "{result}");

    // None of the refused calls reached the app.
    assert_eq!(
        app.seen(),
        [
            Request::PaneInfo { pane: "p2".into() },
            Request::PaneRead {
                pane: "p2".into(),
                lines: None,
                scrollback: false
            }
        ]
    );
    assert_eq!(client.finish(), 0);
}

#[test]
fn an_app_that_refuses_is_a_tool_error_with_its_message() {
    let app = FakeApp::start(app_answer);
    let mut client = Client::initialized(app.env(), "2025-11-25");
    for (tool, arguments) in [
        ("read_pane", json!({ "pane": "p9" })),
        ("describe_pane", json!({ "pane": "p9" })),
        ("send_to_pane", json!({ "pane": "p9", "text": "hello" })),
        ("close_pane", json!({ "pane": "p9" })),
        ("focus_pane", json!({ "pane": "p9" })),
    ] {
        let result = client.call(tool, arguments);
        assert_eq!(result["isError"], true, "{tool}");
        assert_eq!(text(&result), "kou desktop refused: no pane p9", "{tool}");
    }
    assert_eq!(client.finish(), 0);
}

#[test]
fn an_app_that_is_not_running_is_a_tool_error_that_says_so() {
    let dir = socket_dir();
    let socket = dir.path().join("kou.sock");
    let mut client = Client::initialized(env_at(socket.clone()), "2025-11-25");
    for (tool, arguments) in [
        ("list_harnesses", json!({})),
        ("list_panes", json!({})),
        (
            "wait_for_panes",
            json!({ "panes": ["p2"], "timeout_seconds": 5 }),
        ),
    ] {
        let result = client.call(tool, arguments);
        assert_eq!(result["isError"], true, "{tool}");
        let said = text(&result);
        assert!(said.starts_with("kou desktop is not running"), "{said}");
        assert!(said.contains(&socket.display().to_string()), "{said}");
    }
    // A socket file nobody listens on is the same.
    drop(UnixListener::bind(&socket).unwrap());
    let result = client.call("list_harnesses", json!({}));
    assert!(
        text(&result).starts_with("kou desktop is not running"),
        "{}",
        text(&result)
    );
    assert_eq!(client.finish(), 0);
}

// Waiting.

/// An app whose panes work (or are blocked) for `busy` and are then idle;
/// until then each wait is answered when its own timeout runs out, except
/// that a wait for idle ends at once on a blocked pane, as in the app.
fn busy_for(busy: Duration, blocked: bool) -> impl Fn(&Request) -> Answer + Send + 'static {
    let started = Instant::now();
    move |request| match request {
        Request::PaneWait {
            panes,
            timeout_ms,
            until,
            ..
        } => {
            let activity = if blocked {
                Activity::Blocked
            } else {
                Activity::Working
            };
            // The app ends a wait for idle as soon as a pane is blocked.
            if blocked && *until == Until::Idle {
                return Answer::Now(Response::Waited {
                    panes: panes.iter().map(|id| pane(id, activity)).collect(),
                    timed_out: false,
                });
            }
            if started.elapsed() >= busy {
                return Answer::Now(Response::Waited {
                    panes: panes.iter().map(|id| pane(id, Activity::Idle)).collect(),
                    timed_out: false,
                });
            }
            Answer::Later(
                Duration::from_millis(timeout_ms.unwrap_or(0)),
                Response::Waited {
                    panes: panes.iter().map(|id| pane(id, activity)).collect(),
                    timed_out: true,
                },
            )
        }
        other => app_answer(other),
    }
}

#[test]
fn a_long_wait_does_not_hold_up_ping_or_other_calls() {
    let app = FakeApp::start(busy_for(Duration::from_millis(800), false));
    let mut client = Client::initialized(app.env(), "2025-11-25");
    let started = Instant::now();
    let wait = client.send_request(
        "tools/call",
        json!({
            "name": "wait_for_panes",
            "arguments": { "panes": ["p2"], "timeout_seconds": 30 },
        }),
    );
    thread::sleep(Duration::from_millis(100));

    let ping = client.send_request("ping", json!({}));
    assert_eq!(client.answer(ping)["result"], json!({}));
    let result = client.call("describe_pane", json!({ "pane": "p2" }));
    assert!(text(&result).starts_with("p2: codex"));
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "{:?}",
        started.elapsed()
    );
    assert!(
        client.pending.is_empty(),
        "the wait answered early: {:?}",
        client.pending
    );

    let result = client.answer(wait)["result"].clone();
    // The app's panes turn idle 800 ms after it started, a little before this clock did.
    assert!(started.elapsed() >= Duration::from_millis(600));
    assert_eq!(result["structuredContent"]["outcome"], "done");
    // It was asked of the app in slices.
    let slices = app
        .seen()
        .iter()
        .filter(|request| matches!(request, Request::PaneWait { .. }))
        .count();
    assert!(slices >= 5, "{slices} slices");
    assert_eq!(client.finish(), 0);
}

#[test]
fn a_wait_ends_early_when_a_pane_is_blocked() {
    let app = FakeApp::start(busy_for(Duration::from_secs(600), true));
    let mut client = Client::initialized(app.env(), "2025-11-25");
    // The app ends a wait for idle itself; a wait for an exit ends between slices.
    let started = Instant::now();
    let result = client.call(
        "wait_for_panes",
        json!({ "panes": ["p2"], "timeout_seconds": 600, "until": "exit" }),
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(result["structuredContent"]["outcome"], "blocked");
    let started = Instant::now();
    let result = client.call(
        "wait_for_panes",
        json!({ "panes": ["p2"], "timeout_seconds": 600 }),
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    conforms("wait_for_panes", &result);
    assert_eq!(result["structuredContent"]["outcome"], "blocked");
    let said = text(&result);
    assert!(
        said.starts_with("Stopped after 0s because a pane is blocked."),
        "{said}"
    );
    assert!(
        said.contains("read_pane \"p2\"") && said.contains("focus_pane \"p2\""),
        "{said}"
    );
    assert_eq!(client.finish(), 0);
}

#[test]
fn a_wait_that_runs_out_says_so_and_reports_progress_when_asked() {
    let app = FakeApp::start(busy_for(Duration::from_secs(600), false));
    let mut client = Client::initialized(app.env(), "2025-11-25");
    let id = client.send_request(
        "tools/call",
        json!({
            "name": "wait_for_panes",
            "arguments": { "panes": ["p2"], "timeout_seconds": 1 },
            "_meta": { "progressToken": "tok" },
        }),
    );
    let result = client.answer(id)["result"].clone();
    conforms("wait_for_panes", &result);
    assert_eq!(result["structuredContent"]["outcome"], "timed_out");
    assert!(
        text(&result)
            .starts_with("Timed out after 1s: not finished yet.\np2: codex, terminal, working"),
        "{}",
        text(&result)
    );

    let progress: Vec<&Value> = client
        .pending
        .iter()
        .filter(|message| message["method"] == "notifications/progress")
        .collect();
    assert!(!progress.is_empty(), "{:?}", client.pending);
    for message in progress {
        assert_eq!(message["params"]["progressToken"], "tok");
        assert_eq!(message["params"]["total"], 1.0);
        assert_eq!(message["params"]["message"], "waiting: p2 working");
    }
    assert_eq!(client.finish(), 0);
}

#[test]
fn a_cancelled_wait_is_not_answered_and_lets_go_of_the_app() {
    // A bare listener, to see the connection close.
    let dir = socket_dir();
    let socket = dir.path().join("kou.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (asked, asked_rx) = mpsc::channel();
    let (closed, closed_rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        asked.send(line).unwrap();
        let mut rest = Vec::new();
        let read = reader.read_to_end(&mut rest);
        closed.send(read.is_ok()).unwrap();
    });

    let timing = Timing {
        slice: Duration::from_secs(30),
        ..quick()
    };
    let mut client = Client::with_timing(env_at(socket), timing);
    client.request(
        "initialize",
        json!({ "protocolVersion": "2025-11-25", "capabilities": {} }),
    );
    let wait = client.send_request(
        "tools/call",
        json!({
            "name": "wait_for_panes",
            "arguments": { "panes": ["p2"], "timeout_seconds": 600 },
            "_meta": { "progressToken": 7 },
        }),
    );
    let line = asked_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        line.starts_with(r#"{"op":"pane_wait","panes":["p2"]"#),
        "{line}"
    );

    client.send(json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": wait, "reason": "the user pressed escape" },
    }));
    assert!(
        closed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        "the connection was not closed"
    );

    let ping = client.send_request("ping", json!({}));
    assert_eq!(client.answer(ping)["result"], json!({}));
    assert_eq!(
        client.next_within(Duration::from_millis(300)),
        None,
        "the cancelled call was answered"
    );
    assert_eq!(client.finish(), 0);
}

#[test]
fn the_end_of_input_cancels_what_is_in_flight() {
    let app = FakeApp::start(|request| match request {
        Request::PaneWait { .. } => Answer::Hold,
        other => app_answer(other),
    });
    let timing = Timing {
        slice: Duration::from_secs(30),
        ..quick()
    };
    let mut client = Client::with_timing(app.env(), timing);
    client.request(
        "initialize",
        json!({ "protocolVersion": "2025-11-25", "capabilities": {} }),
    );
    client.send_request(
        "tools/call",
        json!({
            "name": "wait_for_panes",
            "arguments": { "panes": ["p2"], "timeout_seconds": 600 },
        }),
    );
    while !app
        .seen()
        .iter()
        .any(|request| matches!(request, Request::PaneWait { .. }))
    {
        thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    assert_eq!(client.finish(), 0);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

// The guidance.

#[test]
fn the_instructions_fit_what_clients_keep() {
    // Claude Code cuts server instructions and tool descriptions at 4,096
    // characters; the command line's text goes into the context the same way.
    for flavour in Flavour::ALL {
        let length = flavour.instructions().len();
        assert!(length < 3600, "{flavour:?}: {length} characters");
        // Claude Code lists at most 1,536 characters of a skill's description
        // and escapes angle brackets.
        let description = flavour.skill_description();
        assert!(
            description.len() <= 1536 && !description.contains(['<', '>']),
            "{flavour:?}"
        );
    }
    let (instructions, guide) = (Flavour::Tools.instructions(), Flavour::Tools.guide());
    for tool in tools::catalog() {
        assert!(
            instructions.contains(tool.name) || ["list_harnesses", "rename"].contains(&tool.name),
            "{}",
            tool.name
        );
        assert!(
            guide.contains(tool.name)
                || ["describe_pane", "list_panes", "rename"].contains(&tool.name),
            "{}",
            tool.name
        );
    }
}

#[test]
fn the_tools_guide_calls_only_tools_and_arguments_that_exist() {
    let guide = Flavour::Tools.guide();
    let calls = guide
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '('))
        .filter(|word| word.len() > 1 && word.ends_with('('));
    for call in calls {
        let name = call.trim_end_matches('(');
        assert!(tools::find(name).is_some(), "the guide calls {name}");
    }
    for line in guide.lines() {
        for tool in tools::catalog() {
            let Some(at) = line.find(&format!("{}(", tool.name)) else {
                continue;
            };
            let arguments: Vec<&str> = tool.arguments().collect();
            // The argument names: words before `=`, outside quotes, up to the closing parenthesis.
            let (mut quoted, mut word) = (false, String::new());
            for c in line[at + tool.name.len() + 1..].chars() {
                match c {
                    '"' => quoted = !quoted,
                    ')' if !quoted => break,
                    '=' if !quoted => {
                        assert!(
                            arguments.contains(&word.as_str()),
                            "{}: no argument {word} in: {line}",
                            tool.name
                        );
                    }
                    c if !quoted && (c.is_ascii_alphanumeric() || c == '_') => word.push(c),
                    _ if !quoted => word.clear(),
                    _ => {}
                }
            }
        }
    }
}

/// The passages that belong to one flavour only, by template and label. A
/// rule, a pattern or a caution added to one flavour alone must be added
/// here too, on purpose; anything else is shared or comes in pairs.
const ONE_FLAVOUR_ONLY: &[(&str, &str, &str)] = &[
    ("instructions", "commands", "own-pane"),
    ("guide", "commands", "command-line"),
    ("guide", "commands", "passing-the-brief"),
    ("guide", "commands", "shell-time-limit"),
    ("guide", "commands", "sandbox"),
];

#[test]
fn both_flavours_come_from_one_source_and_differ_only_where_marked() {
    let templates = [
        ("instructions", guidance::INSTRUCTIONS),
        ("guide", guidance::GUIDE),
    ];
    let mut only = Vec::new();
    for (name, source) in templates {
        let (pieces, problems) = guidance::parse(source);
        assert!(problems.is_empty(), "{name}: {problems:?}");
        for piece in &pieces {
            match piece {
                Piece::Pair { tools, commands } => {
                    let blank = |lines: &Vec<&str>| lines.iter().all(|line| line.trim().is_empty());
                    assert!(
                        !blank(tools) && !blank(commands),
                        "{name}: a pair with an empty side: {piece:?}"
                    );
                }
                Piece::Only { flavour, label, .. } => only.push((name, flavour.word(), *label)),
                Piece::Shared(_) => {}
            }
        }
        for flavour in Flavour::ALL {
            let rendered = guidance::render(source, flavour, Pointer::Skill);
            assert!(
                rendered.problems.is_empty(),
                "{name} {flavour:?}: {:?}",
                rendered.problems
            );
            assert!(
                !rendered.text.contains("{{") && !rendered.text.contains("\n:::"),
                "{name} {flavour:?}"
            );
        }
    }
    assert_eq!(only, ONE_FLAVOUR_ONLY, "passages of one flavour only");

    // Every term is used, and has a wording in both flavours.
    for (term, tools, commands) in guidance::TERMS {
        assert!(!tools.is_empty() && !commands.is_empty(), "{term}");
        let used = format!("{{{{{term}}}}}");
        assert!(
            templates.iter().any(|(_, source)| source.contains(&used)),
            "{term} is never used"
        );
    }

    // The same sections, in the same order.
    let headings = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| line.starts_with("##"))
            .map(str::to_owned)
            .collect()
    };
    let (tools, commands) = (Flavour::Tools.guide(), Flavour::Commands.guide());
    let mut expected = headings(tools);
    expected.insert(1, "## The command line".into());
    assert_eq!(headings(commands), expected);

    // A shared line that names one flavour's means would leak it into the other.
    // (`rename` is also an ordinary word: only the names with an underscore are telling.)
    let tool_names: Vec<&str> = tools::catalog()
        .iter()
        .map(|tool| tool.name)
        .filter(|name| name.contains('_'))
        .collect();
    for text in [Flavour::Commands.instructions(), commands] {
        for name in &tool_names {
            assert!(
                !text.contains(name),
                "the command line's text names the tool {name}"
            );
        }
    }
    for text in [Flavour::Tools.instructions(), tools] {
        assert!(
            !text.contains("KOU_BIN") && !text.contains("pane spawn"),
            "the tools' text names the command line"
        );
    }
}

#[test]
fn the_bundle_is_written_once_under_a_name_from_its_content() {
    let dir = tempfile::tempdir().unwrap();
    let guidance = dir.path().join("guidance");
    let bundle = crate::write_bundle(&guidance).unwrap();
    let root = bundle
        .tools
        .guide
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let name = root.file_name().unwrap().to_str().unwrap().to_owned();
    let hash = name.strip_prefix("kou-mcp-").unwrap();
    assert!(
        hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()),
        "{name}"
    );

    let mut files = Vec::new();
    for flavour in Flavour::ALL {
        let own = bundle.get(flavour);
        assert_eq!(own.guide.parent().unwrap(), root.join(flavour.word()));
        assert_eq!(own.brief.parent(), own.guide.parent());
        assert_eq!(own.claude_plugin.parent(), own.guide.parent());
        files.push(own.brief.clone());
        files.push(own.guide.clone());
        files.push(own.claude_plugin.join(".claude-plugin/plugin.json"));
        files.push(own.claude_plugin.join("skills/orchestrate/SKILL.md"));
    }
    // The two plugins are separate directories: a launch is given one.
    assert_ne!(bundle.tools.claude_plugin, bundle.commands.claude_plugin);
    let stamps: Vec<_> = files
        .iter()
        .map(|file| std::fs::metadata(file).unwrap().modified().unwrap())
        .collect();
    thread::sleep(Duration::from_millis(20));

    assert_eq!(crate::write_bundle(&guidance).unwrap(), bundle);
    let again: Vec<_> = files
        .iter()
        .map(|file| std::fs::metadata(file).unwrap().modified().unwrap())
        .collect();
    assert_eq!(stamps, again, "the second write touched the files");
    let entries: Vec<_> = std::fs::read_dir(&guidance)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        entries,
        [std::ffi::OsString::from(&name)],
        "left behind: {entries:?}"
    );

    // The name comes from the content alone, not from where it is written.
    let elsewhere = crate::write_bundle(&dir.path().join("elsewhere")).unwrap();
    let elsewhere = elsewhere
        .tools
        .guide
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert_eq!(elsewhere.file_name().unwrap(), name.as_str());
}

#[test]
fn each_flavour_has_a_brief_a_guide_and_a_skill_that_parse() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = crate::write_bundle(dir.path()).unwrap();
    for flavour in Flavour::ALL {
        let own = bundle.get(flavour);
        let manifest =
            std::fs::read_to_string(own.claude_plugin.join(".claude-plugin/plugin.json")).unwrap();
        let manifest: Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(manifest["name"], PLUGIN_NAME);
        assert!(manifest["description"].is_string());

        let skill =
            std::fs::read_to_string(own.claude_plugin.join("skills/orchestrate/SKILL.md")).unwrap();
        let rest = skill
            .strip_prefix("---\n")
            .expect("frontmatter on the first line");
        let (frontmatter, body) = rest
            .split_once("\n---\n")
            .expect("the end of the frontmatter");
        let mut fields = std::collections::BTreeMap::new();
        for line in frontmatter.lines() {
            let (key, value) = line.split_once(": ").unwrap_or_else(|| panic!("{line}"));
            // Double-quoted YAML scalars written as JSON strings.
            let value: String =
                serde_json::from_str(value).unwrap_or_else(|error| panic!("{key}: {error}"));
            fields.insert(key.to_owned(), value);
        }
        assert_eq!(fields.keys().collect::<Vec<_>>(), ["description", "name"]);
        assert_eq!(fields["name"], SKILL_NAME);
        assert_eq!(fields["description"], flavour.skill_description());
        assert!(body.contains(flavour.guide().trim_end()), "{flavour:?}");

        // The brief names the guide's file, which holds the rules and the guide.
        let brief = std::fs::read_to_string(&own.brief).unwrap();
        assert!(
            brief.contains(&own.guide.display().to_string()),
            "{flavour:?}: {brief}"
        );
        let rules = brief.split("\n- ").nth(1).unwrap().to_owned();
        let plain = std::fs::read_to_string(&own.guide).unwrap();
        assert!(plain.starts_with("# "));
        assert!(
            plain.contains(&rules) && plain.contains(flavour.guide().trim_end()),
            "{flavour:?}"
        );
        assert!(
            !plain.contains("Before you orchestrate"),
            "the guide points to itself"
        );
    }
    // The skills' bodies differ: the command line's is not the tools' one.
    assert_ne!(Flavour::Tools.guide(), Flavour::Commands.guide());
}

#[test]
fn a_directory_is_taken_from_home_or_from_the_servers_own() {
    let env = env_at("/nowhere".into());
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert_eq!(tools::directory(&env, "~/src/app"), home.join("src/app"));
    assert_eq!(tools::directory(&env, "~"), home);
    assert_eq!(tools::directory(&env, "sub"), PathBuf::from("/work/sub"));
    assert_eq!(
        tools::directory(&env, "/abs/dir"),
        PathBuf::from("/abs/dir")
    );
    assert_eq!(
        tools::directory(&env, "~other/x"),
        PathBuf::from("/work/~other/x")
    );
}
