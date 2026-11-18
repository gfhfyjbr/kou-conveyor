//! A scripted stand-in for `claude`, for kou-claude's tests.
//!
//! It speaks Claude Code's stream of JSON lines (`--input-format
//! stream-json --output-format stream-json`) with nothing but blocking reads
//! and writes, and calls no model. The shapes are the Agent SDK's types
//! (`sdk.d.ts`), written out as JSON here, independently of the crate. The
//! first word of a prompt chooses what the turn does (see [`Fake::turn`]).
//!
//! The command line it takes is Claude Code's, as far as the tests need it:
//! `--resume=<id>`, `--resume-session-at=<uuid>` and `--fork-session`,
//! `--model <m>`. Everything else is only reported (the `inspect` prompt).
//!
//! Its environment sets it up:
//!
//! - `CLAUDE_CONFIG_DIR`: its home. Each conversation is kept there as
//!   Claude Code keeps it (`projects/<folder>/<session>.jsonl`), which is
//!   what `--resume` reads. Without it nothing is kept on disk.
//! - `CLAUDE_FAKE_NO_STATES=1`: plays an older Claude Code, which says no
//!   `session_state_changed`.
//! - `CLAUDE_FAKE_CRASH=1`: complains on standard error and exits at once.
//! - `CLAUDE_FAKE_SILENT=1`: reads, and never answers anything.
//! - `CLAUDE_FAKE_PIDFILE=<path>`: its process id is written there first.
//! - `CLAUDE_FAKE_MODE=<mode>`: the permission mode it starts in, as a
//!   user's settings may set one.
//! - `CLAUDE_FAKE_EFFORT=<level>`: the level of effort it starts at
//!   (`high` otherwise).
//! - `CLAUDE_FAKE_LATE_REFUSAL=1`: a conversation it cannot go on with is
//!   refused after it has answered the greeting, not before.
//!
//! A new conversation is `sess-new`, a copy of one is its id and `-fork`;
//! when the home has one of that id already, `-2`, `-3`... follows.

use serde_json::{Map, Value, json};
use std::{
    collections::VecDeque,
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    process, thread,
    time::Duration,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    if let Some(path) = env("CLAUDE_FAKE_PIDFILE") {
        fs::write(path, process::id().to_string()).expect("the pid file");
    }
    if env("CLAUDE_FAKE_CRASH").is_some() {
        eprintln!("fake claude: cannot read its settings");
        eprintln!("fake claude: giving up");
        process::exit(3);
    }
    if env("CLAUDE_FAKE_SILENT").is_some() {
        for _ in io::stdin().lock().lines() {}
        return;
    }
    let value = |flag: &str| {
        let prefix = format!("{flag}=");
        args.iter().enumerate().find_map(|(at, arg)| {
            arg.strip_prefix(&prefix)
                .map(str::to_owned)
                .or_else(|| (arg == flag).then(|| args.get(at + 1).cloned()).flatten())
        })
    };
    let cwd = std::env::current_dir().expect("a working folder");
    let home = env("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let mut fake = Fake {
        input: io::stdin().lock().lines(),
        backlog: VecDeque::new(),
        args: args.clone(),
        session: String::new(),
        model: value("--model").unwrap_or_else(|| "default".into()),
        mode: env("CLAUDE_FAKE_MODE").unwrap_or_else(|| "default".into()),
        effort: env("CLAUDE_FAKE_EFFORT").unwrap_or_else(|| "high".into()),
        refusal: None,
        initialize: Value::Null,
        transcript: Vec::new(),
        last: None,
        home,
        cwd,
        states: env("CLAUDE_FAKE_NO_STATES").is_none(),
        counter: 0,
        children: Vec::new(),
    };
    match value("--resume") {
        Some(session) => {
            let at = value("--resume-session-at");
            let fork = args.iter().any(|arg| arg == "--fork-session");
            fake.resume(&session, at.as_deref(), fork);
        }
        None => fake.session = fake.unused("sess-new"),
    }
    while let Some(message) = fake.read() {
        fake.dispatch(message);
    }
}

struct Fake {
    input: io::Lines<io::StdinLock<'static>>,
    /// Messages read while waiting for something else.
    backlog: VecDeque<Value>,
    args: Vec<String>,
    session: String,
    model: String,
    mode: String,
    /// The level of effort asked for; the model in force may run lower.
    effort: String,
    /// Why it cannot go on with the conversation asked for, said once it
    /// has answered the greeting.
    refusal: Option<String>,
    /// The `initialize` request, for `inspect`.
    initialize: Value,
    /// The conversation's entries, as its record keeps them.
    transcript: Vec<Value>,
    /// The uuid of the last of them.
    last: Option<String>,
    home: Option<PathBuf>,
    cwd: PathBuf,
    /// It says `session_state_changed`.
    states: bool,
    counter: u64,
    /// What it started and left running.
    children: Vec<process::Child>,
}

impl Fake {
    // Plumbing.

    fn read(&mut self) -> Option<Value> {
        self.backlog.pop_front().or_else(|| self.read_input())
    }

    fn read_input(&mut self) -> Option<Value> {
        loop {
            let line = self.input.next()?.ok()?;
            if let Ok(message) = serde_json::from_str::<Value>(&line) {
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

    fn uuid(&mut self, kind: &str) -> String {
        self.counter += 1;
        format!("{kind}-{}-{}", process::id(), self.counter)
    }

    fn system(&mut self, subtype: &str, fields: Value) {
        let mut message = json!({"type": "system", "subtype": subtype});
        merge(&mut message, fields);
        message["uuid"] = json!(self.uuid("sys"));
        message["session_id"] = json!(self.session);
        self.send(message);
    }

    fn state(&mut self, state: &str) {
        if self.states {
            self.system("session_state_changed", json!({"state": state}));
        }
    }

    fn answer_control(&mut self, id: &Value, response: Value) {
        self.send(json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": id, "response": response},
        }));
    }

    fn refuse_control(&mut self, id: &Value, error: &str) {
        self.send(json!({
            "type": "control_response",
            "response": {"subtype": "error", "request_id": id, "error": error},
        }));
    }

    // The conversation's record.

    fn record_path(&self, session: &str) -> Option<PathBuf> {
        let slug: String = self
            .cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        Some(
            self.home
                .as_ref()?
                .join("projects")
                .join(slug)
                .join(format!("{session}.jsonl")),
        )
    }

    /// `id`, or the first of `id-2`, `id-3`... that no record has.
    fn unused(&self, id: &str) -> String {
        let taken = |id: &str| self.record_path(id).is_some_and(|path| path.exists());
        if !taken(id) {
            return id.to_owned();
        }
        (2..)
            .map(|n| format!("{id}-{n}"))
            .find(|id| !taken(id))
            .expect("an unused id")
    }

    /// Add an entry to the conversation, and to its record.
    fn keep(&mut self, mut entry: Value) {
        entry["parentUuid"] = json!(self.last);
        entry["isSidechain"] = json!(false);
        entry["sessionId"] = json!(self.session);
        entry["cwd"] = json!(self.cwd);
        entry["timestamp"] = json!("2026-11-15T12:00:00.000Z");
        entry["userType"] = json!("external");
        entry["version"] = json!("9.9.9-fake");
        self.last = entry["uuid"].as_str().map(str::to_owned);
        self.append(&entry);
        self.transcript.push(entry);
    }

    fn append(&self, entry: &Value) {
        let Some(path) = self.record_path(&self.session) else {
            return;
        };
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).expect("the record's folder");
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("the record");
        writeln!(file, "{entry}").expect("a line of the record");
    }

    /// Go on with a conversation kept before: whole, or up to an entry, in
    /// a copy under a new id.
    fn resume(&mut self, session: &str, at: Option<&str>, fork: bool) {
        let Some(text) = self
            .record_path(session)
            .and_then(|path| fs::read_to_string(path).ok())
        else {
            return self.refuse(format!("No conversation found with session ID: {session}"));
        };
        let mut kept: Vec<Value> = text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        if let Some(at) = at {
            let Some(end) = kept.iter().position(|entry| entry["uuid"] == at) else {
                return self.refuse(format!("No message found with message.uuid of: {at}"));
            };
            kept.truncate(end + 1);
        }
        if !fork {
            self.session = session.to_owned();
            self.last = kept
                .last()
                .and_then(|entry| entry["uuid"].as_str().map(str::to_owned));
            self.transcript = kept;
            return;
        }
        // A fork is a copy of what it keeps: new ids, each naming the one
        // it was made from.
        self.session = self.unused(&format!("{session}-fork"));
        for entry in kept {
            let mut copy = entry.clone();
            copy["uuid"] = json!(format!("{}~f", entry["uuid"].as_str().unwrap_or("?")));
            copy["forkedFrom"] = json!({"sessionId": session, "messageUuid": entry["uuid"]});
            self.keep(copy);
        }
    }

    /// A conversation it cannot go on with: it says why and ends, at once,
    /// or (`CLAUDE_FAKE_LATE_REFUSAL`) once it has answered the greeting.
    fn refuse(&mut self, why: String) {
        if std::env::var_os("CLAUDE_FAKE_LATE_REFUSAL").is_some() {
            self.refusal = Some(why);
            return;
        }
        eprintln!("{why}");
        process::exit(1);
    }

    // What comes in.

    fn dispatch(&mut self, message: Value) {
        match message["type"].as_str() {
            Some("control_request") => self.control(&message),
            Some("user") => self.turn(message),
            _ => {}
        }
    }

    /// A control request outside any scenario's own waiting.
    fn control(&mut self, message: &Value) {
        let id = message["request_id"].clone();
        let request = &message["request"];
        match request["subtype"].as_str() {
            Some("initialize") => {
                self.initialize = request.clone();
                self.answer_control(
                    &id,
                    json!({
                        "commands": [
                            {"name": "review", "description": "Review the changes", "argumentHint": "[focus]"},
                            {"name": "compact", "description": "Compact the conversation", "argumentHint": ""},
                        ],
                        "agents": [],
                        "output_style": "default",
                        "available_output_styles": ["default"],
                        "models": models(),
                        "account": {"email": "fake@example.com"},
                        "pending_permission_requests": [],
                        "claude_code_version": "9.9.9-fake",
                        "current_permission_mode": self.mode,
                        "session_state": "idle",
                        "an_extra_field": {"of": "a later Claude Code"},
                    }),
                );
                // As Claude Code 2.1.325 does: a note of its own right
                // after the greeting, the first message to carry the
                // session's id, before anything is said in it.
                let uuid = self.uuid("note");
                self.send(json!({
                    "type": "system",
                    "subtype": "dev_intent",
                    "kind": "none",
                    "trigger": "startup",
                    "uuid": uuid,
                    "session_id": self.session,
                }));
                if let Some(why) = self.refusal.take() {
                    // An error result, as a refused resume ends with, and
                    // its last words.
                    self.result(json!({"subtype": "error_during_execution", "is_error": true, "errors": [why]}));
                    eprintln!("{why}");
                    process::exit(1);
                }
            }
            Some("set_permission_mode") => {
                self.mode = request["mode"].as_str().unwrap_or("default").to_owned();
                self.answer_control(&id, json!({}));
            }
            Some("set_model") => match request["model"].as_str() {
                Some("nope") => self.refuse_control(&id, "Model nope is not available"),
                model => {
                    self.model = model.unwrap_or("default").to_owned();
                    self.answer_control(&id, json!({}));
                }
            },
            Some("interrupt") => self.answer_control(&id, json!({"still_queued": []})),
            Some("get_settings") => {
                let applied =
                    json!({"model": self.running_model(), "effort": self.applied_effort()});
                self.answer_control(
                    &id,
                    json!({"effective": {"permissions": {"defaultMode": self.mode}}, "sources": [], "applied": applied}),
                );
            }
            Some("apply_flag_settings") => {
                // As Claude Code 2.1.325 does: any level is taken, one that
                // does not exist too, and only what it knows changes
                // anything.
                match &request["settings"]["effortLevel"] {
                    Value::Null => self.effort = "high".into(),
                    Value::String(level) if EFFORTS.contains(&level.as_str()) => {
                        self.effort = level.clone();
                    }
                    _ => {}
                }
                self.answer_control(&id, json!({}));
            }
            subtype => {
                let error = format!("Unsupported control request subtype: {subtype:?}");
                self.refuse_control(&id, &error);
            }
        }
    }

    /// Takes a user message: kept, echoed. Its text without the app's note.
    fn take(&mut self, message: &Value) -> String {
        let uuid = match message["uuid"].as_str() {
            Some(uuid) => uuid.to_owned(),
            None => self.uuid("user"),
        };
        self.keep(json!({
            "type": "user",
            "message": message["message"],
            "uuid": uuid,
        }));
        self.send(json!({
            "type": "user",
            "message": message["message"],
            "parent_tool_use_id": null,
            "session_id": self.session,
            "uuid": uuid,
            "isReplay": true,
            "origin": message["origin"],
        }));
        text_of(message)
    }

    fn running_model(&self) -> String {
        models()
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["value"] == self.model.as_str())
            .and_then(|row| row["resolvedModel"].as_str())
            .unwrap_or(&self.model)
            .to_owned()
    }

    /// The level of effort the model in force runs at: the one asked for,
    /// lowered to `high` where the model lacks it; none for a model that
    /// takes no effort.
    fn applied_effort(&self) -> Value {
        let rows = models();
        let row = rows.as_array().into_iter().flatten().find(|row| {
            row["value"] == self.model.as_str() || row["resolvedModel"] == self.model.as_str()
        });
        let Some(levels) = row.and_then(|row| row["supportedEffortLevels"].as_array()) else {
            return Value::Null;
        };
        if levels.iter().any(|level| level == self.effort.as_str()) {
            json!(self.effort)
        } else {
            json!("high")
        }
    }

    /// What it says at the start of each turn: how it runs.
    fn init(&mut self) {
        let (model, mode, effort) = (
            self.running_model(),
            self.mode.clone(),
            self.applied_effort(),
        );
        self.system(
            "init",
            json!({
                "cwd": self.cwd,
                "tools": ["Bash", "Read", "Agent", "TodoWrite", "AskUserQuestion"],
                "mcp_servers": [],
                "model": model,
                "permissionMode": mode,
                "slash_commands": ["review", "compact"],
                "apiKeySource": "none",
                "claude_code_version": "9.9.9-fake",
                "output_style": "default",
                "skills": [],
                "plugins": [],
                "effort": effort,
                "capabilities": ["interrupt_receipt_v1", "interrupt_cancel_queued_v1", "ask_user_question_unanswered_v1"],
            }),
        );
    }

    /// One turn, chosen by the prompt's first word.
    fn turn(&mut self, message: Value) {
        let text = self.take(&message);
        self.state("running");
        self.init();
        let word = text
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        match word.as_str() {
            "inspect" => {
                let env: Map<String, Value> = std::env::vars()
                    .map(|(name, value)| (name, json!(value)))
                    .collect();
                let seen = json!({
                    "args": self.args,
                    "env": env,
                    "cwd": self.cwd,
                    "initialize": self.initialize,
                    "prompt": message,
                    "session": self.session,
                    "mode": self.mode,
                    "effort": self.effort,
                });
                self.reply(None, vec![Part::Text(vec![seen.to_string()])]);
                self.finish();
            }
            "chunks" => {
                self.reply(
                    None,
                    vec![
                        Part::Thinking(vec!["Let me ".into(), "think.".into()]),
                        Part::Text(vec!["Hel".into(), "lo.".into()]),
                    ],
                );
                // A second message: another paragraph of the same answer.
                self.reply(None, vec![Part::Text(vec!["Second.".into()])]);
                self.finish();
            }
            "whole" => {
                // Nothing streams: only the whole messages come.
                self.whole(None, json!([{"type": "text", "text": "All at once."}]));
                self.finish();
            }
            "tool" | "tools" => {
                let read = json!({"file_path": self.cwd.join("notes.txt")});
                self.reply(
                    None,
                    vec![
                        Part::Text(vec!["Reading.".into()]),
                        Part::Tool("read-1".into(), "Read".into(), read, true),
                    ],
                );
                self.tool_result(
                    None,
                    "read-1",
                    json!("hello"),
                    false,
                    json!({"type": "text"}),
                );
                let grep = json!({"pattern": "TODO"});
                self.whole(
                    None,
                    json!([{"type": "tool_use", "id": "grep-1", "name": "Grep", "input": grep}]),
                );
                self.tool_result(
                    None,
                    "grep-1",
                    json!([{"type": "text", "text": "no match"}]),
                    true,
                    Value::Null,
                );
                self.reply(None, vec![Part::Text(vec!["It says hello.".into()])]);
                self.finish();
            }
            "todo" => {
                let todos = json!({"todos": [
                    {"content": "Read the notes", "status": "completed", "activeForm": "Reading the notes"},
                    {"content": "Answer", "status": "in_progress", "activeForm": "Answering"},
                ]});
                self.reply(
                    None,
                    vec![Part::Tool("todo-1".into(), "TodoWrite".into(), todos, true)],
                );
                self.tool_result(
                    None,
                    "todo-1",
                    json!("Todos have been modified successfully"),
                    false,
                    Value::Null,
                );
                self.reply(None, vec![Part::Text(vec!["Planned.".into()])]);
                self.finish();
            }
            "tasks" => {
                let create = json!({"subject": "Fix it", "description": "Fix the bug", "activeForm": "Fixing it"});
                self.reply(
                    None,
                    vec![Part::Tool(
                        "task-1".into(),
                        "TaskCreate".into(),
                        create,
                        true,
                    )],
                );
                self.tool_result(
                    None,
                    "task-1",
                    json!("Task #1 created successfully: Fix it"),
                    false,
                    json!({"task": {"id": "1", "subject": "Fix it"}}),
                );
                let update = json!({"taskId": "1", "status": "in_progress"});
                self.reply(
                    None,
                    vec![Part::Tool(
                        "task-2".into(),
                        "TaskUpdate".into(),
                        update,
                        false,
                    )],
                );
                self.tool_result(
                    None,
                    "task-2",
                    json!("Updated task #1 status"),
                    false,
                    json!({"success": true, "taskId": "1", "updatedFields": ["status"]}),
                );
                self.reply(None, vec![Part::Text(vec!["On it.".into()])]);
                self.finish();
            }
            "agent" => {
                let launch = json!({"description": "Look around", "prompt": "List the files", "subagent_type": "Explore"});
                self.reply(
                    None,
                    vec![Part::Tool("agent-1".into(), "Agent".into(), launch, false)],
                );
                self.system(
                    "task_started",
                    json!({"task_id": "task-a", "tool_use_id": "agent-1", "description": "Look around", "task_type": "local_agent", "is_backgrounded": false}),
                );
                // The subagent's prompt, which is its call's to show.
                let uuid = self.uuid("sub");
                self.send(json!({
                    "type": "user",
                    "message": {"role": "user", "content": [{"type": "text", "text": "List the files"}]},
                    "parent_tool_use_id": "agent-1",
                    "session_id": self.session,
                    "uuid": uuid,
                }));
                let sub = Some("agent-1");
                self.reply(
                    sub,
                    vec![
                        Part::Thinking(vec!["Hmm.".into()]),
                        Part::Text(vec!["Looking ".into(), "around.".into()]),
                    ],
                );
                self.reply(
                    sub,
                    vec![Part::Tool(
                        "sub-1".into(),
                        "Bash".into(),
                        json!({"command": "ls"}),
                        false,
                    )],
                );
                self.tool_result(sub, "sub-1", json!("notes.txt"), false, Value::Null);
                self.whole(sub, json!([{"type": "text", "text": "One file."}]));
                self.tool_result(
                    None,
                    "agent-1",
                    json!([{"type": "text", "text": "There is one file."}]),
                    false,
                    json!({"status": "completed"}),
                );
                self.system(
                    "task_notification",
                    json!({"task_id": "task-a", "tool_use_id": "agent-1", "status": "completed", "output_file": "", "summary": "Looked around"}),
                );
                self.reply(
                    None,
                    vec![Part::Text(vec!["The agent found one file.".into()])],
                );
                self.finish();
            }
            "permission" => {
                let input = json!({"command": "rm -rf build", "description": "Remove the build"});
                self.reply(
                    None,
                    vec![Part::Tool(
                        "bash-1".into(),
                        "Bash".into(),
                        input.clone(),
                        true,
                    )],
                );
                let (answer, interrupted) = self.ask(
                    "perm-1",
                    json!({
                        "subtype": "can_use_tool",
                        "tool_name": "Bash",
                        "input": input,
                        "tool_use_id": "bash-1",
                        "permission_suggestions": [{
                            "type": "addRules",
                            "rules": [{"toolName": "Bash", "ruleContent": "rm -rf build"}],
                            "behavior": "allow",
                            "destination": "localSettings",
                        }],
                        "decision_reason": "a field this client need not know",
                    }),
                );
                if self.said_answer(&answer, interrupted) {
                    return;
                }
                let allowed = answer["response"]["behavior"] == "allow";
                self.tool_result(
                    None,
                    "bash-1",
                    json!(if allowed { "removed" } else { "refused" }),
                    !allowed,
                    Value::Null,
                );
                self.finish();
            }
            "ask" => {
                let input = json!({"questions": [
                    {"question": "Which library?", "header": "Library", "multiSelect": false, "options": [
                        {"label": "chrono", "description": "Full-featured"},
                        {"label": "time", "description": "Small"},
                    ]},
                    {"question": "Which features?", "header": "Features", "multiSelect": true, "options": [
                        {"label": "serde", "description": "Serialization"},
                        {"label": "tz", "description": "Time zones"},
                    ]},
                ]});
                self.reply(
                    None,
                    vec![Part::Tool(
                        "ask-1".into(),
                        "AskUserQuestion".into(),
                        input.clone(),
                        true,
                    )],
                );
                let (answer, interrupted) = self.ask(
                    "perm-ask",
                    json!({"subtype": "can_use_tool", "tool_name": "AskUserQuestion", "input": input, "tool_use_id": "ask-1"}),
                );
                if self.said_answer(&answer, interrupted) {
                    return;
                }
                let response = &answer["response"];
                if response["behavior"] == "allow" {
                    let answers = response["updatedInput"]["answers"].clone();
                    let text = format!(
                        "User has answered your questions: {answers}. You can now continue with the user's answers in mind."
                    );
                    self.tool_result(
                        None,
                        "ask-1",
                        json!(text),
                        false,
                        json!({"questions": input["questions"], "answers": answers}),
                    );
                } else {
                    let refused = response["message"].clone();
                    self.tool_result(None, "ask-1", refused, true, Value::Null);
                }
                self.finish();
            }
            "elicit" => {
                // An MCP server's tool asks the user in the middle of its call.
                let call = json!({"color": "?"});
                self.reply(
                    None,
                    vec![Part::Tool(
                        "mcp-1".into(),
                        "mcp__paint__paint".into(),
                        call,
                        false,
                    )],
                );
                let mut request = json!({
                    "subtype": "elicitation",
                    "mcp_server_name": "paint",
                    "message": "Pick a color",
                    "mode": "form",
                    "requested_schema": {
                        "type": "object",
                        "properties": {
                            "count": {"type": "integer", "title": "How many"},
                            "color": {"type": "string", "title": "Color", "enum": ["red", "green"], "enumNames": ["Red", "Green"]},
                        },
                        "required": ["count"],
                    },
                    "property_order": ["color", "count"],
                });
                match text.split_whitespace().nth(1) {
                    Some("url") => {
                        request["mode"] = json!("url");
                        request["url"] = json!("https://paint.example/login");
                        request["elicitation_id"] = json!("e-1");
                        request
                            .as_object_mut()
                            .map(|fields| fields.remove("requested_schema"));
                    }
                    Some("empty") => {
                        request["requested_schema"] = json!({"type": "object", "properties": {}})
                    }
                    _ => {}
                }
                let (answer, interrupted) = self.ask("elicit-1", request);
                if self.said_answer(&answer, interrupted) {
                    return;
                }
                self.tool_result(None, "mcp-1", json!("Painted."), false, Value::Null);
                self.finish();
            }
            "withdraw" => {
                // Two requests taken back before anyone answers them: no
                // answer to either is expected afterwards.
                let input = json!({"command": "make", "description": "Build"});
                self.reply(
                    None,
                    vec![Part::Tool(
                        "bash-w".into(),
                        "Bash".into(),
                        input.clone(),
                        false,
                    )],
                );
                self.send(json!({
                    "type": "control_request",
                    "request_id": "perm-w",
                    "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": input, "tool_use_id": "bash-w"},
                }));
                self.send(json!({"type": "control_cancel_request", "request_id": "perm-w"}));
                self.send(json!({
                    "type": "control_request",
                    "request_id": "elicit-w",
                    "request": {"subtype": "elicitation", "mcp_server_name": "paint", "message": "Sure?", "requested_schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}},
                }));
                self.send(json!({"type": "control_cancel_request", "request_id": "elicit-w"}));
                // Until the next message of the user's, every answer that
                // comes is noted.
                let mut stray = Vec::new();
                let next = loop {
                    let Some(message) = self.read() else {
                        process::exit(0);
                    };
                    match message["type"].as_str() {
                        Some("user") => break message,
                        Some("control_response") => {
                            stray.push(message["response"]["request_id"].clone())
                        }
                        Some("control_request") => self.control(&message),
                        _ => {}
                    }
                };
                self.take(&next);
                self.reply(
                    None,
                    vec![Part::Text(vec![json!({"stray": stray}).to_string()])],
                );
                self.finish();
            }
            "dialog" => {
                let (answer, _) = self.ask(
                    "dialog-1",
                    json!({"subtype": "request_user_dialog", "dialog_kind": "refusal_fallback_prompt", "payload": {"originalModel": "claude-fake-opus", "fallbackModel": "claude-fake-sonnet"}}),
                );
                self.reply(None, vec![Part::Text(vec![answer.to_string()])]);
                self.finish();
            }
            "background" => {
                let launch =
                    json!({"description": "Dig", "prompt": "Dig deep", "run_in_background": true});
                self.reply(
                    None,
                    vec![Part::Tool("bg-1".into(), "Agent".into(), launch, false)],
                );
                self.system(
                    "task_started",
                    json!({"task_id": "task-bg", "tool_use_id": "bg-1", "description": "Dig", "task_type": "local_agent", "is_backgrounded": true}),
                );
                self.tool_result(
                    None,
                    "bg-1",
                    json!("Async agent launched successfully."),
                    false,
                    json!({"isAsync": true, "status": "async_launched"}),
                );
                self.reply(None, vec![Part::Text(vec!["Started it.".into()])]);
                // The answer is given; the agent runs on, and so does the
                // turn, which takes a prompt meanwhile.
                self.result(success());
                let joined = self.wait_for(|message| message["type"] == "user");
                let text = self.take(&joined);
                self.reply(None, vec![Part::Text(vec![format!("Meanwhile: {text}")])]);
                self.result(success());
                self.system(
                    "task_updated",
                    json!({"task_id": "task-bg", "patch": {"status": "completed", "end_time": 1}}),
                );
                self.system(
                    "task_notification",
                    json!({"task_id": "task-bg", "tool_use_id": "bg-1", "status": "completed", "output_file": "", "summary": "Dug", "usage": {"total_tokens": 1234, "tool_uses": 3, "duration_ms": 10}}),
                );
                // A cycle of its own, for what the agent reported.
                self.reply(None, vec![Part::Text(vec!["The agent is done.".into()])]);
                let mut own = success();
                own["origin"] = json!({"kind": "task-notification"});
                self.result(own);
                self.state("idle");
            }
            "shell" => {
                let input = json!({"command": "sleep 1", "run_in_background": true});
                self.reply(
                    None,
                    vec![Part::Tool("sh-1".into(), "Bash".into(), input, false)],
                );
                self.system(
                    "task_started",
                    json!({"task_id": "task-sh", "tool_use_id": "sh-1", "description": "sleep 1", "task_type": "local_bash", "is_backgrounded": true}),
                );
                self.tool_result(
                    None,
                    "sh-1",
                    json!("Command running in background"),
                    false,
                    json!({"backgroundTaskId": "task-sh"}),
                );
                self.reply(None, vec![Part::Text(vec!["Started a command.".into()])]);
                self.finish();
                // The command ends after the turn: Claude Code goes on by
                // itself.
                thread::sleep(Duration::from_millis(100));
                self.system(
                    "task_notification",
                    json!({"task_id": "task-sh", "tool_use_id": "sh-1", "status": "completed", "output_file": "", "summary": "slept"}),
                );
                self.state("running");
                self.reply(None, vec![Part::Text(vec!["The command finished.".into()])]);
                let mut own = success();
                own["origin"] = json!({"kind": "task-notification"});
                self.result(own);
                self.state("idle");
            }
            "steer" => {
                self.reply(None, vec![Part::Text(vec!["Working.".into()])]);
                let steered = self.wait_for(|message| message["type"] == "user");
                let text = self.take(&steered);
                self.reply(None, vec![Part::Text(vec![format!("Took: {text}")])]);
                self.finish();
            }
            "hang" | "stubborn" => {
                self.reply(None, vec![Part::Text(vec!["Working on it.".into()])]);
                let interrupt = self.wait_for(|message| {
                    message["type"] == "control_request"
                        && message["request"]["subtype"] == "interrupt"
                });
                if word == "stubborn" {
                    // Never says it stopped: only the end of its input ends it.
                    while self.read_input().is_some() {}
                    process::exit(0);
                }
                self.control(&interrupt);
                self.result(json!({"subtype": "error_during_execution", "is_error": true, "terminal_reason": "aborted_streaming", "errors": []}));
                self.state("idle");
            }
            "mode" => {
                // Leaves for the plan mode by itself, as EnterPlanMode does.
                self.mode = "plan".into();
                self.system("status", json!({"status": null, "permissionMode": "plan"}));
                self.reply(None, vec![Part::Text(vec!["Planning.".into()])]);
                self.finish();
            }
            "search" => {
                // Tools the API runs itself: their calls and results are
                // blocks of the answer.
                self.reply(
                    None,
                    vec![
                        Part::ServerTool("srv-1".into(), "web_search".into(), json!({"query": "gpui"})),
                        Part::Whole(json!({"type": "web_search_tool_result", "tool_use_id": "srv-1", "content": [
                            {"type": "web_search_result", "title": "GPUI", "url": "https://gpui.rs", "encrypted_content": "e30=", "page_age": null},
                        ]})),
                        Part::ServerTool("srv-2".into(), "web_fetch".into(), json!({"url": "https://gpui.rs"})),
                        Part::Whole(json!({"type": "web_fetch_tool_result", "tool_use_id": "srv-2", "content": {
                            "type": "web_fetch_tool_result_error", "error_code": "url_not_accessible",
                        }})),
                        Part::Text(vec!["Found it.".into()]),
                    ],
                );
                self.finish();
            }
            "title" => {
                // Names the conversation, as Claude Code does by itself
                // after its first exchanges: a line of the record's own.
                let title = text.split_once(' ').map_or("", |(_, rest)| rest).to_owned();
                self.append(
                    &json!({"type": "ai-title", "aiTitle": title, "sessionId": self.session}),
                );
                self.reply(None, vec![Part::Text(vec!["Named.".into()])]);
                self.finish();
            }
            "effort" => {
                // As `/effort <level>` typed into Claude Code: the level
                // changes, and an `init` said again carries it.
                self.effort = text.split_whitespace().nth(1).unwrap_or("low").to_owned();
                self.init();
                self.reply(None, vec![Part::Text(vec!["Effort set.".into()])]);
                self.finish();
            }
            "junk" => {
                self.send(json!({"type": "something_new", "session_id": self.session}));
                self.system("a_subtype_from_later", json!({"with": ["fields"]}));
                self.raw("this line is not JSON");
                self.raw("[1, 2, 3]");
                self.send(json!({
                    "type": "control_response",
                    "response": {"subtype": "success", "request_id": "nobody-asked", "response": {}},
                }));
                self.send(json!({
                    "type": "control_request",
                    "request_id": "q-1",
                    "request": {"subtype": "a_question_from_later"},
                }));
                let answer = self.wait_for(|message| {
                    message["type"] == "control_response"
                        && message["response"]["request_id"] == "q-1"
                });
                let uuid = self.uuid("asst");
                self.send(json!({
                    "type": "assistant",
                    "message": {"id": "msg-junk", "role": "assistant", "model": "claude-fake-opus", "content": [
                        {"type": "a_block_from_later"},
                        {"type": "text", "text": answer["response"].to_string(), "citations": null},
                    ]},
                    "parent_tool_use_id": null,
                    "uuid": uuid,
                    "session_id": self.session,
                    "a_field_from_later": true,
                }));
                self.finish();
            }
            "usage" => {
                let uuid = self.uuid("asst");
                self.send(json!({
                    "type": "assistant",
                    "message": {"id": "msg-u", "role": "assistant", "model": "claude-fake-opus",
                        "content": [{"type": "text", "text": "Counted."}],
                        "usage": {"input_tokens": 100, "cache_creation_input_tokens": 20, "cache_read_input_tokens": 30, "output_tokens": 50}},
                    "parent_tool_use_id": null,
                    "uuid": uuid,
                    "session_id": self.session,
                }));
                let mut result = success();
                result["total_cost_usd"] = json!(0.25);
                result["modelUsage"] = json!({
                    "claude-fake-opus": {"inputTokens": 1000, "outputTokens": 200, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0, "webSearchRequests": 0, "costUSD": 0.2, "contextWindow": 200000, "maxOutputTokens": 32000},
                    "claude-fake-haiku": {"inputTokens": 10, "outputTokens": 2, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0, "webSearchRequests": 0, "costUSD": 0.05, "contextWindow": 100000, "maxOutputTokens": 8000},
                });
                self.result(result);
                self.state("idle");
            }
            "fail" => {
                self.result(json!({"subtype": "success", "is_error": true, "result": "The fake ran out of luck", "stop_reason": null}));
                self.state("idle");
            }
            "hold" => {
                // Works until the file named after the word is there (or a
                // minute has passed), reading nothing meanwhile; then says
                // it is done, and leaves `<file>.done` once it is idle.
                let file = PathBuf::from(text.split_whitespace().nth(1).unwrap_or("/nonexistent"));
                self.reply(None, vec![Part::Text(vec!["Working on it.".into()])]);
                let deadline = std::time::Instant::now() + Duration::from_secs(60);
                while !file.exists() && std::time::Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(20));
                }
                self.reply(None, vec![Part::Text(vec!["Done.".into()])]);
                self.finish();
                let mut done = file.into_os_string();
                done.push(".done");
                fs::write(done, "").ok();
            }
            "split" => {
                // An answer begun, then held until the file named after the
                // word is there: a client that comes back meanwhile comes
                // in the middle of it.
                let file = PathBuf::from(text.split_whitespace().nth(1).unwrap_or("/nonexistent"));
                let id = self.uuid("msg");
                let model = self.running_model();
                self.event(None, json!({"type": "message_start", "message": {"id": id, "role": "assistant", "model": model, "content": []}}));
                self.event(None, json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}));
                self.event(None, json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Half"}}));
                let deadline = std::time::Instant::now() + Duration::from_secs(60);
                while !file.exists() && std::time::Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(20));
                }
                self.event(None, json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " and the rest."}}));
                self.event(None, json!({"type": "content_block_stop", "index": 0}));
                self.assistant(
                    None,
                    &id,
                    json!([{"type": "text", "text": "Half and the rest."}]),
                );
                self.event(None, json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}));
                self.event(None, json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "More."}}));
                self.event(None, json!({"type": "content_block_stop", "index": 1}));
                self.assistant(None, &id, json!([{"type": "text", "text": "More."}]));
                self.event(None, json!({"type": "message_stop"}));
                self.finish();
            }
            "whoami" => {
                // Its process id and its parent's (the host that keeps it,
                // or the app).
                #[cfg(unix)]
                let parent = std::os::unix::process::parent_id();
                #[cfg(not(unix))]
                let parent = 0;
                let ids = json!({"pid": process::id(), "parent": parent});
                self.reply(None, vec![Part::Text(vec![ids.to_string()])]);
                self.finish();
            }
            "die" => {
                // Ends in the middle of the turn, as a crash does.
                eprintln!("fake claude: out of memory");
                process::exit(7);
            }
            "pid" => {
                // Left running, as a command of Claude Code's can be.
                let child = process::Command::new("sleep")
                    .arg("600")
                    .spawn()
                    .expect("a child");
                let pids = format!("{} {}", process::id(), child.id());
                self.children.push(child);
                self.reply(None, vec![Part::Text(vec![pids])]);
                self.finish();
            }
            "history" => {
                let said: Vec<String> = self
                    .transcript
                    .iter()
                    .filter_map(|entry| {
                        let text = text_of(entry);
                        match entry["type"].as_str()? {
                            "user" if !text.is_empty() => Some(format!("> {text}")),
                            "assistant" if !text.is_empty() => Some(format!("< {text}")),
                            _ => None,
                        }
                    })
                    .collect();
                self.reply(None, vec![Part::Text(vec![json!(said).to_string()])]);
                self.finish();
            }
            _ => {
                self.reply(
                    None,
                    vec![Part::Text(vec!["You said: ".into(), text.clone()])],
                );
                self.finish();
            }
        }
    }

    /// Ask the client something (a control request of id `id`) and wait for
    /// its answer, taking its own requests meanwhile: the answer, and
    /// whether it asked to interrupt before it answered.
    fn ask(&mut self, id: &str, request: Value) -> (Value, bool) {
        self.send(json!({"type": "control_request", "request_id": id, "request": request}));
        let mut interrupted = false;
        loop {
            let message = self.wait_for(|message| {
                message["type"] == "control_request" || message["type"] == "control_response"
            });
            if message["type"] == "control_request" {
                if message["request"]["subtype"] == "interrupt" {
                    interrupted = true;
                }
                self.control(&message);
                continue;
            }
            if message["response"]["request_id"] == id {
                return (message["response"].clone(), interrupted);
            }
        }
    }

    /// Say what the client answered; an interrupted turn ends there, which
    /// is said with true.
    fn said_answer(&mut self, answer: &Value, interrupted: bool) -> bool {
        let said = json!({"answer": answer, "interrupted": interrupted});
        self.reply(None, vec![Part::Text(vec![said.to_string()])]);
        if interrupted {
            self.result(json!({"subtype": "error_during_execution", "is_error": true, "terminal_reason": "aborted_tools", "errors": []}));
            self.state("idle");
        }
        interrupted
    }

    /// One message of an answer, streamed block by block, each block then
    /// coming whole in an `assistant` message of its own. `parent`: the call
    /// of the subagent that says it.
    fn reply(&mut self, parent: Option<&str>, parts: Vec<Part>) {
        let id = self.uuid("msg");
        self.event(parent, json!({"type": "message_start", "message": {"id": id, "role": "assistant", "model": self.running_model(), "content": []}}));
        for (index, part) in parts.into_iter().enumerate() {
            let block = match part {
                Part::Text(pieces) | Part::Thinking(pieces) if pieces.is_empty() => continue,
                Part::Text(pieces) => {
                    self.event(parent, json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}));
                    for piece in &pieces {
                        self.event(parent, json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": piece}}));
                    }
                    json!({"type": "text", "text": pieces.concat()})
                }
                Part::Thinking(pieces) => {
                    self.event(parent, json!({"type": "content_block_start", "index": index, "content_block": {"type": "thinking", "thinking": ""}}));
                    for piece in &pieces {
                        self.event(parent, json!({"type": "content_block_delta", "index": index, "delta": {"type": "thinking_delta", "thinking": piece}}));
                    }
                    self.event(parent, json!({"type": "content_block_delta", "index": index, "delta": {"type": "signature_delta", "signature": "c2ln"}}));
                    json!({"type": "thinking", "thinking": pieces.concat(), "signature": "c2ln"})
                }
                Part::Whole(block) => {
                    self.event(
                        parent,
                        json!({"type": "content_block_start", "index": index, "content_block": block}),
                    );
                    block
                }
                part @ (Part::Tool(..) | Part::ServerTool(..)) => {
                    let (kind, call, name, input, streamed) = match part {
                        Part::Tool(call, name, input, streamed) => {
                            ("tool_use", call, name, input, streamed)
                        }
                        Part::ServerTool(call, name, input) => {
                            ("server_tool_use", call, name, input, true)
                        }
                        _ => unreachable!(),
                    };
                    let block = json!({"type": kind, "id": call, "name": name, "input": input});
                    if !streamed {
                        // Only the whole message says it.
                        self.assistant(parent, &id, json!([block]));
                        continue;
                    }
                    self.event(parent, json!({"type": "content_block_start", "index": index, "content_block": {"type": kind, "id": call, "name": name, "input": {}}}));
                    let text = input.to_string();
                    let (head, tail) = text.split_at(text.len() / 2);
                    for piece in [head, tail] {
                        self.event(parent, json!({"type": "content_block_delta", "index": index, "delta": {"type": "input_json_delta", "partial_json": piece}}));
                    }
                    block
                }
            };
            self.event(
                parent,
                json!({"type": "content_block_stop", "index": index}),
            );
            self.assistant(parent, &id, json!([block]));
        }
        self.event(
            parent,
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
        );
        self.event(parent, json!({"type": "message_stop"}));
    }

    /// A message that comes whole only, as one not streamed.
    fn whole(&mut self, parent: Option<&str>, content: Value) {
        let id = self.uuid("msg");
        self.assistant(parent, &id, content);
    }

    fn event(&mut self, parent: Option<&str>, event: Value) {
        let uuid = self.uuid("ev");
        self.send(json!({
            "type": "stream_event",
            "event": event,
            "parent_tool_use_id": parent,
            "uuid": uuid,
            "session_id": self.session,
        }));
    }

    fn assistant(&mut self, parent: Option<&str>, id: &str, content: Value) {
        let uuid = self.uuid("asst");
        let body = json!({
            "id": id,
            "type": "message",
            "role": "assistant",
            "model": self.running_model(),
            "content": content,
            "stop_reason": null,
        });
        self.send(json!({
            "type": "assistant",
            "message": body,
            "parent_tool_use_id": parent,
            "uuid": uuid,
            "session_id": self.session,
        }));
        if parent.is_none() {
            self.keep(json!({"type": "assistant", "message": body, "uuid": uuid}));
        }
    }

    fn tool_result(
        &mut self,
        parent: Option<&str>,
        call: &str,
        content: Value,
        failed: bool,
        structured: Value,
    ) {
        let uuid = self.uuid("res");
        let message = json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": call, "content": content, "is_error": failed},
        ]});
        let mut sent = json!({
            "type": "user",
            "message": message,
            "parent_tool_use_id": parent,
            "uuid": uuid,
            "session_id": self.session,
        });
        if !structured.is_null() {
            sent["tool_use_result"] = structured;
        }
        self.send(sent);
        if parent.is_none() {
            self.keep(json!({"type": "user", "message": message, "uuid": uuid}));
        }
    }

    fn result(&mut self, fields: Value) {
        let mut result = json!({
            "type": "result",
            "duration_ms": 5,
            "duration_api_ms": 4,
            "num_turns": 1,
            "total_cost_usd": 0.0,
            "usage": {"input_tokens": 0, "output_tokens": 0},
            "modelUsage": {},
            "permission_denials": [],
        });
        merge(&mut result, fields);
        result["uuid"] = json!(self.uuid("res"));
        result["session_id"] = json!(self.session);
        self.send(result);
    }

    /// The answer is given and nothing is left running.
    fn finish(&mut self) {
        self.result(success());
        self.state("idle");
    }
}

/// A block of an answer to write.
enum Part {
    Text(Vec<String>),
    Thinking(Vec<String>),
    /// A call: id, tool, input, and whether its input streams.
    Tool(String, String, Value, bool),
    /// A call of a tool the API runs itself, whose input streams: id, tool,
    /// input.
    ServerTool(String, String, Value),
    /// A block that streams whole in its start (the result of a tool the
    /// API ran).
    Whole(Value),
}

/// The levels of effort Claude Code knows (`EffortLevel`).
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The models it offers, as `ModelInfo` rows of Claude Code 2.1.325.
fn models() -> Value {
    json!([
        {"value": "default", "resolvedModel": "claude-fake-opus", "displayName": "Default (recommended)", "description": "Fake Opus for most work",
         "supportsEffort": true, "supportedEffortLevels": EFFORTS, "supportsAdaptiveThinking": true, "supportsFastMode": true, "supportsAutoMode": true, "supportsThinkingOff": true},
        {"value": "sonnet", "resolvedModel": "claude-fake-sonnet", "displayName": "Sonnet", "description": "Fake Sonnet",
         "supportsEffort": true, "supportedEffortLevels": ["low", "medium", "high"], "supportsAdaptiveThinking": true, "supportsFastMode": false, "supportsAutoMode": true, "supportsThinkingOff": true},
        {"value": "haiku", "resolvedModel": "claude-fake-haiku", "displayName": "Haiku", "description": "Fake Haiku",
         "supportsEffort": false, "supportsAdaptiveThinking": false, "supportsFastMode": false, "supportsAutoMode": false, "supportsThinkingOff": true},
    ])
}

fn success() -> Value {
    json!({"subtype": "success", "is_error": false, "result": "", "stop_reason": "end_turn"})
}

/// The text of a message (a prompt or an entry), without the app's note.
fn text_of(message: &Value) -> String {
    let content = &message["message"]["content"];
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .filter(|text| !text.starts_with("<kou-desktop>"))
        .collect::<Vec<_>>()
        .join("")
}

fn merge(base: &mut Value, more: Value) {
    if let (Some(base), Value::Object(more)) = (base.as_object_mut(), more) {
        for (key, value) in more {
            base.insert(key, value);
        }
    }
}
