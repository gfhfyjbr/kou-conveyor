//! `connect` against the scripted agent in `examples/acp_fake_agent.rs`.
//!
//! Each test asserts the exact updates a scenario produces, in order, and what
//! they add up to when folded through `kou_thread::Thread`, the way the
//! interface will see them. Run the whole suite with `cargo test -p kou-acp`,
//! which also builds the example these tests start.

#![cfg(unix)]

use kou_acp::{Launch, McpServer, Options, connect};
use kou_thread::{
    Backend, Block, Capabilities, Choice, Command, Entry, Level, Location, Offers, PermissionKind,
    PermissionOption, PermissionRequest, PlanStatus, PlanStep, Provider, Request, Stop, Thread,
    ThreadState, ToolCall, ToolContent, ToolKind, ToolPatch, ToolStatus, Update, Usage,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// Longer than anything here takes, short enough to fail a hung test.
const PATIENCE: Duration = Duration::from_secs(30);

/// The scripted agent's executable, built next to this test's.
fn fake_agent() -> PathBuf {
    let mut path = std::env::current_exe().expect("this test's path");
    path.pop();
    path.pop();
    path.push("examples");
    path.push("acp_fake_agent");
    assert!(
        path.exists(),
        "{} is missing: run `cargo test -p kou-acp`, which builds it",
        path.display()
    );
    path
}

/// A connection to the scripted agent, and the thread its updates add up to.
struct Fake {
    backend: Option<Backend>,
    thread: Thread,
    cwd: PathBuf,
    _folder: tempfile::TempDir,
}

impl Fake {
    fn start(flags: &[&str]) -> Self {
        Self::start_with(flags, None)
    }

    fn start_with(flags: &[&str], resume: Option<&str>) -> Self {
        Self::start_configured(flags, |options| {
            options.resume = resume.map(str::to_owned);
        })
    }

    /// Start the scripted agent with options set up by `configure`.
    fn start_configured(flags: &[&str], configure: impl FnOnce(&mut Options)) -> Self {
        let launch = Launch {
            program: fake_agent(),
            args: flags.iter().map(|flag| (*flag).to_owned()).collect(),
            env: vec![],
            cwd: PathBuf::new(),
        };
        Self::launch_configured(launch, configure)
    }

    /// The scripted agent with `--fork` and `flags`, told how it is told the
    /// message a fork ends at, and with options set up by `configure`.
    fn forking(flags: &[&str], configure: impl FnOnce(&mut Options)) -> Self {
        let flags: Vec<&str> = ["--fork"]
            .into_iter()
            .chain(flags.iter().copied())
            .collect();
        Self::start_configured(&flags, |options| {
            options.fork_meta = Some(air_fork());
            configure(options);
        })
    }

    fn launch(launch: Launch, resume: Option<&str>) -> Self {
        Self::launch_configured(launch, |options| {
            options.resume = resume.map(str::to_owned);
        })
    }

    fn launch_configured(launch: Launch, configure: impl FnOnce(&mut Options)) -> Self {
        let folder = tempfile::tempdir().expect("a temporary folder");
        // Resolved, so that a command's own idea of its directory matches.
        let cwd = folder.path().canonicalize().expect("a real path");
        let mut options = Options::new(cwd.clone(), "kou-acp tests", "1.2.3");
        configure(&mut options);
        Self {
            backend: Some(connect(launch, options).expect("the agent starts")),
            thread: Thread::new(),
            cwd,
            _folder: folder,
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
                    return update;
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

    fn send(&mut self, command: Command) {
        if let Command::Permit { request, .. } = &command {
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

    /// Edit the prompt of entry `index` the way the interface does: the
    /// thread lets go of it and what followed, and the backend is asked to
    /// run `text` in its place.
    fn edit(&mut self, index: usize, text: &str) {
        let blocks = vec![Block::Text(text.into())];
        let rewinding = self
            .thread
            .rewind(index, blocks.clone())
            .expect("a prompt to edit");
        self.backend()
            .requests
            .try_send(Request::Rewind {
                prompt: rewinding.prompt,
                blocks,
            })
            .expect("the backend takes requests");
    }

    /// Hand a message to the running turn the way the interface does.
    fn steer(&mut self, text: &str) {
        self.backend()
            .requests
            .try_send(Request::Steer(vec![Block::Text(text.into())]))
            .expect("the backend takes requests");
    }

    /// Send a prompt the scripted agent answers by repeating it, and see it
    /// named `prompt-<n>` and answered.
    fn exchange(&mut self, text: &str, n: usize) {
        self.prompt(text);
        self.expect([
            taken(n),
            Update::TurnStarted,
            said(&format!("You said: {text}")),
            Update::TurnEnded(Stop::Finished),
        ]);
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

    /// What the default agent reports when its session is ready.
    fn expect_ready(&mut self) {
        self.expect([ready("sess-1", false), providers(), commands()]);
        assert_eq!(self.thread.state(), &ThreadState::Idle);
    }

    fn in_cwd(&self, name: &str) -> PathBuf {
        self.cwd.join(name)
    }
}

fn choice(id: &str, name: &str, description: Option<&str>) -> Choice {
    Choice {
        id: id.into(),
        name: name.into(),
        description: description.map(str::to_owned),
    }
}

fn modes() -> Vec<Choice> {
    vec![
        choice("ask", "Ask", Some("Ask before changing anything")),
        choice("code", "Code", None),
    ]
}

fn ready(session: &str, resume: bool) -> Update {
    Update::Ready {
        session: session.into(),
        capabilities: Capabilities {
            images: true,
            embedded_files: true,
            resume,
        },
        modes: modes(),
        mode: Some("ask".into()),
        // The agent groups its models; the thread gets them flat.
        models: vec![
            choice("model-1", "Model 1", Some("The fast one")),
            choice("model-2", "Model 2", None),
        ],
        model: Some("model-1".into()),
    }
}

/// The groups the agent offers its models in, told after it is ready.
fn providers() -> Update {
    let group = |id: &str, name: &str, model: &str| Provider {
        id: id.into(),
        name: name.into(),
        models: vec![model.into()],
    };
    Update::Providers(vec![
        group("small", "Small", "model-1"),
        group("large", "Large", "model-2"),
    ])
}

fn commands() -> Update {
    Update::Commands(vec![
        choice("web", "web", Some("Search the web")),
        choice("test", "test", Some("Run the tests")),
    ])
}

fn said(text: &str) -> Update {
    Update::AssistantChunk(text.into())
}

/// How the scripted agent, as the Claude and Codex adapters, takes the
/// message a fork ends at: JetBrains AIR's extension.
fn air_fork() -> serde_json::Map<String, Value> {
    let fork = json!({"version": 1, "messageId": kou_acp::FORK_MESSAGE_ID});
    json!({"jetbrains": {"air": {"fork": fork}}})
        .as_object()
        .cloned()
        .expect("an object")
}

/// What a backend that edits prompts offers.
/// What an agent that edits no prompts says when it begins, or ceases, to
/// take a prompt with a turn open.
fn listening(listening: bool) -> Update {
    Update::Offers(Offers {
        listening,
        ..Offers::default()
    })
}

fn offers() -> Update {
    Update::Offers(Offers {
        rewind: true,
        ..Offers::default()
    })
}

/// The `n`th prompt this connection named, taken.
fn taken(n: usize) -> Update {
    Update::PromptTaken(Some(format!("prompt-{n}")))
}

fn user(text: &str) -> Entry {
    Entry::User(vec![Block::Text(text.into())])
}

fn status(id: &str, status: ToolStatus) -> Update {
    Update::ToolPatch(ToolPatch {
        id: id.into(),
        status: Some(status),
        ..ToolPatch::default()
    })
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

#[test]
fn the_handshake_tells_the_agent_what_this_client_can_do() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    assert_eq!(fake.thread.session.as_deref(), Some("sess-1"));
    assert_eq!(fake.thread.commands.len(), 2);

    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    assert_eq!(fake.thread.state(), &ThreadState::Working);
    let seen = fake.next_json();
    assert_eq!(
        seen["initialize"],
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {"readTextFile": true, "writeTextFile": true},
                "terminal": true,
                "session": {"notices": {}},
                "auth": {"terminal": true},
            },
            "clientInfo": {"name": "kou-acp tests", "version": "1.2.3"},
        })
    );
    assert_eq!(
        seen["opened"],
        json!({
            "method": "session/new",
            "params": {"cwd": fake.cwd, "mcpServers": []},
        })
    );
    assert_eq!(seen["prompt"], json!([{"type": "text", "text": "inspect"}]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);

    fake.send(Command::Close);
    fake.expect([Update::Closed(None)]);
    fake.expect_end();
    assert_eq!(fake.thread.state(), &ThreadState::Closed(None));
}

#[test]
fn a_prompt_streams_thoughts_and_an_answer_in_two_messages() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("chunks please");
    fake.expect([
        Update::TurnStarted,
        Update::ThoughtChunk("Let me ".into()),
        Update::ThoughtChunk("think.".into()),
        said("Hel"),
        said("lo."),
        // A new message id starts a new paragraph of the same answer.
        said("\n\nSecond."),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(
        fake.thread.entries(),
        [
            user("chunks please"),
            Entry::Thought("Let me think.".into()),
            Entry::Assistant("Hello.\n\nSecond.".into()),
        ]
    );
    assert_eq!(fake.thread.last_stop, Some(Stop::Finished));
}

#[test]
fn a_tool_call_is_announced_patched_and_ends_with_its_diffs() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("tool");
    let notes = fake.in_cwd("notes.txt");
    let announced = ToolCall {
        id: "call-1".into(),
        title: "Edit notes.txt".into(),
        kind: ToolKind::Edit,
        status: ToolStatus::Pending,
        content: vec![],
        locations: vec![Location {
            path: notes.clone(),
            line: Some(3),
        }],
        raw_input: Some(json!({"path": "notes.txt"})),
        raw_output: None,
        ..ToolCall::default()
    };
    let content = vec![
        ToolContent::Diff {
            path: notes,
            old: Some("old".into()),
            new: "new".into(),
        },
        ToolContent::Diff {
            path: fake.in_cwd("created.txt"),
            old: None,
            new: "fresh".into(),
        },
        ToolContent::Text("1 line changed".into()),
    ];
    fake.expect([
        Update::TurnStarted,
        Update::Tool(announced.clone()),
        status("call-1", ToolStatus::Running),
        Update::ToolPatch(ToolPatch {
            id: "call-1".into(),
            status: Some(ToolStatus::Done),
            content: Some(content.clone()),
            raw_output: Some(json!({"ok": true})),
            ..ToolPatch::default()
        }),
        said("Edited."),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(
        fake.thread.entries(),
        [
            user("tool"),
            Entry::Tool(ToolCall {
                status: ToolStatus::Done,
                content,
                raw_output: Some(json!({"ok": true})),
                ..announced
            }),
            Entry::Assistant("Edited.".into()),
        ]
    );
}

#[test]
fn what_a_subagent_says_and_does_goes_to_the_call_that_started_it() {
    use kou_thread::{AgentNote, AgentWork};
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("subagent");
    let asked = "Run the parser's tests.";
    let launch = ToolCall {
        id: "a1".into(),
        title: "Check the tests".into(),
        kind: ToolKind::Think,
        status: ToolStatus::Pending,
        content: vec![ToolContent::Text(asked.into())],
        raw_input: Some(json!({
            "description": "Check the tests", "prompt": asked, "subagent_type": "Explore",
        })),
        agent: Some("Explore".into()),
        ..ToolCall::default()
    };
    let step = ToolCall {
        id: "c1".into(),
        title: "Read src/parser.rs".into(),
        kind: ToolKind::Read,
        status: ToolStatus::Running,
        parent: Some("a1".into()),
        ..ToolCall::default()
    };
    let its = |text: &str| Update::AgentSaid {
        call: "a1".into(),
        text: text.into(),
    };
    // What the subagent was given and what it thought are not passed on;
    // what it says is, to its call, and never as the agent's own answer.
    fake.expect([
        Update::TurnStarted,
        Update::Tool(launch.clone()),
        Update::ToolPatch(ToolPatch {
            id: "a1".into(),
            status: Some(ToolStatus::Running),
            agent: Some(String::new()),
            ..ToolPatch::default()
        }),
        its("I will read "),
        its("the parser."),
        Update::Tool(step.clone()),
        Update::ToolPatch(ToolPatch {
            id: "c1".into(),
            status: Some(ToolStatus::Done),
            parent: Some("a1".into()),
            ..ToolPatch::default()
        }),
        // Another message of the subagent's: another paragraph.
        its("\n\nAll 12 pass."),
        Update::ToolPatch(ToolPatch {
            id: "a1".into(),
            status: Some(ToolStatus::Done),
            content: Some(vec![ToolContent::Text("All 12 pass.".into())]),
            agent: Some(String::new()),
            tokens: Some(48211),
            ..ToolPatch::default()
        }),
        said("They pass."),
        Update::TurnEnded(Stop::Finished),
    ]);
    // In the thread: the call keeps its kind, counts its subagent's call,
    // and holds what the subagent said before and after it.
    assert_eq!(
        fake.thread.entries(),
        [
            user("subagent"),
            Entry::Tool(ToolCall {
                status: ToolStatus::Done,
                content: vec![ToolContent::Text("All 12 pass.".into())],
                work: AgentWork {
                    steps: 1,
                    said: vec![
                        AgentNote {
                            after: 0,
                            text: "I will read the parser.".into(),
                        },
                        AgentNote {
                            after: 1,
                            text: "\n\nAll 12 pass.".into(),
                        },
                    ],
                    tokens: Some(48211),
                    ..AgentWork::default()
                },
                ..launch
            }),
            Entry::Tool(ToolCall {
                status: ToolStatus::Done,
                ..step
            }),
            Entry::Assistant("They pass.".into()),
        ]
    );
}

#[test]
fn a_command_left_running_is_told_apart_and_the_agent_s_own_turn_shows_as_one() {
    use kou_thread::AgentWork;
    let mut fake = Fake::start_configured(&[], |options| options.display_terminals = true);
    fake.expect_ready();
    fake.prompt("detach");
    let command = |id: &str, title: &str| ToolCall {
        id: id.into(),
        title: format!("`{title}`"),
        kind: ToolKind::Execute,
        status: ToolStatus::Pending,
        content: vec![ToolContent::Terminal {
            id: id.into(),
            output: String::new(),
            exit: None,
        }],
        raw_input: Some(json!({"command": title})),
        ..ToolCall::default()
    };
    let shows = |id: &str, output: &str, exit: Option<i32>| {
        Some(vec![ToolContent::Terminal {
            id: id.into(),
            output: output.into(),
            exit,
        }])
    };
    let placeholder = "Command running in background with ID: bash_1";
    fake.expect([
        Update::TurnStarted,
        Update::Tool(command("c1", "ls")),
        Update::Tool(command("b1", "npm run dev")),
        // One that ran and exited: its output, then its exit with its end.
        Update::ToolPatch(ToolPatch {
            id: "c1".into(),
            content: shows("c1", "notes.txt\n", None),
            ..ToolPatch::default()
        }),
        Update::ToolPatch(ToolPatch {
            id: "c1".into(),
            status: Some(ToolStatus::Done),
            content: shows("c1", "notes.txt\n", Some(0)),
            ..ToolPatch::default()
        }),
        // One whose call is over while its terminal has not exited: it was
        // left to run in the background.
        Update::ToolPatch(ToolPatch {
            id: "b1".into(),
            content: shows("b1", placeholder, None),
            ..ToolPatch::default()
        }),
        Update::ToolPatch(ToolPatch {
            id: "b1".into(),
            status: Some(ToolStatus::Done),
            content: shows("b1", placeholder, None),
            background: Some(true),
            ..ToolPatch::default()
        }),
        said("Started."),
        Update::TurnEnded(Stop::Finished),
        // After the turn it ends: said of its call, and no turn for that.
        Update::ToolPatch(ToolPatch {
            id: "b1".into(),
            content: shows("b1", placeholder, None),
            ended: Some(true),
            ..ToolPatch::default()
        }),
        // The agent goes on by itself: a turn of its own, which ends where
        // the cost of its cycle is said, not at a count in the middle.
        Update::TurnStarted,
        said("The server stopped."),
        // (A prompt sent while it goes on by itself is taken.)
        listening(true),
        Update::Tool(ToolCall {
            id: "r1".into(),
            title: "Read server.log".into(),
            kind: ToolKind::Read,
            status: ToolStatus::Done,
            ..ToolCall::default()
        }),
        Update::Usage(Usage {
            context_used: Some(61000),
            context_limit: Some(200000),
            ..Usage::default()
        }),
        said(" It ran out of memory."),
        Update::Usage(Usage {
            context_used: Some(62000),
            context_limit: Some(200000),
            cost_usd: Some(0.2),
            ..Usage::default()
        }),
        Update::TurnEnded(Stop::Finished),
        listening(false),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
    let work = |at: usize| match &fake.thread.entries()[at] {
        Entry::Tool(call) => call.work.clone(),
        other => panic!("no call at {at}: {other:?}"),
    };
    assert_eq!(work(1), AgentWork::default(), "it ran and exited");
    assert_eq!(
        work(2),
        AgentWork {
            background: true,
            ended: true,
            ..AgentWork::default()
        }
    );
    // The agent was told that this client shows its commands as terminals.
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    let seen = fake.next_json();
    assert_eq!(
        seen["initialize"]["clientCapabilities"]["_meta"],
        json!({"terminal_output": true})
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn a_turn_of_the_agent_s_own_is_stopped_or_gives_way_to_a_prompt() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    // It speaks up after its turn: a turn of its own, which nothing ends.
    fake.prompt("wake");
    fake.expect([
        Update::TurnStarted,
        Update::TurnEnded(Stop::Finished),
        Update::TurnStarted,
        said("Something came up."),
        listening(true),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Working);
    // The user stops it, as any turn: it is over at once, and the agent is
    // told a moment later.
    fake.send(Command::Cancel);
    fake.expect([Update::TurnEnded(Stop::Cancelled), listening(false)]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
    std::thread::sleep(Duration::from_millis(1100));
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    assert_eq!(fake.next_json()["cancels"], 1);
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    // Stopped to make way for a prompt, which is how the interface forces
    // one in: the prompt is taken, and the agent is not told to stop, which
    // would stop the prompt and whatever the agent left running.
    fake.prompt("wake");
    fake.expect([
        Update::TurnStarted,
        Update::TurnEnded(Stop::Finished),
        Update::TurnStarted,
        said("Something came up."),
        listening(true),
    ]);
    fake.send(Command::Cancel);
    fake.prompt("inspect");
    fake.expect([
        Update::TurnEnded(Stop::Cancelled),
        listening(false),
        Update::TurnStarted,
    ]);
    assert_eq!(fake.next_json()["cancels"], 1);
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    std::thread::sleep(Duration::from_millis(1100));
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    assert_eq!(fake.next_json()["cancels"], 1);
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    // Again; this time a prompt is sent while it is open: it gives way.
    fake.prompt("wake");
    fake.expect([
        Update::TurnStarted,
        Update::TurnEnded(Stop::Finished),
        Update::TurnStarted,
        said("Something came up."),
        listening(true),
    ]);
    fake.prompt("hello");
    fake.expect([
        Update::TurnEnded(Stop::Finished),
        Update::TurnStarted,
        listening(false),
        said("You said: hello"),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
}

#[test]
fn a_question_asked_in_a_turn_that_gives_way_to_a_prompt_is_asked_again_in_the_prompt_s_turn() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    // After its turn the agent asks for leave by itself: with no turn open
    // the question opens one of the agent's own, so there is a turn to end.
    fake.prompt("wake-ask");
    fake.expect([Update::TurnStarted, Update::TurnEnded(Stop::Finished)]);
    let mut opened = false;
    let request = loop {
        match fake.next() {
            Update::TurnStarted => opened = true,
            Update::Permission(request) => break request.id,
            _ => {}
        }
    };
    assert!(opened, "a turn of the agent's own, for the question");
    assert_eq!(fake.thread.state(), &ThreadState::Blocked);
    // A prompt is sent meanwhile: the agent's turn gives way to it. The
    // question is still the agent's to have answered: it is asked again in
    // the turn that takes over, where it would otherwise be refused at the
    // end without the user having seen it.
    fake.prompt("hello");
    let mut ended = false;
    loop {
        match fake.next() {
            Update::TurnEnded(Stop::Finished) => ended = true,
            Update::Permission(again) => {
                assert_eq!(again.id, request);
                break;
            }
            _ => {}
        }
    }
    assert!(ended, "the agent's own turn gave way");
    assert_eq!(fake.thread.state(), &ThreadState::Blocked);
    assert_eq!(fake.thread.permissions().len(), 1);
    // Answered, the agent goes on, and then takes the prompt.
    fake.send(Command::Permit {
        request,
        option: "allow".into(),
    });
    let mut said_all = String::new();
    loop {
        match fake.next() {
            Update::AssistantChunk(text) => said_all.push_str(&text),
            Update::TurnEnded(_) if said_all.contains("You said: hello") => break,
            _ => {}
        }
    }
    assert!(said_all.contains("selected: allow"), "{said_all}");
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
}

#[test]
fn an_agent_in_the_background_that_ended_is_told_from_one_that_runs_on() {
    // The agent's program says when an agent is started in the background
    // and never that one ended; the turn stays open for the last of them.
    let mut fake = Fake::start_configured(&[], |options| options.holds_turns = true);
    fake.expect_ready();
    fake.prompt("behind");
    let ended = |update: &Update, id: &str, ended: bool| matches!(update, Update::ToolPatch(patch) if patch.id == id && patch.ended == Some(ended));
    // The agent runs a cycle of its own because something in the
    // background ended: it was the one that spoke last before that cycle
    // and has no call running. The other, in the middle of a call, runs on.
    let mut seen = Vec::new();
    loop {
        let update = fake.next();
        let done = ended(&update, "b1", true);
        seen.push(update);
        if done {
            break;
        }
    }
    assert!(
        !seen.iter().any(|update| ended(update, "b2", true)),
        "the one at work is not taken for done"
    );
    let state = |fake: &Fake, id: &str| {
        fake.thread.entries().iter().find_map(|entry| match entry {
            Entry::Tool(call) if call.id == id => Some((call.work.background, call.work.ended)),
            _ => None,
        })
    };
    assert_eq!(state(&fake, "b1"), Some((true, true)));
    assert_eq!(state(&fake, "b2"), Some((true, false)));
    assert_eq!(
        fake.thread.state(),
        &ThreadState::Working,
        "the turn is held on"
    );
    // Heard of again, it is at work again.
    loop {
        if ended(&fake.next(), "b1", false) {
            break;
        }
    }
    assert_eq!(state(&fake, "b1"), Some((true, false)));
    fake.send(Command::Cancel);
    loop {
        if matches!(fake.next(), Update::TurnEnded(_)) {
            break;
        }
    }
}

#[test]
fn a_prompt_goes_at_once_to_an_agent_that_holds_its_turn_open_for_what_it_left_running() {
    let cost = Update::Usage(Usage {
        context_used: Some(1000),
        context_limit: Some(200000),
        cost_usd: Some(0.01),
        ..Usage::default()
    });
    let mut fake = Fake::start_configured(&[], |options| options.holds_turns = true);
    fake.expect_ready();
    // The agent answers, and holds the turn open: something it started
    // still runs. The interface is told that a prompt is taken meanwhile.
    fake.prompt("hold");
    fake.expect([
        Update::TurnStarted,
        said("Started it."),
        cost.clone(),
        listening(true),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Working);
    // One is sent: the held turn is over, the prompt runs, and the answer
    // to the first prompt, which the agent gives now, ends nothing.
    fake.prompt("hello");
    fake.expect([
        Update::TurnEnded(Stop::Finished),
        Update::TurnStarted,
        listening(false),
        said("You said: hello"),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
    // The agent was never told to stop: that would end what it left running.
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    assert_eq!(fake.next_json()["cancels"], 0);
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    // A stop of a held turn is still a stop.
    fake.prompt("hold");
    fake.expect([
        Update::TurnStarted,
        said("Started it."),
        cost,
        listening(true),
    ]);
    fake.send(Command::Cancel);
    fake.expect([listening(false), Update::TurnEnded(Stop::Cancelled)]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
}

#[test]
fn a_prompt_goes_into_the_turn_an_agent_holds_open_and_the_turn_is_held_on() {
    let offers = |listening| {
        Update::Offers(Offers {
            steer: true,
            listening,
            ..Offers::default()
        })
    };
    let cost = Update::Usage(Usage {
        context_used: Some(1000),
        context_limit: Some(200000),
        cost_usd: Some(0.01),
        ..Usage::default()
    });
    let mut fake = Fake::start_configured(&["--steer"], |options| options.holds_turns = true);
    fake.expect([
        ready("sess-1", false),
        providers(),
        offers(false),
        commands(),
    ]);
    // The agent answers, and holds the turn open for what it left running.
    fake.prompt("hold");
    fake.expect([
        Update::TurnStarted,
        said("Started it."),
        cost.clone(),
        offers(true),
    ]);
    // A prompt sent now goes into that turn: no turn ends and none begins,
    // the agent answers it, and holds the turn on. What it left running is
    // still work of a turn that runs.
    fake.prompt("hello");
    fake.expect([
        offers(false),
        said("Steered: hello"),
        cost.clone(),
        offers(true),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Working);
    assert_eq!(fake.thread.turn_from(), Some(1));
    // A stop is the one thing that stops it.
    fake.send(Command::Cancel);
    fake.expect([offers(false), Update::TurnEnded(Stop::Cancelled)]);
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    assert_eq!(fake.next_json()["cancels"], 1);
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn a_message_is_handed_to_the_running_turn_of_an_agent_that_takes_one_there() {
    let mut fake = Fake::start(&["--steer"]);
    // The agent said at the handshake that it takes one: the interface is told.
    fake.expect([
        ready("sess-1", false),
        providers(),
        Update::Offers(Offers {
            steer: true,
            ..Offers::default()
        }),
        commands(),
    ]);
    assert!(fake.thread.offers.steer);
    fake.prompt("steerable");
    fake.expect([Update::TurnStarted, said("Working.")]);
    // Handed over while the turn runs: the agent has it, the thread shows
    // it where it came in, and the same turn goes on and answers it.
    fake.steer("go left instead");
    fake.expect([
        Update::UserChunk(Block::Text("go left instead".into())),
        said("Steered: go left instead"),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
    // One for a turn that is over is not sent: the interface still holds
    // it, and sends it as a prompt.
    fake.steer("too late");
    // Nothing was stopped for any of it.
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    assert_eq!(fake.next_json()["cancels"], 0);
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn an_agent_that_takes_no_instructions_is_told_with_its_first_prompt() {
    let note = "<kou-desktop>\nA note from the app this session runs in, not from the user:\n\
                You run in a pane of kou desktop.\n</kou-desktop>";
    let configure = |options: &mut Options| {
        options.instructions = Some("The whole guidance.".into());
        options.preface = Some("You run in a pane of kou desktop.".into());
    };
    // It does not take instructions: the note goes before its first prompt,
    // once, and the thread shows the prompt as the user wrote it.
    let mut fake = Fake::start_configured(&[], configure);
    fake.expect_ready();
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    let seen = fake.next_json();
    assert_eq!(seen["opened"]["params"].get("systemPrompt"), None);
    assert_eq!(
        seen["prompt"],
        json!([{"type": "text", "text": note}, {"type": "text", "text": "inspect"}])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    assert_eq!(
        fake.next_json()["prompt"],
        json!([{"type": "text", "text": "inspect"}])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(
        fake.thread.entries()[0],
        user("inspect"),
        "the note is no part of what the user said"
    );
    // One that takes instructions gets those when its session opens, and
    // no note with a prompt.
    let mut fake = Fake::start_configured(&["--instructions"], configure);
    fake.expect_ready();
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    let seen = fake.next_json();
    assert_eq!(
        seen["opened"]["params"]["systemPrompt"]["content"][0]["text"],
        "The whole guidance."
    );
    assert_eq!(seen["prompt"], json!([{"type": "text", "text": "inspect"}]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

fn rm_call() -> ToolCall {
    ToolCall {
        id: "call-rm".into(),
        title: "Run rm -rf build".into(),
        kind: ToolKind::Execute,
        status: ToolStatus::Pending,
        raw_input: Some(json!({"command": "rm -rf build"})),
        ..ToolCall::default()
    }
}

fn rm_permission(id: u64) -> Update {
    let option = |id: &str, label: &str, kind| PermissionOption {
        id: id.into(),
        label: label.into(),
        kind,
    };
    Update::Permission(PermissionRequest {
        questions: Vec::new(),
        id,
        // The agent named only the call's id; the rest is what it announced.
        tool: rm_call(),
        options: vec![
            option("allow", "Allow once", PermissionKind::AllowOnce),
            option("always", "Always allow rm", PermissionKind::AllowAlways),
            option("reject", "Reject", PermissionKind::RejectOnce),
        ],
    })
}

#[test]
fn a_permission_request_waits_for_the_user_who_allows_then_rejects() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("permission");
    fake.expect([
        Update::TurnStarted,
        Update::Tool(rm_call()),
        rm_permission(1),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Blocked);
    assert_eq!(fake.thread.permissions()[0].request, 1);
    assert_eq!(fake.thread.permissions()[0].entry, 1);

    // A prompt sent now waits its turn: the agent must not see it yet.
    fake.prompt("second");
    // An answer the agent did not offer is not passed on, and the request stays.
    fake.send(Command::Permit {
        request: 1,
        option: "whatever".into(),
    });
    fake.expect([rm_permission(1)]);
    assert_eq!(fake.thread.state(), &ThreadState::Blocked);

    fake.send(Command::Permit {
        request: 1,
        option: "always".into(),
    });
    assert_eq!(fake.thread.state(), &ThreadState::Working);
    fake.expect([
        status("call-rm", ToolStatus::Done),
        said("selected: always; prompts received meanwhile: 0"),
        Update::TurnEnded(Stop::Finished),
        // Now the queued prompt goes out.
        Update::TurnStarted,
        said("You said: second"),
        Update::TurnEnded(Stop::Finished),
    ]);

    fake.prompt("permission again");
    fake.expect([
        Update::TurnStarted,
        Update::Tool(rm_call()),
        rm_permission(2),
    ]);
    fake.send(Command::Permit {
        request: 2,
        option: "reject".into(),
    });
    fake.expect([
        status("call-rm", ToolStatus::Failed),
        said("selected: reject; prompts received meanwhile: 0"),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert!(fake.thread.permissions().is_empty());
    assert_eq!(
        fake.thread.entries()[1],
        Entry::Tool(ToolCall {
            status: ToolStatus::Failed,
            ..rm_call()
        })
    );
}

#[test]
fn the_agent_reads_and_writes_files_through_the_client() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("files");
    fake.expect([Update::TurnStarted]);
    assert_eq!(
        fake.next_json(),
        json!({
            "written": {},
            "window": "beta\n",
            "whole": "alpha\nbeta\ngamma\n",
            "missing": -32002,
        })
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(
        std::fs::read_to_string(fake.in_cwd("made/by/agent.txt")).unwrap(),
        "alpha\nbeta\ngamma\n"
    );
}

#[test]
fn a_command_runs_through_the_client_and_shows_in_its_tool_call() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("terminal");
    // The agent asked for the last 24 bytes of what the command prints: its
    // variable, its directory (the session's), and ten digits on the error stream.
    let printed = format!("from the agent\n{}\n0123456789", fake.cwd.display());
    let kept = &printed[printed.len() - 24..];
    let shown = ToolCall {
        id: "call-sh".into(),
        title: "Run the script".into(),
        kind: ToolKind::Execute,
        status: ToolStatus::Running,
        content: vec![ToolContent::Terminal {
            id: "term-1".into(),
            output: kept.into(),
            exit: Some(3),
        }],
        ..ToolCall::default()
    };
    fake.expect([
        Update::TurnStarted,
        Update::Tool(shown.clone()),
        status("call-sh", ToolStatus::Failed),
    ]);
    assert_eq!(
        fake.next_json(),
        json!({
            "exit": {"exitCode": 3},
            "output": {"output": kept, "truncated": true, "exitStatus": {"exitCode": 3}},
            "killed": {},
            "released": {},
            // Released, the terminal is gone for the agent.
            "after": -32002,
        })
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    // The thread keeps showing what it printed.
    assert_eq!(
        fake.thread.entries()[1],
        Entry::Tool(ToolCall {
            status: ToolStatus::Failed,
            ..shown
        })
    );
}

#[test]
fn a_running_command_is_mirrored_into_the_thread_as_it_prints() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("live-terminal");
    fake.expect([Update::TurnStarted]);
    let whole = "ready\ndone\n";
    let terminal = |update: &Update| match update {
        Update::Tool(ToolCall { id, content, .. })
        | Update::ToolPatch(ToolPatch {
            id,
            content: Some(content),
            ..
        }) if id == "call-live" => match content.as_slice() {
            [ToolContent::Terminal { id, output, exit }] if id == "term-1" => {
                Some((output.clone(), *exit))
            }
            _ => None,
        },
        _ => None,
    };

    let announced = fake.next();
    let (first, exit) = terminal(&announced).expect("a tool call showing the terminal");
    assert!(whole.starts_with(&first), "{first:?}");
    assert_eq!(exit, None);

    // How often the output is refreshed depends on timing; that it grows,
    // and ends complete with the exit status before the agent goes on, does not.
    let mut seen = vec![(first, exit)];
    let after = loop {
        let update = fake.next();
        match terminal(&update) {
            Some(state) => seen.push(state),
            None => break update,
        }
    };
    assert!(seen.len() >= 2, "{seen:?}");
    for pair in seen.windows(2) {
        assert!(pair[1].0.starts_with(&pair[0].0), "{seen:?}");
        assert_ne!(pair[0], pair[1], "{seen:?}");
    }
    assert_eq!(seen.last(), Some(&(whole.to_owned(), Some(0))));
    assert_eq!(after, status("call-live", ToolStatus::Done));
    assert_eq!(fake.next_json(), json!({"exit": {"exitCode": 0}}));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(
        fake.thread.entries()[1],
        Entry::Tool(ToolCall {
            id: "call-live".into(),
            title: "Run slowly".into(),
            kind: ToolKind::Execute,
            status: ToolStatus::Done,
            content: vec![ToolContent::Terminal {
                id: "term-1".into(),
                output: whole.into(),
                exit: Some(0),
            }],
            ..ToolCall::default()
        })
    );
}

#[test]
fn a_plan_replaces_the_one_before_it() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("plan");
    let step = |text: &str, status| PlanStep {
        text: text.into(),
        status,
    };
    let last = vec![
        step("Read the code", PlanStatus::Done),
        step("Change it", PlanStatus::Running),
        step("Test it", PlanStatus::Pending),
    ];
    fake.expect([
        Update::TurnStarted,
        Update::Plan(vec![
            step("Read the code", PlanStatus::Running),
            step("Change it", PlanStatus::Pending),
        ]),
        said("Working."),
        Update::Plan(last.clone()),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(
        fake.thread.entries(),
        [
            user("plan"),
            Entry::Plan(last),
            Entry::Assistant("Working.".into())
        ]
    );
}

#[test]
fn modes_and_models_are_config_options_either_side_can_change() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    // The agent switches by itself, and sends its whole configuration again.
    fake.prompt("mode");
    fake.expect([
        Update::TurnStarted,
        Update::Mode("code".into()),
        Update::TurnEnded(Stop::Finished),
    ]);
    // The mode is already `code`, so the larger model moves nothing else.
    fake.send(Command::SetModel("model-2".into()));
    fake.expect([Update::Model("model-2".into())]);
    fake.send(Command::SetMode("ask".into()));
    fake.expect([Update::Mode("ask".into())]);
    fake.send(Command::SetModel("model-1".into()));
    fake.expect([Update::Model("model-1".into())]);
    // The larger model forces `code`: one answer, two changes.
    fake.send(Command::SetModel("model-2".into()));
    fake.expect([Update::Mode("code".into()), Update::Model("model-2".into())]);
    fake.send(Command::SetModel("model-9".into()));
    fake.expect([Update::Notice(
        Level::Warning,
        "Fake Agent did not switch to model model-9: Invalid params: no such value".into(),
    )]);
    assert_eq!(fake.thread.mode.as_deref(), Some("code"));
    assert_eq!(fake.thread.model.as_deref(), Some("model-2"));
}

#[test]
fn an_agents_sessions_are_listed_without_opening_one() {
    let folder = tempfile::tempdir().expect("a temporary folder");
    let cwd = folder.path().canonicalize().expect("a real path");
    let launch = |flags: &[&str]| Launch {
        program: fake_agent(),
        args: flags.iter().map(|flag| (*flag).to_owned()).collect(),
        env: vec![],
        cwd: PathBuf::new(),
    };
    let client = ("kou-acp tests", "1.2.3");
    let listed = kou_acp::sessions(&launch(&["--list"]), &cwd, client, PATIENCE)
        .expect("the agent answers")
        .expect("it lists its sessions");
    // Both pages, the latest first, the one without a time last.
    let ids: Vec<&str> = listed.iter().map(|session| session.id.as_str()).collect();
    assert_eq!(ids, ["sess-new", "sess-old", "sess-untimed"]);
    assert_eq!(listed[0].title, "Review the diff");
    assert_eq!(listed[0].cwd, cwd);
    assert!(listed[0].updated > listed[1].updated && listed[1].updated.is_some());
    assert_eq!((listed[2].title.as_str(), listed[2].updated), ("", None));

    // An agent that does not list them says so, and one that never comes up
    // is an error, not a wait for ever.
    assert_eq!(
        kou_acp::sessions(&launch(&[]), &cwd, client, PATIENCE).expect("the agent answers"),
        None
    );
    let failed = kou_acp::sessions(&launch(&["--crash-on-start"]), &cwd, client, PATIENCE)
        .expect_err("no agent, no list");
    assert!(
        format!("{failed:#}").contains("ended without answering"),
        "{failed:#}"
    );
}

#[test]
fn the_models_of_a_session_are_asked_before_there_is_one_and_one_can_be_chosen_ahead() {
    let folder = tempfile::tempdir().expect("a temporary folder");
    let cwd = folder.path().canonicalize().expect("a real path");
    let launch = |flags: &[&str]| Launch {
        program: fake_agent(),
        args: flags.iter().map(|flag| (*flag).to_owned()).collect(),
        env: vec![],
        cwd: PathBuf::new(),
    };
    let client = ("kou-acp tests", "1.2.3");
    let offered = kou_acp::models(&launch(&[]), &cwd, client, PATIENCE)
        .expect("the agent answers")
        .expect("it offers models");
    let ids: Vec<&str> = offered
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    assert_eq!(ids, ["model-1", "model-2"]);
    assert_eq!(offered.current.as_deref(), Some("model-1"));
    assert_eq!(Update::Providers(offered.providers), providers());
    // An agent with nothing to choose says so.
    assert_eq!(
        kou_acp::models(&launch(&["--no-selectors"]), &cwd, client, PATIENCE)
            .expect("the agent answers"),
        None
    );

    // A model chosen ahead is set as soon as the session is ready.
    let mut fake = Fake::start_configured(&[], |options| {
        options.model = Some("model-2".into());
    });
    fake.expect([ready("sess-1", false), providers(), commands()]);
    // (This agent answers a change of model with its whole list of options,
    // in which it has moved the mode too.)
    let changed = [fake.next(), fake.next()];
    assert!(
        changed.contains(&Update::Model("model-2".into())),
        "{changed:?}"
    );
    assert_eq!(fake.thread.model.as_deref(), Some("model-2"));
    // One the agent does not offer is said, and the session keeps its own.
    let mut fake = Fake::start_configured(&[], |options| {
        options.model = Some("model-9".into());
    });
    fake.expect([ready("sess-1", false), providers()]);
    let mut rest = [fake.next(), fake.next()];
    rest.sort_by_key(|update| matches!(update, Update::Notice(..)));
    assert_eq!(rest[0], commands());
    assert_eq!(
        rest[1],
        Update::Notice(
            Level::Warning,
            "Fake Agent does not offer the model model-9, and keeps its own".into()
        )
    );
}

#[test]
fn a_session_opened_again_gets_the_model_chosen_for_it_before() {
    // The agent opens an old session with its own default model; the one
    // the session was given earlier is set again as soon as it is ready.
    for flags in [&["--resume-only"][..], &["--load"][..]] {
        let mut fake = Fake::start_configured(flags, |options| {
            options.resume = Some("sess-old".into());
            options.model = Some("model-2".into());
        });
        let mut seen = Vec::new();
        while seen.len() < 12 && !seen.contains(&Update::Model("model-2".into())) {
            seen.push(fake.next());
        }
        assert!(
            seen.contains(&Update::Model("model-2".into())),
            "{flags:?}: {seen:?}"
        );
        assert!(
            seen.iter().any(
                |update| matches!(update, Update::Ready { session, .. } if session == "sess-old")
            ),
            "{flags:?}: {seen:?}"
        );
        assert_eq!(fake.thread.model.as_deref(), Some("model-2"), "{flags:?}");
    }
}

#[test]
fn an_agent_with_only_the_older_modes_switches_with_set_mode() {
    let mut fake = Fake::start(&["--legacy-modes"]);
    fake.expect([
        Update::Ready {
            session: "sess-1".into(),
            capabilities: Capabilities {
                images: true,
                embedded_files: true,
                resume: false,
            },
            modes: modes(),
            mode: Some("ask".into()),
            models: vec![],
            model: None,
        },
        commands(),
    ]);
    fake.send(Command::SetMode("code".into()));
    fake.expect([Update::Mode("code".into())]);
    fake.send(Command::SetMode("ask".into()));
    fake.expect([Update::Mode("ask".into())]);
    fake.send(Command::SetModel("model-2".into()));
    fake.expect([Update::Notice(
        Level::Warning,
        "Fake Agent offers no model to choose".into(),
    )]);
    fake.prompt("mode");
    fake.expect([
        Update::TurnStarted,
        Update::Mode("code".into()),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.mode.as_deref(), Some("code"));

    let mut bare = Fake::start(&["--no-selectors"]);
    let Update::Ready { modes, mode, .. } = bare.next() else {
        panic!("ready first");
    };
    assert_eq!((modes, mode), (vec![], None));
    bare.expect([commands()]);
    bare.send(Command::SetMode("code".into()));
    bare.expect([Update::Notice(
        Level::Warning,
        "Fake Agent offers no mode to choose".into(),
    )]);
}

#[test]
fn cancelling_answers_the_open_question_and_ends_the_turn_as_cancelled() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    // Nothing runs yet, so there is nothing to cancel.
    fake.send(Command::Cancel);
    fake.prompt("cancel");
    let thinking = ToolCall {
        id: "call-long".into(),
        title: "Think for a long time".into(),
        kind: ToolKind::Think,
        status: ToolStatus::Running,
        ..ToolCall::default()
    };
    let asked = ToolCall {
        id: "call-ask".into(),
        title: "Delete everything".into(),
        kind: ToolKind::Delete,
        status: ToolStatus::Pending,
        ..ToolCall::default()
    };
    fake.expect([
        Update::TurnStarted,
        Update::Tool(thinking.clone()),
        Update::Permission(PermissionRequest {
            questions: Vec::new(),
            id: 1,
            tool: asked.clone(),
            options: vec![PermissionOption {
                id: "allow".into(),
                label: "Allow".into(),
                kind: PermissionKind::AllowOnce,
            }],
        }),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Blocked);

    fake.send(Command::Cancel);
    // A second one changes nothing.
    fake.send(Command::Cancel);
    fake.expect([
        // Marked at once, before the agent confirms.
        status("call-long", ToolStatus::Cancelled),
        status("call-ask", ToolStatus::Cancelled),
        // The agent got `cancelled` for its open question, and again for one
        // it asked after the cancellation, which never reached the user.
        said("stopped; the question got \"cancelled\", then \"cancelled\""),
        Update::TurnEnded(Stop::Cancelled),
    ]);
    assert_eq!(fake.thread.state(), &ThreadState::Idle);
    assert!(fake.thread.permissions().is_empty());
    assert_eq!(
        fake.thread.entries()[1..3],
        [
            Entry::Tool(ToolCall {
                status: ToolStatus::Cancelled,
                ..thinking
            }),
            Entry::Tool(ToolCall {
                status: ToolStatus::Cancelled,
                ..asked
            }),
        ]
    );
    // An answer that comes too late goes nowhere.
    fake.send(Command::Permit {
        request: 1,
        option: "allow".into(),
    });
    fake.prompt("still there?");
    fake.expect([
        Update::TurnStarted,
        said("You said: still there?"),
        Update::TurnEnded(Stop::Finished),
    ]);
}

#[test]
fn an_agent_that_wants_a_login_says_how_and_closes() {
    let mut fake = Fake::start(&["--auth"]);
    let hint = format!(
        "Fake Agent requires authentication. Log in outside this app, then open the session again:\n\
         - Log in with Fake (Opens the browser)\n\
         - Terminal login: run `FAKE_LOGIN=1 {} --auth --login` in a terminal",
        fake_agent().display()
    );
    fake.expect([
        Update::Notice(Level::Error, hint.clone()),
        Update::Closed(Some("Fake Agent requires authentication".into())),
    ]);
    fake.expect_end();
    assert_eq!(fake.thread.entries(), [Entry::Notice(Level::Error, hint)]);
    assert_eq!(
        fake.thread.state(),
        &ThreadState::Closed(Some("Fake Agent requires authentication".into()))
    );
}

#[test]
fn a_login_the_harness_knows_is_named_first() {
    let mut fake = Fake::start_configured(&["--auth"], |options| {
        options.login_hint = Some("Run `fake login`.".into());
    });
    let hint = format!(
        "Fake Agent requires authentication. Log in outside this app, then open the session again:\n\
         - Run `fake login`.\n\
         - Log in with Fake (Opens the browser)\n\
         - Terminal login: run `FAKE_LOGIN=1 {} --auth --login` in a terminal",
        fake_agent().display()
    );
    fake.expect([
        Update::Notice(Level::Error, hint),
        Update::Closed(Some("Fake Agent requires authentication".into())),
    ]);
    fake.expect_end();
}

/// The orchestration server as the app attaches it: a stdio program with
/// the pane's variables, and an HTTP server for agents that take one.
fn orchestration(options: &mut Options) {
    options.mcp_servers = vec![
        McpServer::Stdio {
            name: "kou".into(),
            command: "/Applications/kou desktop/kou-desktop".into(),
            args: vec!["mcp".into()],
            env: vec![
                ("KOU_PANE".into(), "p3".into()),
                ("KOU_SOCKET".into(), "/tmp/kou.sock".into()),
            ],
        },
        McpServer::Http {
            name: "web".into(),
            url: "http://127.0.0.1:9/mcp".into(),
            headers: vec![("Authorization".into(), "Bearer x".into())],
        },
    ];
    options.instructions = Some("Drive other panes with the kou tools.".into());
    let mut meta = serde_json::Map::new();
    meta.insert(
        "claudeCode".into(),
        json!({"options": {"plugins": [{"type": "local", "path": "/kou/plugin"}]}}),
    );
    options.meta = Some(meta);
}

fn stdio_server() -> Value {
    json!({
        "name": "kou",
        "command": "/Applications/kou desktop/kou-desktop",
        "args": ["mcp"],
        "env": [
            {"name": "KOU_PANE", "value": "p3"},
            {"name": "KOU_SOCKET", "value": "/tmp/kou.sock"},
        ],
    })
}

fn http_server() -> Value {
    json!({
        "type": "http",
        "name": "web",
        "url": "http://127.0.0.1:9/mcp",
        "headers": [{"name": "Authorization", "value": "Bearer x"}],
    })
}

fn plugin_meta() -> Value {
    json!({"claudeCode": {"options": {"plugins": [{"type": "local", "path": "/kou/plugin"}]}}})
}

#[test]
fn mcp_servers_and_meta_go_with_a_new_session_as_far_as_the_agent_takes_them() {
    // An agent with stdio only and no added instructions: the HTTP server
    // and the instructions are left out, the first with a notice.
    let mut plain = Fake::start_configured(&[], orchestration);
    plain.expect([
        Update::Notice(
            Level::Warning,
            "Fake Agent cannot reach MCP servers over HTTP, so this session goes without web"
                .into(),
        ),
        ready("sess-1", false),
        providers(),
        commands(),
    ]);
    plain.prompt("inspect");
    plain.expect([Update::TurnStarted]);
    assert_eq!(
        plain.next_json()["opened"],
        json!({
            "method": "session/new",
            "params": {
                "cwd": plain.cwd,
                "mcpServers": [stdio_server()],
                "_meta": plugin_meta(),
            },
        })
    );
    plain.expect([Update::TurnEnded(Stop::Finished)]);

    // An agent that takes both gets both.
    let mut able = Fake::start_configured(&["--mcp-http", "--instructions"], orchestration);
    able.expect_ready();
    able.prompt("inspect");
    able.expect([Update::TurnStarted]);
    assert_eq!(
        able.next_json()["opened"],
        json!({
            "method": "session/new",
            "params": {
                "cwd": able.cwd,
                "mcpServers": [stdio_server(), http_server()],
                "systemPrompt": {
                    "mode": "append",
                    "content": [{"type": "text", "text": "Drive other panes with the kou tools."}],
                },
                "_meta": plugin_meta(),
            },
        })
    );
    able.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn mcp_servers_go_again_when_a_session_is_reopened() {
    // Loaded: the replay comes first, as in any load.
    let mut loaded = Fake::start_configured(&["--load", "--mcp-http"], |options| {
        orchestration(options);
        options.resume = Some("sess-old".into());
    });
    for _ in 0..5 {
        assert!(!matches!(loaded.next(), Update::Ready { .. }));
    }
    assert_eq!(loaded.next(), ready("sess-old", true));
    assert_eq!(loaded.next(), providers());
    loaded.prompt("inspect");
    loaded.expect([Update::TurnStarted]);
    assert_eq!(
        loaded.next_json()["opened"],
        json!({
            "method": "session/load",
            "params": {
                "sessionId": "sess-old",
                "cwd": loaded.cwd,
                "mcpServers": [stdio_server(), http_server()],
                "_meta": plugin_meta(),
            },
        })
    );
    loaded.expect([Update::TurnEnded(Stop::Finished)]);

    // Resumed without a replay.
    let mut resumed = Fake::start_configured(&["--resume-only", "--instructions"], |options| {
        orchestration(options);
        options.mcp_servers.truncate(1);
        options.resume = Some("sess-old".into());
    });
    resumed.expect([ready("sess-old", true), providers()]);
    resumed.prompt("inspect");
    resumed.expect([Update::TurnStarted]);
    assert_eq!(
        resumed.next_json()["opened"],
        json!({
            "method": "session/resume",
            "params": {
                "sessionId": "sess-old",
                "cwd": resumed.cwd,
                "mcpServers": [stdio_server()],
                "systemPrompt": {
                    "mode": "append",
                    "content": [{"type": "text", "text": "Drive other panes with the kou tools."}],
                },
                "_meta": plugin_meta(),
            },
        })
    );
    resumed.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn an_agent_without_load_is_never_asked_to_load() {
    let mut fake = Fake::start_configured(&[], |options| {
        orchestration(options);
        options.mcp_servers.truncate(1);
        options.resume = Some("sess-old".into());
    });
    fake.expect([
        Update::Notice(
            Level::Warning,
            "Fake Agent cannot resume sessions, so this is a new one".into(),
        ),
        ready("sess-1", false),
        providers(),
        commands(),
    ]);
    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    let seen = fake.next_json();
    assert_eq!(seen["opened"]["method"], "session/new");
    assert_eq!(
        seen["opened"]["params"]["mcpServers"],
        json!([stdio_server()])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn an_agent_that_dies_mid_turn_fails_the_turn_with_its_last_words() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("crash");
    let reason = "acp_fake_agent exited with code 3:\n\
                  fake agent: the model fell over\n\
                  fake agent: fatal, out of cheese"
        .to_owned();
    fake.expect([
        Update::TurnStarted,
        said("About to fail."),
        Update::TurnEnded(Stop::Failed(reason.clone())),
        Update::Closed(Some(reason.clone())),
    ]);
    fake.expect_end();
    assert_eq!(
        fake.thread.entries(),
        [
            user("crash"),
            Entry::Assistant("About to fail.".into()),
            Entry::Notice(Level::Error, reason.clone()),
        ]
    );
    assert_eq!(fake.thread.state(), &ThreadState::Closed(Some(reason)));
}

#[test]
fn an_agent_that_never_gets_going_closes_with_the_reason() {
    let mut dead = Fake::start(&["--crash-on-start"]);
    dead.expect([Update::Closed(Some(
        "acp_fake_agent exited with code 2:\n\
         fake agent: cannot find its configuration\n\
         fake agent: giving up"
            .into(),
    ))]);
    dead.expect_end();

    let mut newer = Fake::start(&["--protocol", "2"]);
    newer.expect([Update::Closed(Some(
        "Fake Agent speaks version 2 of the Agent Client Protocol, and this client only version 1"
            .into(),
    ))]);
    newer.expect_end();
}

#[test]
fn garbage_and_unknown_messages_are_skipped_without_losing_the_rest() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("malformed");
    fake.expect([
        Update::TurnStarted,
        // Lines that are not JSON, an answer to nothing, an unknown
        // notification, an update of an unknown kind and one for another
        // session came first, and left no trace. Of a tool call with fields
        // of the wrong type, what can be read is kept.
        Update::Tool(ToolCall {
            id: "call-odd".into(),
            title: "Odd".into(),
            kind: ToolKind::Other,
            status: ToolStatus::Pending,
            content: vec![ToolContent::Text("kept".into())],
            ..ToolCall::default()
        }),
    ]);
    // A method this client does not have, and one called without its parameters.
    assert_eq!(
        fake.next_json(),
        json!({"unknown": -32601, "badParams": -32602})
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn requests_the_agent_takes_back_are_answered_as_cancelled() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("takeback");
    fake.expect([Update::TurnStarted]);
    let Update::Permission(request) = fake.next() else {
        panic!("the question, before it is taken back");
    };
    assert_eq!((request.id, request.tool.id.as_str()), (1, "call-maybe"));
    assert_eq!(
        fake.next_json(),
        json!({
            "question": "cancelled",
            "wait": -32800,
            "killed": {"signal": "SIGKILL"},
        })
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn every_way_a_turn_can_end_reaches_the_thread() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    let cases = [
        ("refuse", Stop::Refused),
        ("limit", Stop::Limit),
        ("turns", Stop::Limit),
        (
            "weird",
            Stop::Failed(
                "Fake Agent stopped for a reason this client does not know: exploded".into(),
            ),
        ),
        (
            "error",
            Stop::Failed("Internal error: the model is overloaded".into()),
        ),
        (
            "login",
            Stop::Failed(
                "Fake Agent requires authentication but does not say how to log in. \
                 Log in with its own command line, then open the session again."
                    .into(),
            ),
        ),
    ];
    for (word, stop) in cases {
        fake.prompt(word);
        fake.expect([Update::TurnStarted, Update::TurnEnded(stop.clone())]);
        assert_eq!(fake.thread.last_stop, Some(stop));
        assert_eq!(fake.thread.state(), &ThreadState::Idle);
    }
    // A failed turn leaves its reason in the conversation.
    assert_eq!(
        fake.thread.entries().last(),
        Some(&Entry::Notice(
            Level::Error,
            "Fake Agent requires authentication but does not say how to log in. \
             Log in with its own command line, then open the session again."
                .into()
        ))
    );
}

#[test]
fn title_usage_notices_and_commands_reach_the_thread() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("meta");
    let context = Usage {
        context_used: Some(53_000),
        context_limit: Some(200_000),
        cost_usd: Some(0.045),
        ..Usage::default()
    };
    let totals = Usage {
        input_tokens: Some(10),
        output_tokens: Some(20),
        ..context
    };
    fake.expect([
        Update::TurnStarted,
        Update::Title("A fake session".into()),
        Update::Usage(context),
        Update::Notice(
            Level::Warning,
            "MCP server unavailable: Continuing without it.".into(),
        ),
        Update::Notice(Level::Info, "The conversation was compacted".into()),
        Update::Commands(vec![choice("compact", "compact", None)]),
        // The agent cleared the title.
        Update::Title(String::new()),
        // The answer to the prompt carried the turn's tokens.
        Update::Usage(totals),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(fake.thread.usage, totals);
    assert_eq!(fake.thread.commands, [choice("compact", "compact", None)]);

    // The tokens of the next turn add up.
    fake.prompt("meta");
    fake.expect([Update::TurnStarted]);
    let last = loop {
        match fake.next() {
            Update::TurnEnded(stop) => break stop,
            Update::Notice(Level::Info, text) => panic!("announced twice: {text}"),
            _ => {}
        }
    };
    assert_eq!(last, Stop::Finished);
    assert_eq!(
        (
            fake.thread.usage.input_tokens,
            fake.thread.usage.output_tokens
        ),
        (Some(20), Some(40))
    );
}

#[test]
fn an_agent_repeating_the_prompt_does_not_double_it() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("echo this back to me");
    fake.expect([
        Update::TurnStarted,
        // The two halves of the prompt came back first and were dropped.
        Update::UserChunk(Block::Text("an aside from the agent's side".into())),
        said("Heard."),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert_eq!(
        fake.thread.entries(),
        [
            user("echo this back to me"),
            user("an aside from the agent's side"),
            Entry::Assistant("Heard.".into()),
        ]
    );
}

#[test]
fn prompts_sent_before_the_session_exists_run_one_after_the_other() {
    let mut fake = Fake::start(&[]);
    fake.prompt("first");
    fake.prompt("second");
    fake.expect_ready();
    fake.expect([
        Update::TurnStarted,
        said("You said: first"),
        Update::TurnEnded(Stop::Finished),
        Update::TurnStarted,
        said("You said: second"),
        Update::TurnEnded(Stop::Finished),
    ]);
}

fn attachments(folder: &Path) -> (Vec<Block>, PathBuf) {
    let existing = folder.join("on disk.rs");
    std::fs::write(&existing, "old").unwrap();
    let blocks = vec![
        Block::Text("inspect ".into()),
        Block::Image {
            mime: "image/png".into(),
            data: vec![1, 2, 3].into(),
        },
        Block::File {
            path: existing.clone(),
            text: Some("new".into()),
        },
        Block::File {
            path: folder.join("unsaved.rs"),
            text: Some("draft".into()),
        },
        Block::File {
            path: folder.join("linked.rs"),
            text: None,
        },
    ];
    (blocks, existing)
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display()).replace(' ', "%20")
}

#[test]
fn a_prompt_carries_images_and_files_when_the_agent_takes_them() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    let (blocks, existing) = attachments(&fake.cwd);
    fake.prompt_blocks(blocks);
    fake.expect([Update::TurnStarted]);
    assert_eq!(
        fake.next_json()["prompt"],
        json!([
            {"type": "text", "text": "inspect "},
            {"type": "image", "data": "AQID", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": file_uri(&existing), "text": "new"}},
            {"type": "resource", "resource": {"uri": file_uri(&fake.in_cwd("unsaved.rs")), "text": "draft"}},
            {"type": "resource_link", "uri": file_uri(&fake.in_cwd("linked.rs")), "name": "linked.rs"},
        ])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn a_prompt_falls_back_to_links_and_text_when_the_agent_takes_less() {
    let mut fake = Fake::start(&["--no-images", "--no-embedded"]);
    let Update::Ready { capabilities, .. } = fake.next() else {
        panic!("ready first");
    };
    assert_eq!(
        capabilities,
        Capabilities {
            images: false,
            embedded_files: false,
            resume: false
        }
    );
    fake.expect([providers(), commands()]);

    // Nothing of this prompt is something the agent takes, so no turn starts.
    fake.prompt_blocks(vec![Block::Image {
        mime: "image/png".into(),
        data: vec![1, 2, 3].into(),
    }]);
    let left_out = Update::Notice(
        Level::Warning,
        "Fake Agent does not accept an image, so the prompt went without".into(),
    );
    fake.expect([left_out.clone()]);

    let (blocks, existing) = attachments(&fake.cwd);
    fake.prompt_blocks(blocks);
    fake.expect([left_out, Update::TurnStarted]);
    let unsaved = fake.in_cwd("unsaved.rs");
    assert_eq!(
        fake.next_json()["prompt"],
        json!([
            {"type": "text", "text": "inspect "},
            // On disk, so the agent can read it itself.
            {"type": "resource_link", "uri": file_uri(&existing), "name": "on disk.rs"},
            // Only in the editor, so the text travels in the prompt.
            {"type": "text", "text": format!("{}\n```\ndraft\n```", unsaved.display())},
            {"type": "resource_link", "uri": file_uri(&fake.in_cwd("linked.rs")), "name": "linked.rs"},
        ])
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn loading_a_session_replays_its_conversation_before_it_is_ready() {
    let mut fake = Fake::start_with(&["--load"], Some("sess-old"));
    let notes = fake.in_cwd("notes.txt");
    let read = ToolCall {
        id: "old-1".into(),
        title: "Read notes.txt".into(),
        kind: ToolKind::Read,
        status: ToolStatus::Done,
        content: vec![ToolContent::Text("hello".into())],
        locations: vec![Location {
            path: notes.clone(),
            line: None,
        }],
        ..ToolCall::default()
    };
    let file = Block::File {
        path: notes,
        text: None,
    };
    fake.expect([
        Update::UserChunk(Block::Text("What is in ".into())),
        Update::UserChunk(file.clone()),
        Update::ThoughtChunk("Reading.".into()),
        Update::Tool(read.clone()),
        said("It says hello."),
        ready("sess-old", true),
        providers(),
    ]);
    assert_eq!(
        fake.thread.entries(),
        [
            Entry::User(vec![Block::Text("What is in ".into()), file]),
            Entry::Thought("Reading.".into()),
            Entry::Tool(read),
            Entry::Assistant("It says hello.".into()),
        ]
    );
    assert_eq!(fake.thread.state(), &ThreadState::Idle);

    fake.prompt("inspect");
    fake.expect([Update::TurnStarted]);
    assert_eq!(
        fake.next_json()["opened"],
        json!({
            "method": "session/load",
            "params": {"sessionId": "sess-old", "cwd": fake.cwd, "mcpServers": []},
        })
    );
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn a_session_that_cannot_be_resumed_becomes_a_new_one_with_a_notice() {
    // The agent does not know the session.
    let mut gone = Fake::start_with(&["--load"], Some("gone"));
    gone.expect([
        Update::Notice(
            Level::Warning,
            "Session gone could not be resumed (Resource not found), so this is a new one".into(),
        ),
        ready("sess-1", true),
        providers(),
        commands(),
    ]);

    // The agent cannot load at all.
    let mut unable = Fake::start_with(&[], Some("sess-old"));
    unable.expect([
        Update::Notice(
            Level::Warning,
            "Fake Agent cannot resume sessions, so this is a new one".into(),
        ),
        ready("sess-1", false),
        providers(),
        commands(),
    ]);

    // The agent can only resume without replaying: the context is back, the
    // conversation is not shown again.
    let mut resumed = Fake::start_with(&["--resume-only"], Some("sess-old"));
    resumed.expect([ready("sess-old", true), providers()]);
    resumed.prompt("inspect");
    resumed.expect([Update::TurnStarted]);
    assert_eq!(
        resumed.next_json()["opened"],
        // Unlike `session/new` and `session/load`, the schema does not
        // require `mcpServers` here, and none are configured.
        json!({
            "method": "session/resume",
            "params": {"sessionId": "sess-old", "cwd": resumed.cwd},
        })
    );
    resumed.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn prompts_are_edited_only_with_an_agent_that_forks_and_a_way_to_tell_it_where() {
    // The agent forks, but the launch does not say how it is told where to.
    let mut unnamed = Fake::start(&["--fork"]);
    unnamed.expect_ready();
    unnamed.prompt("one");
    unnamed.expect([
        Update::TurnStarted,
        said("You said: one"),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert!(!unnamed.thread.offers.rewind);
    // A request it did not offer is turned down, and the thread takes back
    // what it let go of.
    unnamed.edit(0, "two");
    unnamed.expect([Update::Rewound(Err(
        "Fake Agent cannot go back to an earlier prompt.".into(),
    ))]);
    assert_eq!(
        unnamed.thread.entries(),
        [user("one"), Entry::Assistant("You said: one".into())]
    );

    // The launch says how, but the agent does not fork.
    let mut unable = Fake::start_configured(&[], |options| options.fork_meta = Some(air_fork()));
    unable.expect_ready();
    unable.prompt("one");
    unable.expect([
        Update::TurnStarted,
        said("You said: one"),
        Update::TurnEnded(Stop::Finished),
    ]);
    assert!(!unable.thread.offers.rewind);

    // Both: offered once the session is ready, and every prompt is named as
    // it goes out.
    let mut able = Fake::forking(&[], |_| {});
    able.expect([ready("sess-1", false), providers(), offers(), commands()]);
    assert!(able.thread.offers.rewind);
    able.exchange("one", 1);
    assert_eq!(able.thread.prompt_id(0), Some("prompt-1"));
}

#[test]
fn an_edited_prompt_runs_in_a_fork_of_the_session_up_to_the_answer_before_it() {
    let mut fake = Fake::forking(&["--mcp-http", "--instructions"], orchestration);
    fake.expect([ready("sess-1", false), providers(), offers(), commands()]);
    fake.exchange("one", 1);
    fake.exchange("two", 2);
    fake.exchange("three", 3);

    // "two" and what followed make way for the edit.
    fake.edit(2, "history");
    assert_eq!(fake.thread.rewinding(), Some(2));
    fake.expect([
        ready("fork-1", false),
        providers(),
        // Said by the fork before it was named, and held until then.
        commands(),
        Update::Rewound(Ok("prompt-4".into())),
        Update::TurnStarted,
    ]);
    // The fork holds the first exchange and nothing after it.
    assert_eq!(fake.next_json(), json!(["> one", "a-1"]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(fake.thread.session.as_deref(), Some("fork-1"));
    assert_eq!(fake.thread.rewinding(), None);
    assert_eq!(fake.thread.prompt_id(2), Some("prompt-4"));
    assert_eq!(
        fake.thread.entries(),
        [
            user("one"),
            Entry::Assistant("You said: one".into()),
            user("history"),
            Entry::Assistant(r#"["> one","a-1"]"#.into()),
        ]
    );

    // The fork was asked for with what the session was opened with, and the
    // session left behind was let go of.
    fake.prompt("inspect");
    fake.expect([taken(5), Update::TurnStarted]);
    let seen = fake.next_json();
    assert_eq!(
        seen["opened"],
        json!({
            "method": "session/fork",
            "params": {
                "sessionId": "sess-1",
                "cwd": fake.cwd,
                "mcpServers": [stdio_server(), http_server()],
                "systemPrompt": {
                    "mode": "append",
                    "content": [{"type": "text", "text": "Drive other panes with the kou tools."}],
                },
                "_meta": {
                    "claudeCode": plugin_meta()["claudeCode"],
                    "jetbrains": {"air": {"fork": {"version": 1, "messageId": "a-1"}}},
                },
            },
        })
    );
    assert_eq!(seen["session"], "fork-1");
    assert_eq!(seen["closed"], json!(["sess-1"]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);

    // The edited prompt is edited again: the fork is forked back to the
    // same answer.
    fake.edit(2, "history");
    fake.expect([
        ready("fork-2", false),
        providers(),
        commands(),
        Update::Rewound(Ok("prompt-6".into())),
        Update::TurnStarted,
    ]);
    assert_eq!(fake.next_json(), json!(["> one", "a-1"]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(fake.thread.session.as_deref(), Some("fork-2"));
}

#[test]
fn editing_the_first_prompt_starts_a_new_session_with_the_mode_and_model_in_force() {
    let mut fake = Fake::forking(&[], |_| {});
    fake.expect([ready("sess-1", false), providers(), offers(), commands()]);
    fake.exchange("one", 1);
    // The large model, which moves the mode to code too.
    fake.send(Command::SetModel("model-2".into()));
    fake.expect([Update::Mode("code".into()), Update::Model("model-2".into())]);

    fake.edit(0, "history");
    fake.expect([
        // A new session starts as every one does...
        ready("sess-2", false),
        providers(),
        commands(),
        Update::Rewound(Ok("prompt-2".into())),
        Update::TurnStarted,
        // ...and is set to what was in force before the prompt runs.
        Update::Mode("code".into()),
        Update::Model("model-2".into()),
    ]);
    // Nothing came before the first prompt.
    assert_eq!(fake.next_json(), json!([]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(fake.thread.session.as_deref(), Some("sess-2"));
    assert_eq!(fake.thread.model.as_deref(), Some("model-2"));
    assert_eq!(fake.thread.prompt_id(0), Some("prompt-2"));

    fake.prompt("inspect");
    fake.expect([taken(3), Update::TurnStarted]);
    let seen = fake.next_json();
    assert_eq!(seen["opened"]["method"], "session/new");
    assert_eq!(seen["session"], "sess-2");
    assert_eq!(
        (&seen["mode"], &seen["model"]),
        (&json!("code"), &json!("model-2"))
    );
    assert_eq!(seen["closed"], json!(["sess-1"]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

#[test]
fn an_edit_the_agent_cannot_fork_for_leaves_the_session_as_it_was() {
    // The agent fails the fork.
    let mut fake = Fake::forking(&["--refuse-fork"], |_| {});
    fake.expect([ready("sess-1", false), providers(), offers(), commands()]);
    fake.exchange("one", 1);
    fake.exchange("two", 2);
    let before = fake.thread.entries().to_vec();
    fake.edit(2, "two, edited");
    fake.expect([Update::Rewound(Err(
        "Fake Agent could not fork the session: Internal error: forking is broken".into(),
    ))]);
    assert_eq!(fake.thread.entries(), before.as_slice());
    assert_eq!(fake.thread.rewinding(), None);
    assert_eq!(fake.thread.session.as_deref(), Some("sess-1"));
    assert_eq!(fake.thread.prompt_id(2), Some("prompt-2"));
    // The conversation goes on where it was.
    fake.prompt("history");
    fake.expect([taken(3), Update::TurnStarted]);
    assert_eq!(fake.next_json(), json!(["> one", "a-1", "> two", "a-2"]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);

    // The agent does not know the message the fork is to end at: here an
    // answer it never kept.
    let mut lost = Fake::forking(&[], |_| {});
    lost.expect([ready("sess-1", false), providers(), offers(), commands()]);
    lost.prompt("chunks");
    lost.expect([
        taken(1),
        Update::TurnStarted,
        Update::ThoughtChunk("Let me ".into()),
        Update::ThoughtChunk("think.".into()),
        said("Hel"),
        said("lo."),
        said("\n\nSecond."),
        Update::TurnEnded(Stop::Finished),
    ]);
    lost.exchange("two", 2);
    lost.edit(3, "two, edited");
    lost.expect([Update::Rewound(Err(
        "Fake Agent could not fork the session: Invalid params: Fork point message m2 was not found in session sess-1".into(),
    ))]);
    assert_eq!(lost.thread.session.as_deref(), Some("sess-1"));
    assert_eq!(lost.thread.entries()[3], user("two"));
    assert_eq!(lost.thread.entries().len(), 5);
}

#[test]
fn a_prompt_of_a_loaded_session_is_edited_in_a_fork_of_it() {
    let mut fake = Fake::forking(&["--load"], |options| {
        options.resume = Some("sess-old".into());
    });
    let file = Block::File {
        path: fake.in_cwd("notes.txt"),
        text: None,
    };
    // Each replayed prompt is named once its chunks end.
    fake.expect([
        Update::UserChunk(Block::Text("What is in ".into())),
        Update::UserChunk(file.clone()),
        Update::PromptId("prompt-1".into()),
        Update::ThoughtChunk("Reading.".into()),
    ]);
    assert!(matches!(fake.next(), Update::Tool(_)));
    fake.expect([
        said("It says hello."),
        Update::UserChunk(Block::Text("And now?".into())),
        Update::PromptId("prompt-2".into()),
        said("Nothing more."),
        ready("sess-old", true),
        providers(),
        offers(),
    ]);
    assert_eq!(fake.thread.prompt_id(0), Some("prompt-1"));
    assert_eq!(fake.thread.prompt_id(4), Some("prompt-2"));

    // The second goes, with the answer to it.
    fake.edit(4, "history");
    fake.expect([
        ready("fork-1", true),
        providers(),
        commands(),
        Update::Rewound(Ok("prompt-3".into())),
        Update::TurnStarted,
    ]);
    assert_eq!(fake.next_json(), json!(["> What is in ", "t1", "a1"]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
    assert_eq!(fake.thread.session.as_deref(), Some("fork-1"));
    let entries = fake.thread.entries();
    assert_eq!(entries.len(), 6);
    assert_eq!(
        entries[0],
        Entry::User(vec![Block::Text("What is in ".into()), file])
    );
    assert_eq!(entries[3], Entry::Assistant("It says hello.".into()));
    assert_eq!(entries[4], user("history"));
}

#[test]
fn a_session_resumed_without_its_history_edits_only_prompts_after_an_answer_seen_here() {
    let mut fake = Fake::forking(&["--resume-only"], |options| {
        options.resume = Some("sess-old".into());
    });
    fake.expect([ready("sess-old", true), providers(), offers()]);
    fake.exchange("one", 1);
    fake.exchange("two", 2);
    // What came before the first prompt sent here was never shown.
    fake.edit(0, "one, edited");
    fake.expect([Update::Rewound(Err(
        "Fake Agent cannot go back to before this prompt: what came before it was not seen here, or has no id to go back to.".into(),
    ))]);
    assert_eq!(fake.thread.entries().len(), 4);
    // The second follows an answer seen here.
    fake.edit(2, "history");
    fake.expect([
        ready("fork-1", true),
        providers(),
        commands(),
        Update::Rewound(Ok("prompt-3".into())),
        Update::TurnStarted,
    ]);
    assert_eq!(fake.next_json(), json!(["> one", "a-1"]));
    fake.expect([Update::TurnEnded(Stop::Finished)]);
}

/// Start the turn that never ends, and return the agent's process id and
/// the id of the command it runs through the client.
fn hang(fake: &mut Fake) -> (u32, u32) {
    fake.prompt("hang");
    fake.expect([Update::TurnStarted]);
    let Update::Tool(call) = fake.next() else {
        panic!("the tool call showing the command");
    };
    let [
        ToolContent::Terminal {
            output, exit: None, ..
        },
    ] = call.content.as_slice()
    else {
        panic!("a running terminal, got {:?}", call.content);
    };
    let command: u32 = output.trim().parse().expect("the command's process id");
    let Update::AssistantChunk(text) = fake.next() else {
        panic!("the agent's process id");
    };
    let words: Vec<&str> = text.split(' ').collect();
    let ["agent", agent, "command", shown] = words.as_slice() else {
        panic!("both process ids, got {text}");
    };
    assert_eq!(shown.parse(), Ok(command));
    let agent: u32 = agent.parse().expect("the agent's process id");
    assert!(alive(agent) && alive(command));
    (agent, command)
}

#[test]
fn close_ends_the_turn_the_agent_and_its_commands() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    let pids = hang(&mut fake);
    fake.send(Command::Close);
    fake.expect([Update::TurnEnded(Stop::Cancelled), Update::Closed(None)]);
    fake.expect_end();
    // The agent is gone by the time the thread hears of it.
    assert!(!alive(pids.0));
    wait_until_gone(&[pids.1]);
    assert_eq!(fake.thread.state(), &ThreadState::Closed(None));
    // Commands after the end go nowhere.
    assert!(fake.backend().commands.try_send(Command::Cancel).is_err());
}

#[test]
fn dropping_the_backend_ends_the_agent_and_its_commands() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    let pids = hang(&mut fake);
    fake.backend = None;
    wait_until_gone(&[pids.0, pids.1]);
}

#[test]
fn dropping_only_the_command_end_keeps_the_turn_running() {
    let mut fake = Fake::start(&[]);
    fake.expect_ready();
    fake.prompt("pid");
    fake.expect([Update::TurnStarted]);
    let Update::AssistantChunk(pid) = fake.next() else {
        panic!("the agent's process id");
    };
    let pid: u32 = pid.parse().expect("a process id");
    fake.expect([Update::TurnEnded(Stop::Finished)]);

    fake.prompt("chunks");
    let updates = fake.backend.take().expect("a backend").updates;
    let mut seen = Vec::new();
    let deadline = Instant::now() + PATIENCE;
    while seen.last() != Some(&Update::TurnEnded(Stop::Finished)) {
        assert!(
            Instant::now() < deadline,
            "the turn did not finish: {seen:?}"
        );
        match updates.try_recv() {
            Ok(update) => seen.push(update),
            Err(error) => {
                assert!(!error.is_closed(), "closed mid-turn: {seen:?}");
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
    assert_eq!(seen.len(), 7, "{seen:?}");
    // Someone still reads, so the agent stays.
    thread::sleep(Duration::from_millis(600));
    assert!(alive(pid) && !updates.is_closed());
    // With the reading end gone too, the session ends by itself.
    drop(updates);
    wait_until_gone(&[pid]);
}

#[test]
fn an_agent_that_ignores_being_asked_to_stop_is_killed() {
    // The shell makes the agent deaf to the termination signal, and
    // `--linger` to its input ending.
    let launch = Launch {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "trap '' TERM; exec \"$0\" \"$@\"".into(),
            fake_agent().display().to_string(),
            "--linger".into(),
        ],
        env: vec![],
        cwd: PathBuf::new(),
    };
    let mut fake = Fake::launch(launch, None);
    fake.expect_ready();
    fake.prompt("pid");
    fake.expect([Update::TurnStarted]);
    let Update::AssistantChunk(pid) = fake.next() else {
        panic!("the agent's process id");
    };
    let pid: u32 = pid.parse().unwrap();
    fake.expect([Update::TurnEnded(Stop::Finished)]);

    let asked = Instant::now();
    fake.send(Command::Close);
    fake.expect([Update::Closed(None)]);
    fake.expect_end();
    // It got its grace period, and no more than that and the kill.
    assert!(
        asked.elapsed() >= Duration::from_secs(2),
        "{:?}",
        asked.elapsed()
    );
    assert!(
        asked.elapsed() < Duration::from_secs(10),
        "{:?}",
        asked.elapsed()
    );
    assert!(!alive(pid));
}

/// Start the real OpenCode agent, create a session, send nothing, and print
/// what it advertised. Costs nothing: no prompt reaches a model.
///
/// `cargo test -p kou-acp --test acp -- --ignored --nocapture opencode`
#[test]
#[ignore = "needs `opencode` on the PATH; run by hand"]
fn opencode_initializes_and_creates_a_session() {
    let launch = Launch {
        program: "opencode".into(),
        args: vec!["acp".into()],
        env: vec![],
        cwd: PathBuf::new(),
    };
    let mut fake = Fake::launch(launch, None);
    let Update::Ready {
        session,
        capabilities,
        modes,
        mode,
        models,
        model,
    } = fake.next()
    else {
        panic!("ready first");
    };
    println!("session: {session}");
    println!("capabilities: {capabilities:?}");
    println!("mode: {mode:?} of {modes:?}");
    println!("model: {model:?} of {} models", models.len());
    for model in models.iter().take(5) {
        println!("  {} ({})", model.id, model.name);
    }
    assert!(!session.is_empty());

    // Whatever it announces right after: commands, usually.
    let settle = Instant::now() + Duration::from_secs(3);
    while Instant::now() < settle {
        match fake.backend().updates.try_recv() {
            Ok(Update::Commands(commands)) => {
                let names: Vec<&str> = commands.iter().map(|c| c.id.as_str()).collect();
                println!("commands: {names:?}");
            }
            Ok(update) => println!("update: {update:?}"),
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
    fake.send(Command::Close);
    fake.expect([Update::Closed(None)]);
    fake.expect_end();
}

/// Start a real agent, open a session with an MCP server attached, send
/// nothing, and print what came back. Costs nothing: no prompt reaches a model.
///
/// `KOU_ACP_AGENT` is the agent's command line (default `opencode acp`),
/// `KOU_ACP_MCP` the MCP server's (none by default), `KOU_ACP_RESUME` a
/// session to reopen, and `KOU_ACP_ENV` `NAME=value` pairs for the agent.
///
/// `cargo test -p kou-acp --test acp -- --ignored --nocapture real_agent`
/// A real agent's sessions for a folder, counted: `KOU_ACP_AGENT` is its
/// command line (a path with a space in it goes in `KOU_ACP_PROGRAM`
/// instead), `KOU_ACP_ENV` `NAME=value` pairs for it, `KOU_ACP_CWD` the
/// folder. Nothing is opened on it; titles are not printed.
///
/// `cargo test -p kou-acp --test acp -- --ignored --nocapture real_agents_sessions`
#[test]
#[ignore = "starts a real agent named in KOU_ACP_AGENT; run by hand"]
fn a_real_agents_sessions_are_listed() {
    let words = |name: &str| -> Vec<String> {
        std::env::var(name)
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    };
    let mut agent = words("KOU_ACP_AGENT");
    if let Ok(program) = std::env::var("KOU_ACP_PROGRAM") {
        agent.insert(0, program);
    }
    assert!(!agent.is_empty(), "name the agent in KOU_ACP_AGENT");
    let launch = Launch {
        program: agent.remove(0).into(),
        args: agent,
        env: words("KOU_ACP_ENV")
            .into_iter()
            .filter_map(|pair| {
                let (name, value) = pair.split_once('=')?;
                Some((name.to_owned(), value.to_owned()))
            })
            .collect(),
        cwd: PathBuf::new(),
    };
    let cwd = PathBuf::from(std::env::var("KOU_ACP_CWD").expect("the folder in KOU_ACP_CWD"));
    let started = Instant::now();
    let listed = kou_acp::sessions(&launch, &cwd, ("kou-acp tests", "1.2.3"), PATIENCE * 4)
        .expect("the agent answers");
    // And the models a session of its would offer, by id.
    let offered = kou_acp::models(&launch, &cwd, ("kou-acp tests", "1.2.3"), PATIENCE * 4)
        .expect("the agent answers");
    match offered {
        None => println!("the agent offers no choice of model"),
        Some(offered) => println!(
            "{} models in {} groups, a new session starts with {:?}: {:?}",
            offered.models.len(),
            offered.providers.len(),
            offered.current,
            offered
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
        ),
    }
    match listed {
        None => println!("the agent does not list its sessions"),
        Some(sessions) => println!(
            "{} sessions in {:?}: {} named, {} with a time, latest first: {}",
            sessions.len(),
            started.elapsed(),
            sessions
                .iter()
                .filter(|session| !session.title.is_empty())
                .count(),
            sessions
                .iter()
                .filter(|session| session.updated.is_some())
                .count(),
            sessions
                .windows(2)
                .all(|pair| pair[0].updated >= pair[1].updated),
        ),
    }
}

#[test]
#[ignore = "starts a real agent named in KOU_ACP_AGENT; run by hand"]
fn a_real_agent_opens_a_session_with_what_is_attached() {
    let words = |name: &str| -> Vec<String> {
        std::env::var(name)
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    };
    let mut agent = words("KOU_ACP_AGENT");
    if agent.is_empty() {
        agent = vec!["opencode".into(), "acp".into()];
    }
    let env = words("KOU_ACP_ENV")
        .into_iter()
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            Some((name.to_owned(), value.to_owned()))
        })
        .collect();
    let launch = Launch {
        program: agent.remove(0).into(),
        args: agent,
        env,
        cwd: PathBuf::new(),
    };
    let mut fake = Fake::launch_configured(launch, |options| {
        let mut server = words("KOU_ACP_MCP");
        if !server.is_empty() {
            options.mcp_servers = vec![McpServer::Stdio {
                name: "kou-probe".into(),
                command: server.remove(0).into(),
                args: server,
                env: vec![("KOU_PROBE_EXPLICIT".into(), "1".into())],
            }];
        }
        options.instructions = Some("kou probe: nothing to do.".into());
        options.resume = std::env::var("KOU_ACP_RESUME").ok();
        // `KOU_ACP_FORK` set: told how the Claude and Codex adapters name the
        // point a fork ends at, so `Offers` says whether prompts can be edited.
        if std::env::var_os("KOU_ACP_FORK").is_some() {
            options.fork_meta = Some(air_fork());
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        match fake.backend().updates.try_recv() {
            Ok(Update::Ready {
                session,
                capabilities,
                modes,
                mode,
                models,
                model,
            }) => {
                println!("ready: session {session}, {capabilities:?}");
                println!("mode: {mode:?} of {}", modes.len());
                println!("model: {model:?} of {}", models.len());
            }
            Ok(Update::Commands(commands)) => println!("commands: {}", commands.len()),
            Ok(Update::Closed(reason)) => {
                println!("closed: {reason:?}");
                return;
            }
            Ok(update) => println!("update: {update:?}"),
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
    fake.send(Command::Close);
}
