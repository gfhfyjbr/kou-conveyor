//! A scripted Agent Client Protocol agent, for kou-acp's integration tests.
//!
//! It speaks protocol version 1 over standard input and output with nothing
//! but blocking reads and writes, and calls no model. Flags on the command line
//! choose how it starts; the first word of a prompt chooses what the turn does.
//! The wire shapes are the ones in the specification at
//! <https://agentclientprotocol.com/protocol/v1/schema>, written out as JSON
//! here rather than through the schema crate, so that the client's use of that
//! crate is tested against something independent of it.
//!
//! Flags:
//!
//! - `--auth`: `session/new` answers "authentication required".
//! - `--protocol <n>`: `initialize` answers with this protocol version.
//! - `--no-images`, `--no-embedded`: prompt capabilities left out.
//! - `--load`: `session/load` is offered and replays a short conversation.
//! - `--resume-only`: only `session/resume` is offered.
//! - `--legacy-modes`: modes come in the `modes` field, with `session/set_mode`.
//! - `--no-selectors`: neither modes nor config options.
//! - `--mcp-http`: MCP servers over HTTP are advertised as taken.
//! - `--instructions`: `sessionCapabilities.systemPrompt` is advertised.
//! - `--list`: `session/list` is offered, and answers in two pages.
//! - `--fork`: `session/fork` and `session/close` are offered. The agent keeps
//!   each session's transcript (the user's prompts and the ids of its own
//!   messages, which its answers then carry), forks up to the message that
//!   `_meta.jetbrains.air.fork` names (version 1, `messageId`) as the Claude
//!   and Codex adapters do, and a load replays a second exchange.
//! - `--refuse-fork`: with `--fork`, `session/fork` fails.
//! - `--crash-on-start`: complain on standard error and exit before answering.
//! - `--linger`: keep running when standard input ends.

use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, BufRead, Write},
    process, thread,
    time::Duration,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let protocol = args
        .iter()
        .position(|arg| arg == "--protocol")
        .and_then(|index| args.get(index + 1))
        .and_then(|version| version.parse().ok())
        .unwrap_or(1);
    if flag("--crash-on-start") {
        eprintln!("fake agent: cannot find its configuration");
        eprintln!("fake agent: giving up");
        process::exit(2);
    }
    let mut agent = Agent {
        input: io::stdin().lock().lines(),
        backlog: VecDeque::new(),
        next_id: 100,
        session: String::new(),
        cwd: String::new(),
        initialize: Value::Null,
        opened: Value::Null,
        mode: "ask".into(),
        model: "model-1".into(),
        early_prompts: 0,
        opened_sessions: 0,
        forks: 0,
        messages: 0,
        transcripts: HashMap::new(),
        closed: Vec::new(),
        flags: Flags {
            auth: flag("--auth"),
            protocol,
            images: !flag("--no-images"),
            embedded: !flag("--no-embedded"),
            load: flag("--load"),
            resume_only: flag("--resume-only"),
            legacy_modes: flag("--legacy-modes"),
            no_selectors: flag("--no-selectors"),
            mcp_http: flag("--mcp-http"),
            instructions: flag("--instructions"),
            list: flag("--list"),
            fork: flag("--fork"),
            refuse_fork: flag("--refuse-fork"),
        },
    };
    while let Some(message) = agent.read() {
        agent.dispatch(message);
    }
    if flag("--linger") {
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
}

struct Flags {
    auth: bool,
    protocol: u64,
    images: bool,
    embedded: bool,
    load: bool,
    resume_only: bool,
    legacy_modes: bool,
    no_selectors: bool,
    mcp_http: bool,
    instructions: bool,
    list: bool,
    fork: bool,
    refuse_fork: bool,
}

struct Agent {
    input: io::Lines<io::StdinLock<'static>>,
    /// Messages read while waiting for something else.
    backlog: VecDeque<Value>,
    next_id: u64,
    session: String,
    cwd: String,
    /// The parameters `initialize` came with, for the `inspect` prompt.
    initialize: Value,
    /// The method and parameters the session was opened with.
    opened: Value,
    mode: String,
    model: String,
    /// Prompts that arrived while a turn was still running.
    early_prompts: u64,
    /// Sessions `session/new` made.
    opened_sessions: u64,
    /// Sessions `session/fork` made.
    forks: u64,
    /// Messages of the agent's that carried an id.
    messages: u64,
    /// With `--fork`, each session's transcript: `> ` and the text of a
    /// prompt, or the id of a message of the agent's.
    transcripts: HashMap<String, Vec<String>>,
    /// Sessions `session/close` let go of.
    closed: Vec<String>,
    flags: Flags,
}

impl Agent {
    // Plumbing.

    fn read(&mut self) -> Option<Value> {
        self.backlog.pop_front().or_else(|| self.read_input())
    }

    fn read_input(&mut self) -> Option<Value> {
        loop {
            let line = self.input.next()?.ok()?;
            if let Ok(message) = serde_json::from_str(&line) {
                return Some(message);
            }
        }
    }

    /// Read until `wanted` picks a message, keeping the others for later.
    fn wait_for(&mut self, mut wanted: impl FnMut(&Value) -> bool) -> Value {
        if let Some(index) = self.backlog.iter().position(&mut wanted) {
            return self.backlog.remove(index).expect("an index just found");
        }
        loop {
            let Some(message) = self.read_input() else {
                // The client hung up in the middle of a scenario.
                process::exit(0);
            };
            if wanted(&message) {
                return message;
            }
            if message["method"] == "session/prompt" {
                self.early_prompts += 1;
            }
            self.backlog.push_back(message);
        }
    }

    fn raw(&mut self, line: &str) {
        let mut out = io::stdout().lock();
        writeln!(out, "{line}").and_then(|()| out.flush()).ok();
    }

    fn send(&mut self, message: Value) {
        self.raw(&message.to_string());
    }

    fn respond(&mut self, id: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn fail(&mut self, id: &Value, code: i64, message: &str, data: Value) {
        let mut error = json!({"code": code, "message": message});
        if !data.is_null() {
            error["data"] = data;
        }
        self.send(json!({"jsonrpc": "2.0", "id": id, "error": error}));
    }

    fn update(&mut self, update: Value) {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"sessionId": self.session, "update": update},
        }));
    }

    fn say(&mut self, text: &str) {
        let mut update = json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": text},
        });
        if self.flags.fork {
            self.messages += 1;
            let id = format!("a-{}", self.messages);
            update["messageId"] = json!(id);
            self.record(id);
        }
        self.update(update);
    }

    /// Add to the transcript of the session in use.
    fn record(&mut self, entry: String) {
        self.transcripts
            .entry(self.session.clone())
            .or_default()
            .push(entry);
    }

    /// Send a request to the client without waiting for the answer.
    fn ask(&mut self, method: &str, params: Value) -> Value {
        let id = json!(self.next_id);
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    /// The answer to a request sent with [`Self::ask`]: the result, or the error.
    fn answer(&mut self, id: &Value) -> Result<Value, Value> {
        let message =
            self.wait_for(|message| message["id"] == *id && message.get("method").is_none());
        match message.get("error") {
            Some(error) => Err(error.clone()),
            None => Ok(message["result"].clone()),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self.ask(method, params);
        self.answer(&id)
    }

    fn in_cwd(&self, name: &str) -> String {
        format!("{}/{name}", self.cwd)
    }

    // Requests from the client.

    fn dispatch(&mut self, message: Value) {
        let id = message["id"].clone();
        let params = message["params"].clone();
        match message["method"].as_str() {
            Some("initialize") => self.initialize(&id, params),
            Some("session/list") if self.flags.list => self.list_sessions(&id, &params),
            Some("session/new") => self.new_session(&id, params),
            Some("session/load") => self.load_session(&id, params),
            Some("session/resume") => {
                self.open("session/resume", &params);
                let state = self.session_state();
                self.respond(&id, state);
            }
            Some("session/fork") if self.flags.fork => self.fork_session(&id, params),
            Some("session/close") if self.flags.fork => {
                let session = params["sessionId"].as_str().unwrap_or_default();
                self.closed.push(session.to_owned());
                self.respond(&id, json!({}));
            }
            Some("session/prompt") => self.prompt(&id, &params),
            Some("session/set_mode") => {
                self.mode = params["modeId"].as_str().unwrap_or_default().to_owned();
                self.respond(&id, json!({}));
            }
            Some("session/set_config_option") => self.set_config_option(&id, &params),
            // A cancellation with no turn running, and whatever else is not a request.
            Some(_) if id.is_null() => {}
            Some(_) => self.fail(&id, -32601, "Method not found", Value::Null),
            None => {}
        }
    }

    fn initialize(&mut self, id: &Value, params: Value) {
        let terminal_login = params["clientCapabilities"]["auth"]["terminal"] == true;
        self.initialize = params;
        let mut auth_methods = vec![json!({
            "id": "fake-login",
            "name": "Log in with Fake",
            "description": "Opens the browser",
        })];
        if terminal_login {
            auth_methods.push(json!({
                "id": "fake-terminal",
                "name": "Terminal login",
                "type": "terminal",
                "args": ["--login"],
                "env": {"FAKE_LOGIN": "1"},
            }));
        }
        let mut sessions = json!({});
        if self.flags.resume_only {
            sessions["resume"] = json!({});
        }
        if self.flags.instructions {
            sessions["systemPrompt"] = json!({});
        }
        if self.flags.list {
            sessions["list"] = json!({});
        }
        if self.flags.fork {
            sessions["fork"] = json!({});
            sessions["close"] = json!({});
        }
        self.respond(
            id,
            json!({
                "protocolVersion": self.flags.protocol,
                "agentCapabilities": {
                    "loadSession": self.flags.load,
                    "promptCapabilities": {
                        "image": self.flags.images,
                        "embeddedContext": self.flags.embedded,
                    },
                    "sessionCapabilities": sessions,
                    "mcpCapabilities": {"http": self.flags.mcp_http, "sse": false},
                },
                "agentInfo": {"name": "fake", "title": "Fake Agent", "version": "1.0.0"},
                "authMethods": if self.flags.auth { json!(auth_methods) } else { json!([]) },
            }),
        );
    }

    // Two pages: the older sessions first, to see that a client sorts them.
    fn list_sessions(&mut self, id: &Value, params: &Value) {
        let cwd = params["cwd"].as_str().unwrap_or_default();
        let session = |id: &str, title: Value, updated: Value| json!({"sessionId": id, "cwd": cwd, "title": title, "updatedAt": updated});
        if params["cursor"].is_null() {
            self.respond(
                id,
                json!({
                    "sessions": [
                        session("sess-old", json!("Fix the parser"), json!("2026-11-10T08:00:00Z")),
                        session("sess-untimed", json!("  "), Value::Null),
                    ],
                    "nextCursor": "page-2",
                }),
            );
        } else {
            self.respond(
                id,
                json!({
                    "sessions": [
                        session("sess-new", json!("Review the diff"), json!("2026-11-12T12:06:23.336Z")),
                    ],
                }),
            );
        }
    }

    fn open(&mut self, method: &str, params: &Value) {
        self.cwd = params["cwd"].as_str().unwrap_or_default().to_owned();
        if let Some(session) = params["sessionId"].as_str() {
            self.session = session.to_owned();
        }
        self.opened = json!({"method": method, "params": params});
    }

    fn new_session(&mut self, id: &Value, params: Value) {
        if self.flags.auth {
            return self.fail(id, -32000, "Authentication required", Value::Null);
        }
        self.open("session/new", &params);
        self.opened_sessions += 1;
        self.session = format!("sess-{}", self.opened_sessions);
        // A new session starts as every one does.
        self.mode = "ask".into();
        self.model = "model-1".into();
        self.announce_commands();
        let mut result = self.session_state();
        result["sessionId"] = json!(self.session);
        self.respond(id, result);
    }

    /// The session's commands, sent before the answer that names the
    /// session: a client must hold on to updates that reach it that early.
    fn announce_commands(&mut self) {
        self.update(json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [
                {"name": "web", "description": "Search the web", "input": {"hint": "query"}},
                {"name": "test", "description": "Run the tests"},
            ],
        }));
    }

    /// A copy of a session up to the agent's message the fork point names,
    /// or of all of it, which then takes prompts.
    fn fork_session(&mut self, id: &Value, params: Value) {
        if self.flags.refuse_fork {
            return self.fail(
                id,
                -32603,
                "Internal error",
                json!({"details": "forking is broken"}),
            );
        }
        let source = params["sessionId"].as_str().unwrap_or_default().to_owned();
        let mut kept = self.transcripts.get(&source).cloned().unwrap_or_default();
        let point = &params["_meta"]["jetbrains"]["air"]["fork"];
        if point["version"] == 1
            && let Some(message) = point["messageId"].as_str()
        {
            let Some(at) = kept.iter().position(|entry| entry == message) else {
                let reason =
                    format!("Fork point message {message} was not found in session {source}");
                return self.fail(id, -32602, "Invalid params", json!(reason));
            };
            kept.truncate(at + 1);
        }
        self.open("session/fork", &params);
        self.forks += 1;
        self.session = format!("fork-{}", self.forks);
        self.transcripts.insert(self.session.clone(), kept);
        self.announce_commands();
        let mut result = self.session_state();
        result["sessionId"] = json!(self.session);
        self.respond(id, result);
    }

    fn load_session(&mut self, id: &Value, params: Value) {
        if params["sessionId"] == "gone" {
            return self.fail(
                id,
                -32002,
                "Resource not found",
                json!({"sessionId": "gone"}),
            );
        }
        self.open("session/load", &params);
        let notes = self.in_cwd("notes.txt");
        self.update(json!({
            "sessionUpdate": "user_message_chunk",
            "messageId": "u1",
            "content": {"type": "text", "text": "What is in "},
        }));
        self.update(json!({
            "sessionUpdate": "user_message_chunk",
            "messageId": "u1",
            "content": {"type": "resource_link", "uri": format!("file://{notes}"), "name": "notes.txt"},
        }));
        self.update(json!({
            "sessionUpdate": "agent_thought_chunk",
            "messageId": "t1",
            "content": {"type": "text", "text": "Reading."},
        }));
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "old-1",
            "title": "Read notes.txt",
            "kind": "read",
            "status": "completed",
            "locations": [{"path": notes}],
            "content": [{"type": "content", "content": {"type": "text", "text": "hello"}}],
        }));
        self.update(json!({
            "sessionUpdate": "agent_message_chunk",
            "messageId": "a1",
            "content": {"type": "text", "text": "It says hello."},
        }));
        if self.flags.fork {
            self.update(json!({
                "sessionUpdate": "user_message_chunk",
                "messageId": "u2",
                "content": {"type": "text", "text": "And now?"},
            }));
            self.update(json!({
                "sessionUpdate": "agent_message_chunk",
                "messageId": "a2",
                "content": {"type": "text", "text": "Nothing more."},
            }));
            let transcript = ["> What is in ", "t1", "a1", "> And now?", "a2"];
            self.transcripts
                .insert(self.session.clone(), transcript.map(str::to_owned).to_vec());
        }
        let state = self.session_state();
        self.respond(id, state);
    }

    /// The modes and models, in the shape the flags ask for.
    fn session_state(&self) -> Value {
        if self.flags.no_selectors {
            return json!({});
        }
        if self.flags.legacy_modes {
            return json!({"modes": {
                "currentModeId": self.mode,
                "availableModes": [
                    {"id": "ask", "name": "Ask", "description": "Ask before changing anything"},
                    {"id": "code", "name": "Code"},
                ],
            }});
        }
        json!({"configOptions": self.config_options()})
    }

    fn config_options(&self) -> Value {
        json!([
            {"id": "mode", "name": "Mode", "category": "mode", "type": "select",
             "currentValue": self.mode,
             "options": [
                {"value": "ask", "name": "Ask", "description": "Ask before changing anything"},
                {"value": "code", "name": "Code"}]},
            {"id": "model", "name": "Model", "category": "model", "type": "select",
             "currentValue": self.model,
             "options": [
                {"group": "small", "name": "Small", "options": [
                    {"value": "model-1", "name": "Model 1", "description": "The fast one"}]},
                {"group": "large", "name": "Large", "options": [
                    {"value": "model-2", "name": "Model 2"}]}]},
            {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
             "currentValue": "low",
             "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]},
        ])
    }

    fn set_config_option(&mut self, id: &Value, params: &Value) {
        let value = params["value"].as_str().unwrap_or_default().to_owned();
        match (params["configId"].as_str(), value.as_str()) {
            (Some("mode"), "ask" | "code") => self.mode = value,
            (Some("model"), "model-1") => self.model = value,
            // The large model works only in code mode: one choice moves another.
            (Some("model"), "model-2") => {
                self.model = value;
                self.mode = "code".into();
            }
            _ => return self.fail(id, -32602, "Invalid params", json!("no such value")),
        }
        let options = self.config_options();
        self.respond(id, json!({"configOptions": options}));
    }

    // Turns.

    fn prompt(&mut self, id: &Value, params: &Value) {
        if let Some(session) = params["sessionId"].as_str() {
            self.session = session.to_owned();
        }
        let blocks = params["prompt"].as_array().cloned().unwrap_or_default();
        let text = blocks
            .iter()
            .find_map(|block| block["text"].as_str())
            .unwrap_or_default()
            .to_owned();
        // What the session held before this prompt.
        let held = self
            .transcripts
            .get(&self.session)
            .cloned()
            .unwrap_or_default();
        if self.flags.fork {
            self.record(format!("> {text}"));
        }
        let stop = match text.split_whitespace().next().unwrap_or_default() {
            "history" => {
                self.say(&json!(held).to_string());
                "end_turn"
            }
            "chunks" => self.chunks(),
            "tool" => self.tool(),
            "subagent" => self.subagent(),
            "detach" => {
                self.detach();
                // The turn is over; what was left running ends later, and
                // the agent goes on by itself from there.
                self.respond(id, json!({"stopReason": "end_turn"}));
                return self.woken();
            }
            "wake" => {
                self.respond(id, json!({"stopReason": "end_turn"}));
                return self.say("Something came up.");
            }
            "permission" => self.permission(),
            "files" => self.files(),
            "terminal" => self.terminal(),
            "live-terminal" => self.live_terminal(),
            "hang" => self.hang(),
            "plan" => self.plan(),
            "mode" => self.switch_mode(),
            "cancel" => self.cancelled(),
            "crash" => self.crash(),
            "malformed" => self.malformed(),
            "echo" => self.echo(&text),
            "takeback" => self.take_back(),
            "pid" => {
                self.say(&process::id().to_string());
                "end_turn"
            }
            "inspect" => {
                let seen = json!({
                    "initialize": self.initialize,
                    "opened": self.opened,
                    "prompt": blocks,
                    "session": self.session,
                    "mode": self.mode,
                    "model": self.model,
                    "closed": self.closed,
                });
                self.say(&seen.to_string());
                "end_turn"
            }
            "meta" => {
                self.meta();
                return self.respond(
                    id,
                    json!({
                        "stopReason": "end_turn",
                        "usage": {"totalTokens": 30, "inputTokens": 10, "outputTokens": 20},
                    }),
                );
            }
            "error" => {
                return self.fail(
                    id,
                    -32603,
                    "Internal error",
                    json!({"details": "the model is overloaded"}),
                );
            }
            "login" => return self.fail(id, -32000, "Authentication required", Value::Null),
            "refuse" => "refusal",
            "limit" => "max_tokens",
            "turns" => "max_turn_requests",
            "weird" => "exploded",
            _ => {
                self.say(&format!("You said: {text}"));
                "end_turn"
            }
        };
        self.respond(id, json!({"stopReason": stop}));
    }

    fn chunks(&mut self) -> &'static str {
        for text in ["Let me ", "think."] {
            self.update(json!({
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": text},
            }));
        }
        for (message, text) in [("m1", "Hel"), ("m1", "lo."), ("m2", "Second.")] {
            self.update(json!({
                "sessionUpdate": "agent_message_chunk",
                "messageId": message,
                "content": {"type": "text", "text": text},
            }));
        }
        "end_turn"
    }

    /// Two commands as Claude's adapter reports them to a client that shows
    /// its commands as terminals: one that runs and exits, and one that is
    /// left to run in the background, whose call is over without an exit.
    fn detach(&mut self) {
        let bash = json!({"claudeCode": {"toolName": "Bash"}});
        for (id, command) in [("c1", "ls"), ("b1", "npm run dev")] {
            self.update(json!({
                "sessionUpdate": "tool_call",
                "toolCallId": id,
                "title": format!("`{command}`"),
                "kind": "execute",
                "status": "pending",
                "content": [{"type": "terminal", "terminalId": id}],
                "rawInput": {"command": command},
                "_meta": {"claudeCode": {"toolName": "Bash"}, "terminal_info": {"terminal_id": id}},
            }));
        }
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "c1",
            "_meta": {"terminal_output": {"terminal_id": "c1", "data": "notes.txt\n"}},
        }));
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "c1",
            "status": "completed",
            "content": [{"type": "terminal", "terminalId": "c1"}],
            "_meta": {
                "claudeCode": {"toolName": "Bash"},
                "terminal_exit": {"terminal_id": "c1", "exit_code": 0, "signal": null},
            },
        }));
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "b1",
            "_meta": {"terminal_output": {
                "terminal_id": "b1",
                "data": "Command running in background with ID: bash_1",
            }},
        }));
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "b1",
            "status": "completed",
            "content": [{"type": "terminal", "terminalId": "b1"}],
            "_meta": bash,
        }));
        self.say("Started.");
    }

    /// With no prompt open: the command left running ends, and the agent
    /// works on what came of it, a cycle of its own that ends where its
    /// cost is said.
    fn woken(&mut self) {
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "b1",
            "_meta": {"terminal_exit": {"terminal_id": "b1", "exit_code": null, "signal": null}},
        }));
        self.say("The server stopped.");
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "r1",
            "title": "Read server.log",
            "kind": "read",
            "status": "completed",
        }));
        // In the middle of a cycle no cost is said.
        self.update(json!({"sessionUpdate": "usage_update", "used": 61000, "size": 200000}));
        self.say(" It ran out of memory.");
        self.update(json!({
            "sessionUpdate": "usage_update",
            "used": 62000,
            "size": 200000,
            "cost": {"amount": 0.2, "currency": "USD"},
            "_meta": {"_claude/origin": {"kind": "task-notification"}},
        }));
    }

    /// A subagent of the agent's own, as Claude's adapter reports one to a
    /// client that asked for its words (`forwardSubagentText`): the call
    /// that starts it names its tool, and everything the subagent is given,
    /// thinks, says and does carries that call's id.
    fn subagent(&mut self) -> &'static str {
        let asked = "Run the parser's tests.";
        let own = json!({"claudeCode": {"toolName": "Agent"}});
        let its = |tool: Option<&str>| match tool {
            Some(tool) => json!({"claudeCode": {"toolName": tool, "parentToolUseId": "a1"}}),
            None => json!({"claudeCode": {"parentToolUseId": "a1"}}),
        };
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "a1",
            "title": "Check the tests",
            "kind": "think",
            "status": "pending",
            "content": [{"type": "content", "content": {"type": "text", "text": asked}}],
            "rawInput": {
                "description": "Check the tests", "prompt": asked, "subagent_type": "Explore",
            },
            "_meta": own,
        }));
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "a1",
            "status": "in_progress",
            "_meta": own,
        }));
        self.update(json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": asked},
            "_meta": its(None),
        }));
        self.update(json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "Where are the tests?"},
            "_meta": its(None),
        }));
        for text in ["I will read ", "the parser."] {
            self.update(json!({
                "sessionUpdate": "agent_message_chunk",
                "messageId": "s1",
                "content": {"type": "text", "text": text},
                "_meta": its(None),
            }));
        }
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "c1",
            "title": "Read src/parser.rs",
            "kind": "read",
            "status": "in_progress",
            "_meta": its(Some("Read")),
        }));
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "c1",
            "status": "completed",
            "_meta": its(Some("Read")),
        }));
        self.update(json!({
            "sessionUpdate": "agent_message_chunk",
            "messageId": "s2",
            "content": {"type": "text", "text": "All 12 pass."},
            "_meta": its(None),
        }));
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "a1",
            "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "All 12 pass."}}],
            "_meta": {"claudeCode": {
                "toolName": "Agent",
                "toolResponse": {"status": "completed", "totalTokens": 48211},
            }},
        }));
        self.say("They pass.");
        "end_turn"
    }

    fn tool(&mut self) -> &'static str {
        let path = self.in_cwd("notes.txt");
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-1",
            "title": "Edit notes.txt",
            "name": "edit_file",
            "kind": "edit",
            "status": "pending",
            "locations": [{"path": path, "line": 3}],
            "rawInput": {"path": "notes.txt"},
        }));
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-1",
            "status": "in_progress",
        }));
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-1",
            "status": "completed",
            "content": [
                {"type": "diff", "path": path, "oldText": "old", "newText": "new"},
                {"type": "diff", "path": self.in_cwd("created.txt"), "newText": "fresh"},
                {"type": "content", "content": {"type": "text", "text": "1 line changed"}},
            ],
            "rawOutput": {"ok": true},
        }));
        self.say("Edited.");
        "end_turn"
    }

    fn permission(&mut self) -> &'static str {
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-rm",
            "title": "Run rm -rf build",
            "kind": "execute",
            "status": "pending",
            "rawInput": {"command": "rm -rf build"},
        }));
        self.early_prompts = 0;
        // Only the id, as in the specification's example: the client must
        // know the rest from the announcement.
        let outcome = self
            .call(
                "session/request_permission",
                json!({
                    "sessionId": self.session,
                    "toolCall": {"toolCallId": "call-rm"},
                    "options": [
                        {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                        {"optionId": "always", "name": "Always allow rm", "kind": "allow_always"},
                        {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
                    ],
                }),
            )
            .map(|result| result["outcome"].clone())
            .unwrap_or_default();
        let chosen = outcome["optionId"].as_str().unwrap_or_default().to_owned();
        let allowed = outcome["outcome"] == "selected" && chosen != "reject";
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-rm",
            "status": if allowed { "completed" } else { "failed" },
        }));
        self.say(&format!(
            "{}: {chosen}; prompts received meanwhile: {}",
            outcome["outcome"].as_str().unwrap_or("no outcome"),
            self.early_prompts
        ));
        "end_turn"
    }

    fn files(&mut self) -> &'static str {
        let path = self.in_cwd("made/by/agent.txt");
        let written = self.call(
            "fs/write_text_file",
            json!({"sessionId": self.session, "path": path, "content": "alpha\nbeta\ngamma\n"}),
        );
        let window = self.call(
            "fs/read_text_file",
            json!({"sessionId": self.session, "path": path, "line": 2, "limit": 1}),
        );
        let whole = self.call(
            "fs/read_text_file",
            json!({"sessionId": self.session, "path": path}),
        );
        let missing = self.call(
            "fs/read_text_file",
            json!({"sessionId": self.session, "path": self.in_cwd("nothing.txt")}),
        );
        self.say(
            &json!({
                "written": written.ok(),
                "window": window.ok().map(|result| result["content"].clone()),
                "whole": whole.ok().map(|result| result["content"].clone()),
                "missing": missing.err().map(|error| error["code"].clone()),
            })
            .to_string(),
        );
        "end_turn"
    }

    fn create_terminal(&mut self, script: &str, limit: Option<u64>) -> String {
        let mut params = json!({
            "sessionId": self.session,
            "command": "/bin/sh",
            "args": ["-c", script],
            "env": [{"name": "FAKE_VARIABLE", "value": "from the agent"}],
        });
        if let Some(limit) = limit {
            params["outputByteLimit"] = json!(limit);
        }
        let created = self.call("terminal/create", params).unwrap_or_default();
        created["terminalId"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    /// A command that is over before the thread hears of it, so that what the
    /// thread is told does not depend on timing.
    fn terminal(&mut self) -> &'static str {
        let terminal = self.create_terminal(
            "printf '%s\\n' \"$FAKE_VARIABLE\"; pwd; printf 0123456789 >&2; exit 3",
            Some(24),
        );
        let ids = json!({"sessionId": self.session, "terminalId": terminal});
        let exit = self.call("terminal/wait_for_exit", ids.clone());
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-sh",
            "title": "Run the script",
            "kind": "execute",
            "status": "in_progress",
            "content": [{"type": "terminal", "terminalId": terminal}],
        }));
        let output = self.call("terminal/output", ids.clone());
        let killed = self.call("terminal/kill", ids.clone());
        let released = self.call("terminal/release", ids.clone());
        let after = self.call("terminal/output", ids);
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-sh",
            "status": "failed",
        }));
        self.say(
            &json!({
                "exit": exit.ok(),
                "output": output.ok(),
                "killed": killed.ok(),
                "released": released.ok(),
                "after": after.err().map(|error| error["code"].clone()),
            })
            .to_string(),
        );
        "end_turn"
    }

    /// A command the thread watches while it runs.
    fn live_terminal(&mut self) -> &'static str {
        // The command goes on only once the tool call showing it is out, so
        // that the thread always sees it running first, whatever the timing.
        let terminal = self.create_terminal(
            "echo ready; while [ ! -e go ]; do sleep 0.02; done; echo done",
            None,
        );
        let ids = json!({"sessionId": self.session, "terminalId": terminal});
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-live",
            "title": "Run slowly",
            "kind": "execute",
            "status": "in_progress",
            "content": [{"type": "terminal", "terminalId": terminal}],
        }));
        self.call(
            "fs/write_text_file",
            json!({"sessionId": self.session, "path": self.in_cwd("go"), "content": ""}),
        )
        .ok();
        let exit = self.call("terminal/wait_for_exit", ids.clone());
        self.update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-live",
            "status": "completed",
        }));
        self.call("terminal/release", ids).ok();
        self.say(&json!({"exit": exit.ok()}).to_string());
        "end_turn"
    }

    /// A turn that never ends by itself, with a command that never does either.
    fn hang(&mut self) -> &'static str {
        let terminal = self.create_terminal("echo $$; exec sleep 600", None);
        let ids = json!({"sessionId": self.session, "terminalId": terminal});
        // Wait for the command to say who it is before showing it.
        let mut command = String::new();
        while !command.ends_with('\n') {
            thread::sleep(Duration::from_millis(10));
            let output = self
                .call("terminal/output", ids.clone())
                .unwrap_or_default();
            command = output["output"].as_str().unwrap_or_default().to_owned();
        }
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-hang",
            "title": "Sleep",
            "kind": "execute",
            "status": "in_progress",
            "content": [{"type": "terminal", "terminalId": terminal}],
        }));
        self.say(&format!(
            "agent {} command {}",
            process::id(),
            command.trim()
        ));
        self.wait_for(|message| message["method"] == "session/cancel");
        "cancelled"
    }

    fn plan(&mut self) -> &'static str {
        self.update(json!({
            "sessionUpdate": "plan",
            "entries": [
                {"content": "Read the code", "priority": "high", "status": "in_progress"},
                {"content": "Change it", "priority": "medium", "status": "pending"},
            ],
        }));
        self.say("Working.");
        self.update(json!({
            "sessionUpdate": "plan",
            "entries": [
                {"content": "Read the code", "priority": "high", "status": "completed"},
                {"content": "Change it", "priority": "medium", "status": "in_progress"},
                {"content": "Test it", "priority": "low", "status": "pending"},
            ],
        }));
        "end_turn"
    }

    fn switch_mode(&mut self) -> &'static str {
        self.mode = "code".into();
        if self.flags.legacy_modes {
            self.update(json!({"sessionUpdate": "current_mode_update", "currentModeId": "code"}));
        } else {
            let options = self.config_options();
            self.update(json!({"sessionUpdate": "config_option_update", "configOptions": options}));
        }
        "end_turn"
    }

    /// Work that only a cancellation ends, with a question left open.
    fn cancelled(&mut self) -> &'static str {
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-long",
            "title": "Think for a long time",
            "kind": "think",
            "status": "in_progress",
        }));
        let question = self.ask(
            "session/request_permission",
            json!({
                "sessionId": self.session,
                "toolCall": {
                    "toolCallId": "call-ask",
                    "title": "Delete everything",
                    "kind": "delete",
                    "status": "pending",
                },
                "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"}],
            }),
        );
        self.wait_for(|message| message["method"] == "session/cancel");
        let outcome = self
            .answer(&question)
            .map(|result| result["outcome"]["outcome"].clone())
            .unwrap_or_default();
        // Asking again after the cancellation gets the same answer at once.
        let again = self
            .call(
                "session/request_permission",
                json!({
                    "sessionId": self.session,
                    "toolCall": {"toolCallId": "call-ask"},
                    "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"}],
                }),
            )
            .map(|result| result["outcome"]["outcome"].clone())
            .unwrap_or_default();
        self.say(&format!(
            "stopped; the question got {outcome}, then {again}"
        ));
        "cancelled"
    }

    /// Requests the agent withdraws with `$/cancel_request` before they are answered.
    fn take_back(&mut self) -> &'static str {
        let question = self.ask(
            "session/request_permission",
            json!({
                "sessionId": self.session,
                "toolCall": {"toolCallId": "call-maybe", "title": "Maybe", "status": "pending"},
                "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"}],
            }),
        );
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "$/cancel_request",
            "params": {"requestId": question},
        }));
        let outcome = self
            .answer(&question)
            .map(|result| result["outcome"]["outcome"].clone())
            .unwrap_or_default();

        let terminal = self.create_terminal("exec sleep 600", None);
        let ids = json!({"sessionId": self.session, "terminalId": terminal});
        let waiting = self.ask("terminal/wait_for_exit", ids.clone());
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "$/cancel_request",
            "params": {"requestId": waiting},
        }));
        let waited = self
            .answer(&waiting)
            .err()
            .map(|error| error["code"].clone());
        // A second wait is answered by the release that kills the command.
        let waiting = self.ask("terminal/wait_for_exit", ids.clone());
        self.call("terminal/release", ids).ok();
        let killed = self.answer(&waiting).ok();
        self.say(&json!({"question": outcome, "wait": waited, "killed": killed}).to_string());
        "end_turn"
    }

    fn crash(&mut self) -> ! {
        self.say("About to fail.");
        eprintln!("fake agent: the model fell over");
        eprintln!("fake agent: fatal, out of cheese");
        process::exit(3);
    }

    fn malformed(&mut self) -> &'static str {
        self.raw("this is not a message");
        self.raw(r#"{"jsonrpc":"2.0","method":"#);
        self.raw("[]");
        self.raw("");
        self.raw(r#"{"jsonrpc":"2.0","id":9999,"result":{}}"#);
        self.raw(r#"{"jsonrpc":"2.0","method":"_vendor/heartbeat","params":{"beat":1}}"#);
        self.update(json!({"sessionUpdate": "hologram", "content": "not in this version"}));
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "another-session",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "not for you"},
                },
            },
        }));
        // A tool call with fields of the wrong type keeps what is readable.
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-odd",
            "title": "Odd",
            "kind": "teleport",
            "status": 7,
            "content": [
                {"type": "hologram"},
                {"type": "content", "content": {"type": "text", "text": "kept"}},
            ],
        }));
        let unknown = self
            .call("_vendor/not_a_client_method", json!({}))
            .err()
            .map(|error| error["code"].clone());
        let bad_params = self
            .call("fs/read_text_file", json!({"sessionId": self.session}))
            .err()
            .map(|error| error["code"].clone());
        self.say(&json!({"unknown": unknown, "badParams": bad_params}).to_string());
        "end_turn"
    }

    fn echo(&mut self, text: &str) -> &'static str {
        let (first, second) = text.split_at(text.len() / 2);
        for part in [first, second, "an aside from the agent's side"] {
            self.update(json!({
                "sessionUpdate": "user_message_chunk",
                "content": {"type": "text", "text": part},
            }));
        }
        self.say("Heard.");
        "end_turn"
    }

    fn meta(&mut self) {
        self.update(json!({"sessionUpdate": "session_info_update", "title": "A fake session"}));
        self.update(json!({
            "sessionUpdate": "usage_update",
            "used": 53000,
            "size": 200000,
            "cost": {"amount": 0.045, "currency": "USD"},
        }));
        self.update(json!({
            "sessionUpdate": "notice",
            "severity": "warning",
            "title": "MCP server unavailable",
            "description": "Continuing without it.",
        }));
        self.update(json!({
            "sessionUpdate": "compaction_update",
            "compactionId": "cmp-1",
            "status": "in_progress",
        }));
        self.update(json!({
            "sessionUpdate": "compaction_update",
            "compactionId": "cmp-1",
            "status": "completed",
        }));
        self.update(json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": "compact", "description": ""}],
        }));
        self.update(json!({"sessionUpdate": "session_info_update", "title": null}));
    }
}
