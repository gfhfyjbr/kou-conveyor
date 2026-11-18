//! `connect` against the scripted Claude Code in `examples/claude_fake.rs`.
//!
//! Each test asserts the updates a scenario produces, in order, and what
//! they add up to folded through `kou_thread::Thread`, as the interface sees
//! them. What a tool call shows (its title, kind, content) is the records'
//! conversion and is not asserted here: only ids, order, nesting, status
//! and the marks of work left in the background. Run with `cargo test -p
//! kou-claude`, which also builds the fake these tests start.

#![cfg(unix)]

use kou_claude::{Keeper, McpServer, Options, connect, records};
use kou_thread::{
    Backend, Block, Capabilities, Choice, Command, Entry, Level, Offers, PermissionKind,
    PermissionOption, Request, Stop, Thread, ThreadState, ToolCall, ToolPatch, Update, Usage,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// Longer than anything here takes, short enough to fail a hung test.
const PATIENCE: Duration = Duration::from_secs(30);

/// The scripted Claude Code, built next to this test.
fn fake_claude() -> PathBuf {
    let mut path = std::env::current_exe().expect("this test's path");
    path.pop();
    path.pop();
    path.push("examples");
    path.push("claude_fake");
    assert!(
        path.exists(),
        "{} is missing: run `cargo test -p kou-claude`, which builds it",
        path.display()
    );
    path
}

/// A session of the scripted Claude Code, and the thread its updates add
/// up to.
struct Fake {
    backend: Option<Backend>,
    thread: Thread,
    cwd: PathBuf,
    home: PathBuf,
    /// Pass [`Update::Usage`] on; otherwise it is folded in and skipped.
    usage: bool,
    /// Pass [`Update::Offers`] on; otherwise it is folded in and skipped.
    offers: bool,
    /// Pass [`Update::Efforts`] on; otherwise it is folded in and skipped.
    efforts: bool,
    /// Its folder, when it is the session's own.
    _folder: Option<tempfile::TempDir>,
}

impl Fake {
    fn start() -> Self {
        Self::start_with(|_| {})
    }

    /// Start it in a folder of its own, with a home of its own (never the
    /// user's), with options set up by `configure`.
    fn start_with(configure: impl FnOnce(&mut Options)) -> Self {
        let folder = tempfile::tempdir().expect("a temporary folder");
        // Resolved, so that the fake's own idea of its folder matches.
        let root = folder.path().canonicalize().expect("a real path");
        let (cwd, home) = (root.join("work"), root.join("home"));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let mut options = Options::new(fake_claude(), &cwd);
        options.home = Some(home.clone());
        options.env = vec![("CLAUDE_CONFIG_DIR".into(), home.display().to_string())];
        configure(&mut options);
        Self {
            backend: Some(connect(options).expect("the fake starts")),
            thread: Thread::new(),
            cwd,
            home,
            usage: false,
            offers: false,
            efforts: false,
            _folder: Some(folder),
        }
    }

    fn backend(&self) -> &Backend {
        self.backend.as_ref().expect("a live backend")
    }

    /// The next update, folded into the thread.
    fn next(&mut self) -> Update {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.backend().updates.try_recv() {
                Ok(update) => {
                    self.thread.apply(update.clone());
                    let skipped = match &update {
                        Update::Usage(_) => !self.usage,
                        Update::Offers(_) => !self.offers,
                        Update::Efforts { .. } => !self.efforts,
                        _ => false,
                    };
                    if !skipped {
                        return update;
                    }
                }
                Err(error) if error.is_closed() => panic!("the updates ended"),
                Err(_) if Instant::now() > deadline => panic!("no update for {PATIENCE:?}"),
                Err(_) => thread::sleep(Duration::from_millis(2)),
            }
        }
    }

    /// Assert the next updates, in order.
    fn expect(&mut self, expected: impl IntoIterator<Item = Update>) {
        for expected in expected {
            assert_eq!(self.next(), expected);
        }
    }

    /// Every update up to and with the end of the turn.
    fn until_turn_ends(&mut self) -> Vec<Update> {
        let mut seen = Vec::new();
        loop {
            let update = self.next();
            let end = matches!(update, Update::TurnEnded(_));
            seen.push(update);
            if end {
                return seen;
            }
        }
    }

    /// Every update up to and with [`Update::Ready`].
    fn until_ready(&mut self) -> Vec<Update> {
        let mut seen = Vec::new();
        loop {
            let update = self.next();
            let ready = matches!(update, Update::Ready { .. });
            seen.push(update);
            if ready {
                return seen;
            }
        }
    }

    /// Assert that nothing follows and the channel is closed.
    fn expect_end(&mut self) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.backend().updates.try_recv() {
                Ok(update) => panic!("an update after the end: {update:?}"),
                Err(error) if error.is_closed() => return,
                Err(_) if Instant::now() > deadline => panic!("the updates did not end"),
                Err(_) => thread::sleep(Duration::from_millis(2)),
            }
        }
    }

    /// Assert that nothing comes for a moment.
    fn expect_quiet(&mut self) {
        thread::sleep(Duration::from_millis(300));
        while let Ok(update) = self.backend().updates.try_recv() {
            self.thread.apply(update.clone());
            let skipped = matches!(
                update,
                Update::Usage(_) | Update::Offers(_) | Update::Efforts { .. }
            );
            assert!(skipped, "nothing expected, got {update:?}");
        }
    }

    fn send(&mut self, command: Command) {
        if let Command::Permit { request, .. } | Command::Answer { request, .. } = &command {
            self.thread.answered(*request);
        }
        self.backend()
            .commands
            .try_send(command)
            .expect("the backend takes commands");
    }

    /// Send a prompt the way the interface does: shown at once, then sent.
    fn prompt(&mut self, text: &str) {
        self.prompt_blocks(vec![Block::Text(text.into())]);
    }

    fn prompt_blocks(&mut self, blocks: Vec<Block>) {
        self.thread.push_user(blocks.clone());
        self.send(Command::Prompt(blocks));
    }

    /// The name the backend gave the prompt just sent.
    fn taken(&mut self) -> String {
        match self.next() {
            Update::PromptTaken(Some(id)) => id,
            other => panic!("the prompt's name, got {other:?}"),
        }
    }

    /// A prompt sent to an idle session: named, and a turn begins.
    fn ask(&mut self, text: &str) -> String {
        self.prompt(text);
        let id = self.taken();
        self.expect([Update::TurnStarted]);
        id
    }

    /// The first prompt of a new conversation, which names its session.
    fn ask_first(&mut self, text: &str) -> String {
        let id = self.ask(text);
        self.expect([ready("sess-new", "default", "default")]);
        id
    }

    /// Edit the prompt of entry `index` the way the interface does.
    fn edit(&mut self, index: usize, text: &str) {
        let blocks = vec![Block::Text(text.into())];
        let rewinding = self
            .thread
            .rewind(index, blocks.clone())
            .expect("a prompt to edit");
        self.request(Request::Rewind {
            prompt: rewinding.prompt,
            blocks,
        });
    }

    fn request(&mut self, request: Request) {
        self.backend()
            .requests
            .try_send(request)
            .expect("the backend takes requests");
    }

    /// The next update, which must be an answer chunk holding JSON.
    fn next_json(&mut self) -> Value {
        match self.next() {
            Update::AssistantChunk(text) => {
                serde_json::from_str(&text).unwrap_or_else(|_| panic!("JSON, got {text}"))
            }
            other => panic!("an answer chunk, got {other:?}"),
        }
    }

    /// What the default fake says when it is ready.
    fn expect_ready(&mut self) {
        self.expect([ready("", "default", "default"), commands()]);
        assert_eq!(self.thread.state(), &ThreadState::Idle);
    }

    /// A prompt the fake answers by repeating it.
    fn exchange(&mut self, text: &str) -> String {
        let id = self.ask(text);
        self.expect([
            said("You said: "),
            said(text),
            Update::TurnEnded(Stop::Finished),
        ]);
        id
    }
}

impl Drop for Fake {
    /// No test leaves its Claude Code behind.
    fn drop(&mut self) {
        let Some(backend) = self.backend.take() else {
            return;
        };
        let _ = backend.commands.try_send(Command::Close);
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            match backend.updates.try_recv() {
                Err(error) if error.is_closed() => return,
                _ => thread::sleep(Duration::from_millis(2)),
            }
        }
    }
}

fn choice(id: &str, name: &str, description: &str) -> Choice {
    Choice {
        id: id.into(),
        name: name.into(),
        description: Some(description.into()),
    }
}

/// The session announced: its modes are those of the fake's models, all
/// but Haiku able to run in `auto`.
fn ready(session: &str, mode: &str, model: &str) -> Update {
    let mut modes = vec![
        choice("default", "Manual", "Always ask before making changes"),
        choice(
            "acceptEdits",
            "Accept edits",
            "Automatically accept all file edits",
        ),
        choice("plan", "Plan", "Create a plan before making changes"),
    ];
    if model != "haiku" || mode == "auto" {
        modes.push(choice(
            "auto",
            "Auto",
            "Claude handles permission decisions",
        ));
    }
    modes.push(choice(
        "bypassPermissions",
        "Bypass permissions",
        "Accepts all permissions",
    ));
    Update::Ready {
        session: session.into(),
        capabilities: Capabilities {
            images: true,
            embedded_files: true,
            resume: true,
        },
        modes,
        mode: Some(mode.into()),
        models: vec![
            choice(
                "default",
                "Default (recommended)",
                "Fake Opus for most work",
            ),
            choice("sonnet", "Sonnet", "Fake Sonnet"),
            choice("haiku", "Haiku", "Fake Haiku"),
        ],
        model: Some(model.into()),
    }
}

fn commands() -> Update {
    Update::Commands(vec![
        choice("review", "review", "Review the changes"),
        choice("compact", "compact", "Compact the conversation"),
    ])
}

fn offers(listening: bool) -> Update {
    Update::Offers(Offers {
        steer: true,
        rewind: true,
        listening,
        arguments: vec![("review".into(), "[focus]".into())],
    })
}

fn said(text: &str) -> Update {
    Update::AssistantChunk(text.into())
}

fn user(text: &str) -> Entry {
    Entry::User(vec![Block::Text(text.into())])
}

/// The next update, which must announce call `id` made by the subagent of
/// call `parent`, or by Claude Code itself.
fn expect_call(fake: &mut Fake, id: &str, parent: Option<&str>) -> ToolCall {
    match fake.next() {
        Update::Tool(call) => {
            assert_eq!((call.id.as_str(), call.parent.as_deref()), (id, parent));
            call
        }
        other => panic!("the call {id}, got {other:?}"),
    }
}

/// The next update, which must change call `id`.
fn expect_patch(fake: &mut Fake, id: &str) -> ToolPatch {
    match fake.next() {
        Update::ToolPatch(patch) => {
            assert_eq!(patch.id, id);
            patch
        }
        other => panic!("a change of the call {id}, got {other:?}"),
    }
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_until_gone(pids: &[u32]) {
    let deadline = Instant::now() + PATIENCE;
    while pids.iter().any(|pid| alive(*pid)) {
        assert!(Instant::now() < deadline, "still running: {pids:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// The record Claude Code keeps of session `session` of folder `cwd`.
fn record_of(home: &Path, cwd: &Path, session: &str) -> PathBuf {
    let slug: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join("projects")
        .join(slug)
        .join(format!("{session}.jsonl"))
}

// Start.

#[test]
fn claude_code_is_started_as_the_sdk_starts_it_and_greeted() {
    let mut fake = Fake::start_with(|options| {
        options.model = Some("sonnet".into());
        options.mcp_servers.push(McpServer {
            name: "kou".into(),
            command: "/bin/kou".into(),
            args: vec!["mcp".into()],
            env: vec![("KOU_PANE".into(), "3".into())],
        });
        options.instructions = Some("Drive other panes with the kou tools.".into());
        options.plugins.push("/plugins/kou".into());
        options.env.push(("KOU_ACCOUNT".into(), "two".into()));
    });
    fake.offers = true;
    // Claude Code names the session as soon as it is greeted. Nothing is
    // kept of a conversation nothing was said in, so there is nothing to
    // go on with yet: the interface hears its id with the first prompt.
    fake.expect([ready("", "default", "sonnet"), commands(), offers(false)]);
    let id = fake.ask("inspect");
    fake.expect([ready("sess-new", "default", "sonnet")]);
    let seen = fake.next_json();
    let args: Vec<String> = serde_json::from_value(seen["args"].clone()).unwrap();
    assert_eq!(
        args,
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json",
            "--include-partial-messages",
            "--replay-user-messages",
            "--permission-prompt-tool",
            "stdio",
            "--allow-dangerously-skip-permissions",
            "--model",
            "sonnet",
            "--mcp-config",
            r#"{"mcpServers":{"kou":{"command":"/bin/kou","args":["mcp"],"env":{"KOU_PANE":"3"}}}}"#,
            "--append-system-prompt",
            "Drive other panes with the kou tools.",
            "--plugin-dir",
            "/plugins/kou",
        ]
    );
    let env = &seen["env"];
    assert_eq!(env["CLAUDE_CODE_SDK_READS_SESSION_STATE"], "1");
    assert_eq!(env["KOU_ACCOUNT"], "two");
    assert_eq!(env["CLAUDE_CONFIG_DIR"], json!(fake.home));
    for gone in [
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SSE_PORT",
    ] {
        assert!(env.get(gone).is_none(), "{gone} was passed on");
    }
    // The app's own environment is passed on.
    assert_eq!(env["PATH"], json!(std::env::var("PATH").unwrap()));
    assert_eq!(seen["cwd"], json!(fake.cwd));
    assert_eq!(
        seen["initialize"],
        json!({"subtype": "initialize", "forwardSubagentText": true})
    );
    assert_eq!(
        seen["prompt"],
        json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": "inspect"}]},
            "parent_tool_use_id": null,
            "session_id": "sess-new",
            "uuid": id,
            "origin": {"kind": "human"},
        })
    );
    // The answer is given and nothing runs on: the turn ends at `idle`.
    fake.expect([
        offers(true),
        Update::TurnEnded(Stop::Finished),
        offers(false),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
    assert_eq!(fake.thread.session.as_deref(), Some("sess-new"));
    assert_eq!(fake.thread.prompt_id(0), Some(id.as_str()));

    fake.send(Command::Close);
    fake.expect([Update::Closed(None)]);
    fake.expect_end();
    assert_eq!(fake.thread.state(), &ThreadState::Closed(None));
}

#[test]
fn a_program_that_cannot_be_started_is_an_error() {
    let error = connect(Options::new("/no/such/claude", std::env::temp_dir())).unwrap_err();
    assert_eq!(error.to_string(), "could not start /no/such/claude");
    let error = connect(Options::new(fake_claude(), "relative")).unwrap_err();
    assert!(
        error.to_string().contains("must be an absolute path"),
        "{error}"
    );
    let error = connect(Options::new(fake_claude(), "/no/such/folder")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "the session's folder /no/such/folder does not exist"
    );
}

#[test]
fn claude_code_that_dies_at_once_closes_the_session_with_its_last_words() {
    let mut fake = Fake::start_with(|options| {
        options.env.push(("CLAUDE_FAKE_CRASH".into(), "1".into()));
    });
    fake.expect([Update::Closed(Some(
        "claude_fake exited with code 3:\nfake claude: cannot read its settings\nfake claude: giving up"
            .into(),
    ))]);
    fake.expect_end();
}

// Turns.

#[test]
fn an_answer_streams_as_it_is_written_and_its_whole_message_says_nothing_twice() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("chunks");
    fake.expect([
        Update::ThoughtChunk("Let me ".into()),
        Update::ThoughtChunk("think.".into()),
        said("Hel"),
        said("lo."),
        // Another message of the same answer is another paragraph.
        said("\n\nSecond."),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(
        fake.thread.entries(),
        [
            user("chunks"),
            Entry::Thought("Let me think.".into()),
            Entry::Assistant("Hello.\n\nSecond.".into()),
        ]
    );
    // A message that did not stream is said whole.
    fake.ask("whole");
    fake.expect([said("All at once."), Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn what_a_turn_cost_and_the_context_it_fills_are_said() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.usage = true;
    fake.ask_first("usage");
    let context = Usage {
        context_used: Some(200),
        ..Usage::default()
    };
    fake.expect([
        Update::Usage(context),
        said("Counted."),
        Update::Usage(Usage {
            context_used: Some(200),
            context_limit: Some(200_000),
            input_tokens: Some(1010),
            output_tokens: Some(202),
            cost_usd: Some(0.25),
        }),
        Update::TurnEnded(Stop::Finished),
    ]);
    // A turn that failed says why.
    fake.usage = false;
    fake.ask("fail");
    fake.expect([Update::TurnEnded(Stop::Failed(
        "The fake ran out of luck".into(),
    ))]);
}

// Tools.

#[test]
fn a_tool_call_is_announced_once_and_its_result_patches_it() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("tool");
    fake.expect([said("Reading.")]);
    // Shown as it begins; its input, once whole, changes it.
    expect_call(&mut fake, "read-1", None);
    let input = expect_patch(&mut fake, "read-1");
    assert_eq!(input.status, None);
    // Its result.
    expect_patch(&mut fake, "read-1");
    // A call that comes whole only.
    expect_call(&mut fake, "grep-1", None);
    expect_patch(&mut fake, "grep-1");
    fake.expect([said("It says hello."), Update::TurnEnded(Stop::Finished)]);
    let calls: Vec<&str> = fake
        .thread
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool(call) => Some(call.id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["read-1", "grep-1"]);
    assert_eq!(
        fake.thread.entries()[1],
        Entry::Assistant("Reading.".into())
    );
}

#[test]
fn a_tool_the_api_runs_itself_is_a_call_patched_by_its_result_in_the_answer() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("search");
    expect_call(&mut fake, "srv-1", None);
    // Its input, then its result.
    expect_patch(&mut fake, "srv-1");
    let found = expect_patch(&mut fake, "srv-1");
    assert_eq!(found.status, Some(kou_thread::ToolStatus::Done));
    expect_call(&mut fake, "srv-2", None);
    expect_patch(&mut fake, "srv-2");
    // A failure is said in what it gives back, with no `is_error`.
    let failed = expect_patch(&mut fake, "srv-2");
    assert_eq!(failed.status, Some(kou_thread::ToolStatus::Failed));
    fake.expect([said("Found it."), Update::TurnEnded(Stop::Finished)]);
    let calls: Vec<(&str, kou_thread::ToolStatus)> = fake
        .thread
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool(call) => Some((call.id.as_str(), call.status)),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        [
            ("srv-1", kou_thread::ToolStatus::Done),
            ("srv-2", kou_thread::ToolStatus::Failed)
        ]
    );
}

#[test]
fn the_to_do_tool_states_the_plan() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("todo");
    let seen = fake.until_turn_ends();
    let todos = json!({"todos": [
        {"content": "Read the notes", "status": "completed", "activeForm": "Reading the notes"},
        {"content": "Answer", "status": "in_progress", "activeForm": "Answering"},
    ]});
    match records::tools::plan("TodoWrite", &todos) {
        Some(steps) => {
            assert!(seen.contains(&Update::Plan(steps)), "{seen:#?}");
            assert!(
                !seen.iter().any(
                    |update| matches!(update, Update::ToolPatch(patch) if patch.id == "todo-1")
                ),
                "{seen:#?}"
            );
        }
        // (Where the records' conversion knows no plan, it is a call.)
        None => assert!(
            seen.iter()
                .any(|update| matches!(update, Update::Tool(call) if call.id == "todo-1")),
            "{seen:#?}"
        ),
    }
    assert!(seen.contains(&said("Planned.")));
}

#[test]
fn the_task_tools_change_the_plan_as_their_results_say() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("tasks");
    let seen = fake.until_turn_ends();
    let mut tasks = records::tools::Tasks::default();
    let created = tasks.apply(
        "TaskCreate",
        &json!({"subject": "Fix it", "description": "Fix the bug", "activeForm": "Fixing it"}),
        &json!("Task #1 created successfully: Fix it"),
        Some(&json!({"task": {"id": "1", "subject": "Fix it"}})),
        false,
    );
    let updated = tasks.apply(
        "TaskUpdate",
        &json!({"taskId": "1", "status": "in_progress"}),
        &json!("Updated task #1 status"),
        Some(&json!({"success": true, "taskId": "1", "updatedFields": ["status"]})),
        false,
    );
    let shown = |id: &str| {
        seen.iter().any(|update| {
            matches!(update, Update::Tool(call) if call.id == id)
                || matches!(update, Update::ToolPatch(patch) if patch.id == id)
        })
    };
    if records::tools::is_plan("TaskCreate") {
        let plans: Vec<&Update> = seen
            .iter()
            .filter(|update| matches!(update, Update::Plan(_)))
            .collect();
        let expected: Vec<Update> = [created, updated]
            .into_iter()
            .flatten()
            .map(Update::Plan)
            .collect();
        assert_eq!(plans, expected.iter().collect::<Vec<_>>(), "{seen:#?}");
        assert!(!shown("task-1") && !shown("task-2"), "{seen:#?}");
    } else {
        assert!(shown("task-1") && shown("task-2"), "{seen:#?}");
    }
    assert!(seen.contains(&said("On it.")));
}

#[test]
fn a_subagent_s_calls_nest_under_its_own_and_its_words_are_not_the_answer() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("agent");
    expect_call(&mut fake, "agent-1", None);
    fake.expect([
        Update::AgentSaid {
            call: "agent-1".into(),
            text: "Looking ".into(),
        },
        Update::AgentSaid {
            call: "agent-1".into(),
            text: "around.".into(),
        },
    ]);
    expect_call(&mut fake, "sub-1", Some("agent-1"));
    expect_patch(&mut fake, "sub-1");
    fake.expect([Update::AgentSaid {
        call: "agent-1".into(),
        text: "\n\nOne file.".into(),
    }]);
    expect_patch(&mut fake, "agent-1");
    fake.expect([
        said("The agent found one file."),
        Update::TurnEnded(Stop::Finished),
    ]);
    let Entry::Tool(launch) = &fake.thread.entries()[1] else {
        panic!("the agent's call, got {:?}", fake.thread.entries());
    };
    // It ran in the foreground: nothing of it is left running.
    assert!(!launch.work.background && !launch.work.ended);
    assert_eq!(launch.work.steps, 1);
    assert_eq!(launch.work.said.len(), 2);
    assert_eq!(launch.work.said[0].text, "Looking around.");
    assert_eq!(
        fake.thread.entries().last(),
        Some(&Entry::Assistant("The agent found one file.".into()))
    );
}

// Leave to use a tool.

/// Start the turn that asks leave to run `rm -rf build`, up to the question.
fn ask_leave(fake: &mut Fake) {
    fake.ask_first("permission");
    expect_call(fake, "bash-1", None);
    // What of its input is whole while it streams, then all of it.
    let grown = expect_patch(fake, "bash-1");
    assert_eq!(grown.raw_input, Some(json!({"command": "rm -rf build"})));
    let whole = expect_patch(fake, "bash-1");
    assert_eq!(whole.raw_input, Some(rm_input()));
    let Update::Permission(request) = fake.next() else {
        panic!("a permission request");
    };
    assert_eq!(request.id, 1);
    assert_eq!(request.tool.id, "bash-1");
    let option = |id: &str, label: &str, kind| PermissionOption {
        id: id.into(),
        label: label.into(),
        kind,
    };
    assert_eq!(
        request.options,
        [
            option("allow", "Yes", PermissionKind::AllowOnce),
            option(
                "always-0",
                "Yes, and don't ask again for Bash(rm -rf build)",
                PermissionKind::AllowAlways
            ),
            option("reject", "No", PermissionKind::RejectOnce),
        ]
    );
    assert_eq!(fake.thread.state(), &ThreadState::Blocked);
}

fn rm_input() -> Value {
    json!({"command": "rm -rf build", "description": "Remove the build"})
}

#[test]
fn leave_given_once_is_answered_as_the_sdk_allows() {
    let mut fake = Fake::start();
    fake.expect_ready();
    ask_leave(&mut fake);
    fake.send(Command::Permit {
        request: 1,
        option: "allow".into(),
    });
    let seen = fake.next_json();
    assert_eq!(
        seen["answer"],
        json!({
            "subtype": "success",
            "request_id": "perm-1",
            "response": {
                "behavior": "allow",
                "updatedInput": rm_input(),
                "toolUseID": "bash-1",
                "decisionClassification": "user_temporary",
            },
        })
    );
    assert_eq!(fake.thread.state(), &ThreadState::Working);
    expect_patch(&mut fake, "bash-1");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn leave_given_for_good_remembers_the_rule_claude_code_suggested() {
    let mut fake = Fake::start();
    fake.expect_ready();
    ask_leave(&mut fake);
    // An option never offered is not passed on: the question stands.
    fake.send(Command::Permit {
        request: 1,
        option: "always-7".into(),
    });
    assert!(matches!(fake.next(), Update::Permission(request) if request.id == 1));
    fake.send(Command::Permit {
        request: 1,
        option: "always-0".into(),
    });
    let seen = fake.next_json();
    assert_eq!(
        seen["answer"]["response"],
        json!({
            "behavior": "allow",
            "updatedInput": rm_input(),
            "toolUseID": "bash-1",
            "decisionClassification": "user_permanent",
            "updatedPermissions": [{
                "type": "addRules",
                "rules": [{"toolName": "Bash", "ruleContent": "rm -rf build"}],
                "behavior": "allow",
                "destination": "localSettings",
            }],
        })
    );
    expect_patch(&mut fake, "bash-1");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn leave_refused_is_a_denial() {
    let mut fake = Fake::start();
    fake.expect_ready();
    ask_leave(&mut fake);
    fake.send(Command::Permit {
        request: 1,
        option: "reject".into(),
    });
    let seen = fake.next_json();
    assert_eq!(
        seen["answer"]["response"],
        json!({
            "behavior": "deny",
            "message": "User refused permission to run tool",
            "toolUseID": "bash-1",
            "decisionClassification": "user_reject",
        })
    );
    expect_patch(&mut fake, "bash-1");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn a_question_left_open_when_the_turn_is_stopped_is_answered_no() {
    let mut fake = Fake::start();
    fake.expect_ready();
    ask_leave(&mut fake);
    fake.send(Command::Cancel);
    let seen = fake.next_json();
    assert_eq!(seen["interrupted"], true);
    assert_eq!(
        seen["answer"]["response"],
        json!({
            "behavior": "deny",
            "message": "The user cancelled this prompt.",
            "toolUseID": "bash-1",
            "decisionClassification": "user_reject",
        })
    );
    fake.expect([Update::TurnEnded(Stop::Cancelled)]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
    assert!(fake.thread.permissions().is_empty());
}

// Questions.

fn answers(lists: &[&[&str]]) -> Vec<Vec<String>> {
    lists
        .iter()
        .map(|items| items.iter().map(|item| (*item).to_owned()).collect())
        .collect()
}

fn option(id: &str, label: &str, kind: PermissionKind) -> PermissionOption {
    PermissionOption {
        id: id.into(),
        label: label.into(),
        kind,
    }
}

fn ask_input() -> Value {
    json!({"questions": [
        {"question": "Which library?", "header": "Library", "multiSelect": false, "options": [
            {"label": "chrono", "description": "Full-featured"},
            {"label": "time", "description": "Small"},
        ]},
        {"question": "Which features?", "header": "Features", "multiSelect": true, "options": [
            {"label": "serde", "description": "Serialization"},
            {"label": "tz", "description": "Time zones"},
        ]},
    ]})
}

/// Start the turn whose agent asks two questions, up to the request.
fn ask_questions(fake: &mut Fake) {
    fake.ask_first("ask");
    expect_call(fake, "ask-1", None);
    expect_patch(fake, "ask-1");
    let Update::Permission(request) = fake.next() else {
        panic!("the questions");
    };
    assert_eq!((request.id, request.tool.id.as_str()), (1, "ask-1"));
    assert_eq!(
        request.options,
        [option("skip", "Skip", PermissionKind::RejectOnce)]
    );
    let shown: Vec<(Option<&str>, &str, usize, bool, bool)> = request
        .questions
        .iter()
        .map(|question| {
            (
                question.header.as_deref(),
                question.text.as_str(),
                question.options.len(),
                question.multiple,
                question.free,
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            (Some("Library"), "Which library?", 2, false, true),
            (Some("Features"), "Which features?", 2, true, true),
        ]
    );
    assert_eq!(request.questions[0].options[1].label, "time");
    assert_eq!(
        request.questions[0].options[1].description.as_deref(),
        Some("Small")
    );
    assert_eq!(fake.thread.state(), &ThreadState::Blocked);
    assert_eq!(fake.thread.permissions()[0].questions.len(), 2);
}

#[test]
fn the_agent_s_questions_are_asked_and_the_answers_go_back_in_its_call() {
    let mut fake = Fake::start();
    fake.expect_ready();
    ask_questions(&mut fake);
    // A choice with words of the user's beside it, and a question left.
    fake.send(Command::Answer {
        request: 1,
        answers: answers(&[&["time", "for now"], &[]]),
    });
    let seen = fake.next_json();
    let mut input = ask_input();
    input["answers"] = json!({"Which library?": "time"});
    input["annotations"] = json!({
        "Which library?": {"notes": "for now"},
        "Which features?": {"unanswered": "skipped"},
    });
    assert_eq!(
        seen["answer"]["response"],
        json!({
            "behavior": "allow",
            "updatedInput": input,
            "toolUseID": "ask-1",
            "decisionClassification": "user_temporary",
        })
    );
    assert_eq!(fake.thread.state(), &ThreadState::Working);
    let result = expect_patch(&mut fake, "ask-1");
    assert_eq!(result.status, Some(kou_thread::ToolStatus::Done));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn questions_the_user_skips_are_refused_with_why() {
    let mut fake = Fake::start();
    fake.expect_ready();
    ask_questions(&mut fake);
    // (An option a request with questions does not have.)
    fake.send(Command::Permit {
        request: 1,
        option: "allow".into(),
    });
    assert!(matches!(fake.next(), Update::Permission(request) if request.id == 1));
    fake.send(Command::Permit {
        request: 1,
        option: "skip".into(),
    });
    let seen = fake.next_json();
    assert_eq!(
        seen["answer"]["response"],
        json!({
            "behavior": "deny",
            "message": "The user chose not to answer.",
            "toolUseID": "ask-1",
            "decisionClassification": "user_reject",
        })
    );
    expect_patch(&mut fake, "ask-1");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn questions_left_open_when_the_turn_is_stopped_are_refused() {
    let mut fake = Fake::start();
    fake.expect_ready();
    ask_questions(&mut fake);
    fake.send(Command::Cancel);
    let seen = fake.next_json();
    assert_eq!(seen["interrupted"], true);
    assert_eq!(
        seen["answer"]["response"]["message"],
        "The user cancelled this prompt."
    );
    fake.expect([Update::TurnEnded(Stop::Cancelled)]);
    assert!(fake.thread.permissions().is_empty());
}

/// Start the turn in which an MCP server asks for a form, up to the request.
fn ask_form(fake: &mut Fake) -> kou_thread::PermissionRequest {
    fake.ask_first("elicit");
    expect_call(fake, "mcp-1", None);
    let Update::Permission(request) = fake.next() else {
        panic!("the form");
    };
    assert_eq!(request.tool.id, "elicitation-1");
    assert_eq!(request.tool.title, "paint asks");
    assert_eq!(
        request.tool.content,
        [kou_thread::ToolContent::Text("Pick a color".into())]
    );
    assert_eq!(
        request.options,
        [option("decline", "Decline", PermissionKind::RejectOnce)]
    );
    let shown: Vec<(&str, Vec<&str>, bool)> = request
        .questions
        .iter()
        .map(|question| {
            let labels = question
                .options
                .iter()
                .map(|option| option.label.as_str())
                .collect();
            (question.text.as_str(), labels, question.free)
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("Color", vec!["Red", "Green"], false),
            ("How many", vec![], true),
        ]
    );
    request
}

#[test]
fn an_mcp_server_s_form_is_asked_on_a_call_of_its_own_and_answered_typed() {
    let mut fake = Fake::start();
    fake.expect_ready();
    let request = ask_form(&mut fake);
    // An answer that does not fit is said, and the form asked again.
    fake.send(Command::Answer {
        request: 1,
        answers: answers(&[&["Red"], &["many"]]),
    });
    fake.expect([
        Update::Notice(
            Level::Warning,
            r#""How many" takes a whole number, not "many"."#.into(),
        ),
        Update::Permission(request),
    ]);
    fake.send(Command::Answer {
        request: 1,
        answers: answers(&[&["Red"], &["3"]]),
    });
    let done = expect_patch(&mut fake, "elicitation-1");
    assert_eq!(done.status, Some(kou_thread::ToolStatus::Done));
    assert_eq!(
        done.content,
        Some(vec![
            kou_thread::ToolContent::Text("Pick a color".into()),
            kou_thread::ToolContent::Text("Color: Red\nHow many: 3".into()),
        ])
    );
    assert_eq!(
        fake.next_json()["answer"],
        json!({
            "subtype": "success",
            "request_id": "elicit-1",
            "response": {"action": "accept", "content": {"color": "red", "count": 3}},
        })
    );
    expect_patch(&mut fake, "mcp-1");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn a_form_declined_or_left_when_the_turn_stops_is_answered_so() {
    let mut fake = Fake::start();
    fake.expect_ready();
    ask_form(&mut fake);
    fake.send(Command::Permit {
        request: 1,
        option: "decline".into(),
    });
    let declined = expect_patch(&mut fake, "elicitation-1");
    assert_eq!(declined.status, Some(kou_thread::ToolStatus::Cancelled));
    assert_eq!(
        fake.next_json()["answer"]["response"],
        json!({"action": "decline"})
    );
    expect_patch(&mut fake, "mcp-1");
    fake.expect([Update::TurnEnded(Stop::Finished)]);

    fake.ask("elicit");
    expect_call(&mut fake, "mcp-1", None);
    assert!(matches!(fake.next(), Update::Permission(request) if request.id == 2));
    fake.send(Command::Cancel);
    expect_patch(&mut fake, "elicitation-2");
    let seen = fake.next_json();
    assert_eq!(seen["interrupted"], true);
    assert_eq!(seen["answer"]["response"], json!({"action": "cancel"}));
    fake.expect([Update::TurnEnded(Stop::Cancelled)]);
}

#[test]
fn a_form_with_nothing_to_fill_in_is_accepted_or_declined_and_a_page_to_open_is_declined() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("elicit empty");
    expect_call(&mut fake, "mcp-1", None);
    let Update::Permission(request) = fake.next() else {
        panic!("the form");
    };
    assert!(request.questions.is_empty());
    assert_eq!(
        request.options,
        [
            option("accept", "Accept", PermissionKind::AllowOnce),
            option("decline", "Decline", PermissionKind::RejectOnce),
        ]
    );
    fake.send(Command::Permit {
        request: 1,
        option: "accept".into(),
    });
    let accepted = expect_patch(&mut fake, "elicitation-1");
    assert_eq!(accepted.status, Some(kou_thread::ToolStatus::Done));
    assert_eq!(
        fake.next_json()["answer"]["response"],
        json!({"action": "accept", "content": {}})
    );
    expect_patch(&mut fake, "mcp-1");
    fake.expect([Update::TurnEnded(Stop::Finished)]);

    fake.ask("elicit url");
    expect_call(&mut fake, "mcp-1", None);
    fake.expect([Update::Notice(
        Level::Warning,
        "paint asked to open https://paint.example/login, which this app cannot show, so it was declined."
            .into(),
    )]);
    assert_eq!(
        fake.next_json()["answer"]["response"],
        json!({"action": "decline"})
    );
    expect_patch(&mut fake, "mcp-1");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn a_request_claude_code_takes_back_is_withdrawn_and_never_answered() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("withdraw");
    expect_call(&mut fake, "bash-w", None);
    assert!(matches!(fake.next(), Update::Permission(request) if request.id == 1));
    fake.expect([Update::Withdrawn(1)]);
    assert!(matches!(fake.next(), Update::Permission(request) if request.id == 2));
    fake.expect([Update::Withdrawn(2)]);
    let gone = expect_patch(&mut fake, "elicitation-2");
    assert_eq!(gone.status, Some(kou_thread::ToolStatus::Cancelled));
    assert!(fake.thread.permissions().is_empty());
    // Answers that come late are not passed on.
    fake.send(Command::Permit {
        request: 1,
        option: "allow".into(),
    });
    fake.send(Command::Answer {
        request: 2,
        answers: answers(&[&["Yes"]]),
    });
    fake.prompt("go on");
    fake.taken();
    assert_eq!(fake.next_json(), json!({"stray": []}));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn a_dialog_this_client_never_declared_is_answered_with_an_error() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("dialog");
    assert_eq!(
        fake.next_json(),
        json!({
            "subtype": "error",
            "request_id": "dialog-1",
            "error": "This client shows no dialog of kind refusal_fallback_prompt",
        })
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

// What a turn is.

#[test]
fn after_the_answer_the_session_listens_and_a_prompt_joins_the_open_turn() {
    let mut fake = Fake::start();
    fake.offers = true;
    fake.expect([ready("", "default", "default"), commands(), offers(false)]);
    fake.ask_first("background");
    expect_call(&mut fake, "bg-1", None);
    let launched = expect_patch(&mut fake, "bg-1");
    assert_eq!(launched.background, Some(true));
    // Its result: the agent was started.
    expect_patch(&mut fake, "bg-1");
    fake.expect([said("Started it."), offers(true)]);
    assert_eq!(fake.thread.state(), &ThreadState::Working);

    // Taken at once, into the turn that is open: no turn begins.
    fake.prompt("more");
    let joined = fake.taken();
    fake.expect([offers(false), said("Meanwhile: more"), offers(true)]);
    let ended = expect_patch(&mut fake, "bg-1");
    assert_eq!(ended.ended, Some(true));
    // What it used, said with its end.
    let tokens = expect_patch(&mut fake, "bg-1");
    assert_eq!((tokens.ended, tokens.tokens), (None, Some(1234)));
    // Claude Code goes on by itself for what the agent reported, in the
    // same turn, which is over only at `idle`.
    fake.expect([
        offers(false),
        said("\n\nThe agent is done."),
        offers(true),
        Update::TurnEnded(Stop::Finished),
        offers(false),
    ]);
    assert_eq!(fake.thread.prompt_id(3), Some(joined.as_str()));
    let Entry::Tool(launch) = &fake.thread.entries()[1] else {
        panic!("the agent's call");
    };
    assert!(launch.work.background && launch.work.ended);
    assert_eq!(launch.work.tokens, Some(1234));
}

#[test]
fn claude_code_going_on_by_itself_opens_a_turn_nobody_prompted() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("shell");
    expect_call(&mut fake, "sh-1", None);
    assert_eq!(expect_patch(&mut fake, "sh-1").background, Some(true));
    expect_patch(&mut fake, "sh-1");
    fake.expect([
        said("Started a command."),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(expect_patch(&mut fake, "sh-1").ended, Some(true));
    fake.expect([
        Update::TurnStarted,
        said("The command finished."),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
}

#[test]
fn an_older_claude_code_that_says_no_states_ends_its_turns_at_their_result() {
    let mut fake = Fake::start_with(|options| {
        options
            .env
            .push(("CLAUDE_FAKE_NO_STATES".into(), "1".into()));
    });
    fake.offers = true;
    fake.expect([ready("", "default", "default"), commands(), offers(false)]);
    fake.ask_first("shell");
    expect_call(&mut fake, "sh-1", None);
    expect_patch(&mut fake, "sh-1");
    expect_patch(&mut fake, "sh-1");
    fake.expect([
        said("Started a command."),
        Update::TurnEnded(Stop::Finished),
    ]);
    expect_patch(&mut fake, "sh-1");
    // Its cycle of its own is a turn of its own, over at its result.
    fake.expect([
        Update::TurnStarted,
        said("The command finished."),
        Update::TurnEnded(Stop::Finished),
    ]);
    // Never listening: nothing says when the turn would be over.
    fake.expect_quiet();
    assert!(!fake.thread.offers.listening);
}

// A message into a turn at work.

#[test]
fn a_message_handed_to_the_turn_at_work_is_shown_when_claude_code_takes_it() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("steer");
    fake.expect([said("Working.")]);
    fake.request(Request::Steer(vec![Block::Text("check the tests".into())]));
    fake.expect([Update::UserChunk(Block::Text("check the tests".into()))]);
    let Update::PromptId(id) = fake.next() else {
        panic!("the message's name");
    };
    fake.expect([
        said("Took: check the tests"),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(
        fake.thread.entries(),
        [
            user("steer"),
            Entry::Assistant("Working.".into()),
            user("check the tests"),
            Entry::Assistant("Took: check the tests".into()),
        ]
    );
    assert_eq!(fake.thread.prompt_id(2), Some(id.as_str()));

    // One the turn ended before is not sent: the interface sends it as a
    // prompt.
    fake.request(Request::Steer(vec![Block::Text("too late".into())]));
    fake.expect_quiet();
    fake.ask("history");
    let history = fake.next_json();
    assert!(!history.to_string().contains("too late"), "{history}");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

// Stop.

#[test]
fn a_stop_interrupts_claude_code_and_the_turn_ends_when_it_says_so() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("hang");
    fake.expect([said("Working on it.")]);
    fake.send(Command::Cancel);
    fake.expect([Update::TurnEnded(Stop::Cancelled)]);
    // The session goes on.
    fake.exchange("hello");
}

#[test]
fn a_second_stop_before_claude_code_answers_the_first_ends_the_turn_at_once() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("stubborn");
    fake.expect([said("Working on it.")]);
    fake.send(Command::Cancel);
    fake.expect_quiet();
    assert_eq!(fake.thread.state(), &ThreadState::Working);
    fake.send(Command::Cancel);
    fake.expect([Update::TurnEnded(Stop::Cancelled)]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
    fake.send(Command::Close);
    fake.expect([Update::Closed(None)]);
    fake.expect_end();
}

// Modes and models.

#[test]
fn a_mode_or_model_chosen_is_said_once_claude_code_takes_it() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.send(Command::SetMode("acceptEdits".into()));
    fake.expect([Update::Mode("acceptEdits".into())]);
    fake.send(Command::SetModel("sonnet".into()));
    fake.expect([Update::Model("sonnet".into())]);
    fake.send(Command::SetModel("nope".into()));
    fake.expect([Update::Notice(
        Level::Warning,
        "Claude Code did not switch to the model nope: Model nope is not available".into(),
    )]);
    // Claude Code says the same at its next turn: nothing changes.
    fake.ask("hello");
    fake.expect([
        ready("sess-new", "acceptEdits", "sonnet"),
        said("You said: "),
        said("hello"),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.mode.as_deref(), Some("acceptEdits"));
    assert_eq!(fake.thread.model.as_deref(), Some("sonnet"));
}

fn levels(ids: &[&str]) -> Vec<Choice> {
    ids.iter()
        .map(|id| {
            let name = match *id {
                "low" => "Low",
                "medium" => "Medium",
                "high" => "High",
                "xhigh" => "Extra high",
                "max" => "Max",
                other => panic!("no such level {other}"),
            };
            Choice {
                id: (*id).into(),
                name: name.into(),
                description: None,
            }
        })
        .collect()
}

fn efforts(ids: &[&str], current: Option<&str>) -> Update {
    Update::Efforts {
        levels: levels(ids),
        current: current.map(str::to_owned),
    }
}

#[test]
fn the_levels_of_effort_of_the_model_in_force_are_said_with_the_one_claude_code_runs_at() {
    let mut fake = Fake::start();
    fake.efforts = true;
    let all = ["low", "medium", "high", "xhigh", "max"];
    fake.expect([
        ready("", "default", "default"),
        commands(),
        efforts(&all, Some("high")),
    ]);
    // A level chosen is said once Claude Code takes it.
    fake.send(Command::SetEffort("max".into()));
    fake.expect([efforts(&all, Some("max"))]);
    fake.send(Command::SetEffort("nope".into()));
    fake.expect([Update::Notice(
        Level::Warning,
        "Claude Code did not switch to the effort level nope: it runs at max.".into(),
    )]);
    // Another model has levels of its own, and lowers one it lacks.
    fake.send(Command::SetModel("sonnet".into()));
    fake.expect([
        Update::Model("sonnet".into()),
        efforts(&["low", "medium", "high"], Some("high")),
    ]);
    // One without the choice has none.
    fake.send(Command::SetModel("haiku".into()));
    fake.expect([
        Update::Model("haiku".into()),
        ready("", "default", "haiku"),
        efforts(&[], None),
    ]);
    assert!(fake.thread.efforts.is_empty() && fake.thread.effort.is_none());
    fake.send(Command::SetModel("default".into()));
    fake.expect([
        Update::Model("default".into()),
        ready("", "default", "default"),
        efforts(&all, Some("max")),
    ]);
    // A level Claude Code says it changed to (`/effort` typed to it).
    fake.ask("effort low");
    fake.expect([
        ready("sess-new", "default", "default"),
        efforts(&all, Some("low")),
        said("Effort set."),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.effort.as_deref(), Some("low"));
    assert_eq!(fake.thread.efforts, levels(&all));
}

#[test]
fn auto_is_offered_while_the_model_in_force_can_run_in_it() {
    let mut fake = Fake::start_with(|options| {
        options.env.push(("CLAUDE_FAKE_MODE".into(), "auto".into()));
    });
    fake.expect([ready("", "auto", "default"), commands()]);
    // A model that cannot leaves `auto` for Accept edits, as the adapter
    // does, and the modes no longer offer it.
    fake.send(Command::SetModel("haiku".into()));
    fake.expect([
        Update::Model("haiku".into()),
        Update::Notice(
            Level::Warning,
            "The model chosen cannot run in the Auto mode, so the mode is now Accept edits.".into(),
        ),
        Update::Mode("acceptEdits".into()),
        ready("", "acceptEdits", "haiku"),
    ]);
    fake.send(Command::SetModel("default".into()));
    fake.expect([
        Update::Model("default".into()),
        ready("", "acceptEdits", "default"),
    ]);
    fake.send(Command::SetMode("auto".into()));
    fake.expect([Update::Mode("auto".into())]);
    assert_eq!(fake.thread.mode.as_deref(), Some("auto"));
    assert!(fake.thread.modes.iter().any(|mode| mode.id == "auto"));
}

#[test]
fn a_mode_claude_code_changes_by_itself_is_said() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("mode");
    fake.expect([
        Update::Mode("plan".into()),
        said("Planning."),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.mode.as_deref(), Some("plan"));
}

// Edits.

#[test]
fn an_edited_prompt_runs_in_a_copy_of_the_conversation_that_ends_before_it() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("one");
    fake.expect([
        said("You said: "),
        said("one"),
        Update::TurnEnded(Stop::Finished),
    ]);
    fake.exchange("two");
    fake.exchange("three");

    // "two" and what followed make way for the edit.
    fake.edit(2, "history");
    assert_eq!(fake.thread.rewinding(), Some(2));
    fake.expect([ready("", "default", "default"), commands()]);
    let Update::Rewound(Ok(edited)) = fake.next() else {
        panic!("the edit's answer");
    };
    fake.expect([Update::TurnStarted]);
    let Update::Ready { session, .. } = fake.next() else {
        panic!("the copy's id");
    };
    assert!(session.starts_with("sess-new-fork"), "{session}");
    assert_eq!(
        fake.next_json(),
        json!(["> one", "< You said: one", "> history"])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(fake.thread.rewinding(), None);
    assert_eq!(fake.thread.prompt_id(2), Some(edited.as_str()));
    assert_eq!(
        fake.thread.entries(),
        [
            user("one"),
            Entry::Assistant("You said: one".into()),
            user("history"),
            Entry::Assistant(r#"["> one","< You said: one","> history"]"#.into()),
        ]
    );
    // It was started on the copy, with the mode in force.
    fake.ask("inspect");
    let seen = fake.next_json();
    let args: Vec<String> = serde_json::from_value(seen["args"].clone()).unwrap();
    assert!(args.contains(&"--resume=sess-new".to_owned()), "{args:?}");
    assert!(
        args.iter()
            .any(|arg| arg.starts_with("--resume-session-at=")),
        "{args:?}"
    );
    assert!(args.contains(&"--fork-session".to_owned()), "{args:?}");
    fake.expect([Update::TurnEnded(Stop::Finished)]);

    // The edited prompt is edited again.
    fake.edit(2, "history");
    fake.expect([ready("", "default", "default"), commands()]);
    assert!(matches!(fake.next(), Update::Rewound(Ok(_))));
    fake.expect([Update::TurnStarted]);
    assert!(matches!(fake.next(), Update::Ready { .. }));
    assert_eq!(
        fake.next_json(),
        json!(["> one", "< You said: one", "> history"])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn editing_the_first_prompt_starts_a_new_conversation_with_the_app_s_note() {
    let mut fake = Fake::start_with(|options| {
        options.preface = Some("You run in kou.".into());
    });
    fake.expect_ready();
    fake.ask_first("one");
    fake.expect([
        said("You said: "),
        said("one"),
        Update::TurnEnded(Stop::Finished),
    ]);
    fake.send(Command::SetMode("acceptEdits".into()));
    fake.expect([Update::Mode("acceptEdits".into())]);
    fake.edit(0, "inspect");
    fake.expect([ready("", "acceptEdits", "default"), commands()]);
    assert!(matches!(fake.next(), Update::Rewound(Ok(_))));
    fake.expect([
        Update::TurnStarted,
        ready("sess-new-2", "acceptEdits", "default"),
    ]);
    let seen = fake.next_json();
    let args: Vec<String> = serde_json::from_value(seen["args"].clone()).unwrap();
    assert!(
        !args.iter().any(|arg| arg.starts_with("--resume")),
        "{args:?}"
    );
    let content = &seen["prompt"]["message"]["content"];
    assert!(
        content[0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("You run in kou.")),
        "{content}"
    );
    assert_eq!(content[1], json!({"type": "text", "text": "inspect"}));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(fake.thread.entries().len(), 2);
}

#[test]
fn an_edit_that_cannot_run_leaves_the_conversation_as_it_was() {
    // Nothing is kept on disk, so a copy of the conversation cannot be made.
    let mut fake = Fake::start_with(|options| options.env.clear());
    fake.expect_ready();
    fake.ask_first("one");
    fake.expect([
        said("You said: "),
        said("one"),
        Update::TurnEnded(Stop::Finished),
    ]);
    fake.exchange("two");
    let before = fake.thread.entries().to_vec();
    fake.edit(2, "two, edited");
    let Update::Rewound(Err(reason)) = fake.next() else {
        panic!("a failed edit");
    };
    assert!(
        reason.starts_with("Claude Code could not go back to before that prompt.")
            && reason.contains("No conversation found with session ID: sess-new"),
        "{reason}"
    );
    assert_eq!(fake.thread.entries(), before.as_slice());
    assert_eq!(fake.thread.rewinding(), None);
    // The conversation goes on where it was.
    fake.ask("history");
    assert_eq!(
        fake.next_json(),
        json!([
            "> one",
            "< You said: one",
            "> two",
            "< You said: two",
            "> history"
        ])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);

    // An edit while Claude Code works, or of a prompt it never had, is refused.
    fake.ask("hang");
    fake.expect([said("Working on it.")]);
    fake.request(Request::Rewind {
        prompt: None,
        blocks: vec![Block::Text("x".into())],
    });
    fake.expect([Update::Rewound(Err(
        "Claude Code is working: stop it or let it finish, then edit.".into(),
    ))]);
    fake.send(Command::Cancel);
    fake.expect([Update::TurnEnded(Stop::Cancelled)]);
    fake.request(Request::Rewind {
        prompt: Some("never-sent".into()),
        blocks: vec![Block::Text("x".into())],
    });
    fake.expect([Update::Rewound(Err(
        "That prompt was not sent or shown on this connection to Claude Code.".into(),
    ))]);
}

#[test]
fn an_edit_claude_code_refuses_after_its_greeting_leaves_the_conversation_as_it_was() {
    // No record is kept, and the copy is refused only once the new process
    // has answered the greeting.
    let mut fake = Fake::start_with(|options| {
        options.env = vec![("CLAUDE_FAKE_LATE_REFUSAL".into(), "1".into())];
    });
    fake.expect_ready();
    fake.ask_first("one");
    fake.expect([
        said("You said: "),
        said("one"),
        Update::TurnEnded(Stop::Finished),
    ]);
    fake.exchange("two");
    let before = fake.thread.entries().to_vec();
    fake.edit(2, "two, edited");
    // Nothing of the new process was taken up: no session announced.
    let Update::Rewound(Err(reason)) = fake.next() else {
        panic!("a failed edit");
    };
    assert!(
        reason.starts_with(
            "Claude Code could not go back to before that prompt. No conversation found with session ID: sess-new\nclaude_fake exited with code 1"
        ),
        "{reason}"
    );
    assert_eq!(fake.thread.entries(), before.as_slice());
    assert_eq!(fake.thread.rewinding(), None);
    fake.ask("history");
    assert_eq!(
        fake.next_json(),
        json!([
            "> one",
            "< You said: one",
            "> two",
            "< You said: two",
            "> history"
        ])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn an_edit_s_process_is_set_to_the_mode_and_level_of_effort_in_force() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.send(Command::SetEffort("low".into()));
    fake.send(Command::SetMode("plan".into()));
    fake.expect([Update::Mode("plan".into())]);
    fake.ask("one");
    fake.expect([
        ready("sess-new", "plan", "default"),
        said("You said: "),
        said("one"),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.effort.as_deref(), Some("low"));
    fake.edit(0, "inspect");
    fake.expect([ready("", "plan", "default"), commands()]);
    assert!(matches!(fake.next(), Update::Rewound(Ok(_))));
    fake.expect([Update::TurnStarted, ready("sess-new-2", "plan", "default")]);
    let seen = fake.next_json();
    assert_eq!(
        (&seen["mode"], &seen["effort"]),
        (&json!("plan"), &json!("low"))
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(fake.thread.effort.as_deref(), Some("low"));
}

// Going on with a conversation.

#[test]
fn a_conversation_gone_on_with_is_replayed_before_the_session_is_ready() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    let (cwd, home) = (root.join("work"), root.join("home"));
    std::fs::create_dir_all(&cwd).unwrap();
    let record = record_of(&home, &cwd, "sess-old");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    let entries = [
        json!({"parentUuid": null, "isSidechain": false, "type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "Old question"}]}, "uuid": "u-old", "timestamp": "2026-11-14T10:00:00.000Z", "sessionId": "sess-old", "cwd": cwd, "userType": "external", "version": "9.9.9-fake"}),
        json!({"parentUuid": "u-old", "isSidechain": false, "type": "assistant", "message": {"id": "m-old", "type": "message", "role": "assistant", "model": "claude-fake-opus", "content": [{"type": "text", "text": "Old answer"}], "stop_reason": "end_turn"}, "uuid": "a-old", "timestamp": "2026-11-14T10:00:01.000Z", "sessionId": "sess-old", "cwd": cwd, "userType": "external", "version": "9.9.9-fake"}),
    ];
    let lines: Vec<String> = entries.iter().map(Value::to_string).collect();
    std::fs::write(&record, lines.join("\n") + "\n").unwrap();

    let mut options = Options::new(fake_claude(), &cwd);
    options.home = Some(home.clone());
    options.env = vec![("CLAUDE_CONFIG_DIR".into(), home.display().to_string())];
    options.resume = Some("sess-old".into());
    let mut fake = Fake {
        backend: Some(connect(options).expect("the fake starts")),
        thread: Thread::new(),
        cwd: cwd.clone(),
        home: home.clone(),
        usage: false,
        offers: false,
        efforts: false,
        _folder: Some(folder),
    };
    // What the records make of it, first; then the session.
    let replayed = match records::transcript::record(&home, &cwd, "sess-old") {
        Some(found) => records::transcript::replay(&found),
        None => vec![Update::Notice(
            Level::Warning,
            "The record of session sess-old was not found here, so what was said in it is not shown."
                .into(),
        )],
    };
    fake.expect(replayed);
    fake.expect([ready("sess-old", "default", "default"), commands()]);
    fake.ask("history");
    assert_eq!(
        fake.next_json(),
        json!(["> Old question", "< Old answer", "> history"])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    fake.ask("inspect");
    let seen = fake.next_json();
    let args: Vec<String> = serde_json::from_value(seen["args"].clone()).unwrap();
    assert!(args.contains(&"--resume=sess-old".to_owned()), "{args:?}");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn the_name_claude_code_gives_the_conversation_is_said_after_the_turn_and_on_going_on() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("title Fixing the build");
    fake.expect([
        said("Named."),
        Update::TurnEnded(Stop::Finished),
        Update::Title("Fixing the build".into()),
    ]);
    // Said once only.
    fake.exchange("hello");
    fake.expect_quiet();
    assert_eq!(fake.thread.title.as_deref(), Some("Fixing the build"));

    // Gone on with: the record's replay names it, and nothing more is said
    // until the name changes.
    let mut fake = Fake::start_with(|options| options.resume = Some("sess-none".into()));
    let home = fake.home.clone();
    fake.expect([
        Update::Notice(
            Level::Warning,
            "Session sess-none has no record here, so this is a new conversation.".into(),
        ),
        ready("", "default", "default"),
        commands(),
    ]);
    fake.ask_first("title Old name");
    fake.until_turn_ends();
    fake.expect([Update::Title("Old name".into())]);
    let cwd = fake.cwd.clone();
    // (The folder outlives the first session, which is closed.)
    let folder = fake._folder.take();
    drop(fake);
    let mut options = Options::new(fake_claude(), &cwd);
    options.home = Some(home.clone());
    options.env = vec![("CLAUDE_CONFIG_DIR".into(), home.display().to_string())];
    options.resume = Some("sess-new".into());
    let mut fake = Fake {
        backend: Some(connect(options).expect("the fake starts")),
        thread: Thread::new(),
        cwd: cwd.clone(),
        home,
        usage: false,
        offers: false,
        efforts: false,
        _folder: folder,
    };
    let replayed = fake.until_ready();
    assert!(
        replayed.contains(&Update::Title("Old name".into())),
        "{replayed:#?}"
    );
    fake.expect([commands()]);
    fake.ask("title New name");
    fake.expect([
        said("Named."),
        Update::TurnEnded(Stop::Finished),
        Update::Title("New name".into()),
    ]);
}

#[test]
fn a_conversation_that_has_no_record_is_begun_anew_and_said_so() {
    // One nothing was said in (Claude Code keeps none), or one whose record
    // is gone: Claude Code would refuse to go on with it, and end.
    let mut fake = Fake::start_with(|options| options.resume = Some("sess-gone".into()));
    fake.expect([
        Update::Notice(
            Level::Warning,
            "Session sess-gone has no record here, so this is a new conversation.".into(),
        ),
        ready("", "default", "default"),
        commands(),
    ]);
    fake.ask_first("inspect");
    let seen = fake.next_json();
    let args: Vec<String> = serde_json::from_value(seen["args"].clone()).unwrap();
    assert!(
        !args.iter().any(|arg| arg.starts_with("--resume")),
        "{args:?}"
    );
    assert_eq!(seen["session"], "sess-new");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(fake.thread.session.as_deref(), Some("sess-new"));
}

#[test]
fn the_mode_claude_code_starts_in_is_the_one_it_says_when_greeted() {
    let mut fake = Fake::start_with(|options| {
        options.env.push(("CLAUDE_FAKE_MODE".into(), "plan".into()));
    });
    fake.expect([ready("", "plan", "default"), commands()]);
}

// The end.

/// Start the turn that leaves a child running, and return the process ids
/// of Claude Code and the child.
fn leave_a_child(fake: &mut Fake) -> [u32; 2] {
    fake.ask_first("pid");
    let Update::AssistantChunk(text) = fake.next() else {
        panic!("the process ids");
    };
    let pids: Vec<u32> = text
        .split(' ')
        .map(|pid| pid.parse().expect("a process id"))
        .collect();
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    let pids = [pids[0], pids[1]];
    assert!(pids.iter().all(|pid| alive(*pid)));
    pids
}

#[test]
fn close_ends_claude_code_and_what_it_started() {
    let mut fake = Fake::start();
    fake.expect_ready();
    let pids = leave_a_child(&mut fake);
    fake.send(Command::Close);
    fake.expect([Update::Closed(None)]);
    fake.expect_end();
    wait_until_gone(&pids);
}

#[test]
fn dropping_the_backend_ends_claude_code_and_what_it_started() {
    let mut fake = Fake::start();
    fake.expect_ready();
    let pids = leave_a_child(&mut fake);
    fake.backend = None;
    wait_until_gone(&pids);
}

// What is not known.

#[test]
fn what_is_not_known_is_skipped_and_a_question_not_understood_is_answered() {
    let mut fake = Fake::start();
    fake.expect_ready();
    fake.ask_first("junk");
    assert_eq!(
        fake.next_json(),
        json!({
            "subtype": "error",
            "request_id": "q-1",
            "error": "Unsupported control request subtype: a_question_from_later",
        })
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    fake.exchange("still here");
}

// Kept by a host.

/// A program built next to this test.
fn example(name: &str) -> PathBuf {
    let mut path = fake_claude();
    path.set_file_name(name);
    assert!(path.exists(), "{} is missing", path.display());
    path
}

fn started_at(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// A place for kept sessions: a folder with a short path for the hosts'
/// sockets, the fake's folder and home in it, and the processes the test
/// saw start, each with when it started (so that an id given to another
/// process since is never touched), killed if the test leaves them.
struct Keeping {
    keeper: Keeper,
    root: PathBuf,
    cwd: PathBuf,
    home: PathBuf,
    held: Vec<(u32, String)>,
    _folder: tempfile::TempDir,
}

impl Keeping {
    fn new() -> Self {
        let folder = tempfile::Builder::new()
            .prefix("kl")
            .tempdir_in("/tmp")
            .expect("a temporary folder");
        let root = folder.path().canonicalize().expect("a real path");
        let (cwd, home) = (root.join("work"), root.join("home"));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let keeper = Keeper {
            host: example("claude_host"),
            host_args: Vec::new(),
            stem: folder.path().join("s"),
            idle: Duration::from_secs(600),
            away: Duration::from_secs(86_400),
        };
        Self {
            keeper,
            root,
            cwd,
            home,
            held: Vec::new(),
            _folder: folder,
        }
    }

    fn connect(&self) -> Fake {
        self.connect_with(|_| {})
    }

    fn connect_with(&self, configure: impl FnOnce(&mut Options)) -> Fake {
        let mut options = Options::new(fake_claude(), &self.cwd);
        options.home = Some(self.home.clone());
        options.env = vec![("CLAUDE_CONFIG_DIR".into(), self.home.display().to_string())];
        options.keep = Some(self.keeper.clone());
        configure(&mut options);
        Fake {
            backend: Some(connect(options).expect("the fake starts")),
            thread: Thread::new(),
            cwd: self.cwd.clone(),
            home: self.home.clone(),
            usage: false,
            offers: false,
            efforts: false,
            _folder: None,
        }
    }

    /// Ask the fake who it is, in a turn of its own (`first`: the
    /// conversation's first prompt), and hold it and its host.
    fn hold(&mut self, fake: &mut Fake, first: bool) -> (u32, u32) {
        if first {
            fake.ask_first("whoami");
        } else {
            fake.ask("whoami");
        }
        let ids = fake.next_json();
        fake.expect([Update::TurnEnded(Stop::Finished)]);
        let id = |key: &str| ids[key].as_u64().expect("a process id") as u32;
        let (program, host) = (id("pid"), id("parent"));
        // Kept apart: its parent is no process of this test's.
        assert_ne!(host, std::process::id());
        for pid in [program, host] {
            let started = started_at(pid).expect("the process runs");
            self.held.push((pid, started));
        }
        (program, host)
    }

    fn still(&self, pid: u32) -> bool {
        self.held
            .iter()
            .any(|(held, started)| *held == pid && started_at(pid).as_ref() == Some(started))
    }

    /// Wait until the processes `pids` of those held are gone.
    fn wait_gone(&self, pids: &[u32]) {
        let deadline = Instant::now() + PATIENCE;
        while let Some(pid) = pids.iter().find(|pid| self.still(**pid)) {
            assert!(Instant::now() < deadline, "process {pid} is still running");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_all_gone(&self) {
        let pids: Vec<u32> = self.held.iter().map(|(pid, _)| *pid).collect();
        self.wait_gone(&pids);
    }

    /// Where the session's host of number `n` listens.
    fn socket(&self, n: u32) -> PathBuf {
        PathBuf::from(format!("{}-{n}.sock", self.keeper.stem.display()))
    }

    /// Wait until the record of `session` has a whole line with `text` in
    /// it: Claude Code writes a message there a moment after it has
    /// streamed it, and a line may be read half written.
    fn wait_recorded(&self, session: &str, text: &str) {
        let deadline = Instant::now() + PATIENCE;
        let whole = |record: String| {
            record
                .split_inclusive('\n')
                .any(|line| line.ends_with('\n') && line.contains(text))
        };
        while !std::fs::read_to_string(self.record(session)).is_ok_and(whole) {
            assert!(Instant::now() < deadline, "{text:?} was never recorded");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn record(&self, session: &str) -> PathBuf {
        record_of(&self.home, &self.cwd, session)
    }
}

impl Drop for Keeping {
    fn drop(&mut self) {
        // What a failing test left behind.
        for (pid, _) in &self.held {
            if self.still(*pid) {
                // SAFETY: a plain call, for a process this test started.
                unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL) };
            }
        }
    }
}

/// Close the session, and see it and everything held go.
fn close_kept(place: &Keeping, mut fake: Fake) {
    fake.send(Command::Close);
    fake.expect([Update::Closed(None)]);
    fake.expect_end();
    place.wait_all_gone();
}

/// Leave the session as the app does when it quits: its ends are dropped,
/// with no `Close`.
fn leave(mut fake: Fake) {
    drop(fake.backend.take());
}

#[test]
fn a_kept_session_works_as_one_that_is_not_and_close_ends_its_keeper() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    place.hold(&mut fake, true);
    assert!(place.socket(1).exists());
    fake.ask("permission");
    expect_call(&mut fake, "bash-1", None);
    expect_patch(&mut fake, "bash-1");
    expect_patch(&mut fake, "bash-1");
    assert!(matches!(fake.next(), Update::Permission(request) if request.id == 1));
    fake.send(Command::Permit {
        request: 1,
        option: "allow".into(),
    });
    assert_eq!(fake.next_json()["answer"]["response"]["behavior"], "allow");
    expect_patch(&mut fake, "bash-1");
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    close_kept(&place, fake);
    assert!(!place.socket(1).exists());
}

#[test]
fn a_kept_session_left_mid_turn_is_found_again_where_it_stands() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    place.hold(&mut fake, true);
    let go = place.root.join("go");
    fake.ask(&format!("hold {}", go.display()));
    fake.expect([said("Working on it.")]);
    place.wait_recorded("sess-new", "Working on it.");
    leave(fake);

    // Found again: what was said, from the record, then the session at
    // work, without a greeting.
    let replayed = records::transcript::replay_live(&place.record("sess-new"));
    let mut fake = place.connect();
    fake.expect(replayed);
    fake.expect([
        ready("sess-new", "default", "default"),
        commands(),
        Update::TurnStarted,
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Working);
    // What comes after is shown once, and the turn ends as Claude Code
    // ends it.
    std::fs::write(&go, "").unwrap();
    fake.expect([said("Done."), Update::TurnEnded(Stop::Finished)]);
    let answers: Vec<&Entry> = fake
        .thread
        .entries()
        .iter()
        .filter(|entry| matches!(entry, Entry::Assistant(text) if text.contains("Working on it.") || text == "Done."))
        .collect();
    assert_eq!(
        answers,
        [
            &Entry::Assistant("Working on it.".into()),
            &Entry::Assistant("Done.".into())
        ]
    );
    // It goes on as any session does.
    fake.exchange("more");
    close_kept(&place, fake);
}

#[test]
fn an_answer_begun_before_the_session_was_found_is_shown_whole_and_once() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    place.hold(&mut fake, true);
    let go = place.root.join("go");
    fake.ask(&format!("split {}", go.display()));
    fake.expect([said("Half")]);
    leave(fake);

    let replayed = records::transcript::replay_live(&place.record("sess-new"));
    let mut fake = place.connect();
    fake.expect(replayed);
    fake.expect([
        ready("sess-new", "default", "default"),
        commands(),
        Update::TurnStarted,
    ]);
    std::fs::write(&go, "").unwrap();
    // The block begun before, and the one begun after in a message begun
    // before: neither streams; each is said whole when it comes.
    fake.expect([
        said("Half and the rest."),
        said("\n\nMore."),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(
        fake.thread.entries().last(),
        Some(&Entry::Assistant("Half and the rest.\n\nMore.".into()))
    );
    close_kept(&place, fake);
}

#[test]
fn a_permission_asked_before_the_session_was_left_is_asked_again_when_it_is_found() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    place.hold(&mut fake, true);
    fake.ask("permission");
    expect_call(&mut fake, "bash-1", None);
    expect_patch(&mut fake, "bash-1");
    expect_patch(&mut fake, "bash-1");
    assert!(matches!(fake.next(), Update::Permission(_)));
    // Left: nothing is answered for the user.
    leave(fake);

    let replayed = records::transcript::replay_live(&place.record("sess-new"));
    let mut fake = place.connect();
    fake.expect(replayed);
    fake.expect([
        ready("sess-new", "default", "default"),
        commands(),
        Update::TurnStarted,
    ]);
    let Update::Permission(request) = fake.next() else {
        panic!("the question asked again");
    };
    assert_eq!((request.id, request.tool.id.as_str()), (1, "bash-1"));
    assert_eq!(request.options.len(), 3);
    assert_eq!(fake.thread.state(), &ThreadState::Blocked);
    fake.send(Command::Permit {
        request: 1,
        option: "allow".into(),
    });
    let seen = fake.next_json();
    assert_eq!(seen["answer"]["request_id"], "perm-1");
    assert_eq!(seen["answer"]["response"]["updatedInput"], rm_input());
    let result = expect_patch(&mut fake, "bash-1");
    assert_eq!(result.status, Some(kou_thread::ToolStatus::Done));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    close_kept(&place, fake);
}

#[test]
fn a_turn_that_ended_while_nobody_was_there_is_over_when_the_session_is_found() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    place.hold(&mut fake, true);
    let go = place.root.join("go");
    fake.ask(&format!("hold {}", go.display()));
    fake.expect([said("Working on it.")]);
    leave(fake);
    std::fs::write(&go, "").unwrap();
    let done = place.root.join("go.done");
    let deadline = Instant::now() + PATIENCE;
    while !done.exists() {
        assert!(Instant::now() < deadline, "the turn never ended");
        thread::sleep(Duration::from_millis(20));
    }
    // (The host hears the idle that came just before.)
    thread::sleep(Duration::from_millis(500));

    let replayed = records::transcript::replay(&place.record("sess-new"));
    let mut fake = place.connect();
    fake.expect(replayed);
    fake.expect([ready("sess-new", "default", "default"), commands()]);
    fake.expect_quiet();
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
    // (The record's two answers are one paragraph after the other.)
    assert!(
        matches!(fake.thread.entries().last(), Some(Entry::Assistant(text)) if text.ends_with("Done.")),
        "{:?}",
        fake.thread.entries()
    );
    close_kept(&place, fake);
}

#[test]
fn a_kept_session_closed_is_started_anew_from_its_record() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    place.hold(&mut fake, true);
    fake.exchange("one");
    close_kept(&place, fake);
    assert!(!place.socket(1).exists());

    let mut fake = place.connect_with(|options| options.resume = Some("sess-new".into()));
    let replayed = records::transcript::replay(&place.record("sess-new"));
    fake.expect(replayed);
    fake.expect([ready("sess-new", "default", "default"), commands()]);
    place.hold(&mut fake, false);
    fake.ask("history");
    let history = fake.next_json();
    let said: Vec<&str> = history
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .filter(|line| !line.starts_with("< {"))
        .collect();
    assert_eq!(
        said,
        [
            "> whoami",
            "> one",
            "< You said: one",
            "> whoami",
            "> history"
        ]
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    close_kept(&place, fake);
}

#[test]
fn an_edit_of_a_kept_session_moves_to_a_new_keeper_and_stops_the_old() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    let old = place.hold(&mut fake, true);
    fake.exchange("two");
    // The first prompt edited: a new conversation, in a new host.
    fake.edit(0, "whoami");
    fake.expect([ready("", "default", "default"), commands()]);
    assert!(matches!(fake.next(), Update::Rewound(Ok(_))));
    fake.expect([
        Update::TurnStarted,
        ready("sess-new-2", "default", "default"),
    ]);
    let ids = fake.next_json();
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    let new_host = ids["parent"].as_u64().unwrap() as u32;
    assert_ne!(new_host, old.1);
    for pid in [ids["pid"].as_u64().unwrap() as u32, new_host] {
        place
            .held
            .push((pid, started_at(pid).expect("the new process runs")));
    }
    place.wait_gone(&[old.0, old.1]);
    assert!(place.socket(2).exists());
    fake.exchange("still here");
    close_kept(&place, fake);
}

#[test]
fn an_edit_of_a_kept_session_that_fails_leaves_the_old_keeper() {
    let mut place = Keeping::new();
    // No record is kept, and the copy is refused once the new process has
    // answered the greeting.
    let mut fake = place.connect_with(|options| {
        options.env = vec![("CLAUDE_FAKE_LATE_REFUSAL".into(), "1".into())];
    });
    fake.expect_ready();
    let old = place.hold(&mut fake, true);
    fake.exchange("two");
    fake.edit(2, "two, edited");
    let Update::Rewound(Err(reason)) = fake.next() else {
        panic!("a failed edit");
    };
    assert!(
        reason.contains("No conversation found with session ID: sess-new"),
        "{reason}"
    );
    // The new keeper ended with its program; the old one goes on.
    let deadline = Instant::now() + PATIENCE;
    while place.socket(2).exists() {
        assert!(Instant::now() < deadline, "the new keeper is still there");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(place.still(old.0) && place.still(old.1));
    fake.ask("history");
    let history = fake.next_json();
    assert_eq!(
        history.as_array().unwrap().last(),
        Some(&json!("> history"))
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    close_kept(&place, fake);
}

#[test]
fn claude_code_dying_under_its_keeper_closes_the_session_with_its_last_words() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    place.hold(&mut fake, true);
    fake.ask("die");
    let reason = "claude_fake exited with code 7:\nfake claude: out of memory".to_owned();
    fake.expect([
        Update::TurnEnded(Stop::Failed(reason.clone())),
        Update::Closed(Some(reason)),
    ]);
    fake.expect_end();
    place.wait_all_gone();
}

#[test]
fn a_keeper_that_vanishes_closes_the_session_saying_so() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    let (_, host) = place.hold(&mut fake, true);
    // SAFETY: a plain call, for the host this test saw start.
    unsafe { libc::kill(host as libc::pid_t, libc::SIGKILL) };
    fake.expect([Update::Closed(Some(
        "the connection to the keeper of claude_fake was lost".into(),
    ))]);
    fake.expect_end();
    // Its program, its input gone, ends by itself.
    place.wait_all_gone();
}

#[test]
fn a_kept_session_taken_up_elsewhere_ends_quietly_and_claude_code_goes_on_there() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    let (program, _) = place.hold(&mut fake, true);
    let go = place.root.join("go");
    fake.ask(&format!("hold {}", go.display()));
    fake.expect([said("Working on it.")]);
    place.wait_recorded("sess-new", "Working on it.");
    // The same chat opened again (or another window): it finds the keeper
    // and takes it up.
    let replayed = records::transcript::replay_live(&place.record("sess-new"));
    let mut other = place.connect();
    other.expect(replayed);
    other.expect([
        ready("sess-new", "default", "default"),
        commands(),
        Update::TurnStarted,
    ]);
    // The first is over without a word to Claude Code, and says why.
    fake.expect([
        Update::TurnEnded(Stop::Cancelled),
        Update::Notice(
            Level::Warning,
            "Claude Code was taken up elsewhere (this chat was opened again, or another window of the app took it): it goes on there.".into(),
        ),
        Update::Closed(None),
    ]);
    fake.expect_end();
    assert!(place.still(program));
    std::fs::write(&go, "").unwrap();
    other.expect([said("Done."), Update::TurnEnded(Stop::Finished)]);
    close_kept(&place, other);
}

#[test]
fn a_keeper_that_ended_while_the_connection_was_lost_says_why() {
    let mut place = Keeping::new();
    let mut fake = place.connect();
    fake.expect_ready();
    let (_, host) = place.hold(&mut fake, true);
    // What the host leaves when it ends, there before the session sees the
    // connection end.
    let last = kou_claude::host::Last {
        reason: kou_claude::host::Reason::Idle,
        exit: kou_claude::host::Exit {
            code: None,
            signal: Some(15),
        },
        stderr: vec!["fake claude: bye".into()],
        ended: 0,
    };
    let mut path = place.socket(1).into_os_string();
    path.push(".last");
    std::fs::write(&path, serde_json::to_string(&last).unwrap()).unwrap();
    // Killed, so that it writes no word of its own over this one.
    // SAFETY: a plain call, for the host this test saw start.
    unsafe { libc::kill(host as libc::pid_t, libc::SIGKILL) };
    let reason = "the keeper of claude_fake ended (nobody came back to it, and nothing was going on): claude_fake was killed by signal 15:\nfake claude: bye";
    let closed = fake.next();
    assert_eq!(closed, Update::Closed(Some(reason.into())));
    fake.expect_end();
    place.wait_all_gone();
}

#[test]
fn a_keeper_never_greeted_is_greeted_anew_and_one_left_below_it_is_stopped() {
    let mut place = Keeping::new();
    let env = [(
        "CLAUDE_CONFIG_DIR".to_owned(),
        place.home.display().to_string(),
    )];
    let mut started = Vec::new();
    for n in [1, 2] {
        let keep = kou_claude::host::Keep {
            host: place.keeper.host.clone(),
            host_args: Vec::new(),
            socket: place.socket(n),
            idle: place.keeper.idle,
            away: place.keeper.away,
            stall: kou_claude::host::STALL,
        };
        let attached = keep
            .start(&fake_claude(), &[], &place.cwd, &env)
            .expect("the host starts");
        let program = attached.hello.pid;
        let output = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-p", &program.to_string()])
            .output()
            .unwrap();
        let host: u32 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap();
        for pid in [program, host] {
            place
                .held
                .push((pid, started_at(pid).expect("the process runs")));
        }
        started.push((program, host));
        // Left at once: nothing greeted it.
        drop(attached);
    }
    let mut fake = place.connect();
    fake.expect_ready();
    let (program, host) = place.hold(&mut fake, true);
    assert_eq!((program, host), started[1]);
    place.wait_gone(&[started[0].0, started[0].1]);
    assert!(!place.socket(1).exists());
    close_kept(&place, fake);
}
