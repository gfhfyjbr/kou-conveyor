//! The tools: what each one is for, its schemas, and how a call becomes
//! requests to the app and a result a model can read at a glance.
//!
//! Descriptions are written for a model that has never seen the app: what the
//! tool is for, when to use it and when not, and what comes back. Every
//! result is text first; the structured form follows where the revision has
//! it.

use crate::Env;
use crate::app::{self, CallError, Cancel};
use crate::rpc::{Timing, Version};
use kou_ipc::{Activity, Pane, PaneKind, Placement, Request, Response, Spawn, Until};
use serde_json::{Map, Value, json};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The longest `wait_for_panes` may be asked to wait, in seconds. Longer jobs
/// are waited for in several calls, with a look at the pane in between.
pub(crate) const MAX_WAIT_SECONDS: u64 = 3600;

/// The most a read returns, in characters. Claude Code moves a result over
/// 50,000 characters to a file, and a screen is never that long.
const MAX_READ_CHARS: usize = 40_000;

/// What `read_pane` reads of the scrollback when no number of lines is given.
const DEFAULT_SCROLLBACK_LINES: u64 = 200;

/// The icons a tab can be given, as the app names them. The app refuses any
/// other name and lists its own, so a name added there still works here.
pub(crate) const TAB_ICONS: &[&str] = &[
    "terminal", "code", "bug", "flask", "eye", "book", "branch", "package", "database", "server",
    "globe", "wrench", "bolt", "rocket", "flame", "star",
];

/// What a tool's code gets to work with.
pub(crate) struct Context<'a> {
    pub env: &'a Env,
    pub timing: &'a Timing,
    pub cancel: &'a Cancel,
    /// Reports progress of a long call: done so far, total, and a line of text.
    pub progress: &'a dyn Fn(f64, f64, String),
}

/// How a call ended, short of a result.
enum Fail {
    /// The client cancelled it: nothing is answered.
    Cancelled,
    /// A result with `isError`, saying why.
    Tool(String),
}

impl From<CallError> for Fail {
    fn from(error: CallError) -> Fail {
        match error {
            CallError::Cancelled => Fail::Cancelled,
            CallError::NotRunning(socket) => Fail::Tool(format!(
                "kou desktop is not running, or this server cannot reach it: nothing listens on {}. \
                 These tools only work for an agent started inside the app.",
                socket.display()
            )),
            CallError::Failed(message) => Fail::Tool(message),
        }
    }
}

/// A successful call: the text for the model and its structured form.
struct Done {
    text: String,
    structured: Value,
}

type Run = Result<Done, Fail>;

/// The annotations of a tool. Clients read them as hints: Claude Code runs
/// read-only calls in parallel, Codex asks before calls that write.
struct Hints {
    read_only: bool,
    destructive: bool,
    idempotent: bool,
}

const READ_ONLY: Hints = Hints {
    read_only: true,
    destructive: false,
    idempotent: true,
};

/// One tool as the client sees it, and its code.
pub(crate) struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub description: String,
    pub input: Value,
    pub output: Value,
    hints: Hints,
    run: fn(&Context, &Args) -> Run,
}

impl Tool {
    /// The tool's entry in `tools/list`, with only what `version` knows of.
    pub(crate) fn describe(&self, version: Version) -> Value {
        let mut entry = Map::new();
        entry.insert("name".into(), json!(self.name));
        if version.structured() {
            entry.insert("title".into(), json!(self.title));
        }
        entry.insert("description".into(), json!(self.description));
        entry.insert("inputSchema".into(), self.input.clone());
        if version.structured() {
            entry.insert("outputSchema".into(), self.output.clone());
        }
        if version.annotations() {
            entry.insert(
                "annotations".into(),
                json!({
                    "title": self.title,
                    "readOnlyHint": self.hints.read_only,
                    "destructiveHint": self.hints.destructive,
                    "idempotentHint": self.hints.idempotent,
                    // Everything happens in the app on this machine.
                    "openWorldHint": false,
                }),
            );
        }
        Value::Object(entry)
    }

    /// The names of the tool's arguments.
    pub(crate) fn arguments(&self) -> impl Iterator<Item = &str> {
        self.input["properties"]
            .as_object()
            .into_iter()
            .flat_map(|properties| properties.keys().map(String::as_str))
    }
}

/// Every tool, in the order clients list them.
pub(crate) fn catalog() -> &'static [Tool] {
    static CATALOG: OnceLock<Vec<Tool>> = OnceLock::new();
    CATALOG.get_or_init(build_catalog)
}

pub(crate) fn find(name: &str) -> Option<&'static Tool> {
    catalog().iter().find(|tool| tool.name == name)
}

/// Run a tool call and shape its result for `version`. `None` when the call
/// was cancelled: it gets no answer.
pub(crate) fn call(
    tool: &Tool,
    arguments: &Map<String, Value>,
    context: &Context,
    version: Version,
) -> Option<Value> {
    let args = Args {
        map: arguments,
        tool,
    };
    let outcome = args.check().and_then(|()| (tool.run)(context, &args));
    let (text, structured, error) = match outcome {
        Ok(done) => (done.text, Some(done.structured), false),
        Err(Fail::Cancelled) => return None,
        Err(Fail::Tool(message)) => (message, None, true),
    };
    let mut result = Map::new();
    result.insert("content".into(), json!([{ "type": "text", "text": text }]));
    if let Some(structured) = structured
        && version.structured()
    {
        result.insert("structuredContent".into(), structured);
    }
    if error {
        result.insert("isError".into(), json!(true));
    }
    Some(Value::Object(result))
}

/// The arguments of one call, read with the mistakes models make in mind: a
/// `null` for an optional argument, one id where a list is asked for, a
/// misspelled name.
struct Args<'a> {
    map: &'a Map<String, Value>,
    tool: &'a Tool,
}

impl Args<'_> {
    fn check(&self) -> Result<(), Fail> {
        let known: Vec<&str> = self.tool.arguments().collect();
        if let Some(unknown) = self.map.keys().find(|key| !known.contains(&key.as_str())) {
            let takes = if known.is_empty() {
                "no arguments".to_owned()
            } else {
                known.join(", ")
            };
            return Err(invalid(format!(
                "{} has no argument `{unknown}`; it takes {takes}",
                self.tool.name
            )));
        }
        Ok(())
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key).filter(|value| !value.is_null())
    }

    /// A string as given, untrimmed; empty counts as given.
    fn raw(&self, key: &str) -> Result<Option<String>, Fail> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(invalid(format!("`{key}` must be a string"))),
        }
    }

    /// A trimmed string; empty counts as not given.
    fn text(&self, key: &str) -> Result<Option<String>, Fail> {
        Ok(self
            .raw(key)?
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty()))
    }

    fn required(&self, key: &str, what: &str) -> Result<String, Fail> {
        self.text(key)?
            .ok_or_else(|| invalid(format!("`{key}` is required: {what}")))
    }

    fn flag(&self, key: &str, default: bool) -> Result<bool, Fail> {
        match self.get(key) {
            None => Ok(default),
            Some(Value::Bool(flag)) => Ok(*flag),
            Some(Value::String(text)) if text == "true" || text == "false" => Ok(text == "true"),
            Some(_) => Err(invalid(format!("`{key}` must be true or false"))),
        }
    }

    fn whole(&self, key: &str, min: u64, max: u64) -> Result<Option<u64>, Fail> {
        let number = match self.get(key) {
            None => return Ok(None),
            Some(Value::Number(number)) => number.as_u64().or_else(|| {
                number
                    .as_f64()
                    .filter(|n| n.fract() == 0.0 && *n >= 0.0)
                    .map(|n| n as u64)
            }),
            Some(Value::String(text)) => text.trim().parse().ok(),
            Some(_) => None,
        };
        match number {
            Some(number) if (min..=max).contains(&number) => Ok(Some(number)),
            _ => Err(invalid(format!(
                "`{key}` must be a whole number from {min} to {max}"
            ))),
        }
    }

    fn choice<'c>(
        &self,
        key: &str,
        choices: &[&'c str],
        default: &'c str,
    ) -> Result<&'c str, Fail> {
        match self.text(key)? {
            None => Ok(default),
            Some(given) => choices
                .iter()
                .find(|choice| choice.eq_ignore_ascii_case(&given))
                .copied()
                .ok_or_else(|| invalid(format!("`{key}` must be one of {}", choices.join(", ")))),
        }
    }

    /// Pane ids: a list, or one id on its own.
    fn panes(&self, key: &str) -> Result<Vec<String>, Fail> {
        let items: Vec<&Value> = match self.get(key) {
            None => Vec::new(),
            Some(Value::Array(items)) => items.iter().collect(),
            Some(single @ Value::String(_)) => vec![single],
            Some(_) => return Err(invalid(format!("`{key}` must be a list of pane ids"))),
        };
        let mut panes: Vec<String> = Vec::new();
        for item in items {
            let Some(id) = item.as_str().map(str::trim).filter(|id| !id.is_empty()) else {
                return Err(invalid(format!(
                    "`{key}` must be a list of pane ids, such as [\"p3\"]"
                )));
            };
            if !panes.iter().any(|seen| seen == id) {
                panes.push(id.to_owned());
            }
        }
        Ok(panes)
    }
}

fn invalid(message: String) -> Fail {
    Fail::Tool(format!("Invalid arguments: {message}."))
}

/// Ask the app, within the ordinary time.
fn ask(context: &Context, request: &Request) -> Result<Response, Fail> {
    ask_within(context, request, context.timing.call)
}

/// Ask the app; its refusal becomes the tool's error.
fn ask_within(context: &Context, request: &Request, timeout: Duration) -> Result<Response, Fail> {
    match app::call(&context.env.socket, request, timeout, context.cancel)? {
        Response::Error { message } => Err(Fail::Tool(format!("kou desktop refused: {message}"))),
        response => Ok(response),
    }
}

fn unexpected(response: Response) -> Fail {
    Fail::Tool(format!(
        "kou desktop answered with something this server does not expect: {}",
        serde_json::to_string(&response).unwrap_or_default()
    ))
}

/// The caller's own pane, for requests that act on "mine".
fn own_pane(context: &Context, what: &str) -> Result<String, Fail> {
    context.env.pane.clone().ok_or_else(|| {
        Fail::Tool(format!(
            "{what}: this server does not know which pane it serves ({} is not set in its environment)",
            kou_ipc::env::PANE
        ))
    })
}

fn not_yourself(context: &Context, pane: &str, why: &str) -> Result<(), Fail> {
    if context.env.pane.as_deref() == Some(pane) {
        return Err(Fail::Tool(format!("{pane} is your own pane: {why}")));
    }
    Ok(())
}

// Schemas.

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn no_arguments() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

const PANE_ARGUMENT: &str = "A pane id such as \"p3\", from start_agent or list_panes.";

fn pane_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "harness": { "type": "string", "description": "What runs in it: claude, codex, shell..." },
            "title": { "type": "string" },
            "kind": { "type": "string", "enum": ["terminal", "chat"], "description": "Its harness's terminal interface, or the app's native chat." },
            "activity": { "type": "string", "enum": ["idle", "working", "blocked", "exited"], "description": "blocked: its agent waits for an answer." },
            "seconds_in_state": { "type": "integer", "minimum": 0, "description": "How long it has been in this activity." },
            "cwd": { "type": "string" },
            "session": { "type": "string" },
            "parent": { "type": "string", "description": "The pane that started it." },
            "is_you": { "type": "boolean", "description": "It is the caller's own pane." },
            "started_by_you": { "type": "boolean", "description": "The caller started it." },
        },
        "required": ["id", "harness", "title", "kind", "activity", "seconds_in_state", "cwd", "session", "is_you", "started_by_you"],
    })
}

fn tab_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "name": { "type": "string" },
            "icon": { "type": "string" },
            "layout": { "type": "string", "enum": ["panes", "canvas"] },
            "front": { "type": "boolean", "description": "It is the tab its session shows." },
            "panes": { "type": "array", "items": pane_schema() },
        },
        "required": ["id", "name", "layout", "front", "panes"],
    })
}

// Words and lines.

fn activity_word(activity: Activity) -> &'static str {
    match activity {
        Activity::Idle => "idle",
        Activity::Working => "working",
        Activity::Blocked => "blocked",
        Activity::Exited => "exited",
    }
}

fn kind_word(kind: PaneKind) -> &'static str {
    match kind {
        PaneKind::Terminal => "terminal",
        PaneKind::Thread => "chat",
    }
}

fn age(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// "idle for 20s", "blocked for 1m, waiting for an answer".
fn state(pane: &Pane) -> String {
    let since = age(pane.since_ms);
    match pane.activity {
        Activity::Idle => format!("idle for {since}"),
        Activity::Working => format!("working for {since}"),
        Activity::Blocked => format!("blocked for {since}, waiting for an answer"),
        Activity::Exited => format!("exited {since} ago"),
    }
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pane_json(pane: &Pane, env: &Env) -> Value {
    let mut entry = json!({
        "id": pane.id,
        "harness": pane.harness,
        "title": pane.title,
        "kind": kind_word(pane.kind),
        "activity": activity_word(pane.activity),
        "seconds_in_state": pane.since_ms / 1000,
        "cwd": pane.cwd.display().to_string(),
        "session": pane.session,
        "is_you": env.pane.as_deref() == Some(pane.id.as_str()),
        "started_by_you": env.pane.is_some() && pane.parent == env.pane,
    });
    if let Some(parent) = &pane.parent {
        entry["parent"] = json!(parent);
    }
    entry
}

/// One pane on one line: `p2: codex, terminal, idle for 20s, "Review auth", started by you`.
fn pane_line(pane: &Pane, env: &Env) -> String {
    let mut line = format!(
        "{}: {}, {}, {}",
        pane.id,
        pane.harness,
        kind_word(pane.kind),
        state(pane)
    );
    let title = single_line(&pane.title);
    if !title.is_empty() {
        let _ = write!(line, ", \"{title}\"");
    }
    if env.pane.as_deref() == Some(pane.id.as_str()) {
        line.push_str(" <- you");
    } else if env.pane.is_some() && pane.parent == env.pane {
        line.push_str(", started by you");
    } else if let Some(parent) = &pane.parent {
        let _ = write!(line, ", started by {parent}");
    }
    line
}

// The tools.

fn build_catalog() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_harnesses",
            title: "List agents that can be started",
            description: "List the coding agents (harnesses) this app can start in a new pane, such as claude, codex, \
opencode or a plain shell, with their ids for start_agent. Use it before start_agent when you do not know which \
harnesses exist here. Returns one line per harness: id, name and version, whether it is ready to start (installed and \
signed in) or why not, and whether it can run as a native chat (start_agent's native option).".into(),
            input: no_arguments(),
            output: object(
                json!({
                    "harnesses": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name": { "type": "string" },
                            "version": { "type": "string" },
                            "ready": { "type": "boolean" },
                            "native": { "type": "boolean", "description": "It can run as a native chat." },
                            "message": { "type": "string", "description": "Why it is not ready, or a warning." },
                        },
                        "required": ["id", "name", "ready", "native"],
                    }},
                }),
                &["harnesses"],
            ),
            hints: READ_ONLY,
            run: list_harnesses,
        },
        Tool {
            name: "start_agent",
            title: "Start an agent in a new pane",
            description: "Start another coding agent in a new pane of this app and give it a task. It works on its own, \
in parallel with you, and shares none of your context: `prompt` is everything it will know, so make it a complete \
brief (goal, where to look, what you already know, constraints, what to deliver and how to end its answer). The call \
returns as soon as the pane exists, before the agent has done anything: follow it with wait_for_panes on the returned \
pane id, then read_pane. Do not use it for work you can do in a few steps yourself, and do not use it if another agent \
started you unless your brief says to. Returns the new pane's id and state.".into(),
            input: object(
                json!({
                    "harness": { "type": "string", "description": "Harness id from list_harnesses, such as claude, codex, opencode or shell." },
                    "prompt": { "type": "string", "description": "The agent's first prompt: its whole brief. For shell, a command line to run. Leave it out only to start an agent that waits for a task you will send with send_to_pane." },
                    "title": { "type": "string", "description": "A short name for the pane, 2 to 5 words, such as \"Review auth diff\"." },
                    "cwd": { "type": "string", "description": "The directory to run in: absolute, or relative to your working directory. Default: your pane's directory." },
                    "model": { "type": "string", "description": "A model in the harness's own naming. Default: the harness's own choice." },
                    "placement": { "type": "string", "enum": ["auto", "split", "tab"], "default": "auto", "description": "auto: a tab of its own in the background (or a node beside you when your tab is a canvas), and the user keeps looking at you. split: beside you in your tab, so the user sees both. tab: a tab of its own, brought to the front, which takes the user's view." },
                    "below": { "type": "boolean", "default": false, "description": "With placement split: put the pane below you instead of to your right." },
                    "share": { "type": "integer", "minimum": 10, "maximum": 90, "description": "With placement split: how much of your own room the pane takes, in percent. Default: half." },
                    "native": { "type": "boolean", "default": false, "description": "Run it as the app's native chat instead of its terminal interface, when list_harnesses says it can. read_pane then returns its whole last answer as clean text." },
                }),
                &["harness"],
            ),
            output: object(
                json!({
                    "pane": pane_schema(),
                    "next": { "type": "string", "description": "What to do next." },
                }),
                &["pane", "next"],
            ),
            hints: Hints {
                read_only: false,
                destructive: false,
                idempotent: false,
            },
            run: start_agent,
        },
        Tool {
            name: "wait_for_panes",
            title: "Wait until panes are done",
            description: "Wait until panes finish their work (idle) or their process exits, then return their state. \
Use it after start_agent or send_to_pane instead of guessing or polling: an agent is done when it is idle. \
timeout_seconds is required: the longest you are willing to wait before looking at the work yourself (a small task \
120 to 300, a large one 900 to 1800). When it runs out, read_pane shows how far the agent got; then wait again. The \
wait also ends early when a pane is blocked, meaning its agent is waiting for an answer to a question or to a \
permission request: read it and decide. With several panes it waits for all of them, or with any: true for the first \
one. You cannot wait for your own pane.".into(),
            input: object(
                json!({
                    "panes": { "type": "array", "items": { "type": "string" }, "minItems": 1, "description": "The pane ids to wait for, such as [\"p3\", \"p4\"]." },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": MAX_WAIT_SECONDS, "description": "Stop waiting after this many seconds." },
                    "until": { "type": "string", "enum": ["idle", "exit"], "default": "idle", "description": "idle: the agent finished its turn. exit: its process ended." },
                    "any": { "type": "boolean", "default": false, "description": "Return when the first pane gets there, not all of them." },
                }),
                &["panes", "timeout_seconds"],
            ),
            output: object(
                json!({
                    "outcome": { "type": "string", "enum": ["done", "blocked", "timed_out"], "description": "done: the panes got there. blocked: a pane waits for an answer. timed_out: the time ran out." },
                    "waited_seconds": { "type": "integer", "minimum": 0 },
                    "panes": { "type": "array", "items": pane_schema() },
                    "gone": { "type": "array", "items": { "type": "string" }, "description": "Panes that no longer exist." },
                }),
                &["outcome", "waited_seconds", "panes", "gone"],
            ),
            hints: READ_ONLY,
            run: wait_for_panes,
        },
        Tool {
            name: "read_pane",
            title: "Read what a pane shows",
            description: "Read what another pane shows. For a terminal pane: its screen as text, as a person sees it now, \
the harness's own interface included; the latest answer is usually above its input box, and a long one may have \
scrolled off, so pass scrollback: true with lines (say 300) to read further back. For a native chat pane: the whole \
text of its latest answer (lines and scrollback do not apply). Read after wait_for_panes says the pane is idle: a \
working pane shows a partial picture. Returns the pane's state on the first line, then the text.".into(),
            input: object(
                json!({
                    "pane": { "type": "string", "description": PANE_ARGUMENT },
                    "lines": { "type": "integer", "minimum": 1, "maximum": 5000, "description": "At most this many lines, counted from the end. Default: the whole screen, or the last 200 lines with scrollback." },
                    "scrollback": { "type": "boolean", "default": false, "description": "Include what has scrolled off the screen." },
                }),
                &["pane"],
            ),
            output: object(
                json!({
                    "pane": pane_schema(),
                    "text": { "type": "string" },
                    "truncated": { "type": "boolean", "description": "The start of the text was cut to keep the result small." },
                }),
                &["pane", "text", "truncated"],
            ),
            hints: READ_ONLY,
            run: read_pane,
        },
        Tool {
            name: "send_to_pane",
            title: "Send text to a pane",
            description: "Send text to another pane: a follow-up task, an answer to a question its agent asked, or a \
command line for a plain shell. In a terminal pane it is typed into the input and Enter is pressed (submit: false \
types it without Enter); in a native chat pane it is sent as the next message. It arrives as if the user typed it. \
Send when the pane is idle or blocked on a question: text sent while an agent works may be queued or lost, depending \
on the harness. Do not use it to approve permission requests (to run a command, edit a file, use a tool): those are \
the user's. If the user set the link between your pane and that one to ask first, the text waits for their approval. \
Follow it with wait_for_panes, then read_pane.".into(),
            input: object(
                json!({
                    "pane": { "type": "string", "description": PANE_ARGUMENT },
                    "text": { "type": "string", "description": "The text to send." },
                    "submit": { "type": "boolean", "default": true, "description": "Press Enter after it (a terminal) or send it now (a chat)." },
                }),
                &["pane", "text"],
            ),
            output: object(
                json!({
                    "pane": { "type": "string" },
                    "submitted": { "type": "boolean" },
                }),
                &["pane", "submitted"],
            ),
            hints: Hints {
                read_only: false,
                destructive: false,
                idempotent: false,
            },
            run: send_to_pane,
        },
        Tool {
            name: "list_panes",
            title: "List panes, tabs and sessions",
            description: "List the panes of your session, grouped by tab: what each runs, whether it is working, idle, \
blocked waiting for an answer or exited, which pane is yours and which panes you started. all: true lists every \
session of every space (a space is a project folder; a session is a group of tabs). Use it to find pane, tab and \
session ids, or to see what already runs before you start something.".into(),
            input: object(
                json!({
                    "all": { "type": "boolean", "default": false, "description": "Every session of every space, not only yours." },
                }),
                &[],
            ),
            output: object(
                json!({
                    "you": { "type": "string", "description": "Your own pane's id." },
                    "spaces": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name": { "type": "string" },
                            "path": { "type": "string" },
                            "sessions": { "type": "array", "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "cwd": { "type": "string" },
                                    "selected": { "type": "boolean", "description": "It is the session on screen." },
                                    "archived": { "type": "boolean" },
                                    "tabs": { "type": "array", "items": tab_schema() },
                                },
                                "required": ["id", "name", "cwd", "selected", "archived", "tabs"],
                            }},
                        },
                        "required": ["id", "name", "path", "sessions"],
                    }},
                }),
                &["spaces"],
            ),
            hints: READ_ONLY,
            run: list_panes,
        },
        Tool {
            name: "describe_pane",
            title: "Describe a pane",
            description: "Describe one pane: what runs in it, terminal or native chat, what it is doing now (working, \
idle, blocked waiting for an answer, exited) and for how long, its directory, and which pane started it. Without \
`pane` it describes your own pane, which tells you your pane id and whether another agent started you. It does not \
wait: to wait for a pane to finish, use wait_for_panes.".into(),
            input: object(
                json!({
                    "pane": { "type": "string", "description": "A pane id such as \"p3\". Default: your own pane." },
                }),
                &[],
            ),
            output: object(json!({ "pane": pane_schema() }), &["pane"]),
            hints: READ_ONLY,
            run: describe_pane,
        },
        Tool {
            name: "close_pane",
            title: "Close a pane",
            description: "Close a pane and end the agent or program in it. Use it on panes you started once you have \
read their result and need nothing more from them, so they do not pile up. Whatever the pane showed is gone \
afterwards, so read it first. Do not close panes you did not start unless the user asks: they may be the user's own \
work.".into(),
            input: object(
                json!({ "pane": { "type": "string", "description": PANE_ARGUMENT } }),
                &["pane"],
            ),
            output: object(
                json!({ "pane": { "type": "string" }, "closed": { "type": "boolean" } }),
                &["pane", "closed"],
            ),
            hints: Hints {
                read_only: false,
                destructive: true,
                idempotent: true,
            },
            run: close_pane,
        },
        Tool {
            name: "focus_pane",
            title: "Show a pane to the user",
            description: "Bring a pane to the front of the app, switching to its tab, so the user sees it. Use it when \
the user has to look at or act in that pane, for example to answer a permission request there. It is not needed to \
read from or send to a pane, and it takes the user's view away from what they were looking at, so do not use it to \
show progress.".into(),
            input: object(
                json!({ "pane": { "type": "string", "description": PANE_ARGUMENT } }),
                &["pane"],
            ),
            output: object(json!({ "pane": { "type": "string" } }), &["pane"]),
            hints: Hints {
                read_only: false,
                destructive: false,
                idempotent: true,
            },
            run: focus_pane,
        },
        Tool {
            name: "name_tab",
            title: "Name your tab",
            description: format!(
                "Name your own tab for what you are doing, so the user can tell the panes apart: do it when \
you start a task, in 2 to 5 words (\"Fix login redirect\"), and again when the task changes. icon is optional: one of \
{}, or auto for the default. An empty name gives the tab its default name back. tab names another tab by id (from \
list_panes); leave it out for your own.",
                TAB_ICONS.join(", ")
            ),
            input: object(
                json!({
                    "name": { "type": "string", "description": "The tab's new name." },
                    "icon": { "type": "string", "description": "An icon name, or auto." },
                    "tab": { "type": "string", "description": "A tab id. Default: the tab your pane is in." },
                }),
                &[],
            ),
            output: object(
                json!({ "tab": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "icon": { "type": "string" },
                        "panes": { "type": "array", "items": { "type": "string" }, "description": "The ids of its panes." },
                    },
                    "required": ["id", "name", "panes"],
                }}),
                &["tab"],
            ),
            hints: Hints {
                read_only: false,
                destructive: false,
                idempotent: true,
            },
            run: name_tab,
        },
        Tool {
            name: "rename",
            title: "Rename your session or space",
            description: "Rename your session (the group of tabs you are in) or the space it belongs to (a project \
folder). Do it when the user asks, or when you start a long task in a session whose name says nothing about it. To \
name what you are doing, name_tab is usually the right tool.".into(),
            input: object(
                json!({
                    "target": { "type": "string", "enum": ["session", "space"], "description": "What to rename." },
                    "name": { "type": "string", "description": "The new name; not empty." },
                    "id": { "type": "string", "description": "A session or space id from list_panes. Default: yours." },
                }),
                &["target", "name"],
            ),
            output: object(
                json!({
                    "target": { "type": "string", "enum": ["session", "space"] },
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                }),
                &["target", "id", "name"],
            ),
            hints: Hints {
                read_only: false,
                destructive: false,
                idempotent: true,
            },
            run: rename,
        },
    ]
}

fn list_harnesses(context: &Context, _args: &Args) -> Run {
    let harnesses = match ask(context, &Request::Harnesses)? {
        Response::Harnesses { harnesses } => harnesses,
        other => return Err(unexpected(other)),
    };
    let mut text = String::new();
    for harness in &harnesses {
        let _ = write!(text, "{}: {}", harness.id, harness.name);
        if let Some(version) = &harness.version {
            let _ = write!(text, " {version}");
        }
        if harness.ready {
            text.push_str(", ready");
            if harness.native {
                text.push_str(", can run as a native chat");
            }
            if let Some(message) = &harness.message {
                let _ = write!(text, " (note: {})", single_line(message));
            }
        } else {
            let why = harness.message.as_deref().map(single_line);
            let _ = write!(
                text,
                ", not available: {}",
                why.as_deref().unwrap_or("no reason given")
            );
        }
        text.push('\n');
    }
    if harnesses.is_empty() {
        text.push_str("No harnesses were found on this machine.\n");
    }
    let structured: Vec<Value> = harnesses
        .iter()
        .map(|harness| {
            let mut entry = json!({
                "id": harness.id,
                "name": harness.name,
                "ready": harness.ready,
                "native": harness.native,
            });
            if let Some(version) = &harness.version {
                entry["version"] = json!(version);
            }
            if let Some(message) = &harness.message {
                entry["message"] = json!(message);
            }
            entry
        })
        .collect();
    Ok(Done {
        text: text.trim_end().to_owned(),
        structured: json!({ "harnesses": structured }),
    })
}

fn start_agent(context: &Context, args: &Args) -> Run {
    let harness = args.required(
        "harness",
        "a harness id from list_harnesses, such as claude",
    )?;
    let prompt = args
        .raw("prompt")?
        .filter(|prompt| !prompt.trim().is_empty());
    let placement = match args.choice("placement", &["auto", "split", "tab"], "auto")? {
        "split" => Placement::Split,
        "tab" => Placement::Tab,
        _ => Placement::Auto,
    };
    let native = args.flag("native", false)?;
    let below = args.flag("below", false)?;
    let share = args.whole("share", 10, 90)?.map(|share| share as u8);
    if (below || share.is_some()) && placement != Placement::Split {
        return Err(invalid(
            "`below` and `share` go with placement=\"split\": they say where in your own room the pane goes"
                .to_owned(),
        ));
    }
    let spawn = Spawn {
        from: context.env.pane.clone(),
        harness,
        // The app's working directory is not the caller's.
        cwd: args.text("cwd")?.map(|cwd| directory(context.env, &cwd)),
        title: args.text("title")?,
        prompt: prompt.clone(),
        model: args.text("model")?,
        placement,
        below,
        share,
        native,
    };
    let pane = match ask(context, &Request::PaneSpawn(spawn))? {
        Response::Pane(pane) => pane,
        other => return Err(unexpected(other)),
    };
    let id = &pane.id;
    let next = if prompt.is_some() {
        format!(
            "It has only just started and has done nothing yet. Next: wait_for_panes with panes [\"{id}\"] and a \
             timeout, then read_pane \"{id}\"."
        )
    } else {
        format!(
            "It has no task yet. Once wait_for_panes with panes [\"{id}\"] says it is idle, give it one with \
             send_to_pane."
        )
    };
    let mut text = format!(
        "Started pane {id}: {}, in a {}",
        pane.harness,
        match pane.kind {
            PaneKind::Terminal => "terminal",
            PaneKind::Thread => "native chat",
        }
    );
    let title = single_line(&pane.title);
    if !title.is_empty() {
        let _ = write!(text, ", \"{title}\"");
    }
    let _ = writeln!(text, ", in {}.", pane.cwd.display());
    if native && pane.kind == PaneKind::Terminal {
        text.push_str(
            "It runs in its terminal interface: this harness cannot run as a native chat here.\n",
        );
    }
    text.push_str(&next);
    Ok(Done {
        text,
        structured: json!({ "pane": pane_json(&pane, context.env), "next": next }),
    })
}

/// A directory as a model writes it: absolute, from home (`~/src`), or
/// relative to the server's own directory.
pub(crate) fn directory(env: &Env, given: &str) -> PathBuf {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    match (given.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => env.cwd.join(Path::new(given)),
    }
}

/// Why a wait ended.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ended {
    Done,
    Blocked,
    TimedOut,
}

fn wait_for_panes(context: &Context, args: &Args) -> Run {
    let panes = args.panes("panes")?;
    if panes.is_empty() {
        return Err(invalid(
            "`panes` is required: the ids of the panes to wait for, such as [\"p3\"]".into(),
        ));
    }
    let timeout = args
        .whole("timeout_seconds", 1, MAX_WAIT_SECONDS)?
        .ok_or_else(|| {
            invalid(format!(
                "`timeout_seconds` is required: how long to wait at most, from 1 to {MAX_WAIT_SECONDS}"
            ))
        })?;
    let until = match args.choice("until", &["idle", "exit"], "idle")? {
        "exit" => Until::Exit,
        _ => Until::Idle,
    };
    let any = args.flag("any", false)?;
    if let Some(own) = panes
        .iter()
        .find(|pane| context.env.pane.as_deref() == Some(pane.as_str()))
    {
        return Err(Fail::Tool(format!(
            "{own} is your own pane: waiting for it would only end when you stop. Wait for the panes you started."
        )));
    }

    let started = Instant::now();
    let deadline = started + Duration::from_secs(timeout);
    let mut reported = started;
    // The wait is asked for in short slices: a cancelled call then lets go
    // of the app within one, and a blocked pane ends a wait for an exit too.
    let (states, ended) = loop {
        let slice = deadline
            .saturating_duration_since(Instant::now())
            .min(context.timing.slice);
        let request = Request::PaneWait {
            panes: panes.clone(),
            until,
            any,
            timeout_ms: Some(slice.as_millis() as u64),
        };
        let (states, timed_out) = match ask_within(context, &request, slice + context.timing.call)?
        {
            Response::Waited { panes, timed_out } => (panes, timed_out),
            other => return Err(unexpected(other)),
        };
        // The app ends a wait for idle on a blocked pane itself; a wait for
        // an exit it holds, and the slice ends it here.
        if states.iter().any(|pane| pane.activity == Activity::Blocked) {
            break (states, Ended::Blocked);
        }
        if !timed_out {
            break (states, Ended::Done);
        }
        let now = Instant::now();
        if now >= deadline {
            break (states, Ended::TimedOut);
        }
        if now.duration_since(reported) >= context.timing.progress_every {
            reported = now;
            let summary: Vec<String> = states
                .iter()
                .map(|pane| format!("{} {}", pane.id, activity_word(pane.activity)))
                .collect();
            (context.progress)(
                now.duration_since(started).as_secs_f64().floor(),
                timeout as f64,
                format!("waiting: {}", summary.join(", ")),
            );
        }
    };
    let waited = started.elapsed().as_secs();
    let gone: Vec<&String> = panes
        .iter()
        .filter(|id| !states.iter().any(|pane| &pane.id == *id))
        .collect();

    let env = context.env;
    let mut text = match ended {
        Ended::Done if until == Until::Exit => "Done: the process has exited.".to_owned(),
        Ended::Done if any => "Done: a pane finished.".to_owned(),
        Ended::Done => "Done: every pane finished.".to_owned(),
        Ended::Blocked => format!("Stopped after {waited}s because a pane is blocked."),
        Ended::TimedOut => format!("Timed out after {waited}s: not finished yet."),
    };
    text.push('\n');
    for pane in &states {
        let _ = writeln!(text, "{}", pane_line(pane, env));
    }
    for id in &gone {
        let _ = writeln!(text, "{id}: gone (closed)");
    }
    let blocked: Vec<&str> = states
        .iter()
        .filter(|pane| pane.activity == Activity::Blocked)
        .map(|pane| pane.id.as_str())
        .collect();
    match ended {
        Ended::Done => text.push_str(
            "Next: read_pane each finished pane to get its result. Idle means its turn ended, not that it succeeded.",
        ),
        Ended::Blocked => {
            let first = blocked.first().copied().unwrap_or_default();
            let _ = write!(
                text,
                "Blocked means its agent waits for an answer. read_pane \"{first}\" to see what it asks. Answer a \
                 question about the task with send_to_pane. A permission request (to run a command, edit a file, use \
                 a tool) is the user's decision: tell the user, or focus_pane \"{first}\" so they see it."
            );
        }
        Ended::TimedOut => text.push_str(
            "Next: read_pane to see how far it got, then wait again, send a correction, or close it.",
        ),
    }
    let outcome = match ended {
        Ended::Done => "done",
        Ended::Blocked => "blocked",
        Ended::TimedOut => "timed_out",
    };
    Ok(Done {
        text,
        structured: json!({
            "outcome": outcome,
            "waited_seconds": waited,
            "panes": states.iter().map(|pane| pane_json(pane, env)).collect::<Vec<_>>(),
            "gone": gone,
        }),
    })
}

fn read_pane(context: &Context, args: &Args) -> Run {
    let id = args.required("pane", PANE_ARGUMENT)?;
    let scrollback = args.flag("scrollback", false)?;
    let lines = args
        .whole("lines", 1, 5000)?
        .or(scrollback.then_some(DEFAULT_SCROLLBACK_LINES));
    let pane = match ask(context, &Request::PaneInfo { pane: id.clone() })? {
        Response::Pane(pane) => pane,
        other => return Err(unexpected(other)),
    };
    let request = Request::PaneRead {
        pane: id,
        lines: lines.map(|lines| lines as u32),
        scrollback,
    };
    let raw = match ask(context, &request)? {
        Response::Text { text } => text,
        other => return Err(unexpected(other)),
    };
    let (body, truncated) = tidy(&raw);

    let chat = pane.kind == PaneKind::Thread;
    let mut text = format!(
        "{} ({}, {}) is {}.",
        pane.id,
        pane.harness,
        if chat { "native chat" } else { "terminal" },
        state(&pane)
    );
    match pane.activity {
        Activity::Working => text.push_str(" It is still working: this is not its final state."),
        Activity::Blocked => {
            text.push_str(" It waits for an answer: look for its question or permission request.")
        }
        Activity::Exited => text.push_str(" Its process has ended."),
        Activity::Idle => {}
    }
    text.push_str(match (chat, scrollback) {
        (true, _) => " Its latest answer:\n",
        (false, false) => " Its screen:\n",
        (false, true) => " Its last lines, scrollback included:\n",
    });
    if body.is_empty() {
        text.push_str(if chat {
            "(nothing: it has not answered yet)"
        } else {
            "(the screen is empty)"
        });
    } else {
        if truncated {
            let _ = writeln!(
                text,
                "[the start was cut to keep this under {MAX_READ_CHARS} characters]"
            );
        }
        text.push_str(&body);
    }
    Ok(Done {
        text,
        structured: json!({
            "pane": pane_json(&pane, context.env),
            "text": body,
            "truncated": truncated,
        }),
    })
}

/// A screen as a model wants it: no trailing blanks on a line, no blank lines
/// around it, and no more than [`MAX_READ_CHARS`], keeping the end.
fn tidy(raw: &str) -> (String, bool) {
    let lines: Vec<&str> = raw.lines().map(str::trim_end).collect();
    let start = lines
        .iter()
        .position(|line| !line.is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(start, |end| end + 1);
    let text = lines[start..end].join("\n");
    if text.chars().count() <= MAX_READ_CHARS {
        return (text, false);
    }
    let skip = text.chars().count() - MAX_READ_CHARS;
    let cut = text.char_indices().nth(skip).map_or(0, |(at, _)| at);
    // Start at a line boundary when there is one near.
    let cut = text[cut..]
        .find('\n')
        .map_or(cut, |newline| cut + newline + 1);
    (text[cut..].to_owned(), true)
}

fn send_to_pane(context: &Context, args: &Args) -> Run {
    let pane = args.required("pane", PANE_ARGUMENT)?;
    let text = args
        .raw("text")?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid("`text` is required: what to send".into()))?;
    let submit = args.flag("submit", true)?;
    not_yourself(context, &pane, "send to the panes you started")?;
    let request = Request::PaneSend {
        pane: pane.clone(),
        text,
        submit,
        from: context.env.pane.clone(),
    };
    match ask(context, &request)? {
        Response::Done => {}
        other => return Err(unexpected(other)),
    }
    let text = if submit {
        format!(
            "Sent to {pane}. Next: wait_for_panes with panes [\"{pane}\"], then read_pane \"{pane}\"."
        )
    } else {
        format!("Typed into {pane} without pressing Enter.")
    };
    Ok(Done {
        text,
        structured: json!({ "pane": pane, "submitted": submit }),
    })
}

fn list_panes(context: &Context, args: &Args) -> Run {
    let env = context.env;
    let all = args.flag("all", false)? || env.session.is_none();
    let session = if all { None } else { env.session.clone() };
    let spaces = match ask(context, &Request::SpaceList)? {
        Response::Spaces { spaces } => spaces,
        other => return Err(unexpected(other)),
    };
    let sessions = match ask(context, &Request::SessionList { space: None })? {
        Response::Sessions { sessions } => sessions,
        other => return Err(unexpected(other)),
    };
    let tabs = match ask(
        context,
        &Request::TabList {
            session: session.clone(),
        },
    )? {
        Response::Tabs { tabs } => tabs,
        other => return Err(unexpected(other)),
    };
    let panes = match ask(
        context,
        &Request::PaneList {
            session: session.clone(),
        },
    )? {
        Response::Panes { panes } => panes,
        other => return Err(unexpected(other)),
    };
    let shown = |id: &str| session.as_deref().is_none_or(|session| session == id);

    let mut text = String::new();
    let mut placed: Vec<&str> = Vec::new();
    let mut structured_spaces = Vec::new();
    for space in &spaces {
        let members: Vec<&kou_ipc::Session> = sessions
            .iter()
            .filter(|saved| saved.space == space.id && shown(&saved.id))
            .collect();
        if members.is_empty() && !all {
            continue;
        }
        let _ = writeln!(
            text,
            "Space \"{}\" ({}, {})",
            single_line(&space.name),
            space.id,
            space.path.display()
        );
        let mut structured_sessions = Vec::new();
        for saved in members {
            let mut marks = vec![saved.id.clone()];
            if saved.selected {
                marks.push("on screen".into());
            }
            if saved.archived {
                marks.push("archived".into());
            }
            if env.session.as_deref() == Some(saved.id.as_str()) {
                marks.push("yours".into());
            }
            let _ = writeln!(
                text,
                "  Session \"{}\" ({}, {})",
                single_line(&saved.name),
                marks.join(", "),
                saved.cwd.display()
            );
            let mut structured_tabs = Vec::new();
            for tab in tabs.iter().filter(|tab| tab.session == saved.id) {
                let mut marks = vec![tab.id.clone()];
                if tab.front {
                    marks.push("in front".into());
                }
                if tab.layout == kou_ipc::Layout::Canvas {
                    marks.push("canvas".into());
                }
                if let Some(icon) = &tab.icon {
                    marks.push(format!("icon {icon}"));
                }
                let _ = writeln!(
                    text,
                    "    Tab \"{}\" ({})",
                    single_line(&tab.name),
                    marks.join(", ")
                );
                let mut structured_panes = Vec::new();
                for id in &tab.panes {
                    let Some(pane) = panes.iter().find(|pane| &pane.id == id) else {
                        continue;
                    };
                    placed.push(&pane.id);
                    let mut line = pane_line(pane, env);
                    if pane.cwd != saved.cwd {
                        let _ = write!(line, ", in {}", pane.cwd.display());
                    }
                    let _ = writeln!(text, "      {line}");
                    structured_panes.push(pane_json(pane, env));
                }
                let mut entry = json!({
                    "id": tab.id,
                    "name": tab.name,
                    "layout": match tab.layout { kou_ipc::Layout::Panes => "panes", kou_ipc::Layout::Canvas => "canvas" },
                    "front": tab.front,
                    "panes": structured_panes,
                });
                if let Some(icon) = &tab.icon {
                    entry["icon"] = json!(icon);
                }
                structured_tabs.push(entry);
            }
            structured_sessions.push(json!({
                "id": saved.id,
                "name": saved.name,
                "cwd": saved.cwd.display().to_string(),
                "selected": saved.selected,
                "archived": saved.archived,
                "tabs": structured_tabs,
            }));
        }
        structured_spaces.push(json!({
            "id": space.id,
            "name": space.name,
            "path": space.path.display().to_string(),
            "sessions": structured_sessions,
        }));
    }
    let loose: Vec<&Pane> = panes
        .iter()
        .filter(|pane| !placed.contains(&pane.id.as_str()))
        .collect();
    if !loose.is_empty() {
        text.push_str("Panes in no tab:\n");
        for pane in loose {
            let _ = writeln!(text, "  {}", pane_line(pane, env));
        }
    }
    if text.is_empty() {
        text.push_str("No panes.");
    }
    let mut structured = json!({ "spaces": structured_spaces });
    if let Some(you) = &env.pane {
        structured["you"] = json!(you);
    }
    Ok(Done {
        text: text.trim_end().to_owned(),
        structured,
    })
}

fn describe_pane(context: &Context, args: &Args) -> Run {
    let env = context.env;
    let id = match args.text("pane")? {
        Some(pane) => pane,
        None => own_pane(context, "Name a pane")?,
    };
    let pane = match ask(context, &Request::PaneInfo { pane: id })? {
        Response::Pane(pane) => pane,
        other => return Err(unexpected(other)),
    };
    let yours = env.pane.as_deref() == Some(pane.id.as_str());
    let mut text = format!(
        "{}{}: {} in a {}, {}.",
        pane.id,
        if yours { " (this is you)" } else { "" },
        pane.harness,
        if pane.kind == PaneKind::Thread {
            "native chat"
        } else {
            "terminal"
        },
        state(&pane)
    );
    let title = single_line(&pane.title);
    if !title.is_empty() {
        let _ = write!(text, " Title \"{title}\".");
    }
    let _ = write!(
        text,
        " Directory {}. Session {}.",
        pane.cwd.display(),
        pane.session
    );
    match (&pane.parent, yours) {
        (Some(parent), true) => {
            let _ = write!(text, " Started by {parent}: another agent started you.");
        }
        (Some(parent), false) if env.pane.as_deref() == Some(parent.as_str()) => {
            text.push_str(" Started by you.");
        }
        (Some(parent), false) => {
            let _ = write!(text, " Started by {parent}.");
        }
        (None, true) => text.push_str(" No agent started you: the user did."),
        (None, false) => {}
    }
    Ok(Done {
        text,
        structured: json!({ "pane": pane_json(&pane, env) }),
    })
}

fn close_pane(context: &Context, args: &Args) -> Run {
    let pane = args.required("pane", PANE_ARGUMENT)?;
    not_yourself(
        context,
        &pane,
        "closing it would end you. Ask the user if it should be closed",
    )?;
    match ask(context, &Request::PaneClose { pane: pane.clone() })? {
        Response::Done => {}
        other => return Err(unexpected(other)),
    }
    Ok(Done {
        text: format!("Closed {pane}."),
        structured: json!({ "pane": pane, "closed": true }),
    })
}

fn focus_pane(context: &Context, args: &Args) -> Run {
    let pane = args.required("pane", PANE_ARGUMENT)?;
    match ask(context, &Request::PaneFocus { pane: pane.clone() })? {
        Response::Done => {}
        other => return Err(unexpected(other)),
    }
    Ok(Done {
        text: format!("{pane} is now in front of the user."),
        structured: json!({ "pane": pane }),
    })
}

fn name_tab(context: &Context, args: &Args) -> Run {
    // An empty name is a request, not a mistake: back to the default name.
    let name = args.raw("name")?.map(|name| name.trim().to_owned());
    let icon = args.text("icon")?;
    let tab = args.text("tab")?;
    if name.is_none() && icon.is_none() {
        return Err(invalid("give the tab a `name`, an `icon`, or both".into()));
    }
    if tab.is_none() {
        own_pane(context, "Name the tab with `tab`")?;
    }
    let request = Request::TabSet {
        tab: tab.clone(),
        pane: context.env.pane.clone(),
        name,
        icon,
    };
    let tab = match ask(context, &request)? {
        Response::Tab(tab) => tab,
        other => return Err(unexpected(other)),
    };
    let mut text = format!(
        "Tab {} is now called \"{}\"",
        tab.id,
        single_line(&tab.name)
    );
    match &tab.icon {
        Some(icon) => {
            let _ = write!(text, ", with the {icon} icon.");
        }
        None => text.push('.'),
    }
    let mut structured = json!({ "id": tab.id, "name": tab.name, "panes": tab.panes });
    if let Some(icon) = &tab.icon {
        structured["icon"] = json!(icon);
    }
    Ok(Done {
        text,
        structured: json!({ "tab": structured }),
    })
}

fn rename(context: &Context, args: &Args) -> Run {
    let target = args.choice("target", &["session", "space"], "")?;
    if target.is_empty() {
        return Err(invalid("`target` is required: session or space".into()));
    }
    let name = args.required("name", "the new name")?;
    let id = args.text("id")?;
    let env = context.env;
    let (request, fallback) = if target == "session" {
        let session = id.or_else(|| env.session.clone());
        if session.is_none() {
            own_pane(context, "Name the session with `id`")?;
        }
        let fallback = session.clone();
        let request = Request::SessionRename {
            session,
            pane: env.pane.clone(),
            name: name.clone(),
        };
        (request, fallback)
    } else {
        if id.is_none() {
            own_pane(context, "Name the space with `id`")?;
        }
        let request = Request::SpaceRename {
            space: id.clone(),
            pane: env.pane.clone(),
            name: name.clone(),
        };
        (request, id)
    };
    let (id, name) = match ask(context, &request)? {
        Response::Session(session) => (session.id, session.name),
        Response::Space(space) => (space.id, space.name),
        Response::Done => (fallback.unwrap_or_default(), name),
        other => return Err(unexpected(other)),
    };
    Ok(Done {
        text: format!(
            "The {target} {id} is now called \"{}\".",
            single_line(&name)
        ),
        structured: json!({ "target": target, "id": id, "name": name }),
    })
}
