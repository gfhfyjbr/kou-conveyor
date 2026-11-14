//! Editing a prompt through `connect`: the session goes back to before it
//! and the edited prompt runs from there, or nothing changes and the thread
//! gets back what it let go of.
//!
//! The runner here is a shell script that stands in for it: it records what
//! it is asked, writes the session's records the way the runner does (the
//! settings, the prompt, a turn and an answer that echoes the prompt) and
//! prints them. No model is asked anything, and nothing is logged in to.

#![cfg(unix)]

mod support;

use kou_native::{Options, Setup};
use kou_thread::{Block, Command, Entry, Request, Update};
use std::path::PathBuf;
use support::Follower;

/// The stand-in. `wait` makes a run that takes a second; `fail` one that
/// fails before its prompt is recorded.
const RUNNER: &str = r#"#!/bin/sh
case "$1" in -h|-providers) exit 0 ;; esac
while [ $# -gt 0 ]; do
  case "$1" in -session-directory) dir=$2; shift ;; esac
  shift
done
IFS= read -r request
printf '%s\n' "$request" >> "$dir/../requests.jsonl"
field() { printf '%s' "$request" | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"; }
id=$(field session_id); message=$(field message_id); content=$(field content)
[ "$content" = fail ] && { echo "the stand-in fails" >&2; exit 1; }
[ "$content" = wait ] && sleep 1
file="$dir/$id.session.jsonl"
[ -f "$file" ] || printf '{"type":"session","data":{"Version":2,"Session":{"ID":"%s","CreatedAt":"2026-11-12T00:00:00Z"}}}\n' "$id" > "$file"
n=$(wc -l < "$file")
at=2026-11-12T00:00:00Z
item() {
  n=$((n + 1))
  line="{\"Sequence\":$n,\"RecordedAt\":\"$at\",\"Kind\":\"$1\",\"Data\":$2}"
  printf '{"type":"item","data":{"Item":%s}}\n' "$line" >> "$file"
  printf '%s\n' "$line"
}
item input "{\"ID\":\"settings-$message\",\"Kind\":\"control\",\"Payload\":{\"Mode\":\"settings\",\"Reason\":\"\",\"Parameters\":{\"ReasoningEffort\":\"high\"}}}"
item input "{\"ID\":\"$message\",\"Kind\":\"external\",\"Payload\":\"$content\"}"
item turn "{\"ID\":\"turn-$message\",\"PreviousTurnID\":\"\",\"Type\":\"regular\",\"Model\":\"stub-model\"}"
item model_response "{\"TurnID\":\"turn-$message\",\"Response\":{\"Stop\":\"complete\",\"Output\":[{\"Type\":\"message\",\"Data\":{\"Role\":\"assistant\",\"Text\":\"echo: $content\"}}],\"Usage\":{\"InputTokens\":$n,\"OutputTokens\":1}}}"
"#;

struct World {
    _root: tempfile::TempDir,
    project: PathBuf,
    runner: PathBuf,
}

impl World {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().expect("a directory");
        let project = root.path().join("project");
        std::fs::create_dir_all(project.join(".harness/sessions")).expect("a project");
        let runner = root.path().join("kou-conveyor-runner");
        std::fs::write(&runner, RUNNER).expect("the stand-in");
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
            .expect("an executable");
        Self {
            project: project.canonicalize().expect("a project"),
            _root: root,
            runner,
        }
    }

    fn open(&self) -> Follower {
        let backend = kou_native::connect_with(
            Options {
                runner: self.runner.clone(),
                cwd: self.project.clone(),
                session: Some("edited".into()),
                model: Some("stub-model".into()),
            },
            // A provider configured by its own environment: nothing is listed
            // and nothing is read of the user's settings.
            Setup {
                settings_file: None,
                provider: Some("ollama".into()),
                env: Vec::new(),
            },
        )
        .expect("a backend");
        let mut follower = Follower::new(backend);
        follower.until("the offers", |update| matches!(update, Update::Offers(_)));
        follower
    }

    fn session(&self) -> String {
        std::fs::read_to_string(self.project.join(".harness/sessions/edited.session.jsonl"))
            .unwrap_or_default()
    }

    /// What the runner was asked, run by run.
    fn requests(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.project.join(".harness/requests.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("a request"))
            .collect()
    }
}

fn ended(update: &Update) -> bool {
    matches!(update, Update::TurnEnded(_))
}

fn text(text: &str) -> Vec<Block> {
    vec![Block::Text(text.to_owned())]
}

/// Edits the prompt of user entry `index` as an interface does: the thread
/// lets go of it and what followed, and the backend is asked to rewind.
fn edit(follower: &mut Follower, index: usize, edited: &str) {
    let rewinding = follower
        .thread
        .rewind(index, text(edited))
        .expect("a prompt to edit");
    follower
        .backend
        .requests
        .send_blocking(Request::Rewind {
            prompt: rewinding.prompt,
            blocks: text(edited),
        })
        .expect("a backend");
}

fn rewound(follower: &mut Follower) -> Result<String, String> {
    follower.until("the rewind's answer", |update| {
        matches!(update, Update::Rewound(_))
    });
    match follower.updates.last() {
        Some(Update::Rewound(result)) => result.clone(),
        _ => unreachable!(),
    }
}

fn shown(follower: &Follower) -> Vec<String> {
    follower
        .thread
        .entries()
        .iter()
        .map(|entry| match entry {
            Entry::User(blocks) => match &blocks[0] {
                Block::Text(text) => format!("you: {text}"),
                other => format!("you: {other:?}"),
            },
            Entry::Assistant(text) => text.clone(),
            Entry::Notice(_, text) => format!("notice: {text}"),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn an_edited_prompt_runs_in_the_session_as_it_was_before_it() {
    let world = World::new();
    let mut session = world.open();
    for prompt in ["first", "second", "third"] {
        session.prompt(prompt);
        session.until("the end of the run", ended);
    }
    // Every prompt the session took is named for an edit.
    let asked = world.requests();
    let id = |run: usize| {
        asked[run]["messages"][0]["message_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(session.thread.prompt_id(2).map(str::to_owned), Some(id(1)));

    edit(&mut session, 2, "second, edited");
    let edited = rewound(&mut session).expect("a rewind");
    session.until("the end of the edited run", ended);

    // The runner got the edited prompt under the ID the thread knows it by,
    // in the session as it was after the first run.
    let asked = world.requests();
    assert_eq!(asked.len(), 4);
    assert_eq!(asked[3]["messages"][0]["content"], "second, edited");
    assert_eq!(asked[3]["messages"][0]["message_id"], edited.as_str());
    assert_eq!(session.thread.prompt_id(2), Some(edited.as_str()));
    let log = world.session();
    assert!(!log.contains(&id(1)) && !log.contains(&id(2)), "{log}");
    assert!(log.contains(&id(0)) && log.contains(&edited));
    assert_eq!(
        shown(&session),
        [
            "you: first",
            "echo: first",
            "you: second, edited",
            "echo: second, edited"
        ]
    );
    // The totals are those of what remains, then of the edited run.
    let usage = session
        .updates
        .iter()
        .filter_map(|update| match update {
            Update::Usage(usage) => usage.input_tokens,
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(usage.ends_with(&[24, 4, 12]), "{usage:?}");

    // The edited prompt is edited again, from the start: the title follows.
    edit(&mut session, 0, "a better first");
    rewound(&mut session).expect("a rewind");
    session.until("the end of the run", ended);
    assert!(
        session
            .updates
            .iter()
            .any(|update| *update == Update::Title("a better first".into()))
    );
    assert_eq!(
        shown(&session),
        ["you: a better first", "echo: a better first"]
    );
    let records = world.session().lines().count();
    assert_eq!(records, 5, "the header, then one run");

    // A session opened again names what it replays the same way.
    session.close();
    let reopened = world.open();
    let name = reopened.thread.prompt_id(0).expect("a name").to_owned();
    assert!(world.session().contains(&name));
}

#[test]
fn a_rewind_that_cannot_be_done_changes_nothing() {
    let world = World::new();
    let mut session = world.open();
    session.prompt("first");
    session.until("the end of the run", ended);
    let log = world.session();
    let before = session.thread.entries().to_vec();

    // A prompt the session does not hold: it changed elsewhere.
    session.thread.rewind(0, text("edited")).expect("a prompt");
    session
        .backend
        .requests
        .send_blocking(Request::Rewind {
            prompt: Some("not-a-prompt".into()),
            blocks: text("edited"),
        })
        .expect("a backend");
    let error = rewound(&mut session).expect_err("refused");
    assert!(error.contains("no longer holds"), "{error}");
    assert_eq!(
        session.thread.entries(),
        before.as_slice(),
        "the thread has it back"
    );
    assert_eq!(world.session(), log);
    assert_eq!(world.requests().len(), 1, "no run started");

    // Nothing to run.
    edit(&mut session, 0, "  ");
    let error = rewound(&mut session).expect_err("refused");
    assert!(error.contains("no printable text"), "{error}");
    assert_eq!(world.session(), log);

    // Not while the agent works.
    session.prompt("wait");
    session.until("the run", |update| matches!(update, Update::TurnStarted));
    edit(&mut session, 0, "edited");
    let error = rewound(&mut session).expect_err("refused");
    assert!(error.contains("working"), "{error}");
    session.until("the end of the run", ended);
    assert_eq!(
        shown(&session),
        ["you: first", "echo: first", "you: wait", "echo: wait"]
    );
}

#[test]
fn a_prompt_that_never_reached_the_session_is_simply_run_again() {
    let world = World::new();
    let mut session = world.open();
    session.prompt("first");
    session.until("the end of the run", ended);
    session.prompt("fail");
    session.until("the failed run", ended);
    let log = world.session();
    let prompt = session.thread.entries().len() - 2;
    assert_eq!(shown(&session)[prompt], "you: fail");

    edit(&mut session, prompt, "now it works");
    rewound(&mut session).expect("a rewind");
    session.until("the end of the run", ended);
    // Nothing of the session went: the edited prompt follows the first.
    assert!(world.session().starts_with(&log));
    assert_eq!(
        shown(&session),
        [
            "you: first",
            "echo: first",
            "you: now it works",
            "echo: now it works"
        ]
    );
    // A prompt sent the way an interface sends one is named as it is taken.
    session
        .backend
        .commands
        .send_blocking(Command::Prompt(text("/model other")))
        .expect("a backend");
    session.until("its answer", |update| {
        matches!(update, Update::PromptTaken(None))
    });
}
