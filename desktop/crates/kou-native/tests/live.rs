//! Full turns through `connect`, with the real Go runner and a local stand-in
//! for the model: a prompt, a tool call, the answer; then the same session
//! resumed; a run cancelled in the middle of a command; and a refused request.
//!
//! The tests skip, saying so, when there is neither a Go toolchain to build
//! the runner with nor a runner in the repository's `bin/`.

mod support;

use kou_native::{Options, Setup};
use kou_thread::{
    Block, Command, Entry, Level, Stop, ThreadState, ToolContent, ToolKind, ToolStatus, Update,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use support::{
    Answer, Follower, Piece, Provider, call, has_tool_result, last_user_text, message, reasoning,
};

/// A project folder, a configuration folder of no user, and the setup that
/// points the runner at the stand-in.
struct World {
    _root: tempfile::TempDir,
    project: PathBuf,
    setup: Setup,
}

fn world(provider: &Provider) -> World {
    let root = tempfile::tempdir().expect("a directory");
    let project = root.path().join("project");
    let home = root.path().join("home");
    std::fs::create_dir_all(&project).expect("a project");
    std::fs::create_dir_all(&home).expect("a home");
    std::fs::write(project.join("notes.txt"), "kou\n").expect("a file");
    let setup = Setup {
        // Beside it the runner looks for the user's plugins: there are none.
        settings_file: Some(root.path().join("config").join("settings.json")),
        provider: Some("openai".into()),
        env: vec![
            (
                "KOU_CONVEYOR_LLM_BASE_URL".into(),
                provider.base_url.clone(),
            ),
            (
                "KOU_CONVEYOR_LLM_API_KEY".into(),
                "test-key-of-no-account".into(),
            ),
            ("KOU_CONVEYOR_LLM_MAX_ATTEMPTS".into(), "1".into()),
            // The system-wide skills are the user's own; a run here has none.
            ("HOME".into(), home.to_string_lossy().into_owned()),
        ],
    };
    World {
        project: project.canonicalize().expect("a project"),
        _root: root,
        setup,
    }
}

fn open(runner: &Path, world: &World, session: Option<String>) -> Follower {
    let backend = kou_native::connect_with(
        Options {
            runner: runner.to_owned(),
            cwd: world.project.clone(),
            session,
            model: Some("stub-model".into()),
        },
        world.setup.clone(),
    )
    .expect("a backend");
    let mut follower = Follower::new(backend);
    follower.until("Ready", |update| matches!(update, Update::Ready { .. }));
    follower
}

/// The stand-in's script: a prompt is answered with a command to run, the
/// command's result with the final answer.
fn scripted() -> Provider {
    Provider::start(|request, before| {
        if has_tool_result(request) {
            return Answer::of(
                vec![
                    Piece::Text(0, "The command "),
                    Piece::Pause,
                    Piece::Text(0, "printed kou-native-live."),
                    Piece::Pause,
                ],
                json!([message(
                    &format!("msg-{before}"),
                    "The command printed kou-native-live."
                )]),
            );
        }
        let prompt = last_user_text(request);
        let command = if prompt.contains("slowly") {
            "echo started; sleep 60"
        } else if prompt.contains("briefly") {
            "echo started; sleep 3; echo kou-native-live"
        } else {
            "echo kou-native-live"
        };
        Answer::of(
            vec![
                Piece::Reasoning(0, "The user wants "),
                Piece::Pause,
                Piece::Reasoning(0, "a command run."),
                Piece::Pause,
                Piece::Text(1, "I'll run "),
                Piece::Pause,
                Piece::Text(1, "it."),
                Piece::Pause,
            ],
            json!([
                reasoning(&format!("rs-{before}"), "The user wants a command run."),
                message(&format!("msg-{before}"), "I'll run it."),
                call(
                    &format!("call-{before}"),
                    "Bash",
                    json!({"command": command})
                ),
            ]),
        )
    })
}

fn tool(entry: &Entry) -> &kou_thread::ToolCall {
    match entry {
        Entry::Tool(call) => call,
        other => panic!("not a tool call: {other:?}"),
    }
}

/// One turn with a tool call, as a runner of the given build shows it.
fn full_turn(runner: &Path, streams: bool) {
    let provider = scripted();
    let world = world(&provider);
    let mut follower = open(runner, &world, None);
    let Some(Update::Ready {
        session,
        capabilities,
        modes,
        mode,
        models,
        model,
    }) = follower.updates.last().cloned()
    else {
        panic!(
            "Ready comes first for a new session: {:?}",
            follower.updates
        );
    };
    assert_eq!(follower.updates.len(), 1, "a new session replays nothing");
    assert!(capabilities.images && capabilities.resume && !capabilities.embedded_files);
    assert_eq!(
        modes
            .iter()
            .map(|mode| mode.id.as_str())
            .collect::<Vec<_>>(),
        ["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(mode.as_deref(), Some("high"));
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["stub-model"],
        "the connection's models are the stand-in's"
    );
    assert_eq!(model.as_deref(), Some("stub-model"));
    assert_eq!(follower.thread.state(), &ThreadState::Idle);

    follower.prompt("Run the echo command");
    follower.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    let thread = &follower.thread;
    assert_eq!(
        thread.last_stop,
        Some(Stop::Finished),
        "{:#?}",
        follower.updates
    );
    assert_eq!(thread.state(), &ThreadState::Idle);
    assert_eq!(thread.title.as_deref(), Some("Run the echo command"));
    assert_eq!(thread.commands.len(), 2);

    let entries = thread.entries();
    assert_eq!(entries.len(), 5, "{entries:#?}");
    assert_eq!(
        entries[0],
        Entry::User(vec![Block::Text("Run the echo command".into())])
    );
    assert_eq!(
        entries[1],
        Entry::Thought("The user wants a command run.".into())
    );
    assert_eq!(entries[2], Entry::Assistant("I'll run it.".into()));
    let call = tool(&entries[3]);
    assert_eq!(
        (
            call.id.as_str(),
            call.title.as_str(),
            call.kind,
            call.status
        ),
        (
            "call-0",
            "Run echo kou-native-live",
            ToolKind::Execute,
            ToolStatus::Done
        )
    );
    assert_eq!(
        call.raw_input,
        Some(json!({"command": "echo kou-native-live"}))
    );
    let [ToolContent::Terminal { output, exit, .. }] = call.content.as_slice() else {
        panic!("a command's content is its terminal: {:?}", call.content);
    };
    assert_eq!((output.as_str(), *exit), ("kou-native-live\n", Some(0)));
    assert_eq!(
        entries[4],
        Entry::Assistant("The command printed kou-native-live.".into())
    );
    assert_eq!(thread.usage.input_tokens, Some(201));
    assert_eq!(thread.usage.output_tokens, Some(14));
    assert_eq!(thread.usage.context_used, Some(101));

    // The order things arrive in: the turn, the response, the call from its
    // announcement to its result, the answer, the end.
    let sequence: Vec<&str> = follower
        .updates
        .iter()
        .filter_map(|update| match update {
            Update::TurnStarted => Some("start"),
            Update::ThoughtChunk(_) => Some("thought"),
            Update::AssistantChunk(_) => Some("text"),
            Update::Tool(_) => Some("tool"),
            Update::ToolPatch(patch) => match patch.status {
                Some(ToolStatus::Running) => Some("running"),
                Some(ToolStatus::Done) => Some("done"),
                _ => None,
            },
            Update::TurnEnded(_) => Some("end"),
            _ => None,
        })
        .collect();
    let mut order = sequence.clone();
    order.dedup();
    assert_eq!(
        order,
        [
            "start", "thought", "text", "tool", "running", "done", "text", "end"
        ],
        "{:#?}",
        follower.updates
    );
    let chunks = sequence.iter().filter(|kind| **kind == "text").count();
    if streams {
        assert!(chunks >= 4, "the text arrives as it streams: {sequence:?}");
    } else {
        assert_eq!(
            chunks, 2,
            "a runner that does not stream gives whole messages"
        );
    }
    assert!(
        !follower
            .updates
            .iter()
            .any(|update| matches!(update, Update::UserChunk(_))),
        "a prompt the interface sent is not echoed"
    );

    // What the runner was asked: the prompt under the session, with the model.
    {
        let requests = provider.requests.lock().expect("the requests");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["model"], "stub-model");
        assert!(last_user_text(&requests[0]).contains("Run the echo command"));
        assert!(has_tool_result(&requests[1]));
    }
    let listed = kou_native::sessions(&world.project).expect("sessions");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (
            listed[0].id.as_str(),
            listed[0].title.as_str(),
            listed[0].messages,
            listed[0].model.as_deref(),
            listed[0].running
        ),
        (
            session.as_str(),
            "Run the echo command",
            1,
            Some("stub-model"),
            false
        )
    );
    let live: Vec<Entry> = follower.thread.entries().to_vec();
    let closing = follower.close();
    assert_eq!(closing.last(), Some(&Update::Closed(None)));

    // The same session, resumed: the stored conversation is what was shown.
    let mut resumed = open(runner, &world, Some(session.clone()));
    assert_eq!(
        resumed.thread.entries(),
        live.as_slice(),
        "{:#?}",
        resumed.updates
    );
    assert_eq!(resumed.thread.state(), &ThreadState::Idle);
    assert_eq!(resumed.thread.session.as_deref(), Some(session.as_str()));
    assert_eq!(
        resumed.thread.title.as_deref(),
        Some("Run the echo command")
    );
    assert_eq!(resumed.thread.last_stop, Some(Stop::Finished));
    let replayed: Vec<&str> = resumed
        .updates
        .iter()
        .map(|update| match update {
            Update::UserChunk(_) => "user",
            // The name a stored prompt goes by, for editing it later.
            Update::PromptId(_) => "named",
            Update::TurnStarted => "start",
            Update::ThoughtChunk(_) => "thought",
            Update::AssistantChunk(_) => "text",
            Update::Tool(_) => "tool",
            Update::TurnEnded(_) => "end",
            Update::Title(_) => "title",
            Update::Ready { .. } => "ready",
            _ => "other",
        })
        .collect();
    assert_eq!(
        replayed,
        [
            "user", "named", "start", "thought", "text", "tool", "text", "end", "title", "ready"
        ],
        "a stored call arrives once, in its final state"
    );

    // And it goes on: a second prompt in the resumed session.
    resumed.prompt("Run the echo command again");
    resumed.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    assert_eq!(resumed.thread.last_stop, Some(Stop::Finished));
    assert_eq!(resumed.thread.entries().len(), 10);
    assert_eq!(resumed.thread.usage.input_tokens, Some(201 + 102 + 103));
    resumed.close();
    assert_eq!(
        kou_native::sessions(&world.project).expect("sessions")[0].messages,
        2
    );
}

#[test]
fn a_turn_with_a_tool_call_streams_and_resumes() {
    let Some(runner) = support::fresh_runner() else {
        eprintln!("skipped: no runner could be built");
        return;
    };
    full_turn(&runner, true);
}

#[test]
fn a_runner_built_before_streaming_still_runs_a_turn() {
    let Some(runner) = support::built_runner() else {
        eprintln!("skipped: the repository has no bin/kou-conveyor-runner");
        return;
    };
    let usage = std::process::Command::new(&runner)
        .arg("-h")
        .output()
        .expect("the runner's usage");
    let usage = format!(
        "{}{}",
        String::from_utf8_lossy(&usage.stdout),
        String::from_utf8_lossy(&usage.stderr)
    );
    full_turn(&runner, usage.contains(r#"{"type": "delta""#));
}

#[test]
fn a_run_is_cancelled_in_the_middle_of_a_command_and_says_so_when_resumed() {
    let Some(runner) = support::fresh_runner() else {
        eprintln!("skipped: no runner could be built");
        return;
    };
    let provider = scripted();
    let world = world(&provider);
    let mut follower = open(&runner, &world, None);
    let session = follower.thread.session.clone().expect("a session");
    follower.prompt("Run it slowly");
    follower.until("the command's first output", |update| {
        matches!(update, Update::ToolPatch(patch) if patch.content.as_deref().is_some_and(|content| {
            matches!(content, [ToolContent::Terminal { output, exit: None, .. }] if output.contains("started"))
        }))
    });
    assert_eq!(follower.thread.state(), &ThreadState::Working);
    assert_eq!(
        tool(&follower.thread.entries()[3]).status,
        ToolStatus::Running
    );

    // A prompt written meanwhile waits for the run, which is then stopped.
    follower.prompt("And then this");
    let started = std::time::Instant::now();
    follower
        .backend
        .commands
        .send_blocking(Command::Cancel)
        .expect("a backend");
    follower.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the stop took {:?}",
        started.elapsed()
    );
    assert_eq!(follower.thread.last_stop, Some(Stop::Cancelled));
    assert_eq!(follower.thread.state(), &ThreadState::Idle);
    assert_eq!(
        tool(&follower.thread.entries()[3]).status,
        ToolStatus::Cancelled
    );
    follower.until("word of the queued prompt", |update| {
        matches!(update, Update::Notice(Level::Warning, text) if text.contains("And then this"))
    });
    assert_eq!(provider.requests.lock().expect("the requests").len(), 1);
    follower.close();

    // The session recorded who stopped the run, and that it did not finish.
    let resumed = open(&runner, &world, Some(session));
    let notices: Vec<&str> = resumed
        .thread
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Notice(_, text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        notices,
        [
            "Run stopped: the user stopped the run from kou desktop",
            "The last run did not finish. Send /continue to pick it up."
        ],
        "{:#?}",
        resumed.updates
    );
    assert_eq!(resumed.thread.last_stop, Some(Stop::Cancelled));
    assert_eq!(
        tool(&resumed.thread.entries()[3]).status,
        ToolStatus::Cancelled
    );
    resumed.close();
}

#[test]
fn closing_a_session_stops_its_run() {
    let Some(runner) = support::fresh_runner() else {
        eprintln!("skipped: no runner could be built");
        return;
    };
    let provider = scripted();
    let world = world(&provider);
    let mut follower = open(&runner, &world, None);
    let session = follower.thread.session.clone().expect("a session");
    follower.prompt("Run it slowly");
    follower.until("the command to run", |update| {
        matches!(update, Update::ToolPatch(patch) if patch.status == Some(ToolStatus::Running))
    });
    assert!(kou_native::sessions(&world.project).expect("sessions")[0].running);
    let updates = follower.close();
    assert!(
        updates.contains(&Update::TurnEnded(Stop::Cancelled)),
        "the run ends before the backend does: {updates:#?}"
    );
    assert_eq!(updates.last(), Some(&Update::Closed(None)));
    let listed = kou_native::sessions(&world.project).expect("sessions");
    assert_eq!(
        (listed[0].id.as_str(), listed[0].running),
        (session.as_str(), false)
    );
}

#[test]
fn a_refused_request_fails_the_turn_with_the_provider_s_words() {
    let Some(runner) = support::fresh_runner() else {
        eprintln!("skipped: no runner could be built");
        return;
    };
    let provider = Provider::start(|_, _| Answer::refused(401));
    let world = world(&provider);
    let mut follower = open(&runner, &world, None);
    follower.prompt("Anything");
    follower.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    let Some(Stop::Failed(reason)) = &follower.thread.last_stop else {
        panic!("the turn failed: {:#?}", follower.updates);
    };
    assert!(
        reason.contains("the stand-in refuses this request"),
        "{reason}"
    );
    assert!(matches!(
        follower.thread.entries().last(),
        Some(Entry::Notice(Level::Error, _))
    ));
    assert_eq!(follower.thread.state(), &ThreadState::Idle);
    follower.close();
}

/// The entries of a thread that are the conversation, not word about it.
fn conversation(thread: &kou_thread::Thread) -> Vec<Entry> {
    thread
        .entries()
        .iter()
        .filter(|entry| !matches!(entry, Entry::Notice(..)))
        .cloned()
        .collect()
}

fn notices(thread: &kou_thread::Thread) -> Vec<&str> {
    thread
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Notice(_, text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_session_another_window_runs_is_followed_until_that_run_ends() {
    let Some(runner) = support::fresh_runner() else {
        eprintln!("skipped: no runner could be built");
        return;
    };
    let provider = scripted();
    let world = world(&provider);
    let mut first = open(&runner, &world, None);
    let session = first.thread.session.clone().expect("a session");
    first.prompt("Run it briefly");
    first.until("the command to run", |update| {
        matches!(update, Update::ToolPatch(patch) if patch.status == Some(ToolStatus::Running))
    });

    // A second window opens the session while the first one's run goes on.
    let mut second = open(&runner, &world, Some(session));
    assert_eq!(second.thread.state(), &ThreadState::Working);
    assert_eq!(
        notices(&second.thread),
        ["This session is running in another window or terminal. Following it."]
    );
    assert_eq!(
        tool(&second.thread.entries()[3]).status,
        ToolStatus::Running
    );
    // It neither sends into the other window's run nor stops it.
    second.prompt("Not now");
    second
        .backend
        .commands
        .send_blocking(Command::Cancel)
        .expect("a backend");
    second.until("word that the run is not its own to stop", |update| {
        matches!(update, Update::Notice(Level::Warning, text) if text.contains("Stop it there"))
    });
    assert!(
        notices(&second.thread)
            .iter()
            .any(|text| text.contains("The message was not sent")),
        "{:#?}",
        second.updates
    );

    first.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    assert_eq!(first.thread.last_stop, Some(Stop::Finished));
    second.until("word of the other run's end", |update| {
        matches!(update, Update::Notice(Level::Info, text) if text == "The other run has finished.")
    });
    assert_eq!(second.thread.last_stop, Some(Stop::Finished));
    assert_eq!(second.thread.state(), &ThreadState::Idle);
    // What the second window showed of the run is what the first one did,
    // but for the prompt it would not send.
    let mut followed = conversation(&second.thread);
    followed.retain(|entry| entry != &Entry::User(vec![Block::Text("Not now".into())]));
    assert_eq!(followed, conversation(&first.thread));
    assert_eq!(provider.requests.lock().expect("the requests").len(), 2);

    // The session is free again: the second window runs the next prompt.
    second.prompt("Run the echo command");
    second.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    assert_eq!(second.thread.last_stop, Some(Stop::Finished));
    assert_eq!(provider.requests.lock().expect("the requests").len(), 4);
    first.close();
    second.close();
}

#[cfg(unix)]
#[test]
fn a_run_left_running_apart_from_its_cockpit_is_taken_up_let_go_of_and_stopped() {
    let Some(runner) = support::fresh_runner() else {
        eprintln!("skipped: no runner could be built");
        return;
    };
    let provider = scripted();
    let world = world(&provider);
    let session = "detached-one";
    // A cockpit starts a run apart from itself, as the web cockpit does, and
    // goes away.
    let mut run = support::Detached::start(
        &runner,
        &world.project,
        &world.setup,
        session,
        "Run it slowly",
    );
    run.wait_for_output(r#""stream":"out","text":"started"#);

    let follower = open(&runner, &world, Some(session.to_owned()));
    assert_eq!(follower.thread.state(), &ThreadState::Working);
    let entries = follower.thread.entries();
    assert_eq!(
        entries[0],
        Entry::User(vec![Block::Text("Run it slowly".into())]),
        "{:#?}",
        follower.updates
    );
    assert_eq!(
        entries[1],
        Entry::Thought("The user wants a command run.".into())
    );
    assert_eq!(entries[2], Entry::Assistant("I'll run it.".into()));
    let call = tool(&entries[3]);
    assert_eq!(call.status, ToolStatus::Running);
    assert!(
        matches!(
            call.content.as_slice(),
            [ToolContent::Terminal { output, exit: None, .. }] if output == "started\n"
        ),
        "what the command wrote before the session opened shows: {:?}",
        call.content
    );
    assert!(
        notices(&follower.thread).is_empty(),
        "{:#?}",
        follower.updates
    );

    // Closing the session lets go of the run, which goes on.
    follower.close();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        run.running(),
        "a run apart from its cockpit outlives the session"
    );
    assert!(kou_native::sessions(&world.project).expect("sessions")[0].running);

    // Taken up again, it is stopped as a cockpit that follows it would stop it.
    let mut follower = open(&runner, &world, Some(session.to_owned()));
    assert_eq!(follower.thread.state(), &ThreadState::Working);
    follower
        .backend
        .commands
        .send_blocking(Command::Cancel)
        .expect("a backend");
    follower.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    assert_eq!(follower.thread.last_stop, Some(Stop::Cancelled));
    assert_eq!(
        tool(&follower.thread.entries()[3]).status,
        ToolStatus::Cancelled
    );
    assert_eq!(
        run.wait(),
        Some(130),
        "the runner exits as an interrupted one does"
    );
    assert!(!kou_native::sessions(&world.project).expect("sessions")[0].running);
    follower.close();

    let resumed = open(&runner, &world, Some(session.to_owned()));
    assert_eq!(
        notices(&resumed.thread),
        [
            "Run stopped: the user stopped the run from kou desktop",
            "The last run did not finish. Send /continue to pick it up."
        ]
    );
    resumed.close();
}

#[test]
fn prompts_queue_behind_a_run_and_the_composer_s_choices_reach_the_runner() {
    let Some(runner) = support::fresh_runner() else {
        eprintln!("skipped: no runner could be built");
        return;
    };
    let provider = Provider::start(|request, before| {
        // A compaction asks for a summary in a block of its own.
        if last_user_text(request).contains("<summary>") {
            return Answer::of(
                vec![Piece::Text(0, "<review>short</review>")],
                json!([message(
                    &format!("msg-{before}"),
                    "<review>short</review><summary>The user looked at a picture.</summary>"
                )]),
            );
        }
        Answer::of(
            vec![Piece::Text(0, "Seen.")],
            json!([message(&format!("msg-{before}"), "Seen.")]),
        )
    });
    let world = world(&provider);
    let mut follower = open(&runner, &world, None);
    let session = follower.thread.session.clone().expect("a session");

    // A model and an effort chosen before the first prompt, and a prompt
    // with a picture and a linked file; a second prompt right behind it.
    let commands = follower.backend.commands.clone();
    commands
        .send_blocking(Command::SetModel("other-model".into()))
        .expect("a backend");
    commands
        .send_blocking(Command::SetMode("low".into()))
        .expect("a backend");
    let pixel: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0,
        0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0, 0, 0, 13, b'I', b'D', b'A',
        b'T', b'x', 0x9c, b'c', 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0, 5, 0, 1, 0xff, 0x89, 0x99, b'=',
        0x1d, 0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, b'B', b'`', 0x82,
    ];
    let blocks = vec![
        Block::Text("What is in".into()),
        Block::Image {
            mime: "image/png".into(),
            data: pixel.clone().into(),
        },
        Block::Text("and in".into()),
        Block::File {
            path: world.project.join("notes.txt"),
            text: None,
        },
    ];
    follower.thread.push_user(blocks.clone());
    commands
        .send_blocking(Command::Prompt(blocks))
        .expect("a backend");
    follower.prompt("And a second thing");
    follower.until("the end of the first turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    follower.until("the end of the queued turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    assert_eq!(follower.thread.last_stop, Some(Stop::Finished));
    assert_eq!(follower.thread.model.as_deref(), Some("other-model"));
    assert_eq!(follower.thread.mode.as_deref(), Some("low"));
    assert_eq!(
        notices(&follower.thread),
        ["The connection does not list other-model. Runs try it anyway."]
    );
    assert_eq!(
        follower.thread.title.as_deref(),
        Some("What is in [Image 1] and in $notes.txt")
    );
    {
        let requests = provider.requests.lock().expect("the requests");
        assert_eq!(requests.len(), 2, "one request a prompt, in order");
        assert_eq!(requests[0]["model"], "other-model");
        assert_eq!(requests[0]["reasoning"]["effort"], "low");
        let first = requests[0].to_string();
        assert!(
            first.contains("What is in [Image 1] and in $notes.txt"),
            "{first}"
        );
        assert!(
            first.contains("data:image/png;base64,iVBORw0KGgo"),
            "{first}"
        );
        assert!(
            first.contains("kou"),
            "what the file holds goes with the prompt: {first}"
        );
        assert!(!first.contains("And a second thing"));
        assert!(last_user_text(&requests[1]).contains("And a second thing"));
    }
    // Both cockpits' files have the choices.
    let config = world.setup.settings_file.as_ref().expect("a settings file");
    let preferences: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config.with_file_name("preferences.json")).expect("preferences"),
    )
    .expect("JSON");
    assert_eq!(
        preferences,
        json!({"last_model": "other-model", "effort": "low"})
    );
    let meta: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            world
                .project
                .join(".harness/sessions/.meta")
                .join(format!("{session}.json")),
        )
        .expect("the session's meta"),
    )
    .expect("JSON");
    assert_eq!(meta, json!({"effort": "low"}));

    // A compaction on request, then nothing more to compact.
    follower.prompt("/compact the picture");
    follower.until("the end of the compaction", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    assert_eq!(follower.thread.last_stop, Some(Stop::Finished));
    let said = notices(&follower.thread);
    assert_eq!(said[1], "Compacting context");
    assert!(said[2].starts_with("Context compacted from "), "{said:?}");
    follower.prompt("/compact");
    follower.until("word that nothing is left to compact", |update| {
        matches!(update, Update::Notice(Level::Info, text) if text.starts_with("Nothing to compact"))
    });
    follower.prompt("/continue");
    follower.until("word that nothing is left to continue", |update| {
        matches!(update, Update::Notice(Level::Info, text) if text.contains("nothing to continue"))
    });
    assert_eq!(provider.requests.lock().expect("the requests").len(), 3);
    follower.close();

    // Resumed, the prompt comes back with its picture where its label was
    // and what the model saw of the file, and the session goes on with its
    // model and effort.
    let resumed = open(&runner, &world, Some(session));
    let Entry::User(stored) = &resumed.thread.entries()[0] else {
        panic!("a prompt: {:#?}", resumed.updates);
    };
    assert_eq!(
        stored.as_slice(),
        [
            Block::Text("What is in ".into()),
            Block::Image {
                mime: "image/png".into(),
                data: pixel.into(),
            },
            Block::Text(" and in $notes.txt".into()),
            Block::File {
                path: world.project.join("notes.txt"),
                text: Some("kou".into()),
            },
        ]
    );
    assert_eq!(resumed.thread.mode.as_deref(), Some("low"));
    // `connect` was given a model, which wins over the session's last one.
    assert_eq!(resumed.thread.model.as_deref(), Some("stub-model"));
    resumed.close();
}

#[test]
fn a_session_does_not_open_without_a_runner_a_project_or_a_valid_id() {
    let root = tempfile::tempdir().expect("a directory");
    let options = |runner: &Path, cwd: &Path, session: Option<&str>| Options {
        runner: runner.to_owned(),
        cwd: cwd.to_owned(),
        session: session.map(str::to_owned),
        model: None,
    };
    let exe = std::env::current_exe().expect("this test");
    let error = |options: Options| {
        kou_native::connect_with(options, Setup::default())
            .expect_err("no session")
            .to_string()
    };
    assert!(error(options(&root.path().join("missing"), root.path(), None)).contains("runner"));
    assert!(error(options(&exe, &root.path().join("missing"), None)).contains("project folder"));
    assert!(error(options(&exe, root.path(), Some("../escape"))).contains("invalid session ID"));
    let mut spaced = options(&exe, root.path(), None);
    spaced.model = Some("two words".into());
    assert!(error(spaced).contains("model ID"));
}

/// A run that cannot start ends its turn with why, before any runner runs.
#[cfg(unix)]
#[test]
fn a_run_that_cannot_start_fails_its_turn_with_the_reason() {
    let root = tempfile::tempdir().expect("a directory");
    let config = root.path().join("config");
    std::fs::create_dir_all(&config).expect("a directory");
    // The saved connection goes through the accounts gateway, which is off.
    std::fs::write(
        config.join("settings.json"),
        r#"{"provider_type":"gateway"}"#,
    )
    .expect("settings");
    let backend = kou_native::connect_with(
        Options {
            // Never started: the launch fails before the runner is.
            runner: "/bin/sh".into(),
            cwd: root.path().to_owned(),
            session: None,
            model: None,
        },
        Setup {
            settings_file: Some(config.join("settings.json")),
            ..Setup::default()
        },
    )
    .expect("a backend");
    let mut follower = Follower::new(backend);
    follower.until("Ready", |update| matches!(update, Update::Ready { .. }));
    assert_eq!(follower.thread.model, None);
    follower.until("word that the models are not listed", |update| {
        matches!(update, Update::Notice(Level::Warning, text) if text.contains("Cannot list the connection's models"))
    });
    follower.prompt("Anything");
    follower.prompt("And more");
    follower.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    let Some(Stop::Failed(reason)) = &follower.thread.last_stop else {
        panic!("the turn failed: {:#?}", follower.updates);
    };
    assert!(
        reason.contains("the accounts gateway is not running"),
        "{reason}"
    );
    assert_eq!(follower.thread.state(), &ThreadState::Idle);
    // The second prompt found no run to wait for, and failed the same way.
    follower.until("the end of the second turn", |update| {
        matches!(update, Update::TurnEnded(Stop::Failed(_)))
    });
    assert!(
        !root.path().join(".harness").exists(),
        "a run that never started leaves nothing in the project"
    );
    follower.close();
}

/// A small PNG: what a paste brings.
fn png() -> Vec<u8> {
    // One pixel, as image encoders write it.
    base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
    )
    .expect("a picture")
}

#[test]
fn a_linked_file_and_a_pasted_picture_reach_the_model() {
    let Some(runner) = support::fresh_runner() else {
        eprintln!("skipped: no runner could be built");
        return;
    };
    let provider = Provider::start(|_, before| {
        Answer::of(
            vec![Piece::Text(0, "Seen."), Piece::Pause],
            json!([message(&format!("msg-{before}"), "Seen.")]),
        )
    });
    let world = world(&provider);
    std::fs::write(
        world.project.join("linked.txt"),
        "line one\nthe linked line says kou-linked-42\n",
    )
    .expect("a file");
    let mut follower = open(&runner, &world, None);
    let picture = png();
    // As the view sends it: the picture where its label was, the link in the text.
    let blocks = vec![
        Block::Text("Compare ".into()),
        Block::Image {
            mime: "image/png".into(),
            data: picture.clone().into(),
        },
        Block::Text(", then read $linked.txt:2 please.".into()),
    ];
    follower.thread.push_user(blocks.clone());
    follower
        .backend
        .commands
        .send_blocking(Command::Prompt(blocks))
        .expect("a backend");
    follower.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    let requests = provider.requests.lock().expect("the requests").clone();
    let request = requests.first().expect("the model was asked").to_string();
    // The picture went as a picture, named in the text by its label.
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &picture);
    assert!(
        request.contains(&format!("data:image/png;base64,{encoded}")),
        "{request}"
    );
    assert!(
        request.contains("Compare [Image 1], then read $linked.txt:2 please."),
        "{request}"
    );
    // The runner read the line the link asks for and showed it to the model.
    assert!(request.contains("kou-linked-42"), "{request}");
    assert_eq!(
        follower.thread.last_stop,
        Some(Stop::Finished),
        "{:#?}",
        follower.updates
    );
    follower.close();
}

#[test]
fn a_message_forced_into_a_running_turn_reaches_the_model_after_the_command() {
    let Some(runner) = support::fresh_runner() else {
        eprintln!("skipped: no runner could be built");
        return;
    };
    let provider = scripted();
    let world = world(&provider);
    let mut follower = open(&runner, &world, None);
    // What the backend offers comes right after it is ready.
    follower.until("the offers", |update| matches!(update, Update::Offers(_)));
    assert!(follower.thread.offers.steer, "the runner takes messages");
    follower.prompt("Run it briefly");
    follower.until("the command's first output", |update| {
        matches!(update, Update::ToolPatch(patch) if patch.content.as_deref().is_some_and(|content| {
            matches!(content, [ToolContent::Terminal { output, .. }] if output.contains("started"))
        }))
    });
    follower
        .backend
        .requests
        .send_blocking(kou_thread::Request::Steer(vec![Block::Text(
            "also say kou-steered-7".into(),
        )]))
        .expect("a backend");
    // The agent takes it: the thread shows it as the user's, in its place.
    follower.until("the forced message", |update| {
        matches!(update, Update::UserChunk(Block::Text(text)) if text.contains("kou-steered-7"))
    });
    follower.until("the end of the turn", |update| {
        matches!(update, Update::TurnEnded(_))
    });
    let requests = provider.requests.lock().expect("the requests").clone();
    assert!(
        requests
            .iter()
            .skip(1)
            .any(|request| request.to_string().contains("kou-steered-7")),
        "the model was asked with the message: {requests:#?}"
    );
    // One turn: the message did not start a run of its own.
    let turns = follower
        .updates
        .iter()
        .filter(|update| matches!(update, Update::TurnStarted))
        .count();
    assert_eq!(turns, 1);
    follower.close();
}
