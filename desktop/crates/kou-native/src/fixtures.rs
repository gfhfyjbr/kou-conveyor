//! Recorded output of the real Go runner, mapped to the exact updates a
//! thread takes and folded through [`kou_thread::Thread`].
//!
//! `tests/fixtures/run-*.jsonl` are six runs of one session, each the
//! runner's stdout as `kou-conveyor-runner -steer -stream` wrote it against a
//! local stand-in for the model: a command, file tools, a patch, a script, a
//! compaction, and a run stopped in the middle of a command.
//! `fixture-one.session.jsonl` is the session file those runs left. Only the
//! temporary workspace's path was replaced, by `/workspace`. The expected
//! updates beside them are regenerated with `KOU_NATIVE_BLESS=1`, and read
//! before they are trusted.
//!
//! The Go store's own golden sessions are read where the Go tests keep them.

use crate::{emit::Emitter, fold::Fold, transcript::Transcript};
use kou_thread::{
    Block, Entry, Level, Stop, Thread, ThreadState, ToolCall, ToolContent, ToolKind, ToolStatus,
    Update,
};
use std::path::{Path, PathBuf};

const WORKSPACE: &str = "/workspace";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// Compares updates with the recorded expectation, or records them.
fn expect(name: &str, updates: &[Update]) {
    let path = fixture(name);
    let mut actual = String::from("[\n");
    for (index, update) in updates.iter().enumerate() {
        let separator = if index + 1 == updates.len() { "" } else { "," };
        actual.push_str(&format!(
            "  {}{separator}\n",
            serde_json::to_string(update).expect("an update")
        ));
    }
    actual.push_str("]\n");
    if std::env::var_os("KOU_NATIVE_BLESS").is_some() {
        std::fs::write(&path, &actual).expect("an expectation");
        return;
    }
    let expected: Vec<Update> = serde_json::from_str(&read(name)).expect("the expectation");
    for (index, (actual, expected)) in updates.iter().zip(&expected).enumerate() {
        assert_eq!(actual, expected, "{name}: update {index}");
    }
    assert_eq!(updates.len(), expected.len(), "{name}: {actual}");
}

/// One recorded run through the fold, as the session drives it: the prompt
/// shown by the interface, the turn, the lines, the exit.
fn run(
    fold: &mut Fold,
    thread: &mut Thread,
    name: &str,
    prompt: Option<(&str, &str)>,
    code: i32,
) -> (Vec<Update>, Stop) {
    let mut updates = Vec::new();
    if let Some((message_id, text)) = prompt {
        thread.push_user(vec![Block::Text(text.to_owned())]);
        fold.emitter.sent(message_id);
    }
    fold.begin(prompt.is_none(), &mut |update| updates.push(update));
    for line in read(&format!("run-{name}.jsonl")).lines() {
        fold.line(line, false, &mut |update| updates.push(update));
    }
    let stop = fold.finish(Some(code), &mut |update| updates.push(update));
    for update in &updates {
        thread.apply(update.clone());
    }
    expect(&format!("run-{name}.updates.json"), &updates);
    (updates, stop)
}

fn tool<'a>(thread: &'a Thread, id: &str) -> &'a ToolCall {
    thread
        .entries()
        .iter()
        .find_map(|entry| match entry {
            Entry::Tool(call) if call.id == id => Some(call),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no call {id}"))
}

fn kinds(updates: &[Update]) -> Vec<&'static str> {
    updates
        .iter()
        .map(|update| match update {
            Update::Ready { .. } => "ready",
            Update::UserChunk(_) => "user",
            Update::AssistantChunk(_) => "text",
            Update::ThoughtChunk(_) => "thought",
            Update::AgentSaid { .. } => "said",
            Update::Tool(_) => "tool",
            Update::ToolPatch(_) => "patch",
            Update::Plan(_) => "plan",
            Update::Permission(_) => "permission",
            Update::Commands(_) => "commands",
            Update::Mode(_) => "mode",
            Update::Model(_) => "model",
            Update::Providers(_) => "providers",
            Update::Title(_) => "title",
            Update::Usage(_) => "usage",
            Update::TurnStarted => "start",
            Update::TurnEnded(_) => "end",
            Update::Notice(..) => "notice",
            Update::Closed(_) => "closed",
            Update::Offers(_) => "offers",
            Update::PromptTaken(_) => "taken",
            Update::PromptId(_) => "named",
            Update::Rewound(_) => "rewound",
        })
        .collect()
}

/// The six recorded runs, in the order they ran, in one session.
fn recorded_session() -> (Fold, Thread) {
    let mut fold = Fold::new(Path::new(WORKSPACE), None);
    let mut thread = Thread::new();

    // A command, with reasoning and text streaming before it and its output
    // streaming while it runs.
    let (updates, stop) = run(
        &mut fold,
        &mut thread,
        "bash",
        Some((
            "11111111-1111-4111-8111-111111111111",
            "Run the three echoes",
        )),
        0,
    );
    assert_eq!(stop, Stop::Finished);
    assert_eq!(
        kinds(&updates),
        [
            "start", "model", "thought", "thought", "text", "text", "usage", "tool", "patch",
            "patch", "patch", "patch", "text", "text", "usage", "end"
        ]
    );
    assert_eq!(updates[1], Update::Model("stub-model".into()));
    let call = tool(&thread, "call_bash_1");
    assert_eq!(
        (call.title.as_str(), call.kind, call.status),
        (
            "Run echo one; sleep 0.6; echo two >&2; sleep 0.6; echo three",
            ToolKind::Execute,
            ToolStatus::Done
        )
    );
    let [ToolContent::Terminal { output, exit, .. }] = call.content.as_slice() else {
        panic!("a terminal: {:?}", call.content);
    };
    assert_eq!((output.as_str(), *exit), ("one\nthree\ntwo\n", Some(0)));
    assert_eq!(
        thread.entries()[..3],
        [
            Entry::User(vec![Block::Text("Run the three echoes".into())]),
            Entry::Thought("The user wants a command run.".into()),
            Entry::Assistant("I'll run it now.".into()),
        ]
    );
    assert_eq!(
        thread.entries()[4],
        Entry::Assistant("It printed three lines.".into())
    );

    // File tools: a write, an edit and reads, one of a file that is missing,
    // and a picture. The prompt linked a file.
    let (_, stop) = run(
        &mut fold,
        &mut thread,
        "files",
        Some((
            "22222222-2222-4222-8222-222222222222",
            "Write, edit and read notes/hello.txt; see $pixel.png",
        )),
        0,
    );
    assert_eq!(stop, Stop::Finished);
    let write = tool(&thread, "call_write_1");
    assert_eq!(
        (write.title.as_str(), write.kind, write.status),
        ("Write notes/hello.txt", ToolKind::Edit, ToolStatus::Done)
    );
    assert_eq!(
        write.content,
        [ToolContent::Diff {
            path: "/workspace/notes/hello.txt".into(),
            old: None,
            new: "alpha\nbeta\ngamma\n".into(),
        }]
    );
    let edit = tool(&thread, "call_edit_1");
    assert_eq!(
        (edit.title.as_str(), edit.kind, edit.status),
        ("Edit notes/hello.txt", ToolKind::Edit, ToolStatus::Done)
    );
    assert_eq!(
        edit.content,
        [ToolContent::Diff {
            path: "/workspace/notes/hello.txt".into(),
            old: Some("beta".into()),
            new: "BETA".into(),
        }]
    );
    assert_eq!(edit.locations[0].line, Some(2));
    let reading = tool(&thread, "call_read_1");
    assert_eq!(
        (reading.title.as_str(), reading.kind, reading.status),
        ("Read notes/hello.txt", ToolKind::Read, ToolStatus::Done)
    );
    let [ToolContent::Text(text)] = reading.content.as_slice() else {
        panic!("text: {:?}", reading.content);
    };
    // The edit and the read were calls of one response: the read came first.
    assert_eq!(text, "```\n     1\talpha\n     2\tbeta\n     3\tgamma\n```");
    let missing = tool(&thread, "call_read_2");
    assert_eq!(missing.status, ToolStatus::Failed);
    let picture = tool(&thread, "call_view_1");
    assert_eq!(
        (picture.title.as_str(), picture.kind, picture.status),
        ("View pixel.png", ToolKind::Read, ToolStatus::Done)
    );
    assert_eq!(picture.content, [ToolContent::Text("1×1 image/png".into())]);

    // A patch of two files, and a search of the transcript.
    let (_, stop) = run(
        &mut fold,
        &mut thread,
        "patch",
        Some(("33333333-3333-4333-8333-333333333333", "Patch it")),
        0,
    );
    assert_eq!(stop, Stop::Finished);
    let patch = tool(&thread, "call_patch_1");
    assert_eq!(
        (patch.title.as_str(), patch.kind, patch.status),
        (
            "Patch added.txt, notes/hello.txt",
            ToolKind::Edit,
            ToolStatus::Done
        )
    );
    assert_eq!(
        patch.content,
        [
            ToolContent::Diff {
                path: "/workspace/added.txt".into(),
                old: None,
                new: "first\nsecond\n".into(),
            },
            ToolContent::Diff {
                path: "/workspace/notes/hello.txt".into(),
                old: Some("alpha\nBETA\ngamma\n".into()),
                new: "alpha\nbeta again\ngamma\n".into(),
            },
        ]
    );
    let search = tool(&thread, "call_search_1");
    assert_eq!(
        (search.title.as_str(), search.kind, search.status),
        (
            "Search the transcript for BETA",
            ToolKind::Search,
            ToolStatus::Done
        )
    );

    // A script whose calls are tool calls, with another model.
    let (updates, stop) = run(
        &mut fold,
        &mut thread,
        "code",
        Some(("44444444-4444-4444-8444-444444444444", "Use a script")),
        0,
    );
    assert_eq!(stop, Stop::Finished);
    assert!(
        updates.contains(&Update::Model("other-model".into())),
        "a turn with another model says so"
    );
    let code = tool(&thread, "call_code_1");
    assert_eq!(
        (code.title.as_str(), code.kind, code.status),
        (
            "Run code: 2 tool calls",
            ToolKind::Execute,
            ToolStatus::Done
        )
    );
    let texts: Vec<&str> = code
        .content
        .iter()
        .map(|content| match content {
            ToolContent::Text(text) => text.as_str(),
            other => panic!("text: {other:?}"),
        })
        .collect();
    assert!(
        texts[0].starts_with("```js\nconst out = await bash("),
        "{texts:?}"
    );
    assert!(
        texts[1].starts_with("1. `bash` echo from-code"),
        "{texts:?}"
    );
    assert!(
        texts[1].contains("\n2. `readText` notes/hello.txt"),
        "{texts:?}"
    );
    assert_eq!(texts[2], "Logged:\n\n```\nfrom-code\n```");
    assert_eq!(texts[3], "Returned:\n\n```\n4\n```");

    // A compaction: no prompt, the summary is not part of the conversation.
    let before = thread.entries().len();
    let (updates, stop) = run(&mut fold, &mut thread, "compact", None, 0);
    assert_eq!(stop, Stop::Finished);
    assert_eq!(
        updates,
        [
            Update::TurnStarted,
            Update::Notice(Level::Info, "Compacting context".into()),
            // The conversation is now about the size of the summary.
            Update::Usage(kou_thread::Usage {
                context_used: Some(16),
                context_limit: None,
                input_tokens: Some(13_100),
                output_tokens: Some(240),
                cost_usd: None,
            }),
            Update::Notice(Level::Info, "Context compacted from 1k tokens".into()),
            Update::TurnEnded(Stop::Finished),
        ]
    );
    assert_eq!(thread.entries().len(), before + 2);

    // A run stopped in the middle of a command, as Cancel stops it.
    thread.push_user(vec![Block::Text("Run something slow".into())]);
    fold.emitter.sent("55555555-5555-4555-8555-555555555555");
    let mut updates = Vec::new();
    fold.begin(false, &mut |update| updates.push(update));
    for line in read("run-slow.jsonl").lines() {
        if line.contains(r#""Mode":"hard""#) {
            // The stop is the user's: the session's word of it is the turn's end.
            assert!(fold.cancel());
        }
        fold.line(line, false, &mut |update| updates.push(update));
    }
    let stop = fold.finish(Some(130), &mut |update| updates.push(update));
    for update in &updates {
        thread.apply(update.clone());
    }
    expect("run-slow.updates.json", &updates);
    assert_eq!(stop, Stop::Cancelled);
    assert_eq!(
        kinds(&updates),
        [
            "start", "model", "text", "usage", "tool", "patch", "patch", "patch", "end"
        ]
    );
    let slow = tool(&thread, "call_slow_1");
    assert_eq!(slow.status, ToolStatus::Cancelled);
    assert!(
        matches!(
            slow.content.as_slice(),
            [ToolContent::Terminal { output, exit: None, .. }, ToolContent::Text(_)]
                if output == "begun\n"
        ),
        "a stopped command keeps what it wrote: {:?}",
        slow.content
    );
    assert_eq!(thread.state(), &ThreadState::Idle);
    assert_eq!(thread.last_stop, Some(Stop::Cancelled));
    (fold, thread)
}

#[test]
fn recorded_runs_map_to_their_exact_updates() {
    let (fold, thread) = recorded_session();
    assert!(fold.transcript.interrupted(), "the last run was stopped");
    assert_eq!(thread.model.as_deref(), Some("stub-model"));
    assert_eq!(thread.usage.input_tokens, Some(14_100));
}

#[test]
fn the_stored_session_replays_as_what_the_live_runs_showed() {
    let (_, live) = recorded_session();

    let mut transcript = Transcript::new();
    for line in read("fixture-one.session.jsonl").lines() {
        transcript.apply(line);
    }
    assert_eq!(transcript.skipped, 0);
    let mut emitter = Emitter::new(Path::new(WORKSPACE));
    let mut updates = Vec::new();
    let interrupted = emitter.replay(&mut transcript, false, &mut |update| updates.push(update));
    expect("fixture-one.replay.json", &updates);
    assert!(interrupted);
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, Update::ToolPatch(_))),
        "a stored call arrives once, in its final state"
    );
    let mut replayed = Thread::new();
    for update in updates {
        replayed.apply(update);
    }
    assert_eq!(replayed.state(), &ThreadState::Idle);
    assert_eq!(replayed.last_stop, Some(Stop::Cancelled));

    // What a live run says that the session does not keep, and the other way
    // round: the compaction's start, and who stopped the last run.
    let live: Vec<&Entry> = live
        .entries()
        .iter()
        .filter(|entry| !matches!(entry, Entry::Notice(_, text) if text == "Compacting context"))
        .collect();
    let replayed: Vec<&Entry> = replayed
        .entries()
        .iter()
        .filter(
            |entry| !matches!(entry, Entry::Notice(_, text) if text.starts_with("Run stopped: ")),
        )
        .collect();
    assert_eq!(live.len(), replayed.len());
    for (index, (live, replayed)) in live.iter().zip(&replayed).enumerate() {
        match (live, replayed) {
            // A prompt's linked file comes back with what the model saw of it.
            (Entry::User(sent), Entry::User(stored)) => {
                assert_eq!(sent[0], stored[0], "prompt {index}");
            }
            // What a stopped command wrote is seen live only: the session
            // keeps the output of commands that ended.
            (Entry::Tool(shown), Entry::Tool(stored)) => {
                let mut shown = shown.clone();
                shown
                    .content
                    .retain(|content| !matches!(content, ToolContent::Terminal { exit: None, .. }));
                assert_eq!(&shown, stored, "call {index}");
            }
            _ => assert_eq!(live, replayed, "entry {index}"),
        }
    }
}

#[test]
fn the_go_store_s_golden_sessions_replay() {
    let golden = |name: &str| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../harness/sessionstore/localfile/testdata")
            .join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let mut transcript = Transcript::new();
        for line in text.lines() {
            transcript.apply(line);
        }
        assert_eq!(transcript.skipped, 0, "{name}");
        let mut updates = Vec::new();
        Emitter::new(Path::new(WORKSPACE)).replay(&mut transcript, false, &mut |update| {
            updates.push(update);
        });
        updates
    };
    // `named` says the model made the call; otherwise only its status is known.
    let test_call =
        |id: &str, named: bool, status: ToolStatus, content: Vec<ToolContent>| ToolCall {
            id: id.into(),
            title: if named { "test" } else { "Tool call" }.into(),
            kind: ToolKind::Other,
            status,
            content,
            raw_input: named.then(|| serde_json::json!({})),
            ..ToolCall::default()
        };

    assert_eq!(
        golden("golden-session.session.jsonl"),
        [
            Update::UserChunk(Block::Text(r#"{"message":"hello"}"#.into())),
            Update::PromptId("input-1".into()),
            Update::TurnStarted,
            Update::AssistantChunk("working".into()),
            Update::ThoughtChunk("inspect".into()),
            // An operation of a type no cockpit reads leaves the call running,
            // and nothing goes on: it will never finish.
            Update::Tool(test_call("call-1", false, ToolStatus::Cancelled, vec![])),
            Update::TurnEnded(Stop::Cancelled),
        ]
    );
    assert_eq!(
        golden("golden-fork.session.jsonl"),
        [
            Update::Notice(Level::Info, "Forked from session parent-session".into()),
            Update::UserChunk(Block::Text(r#"{"message":"child"}"#.into())),
            Update::PromptId("child-input".into()),
            Update::TurnStarted,
            Update::AssistantChunk("working".into()),
            Update::Tool(test_call("call-1", true, ToolStatus::Cancelled, vec![])),
            Update::ThoughtChunk("inspect".into()),
            Update::Notice(Level::Error, "server_error: failed".into()),
            Update::Notice(Level::Warning, "The model refused to respond".into()),
            Update::Tool(test_call(
                "call-error",
                false,
                ToolStatus::Failed,
                vec![ToolContent::Text("```\ninvalid arguments\n```".into())]
            )),
            Update::TurnEnded(Stop::Cancelled),
        ]
    );
    assert_eq!(golden("legacy-session.session.jsonl"), []);
    assert_eq!(
        golden("soft-stop-session.session.jsonl"),
        [],
        "a stop of a kind no cockpit records a reason for says nothing"
    );
}
