//! The rewind held to the Go cockpit's own.
//!
//! `tests/fixtures/rewind/*.json` are sessions before and after
//! `rewindSession` of `cmd/internal/cockpit`, written by that code itself (a
//! test of the package's, run through `go test -overlay` so that nothing was
//! added to the Go tree): the recorded session of `fixture-one.session.jsonl`
//! rewound before each of its prompts, with a directory of output for each of
//! its operations; a command that finishes during the run cut off; the notes
//! and the summaries as far as the log tells them, and as the snapshots of
//! the cockpits keep them; the change records of the workspace; branches of
//! branches; and logs that are no session. Each holds the files of the
//! session's directory before the rewind and after it, the session's log as
//! the length of what remains of it and what was added back, and the error
//! the Go code gave.

use super::*;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Case {
    id: String,
    message: String,
    next: String,
    #[serde(default)]
    fixture: Option<String>,
    #[serde(default)]
    session: Option<String>,
    before: BTreeMap<String, String>,
    error: String,
    gone: bool,
    keep: u64,
    tail: String,
    after: BTreeMap<String, String>,
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every file under `dir` but the session's log, by its path from `dir`.
fn files(dir: &Path, id: &str) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, id: &str, found: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).expect("a directory") {
            let entry = entry.expect("an entry");
            let path = entry.path();
            let name = path
                .strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/");
            if entry.file_type().expect("a type").is_dir() {
                walk(root, &path, id, found);
            } else if name != format!("{id}.session.jsonl") {
                found.insert(name, std::fs::read_to_string(&path).expect("a file"));
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(dir, dir, id, &mut found);
    found
}

/// A log with the time of each fork item it adds left out: the Go code made
/// its own as of when it ran.
fn without_fork_times(log: &str) -> String {
    log.split_inclusive('\n')
        .map(|line| {
            if !line.contains(r#""Kind":"fork""#) {
                return line.to_owned();
            }
            let mut record: Value = serde_json::from_str(line).expect("a record");
            let at = &mut record["data"]["Item"]["RecordedAt"];
            assert!(
                at.as_str().and_then(parse_time).is_some(),
                "a fork has a time: {line}"
            );
            *at = Value::Null;
            format!("{record}\n")
        })
        .collect()
}

#[test]
fn a_session_is_rewound_as_the_go_cockpit_rewinds_it() {
    let mut names: Vec<PathBuf> = std::fs::read_dir(fixtures().join("rewind"))
        .expect("the corpus")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    names.sort();
    assert!(names.len() >= 20, "the corpus is there");
    for name in names {
        let label = name.file_stem().unwrap().to_string_lossy().into_owned();
        let case: Case =
            serde_json::from_str(&std::fs::read_to_string(&name).expect("a case")).expect("a case");
        let dir = tempfile::tempdir().expect("a directory");
        for (path, content) in &case.before {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).expect("a directory");
            std::fs::write(path, content).expect("a file");
        }
        let log = match (&case.fixture, &case.session) {
            (Some(fixture), _) => {
                std::fs::read_to_string(fixtures().join(fixture)).expect("a fixture")
            }
            (None, Some(session)) => session.clone(),
            (None, None) => String::new(),
        };
        let path = session_path(dir.path(), &case.id);
        std::fs::write(&path, &log).expect("the session");

        let result = rewind_session(dir.path(), &case.id, &case.message, &case.next);
        let rewound = std::fs::read_to_string(&path).expect("the session");
        if case.error.is_empty() {
            result.unwrap_or_else(|error| panic!("{label}: {error:#}"));
            let expected = format!("{}{}", &log[..case.keep as usize], case.tail);
            assert_eq!(
                without_fork_times(&rewound),
                without_fork_times(&expected),
                "{label}: the session"
            );
        } else {
            let error = result.expect_err(&label);
            assert_eq!(
                error.downcast_ref::<PromptGone>().is_some(),
                case.gone,
                "{label}: {error:#}"
            );
            // What the JSON decoders say of a line differs; where it is is the same.
            let said = |error: &str| -> String {
                match error.split_once(": ") {
                    Some((place, rest)) if place.starts_with("read session record") => {
                        if rest.starts_with("the session header") || rest.starts_with("unsupported")
                        {
                            error.to_owned()
                        } else {
                            place.to_owned()
                        }
                    }
                    _ => error.to_owned(),
                }
            };
            assert_eq!(said(&format!("{error:#}")), said(&case.error), "{label}");
            assert_eq!(rewound, log, "{label}: a failed rewind leaves the session");
        }
        assert_eq!(
            files(dir.path(), &case.id),
            case.after,
            "{label}: the files beside it"
        );
    }
}

#[test]
fn summaries_go_back_to_the_compactions_that_remain() {
    let sections = [
        "## Compaction at 2026-10-20T11:59:59Z\n\nbefore the cut\n\n",
        // The headers tell the time to the second: of those in the second of
        // the cut, as many came after it as compactions were cut off. A
        // header in a summary is no section's.
        "## Compaction at 2026-10-20T12:00:00Z, workspace snapshot 0123abcd\n\nin its second\n## Compaction at 2026-10-20T12:00:05Z\n\n",
        "## Compaction at 2026-10-20T12:00:01Z\n\nafter it\n\n",
        "## Compaction at 2026-10-20T13:00:00Z\n\nlater\n\n",
    ];
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join("summaries.md");
    let time = |text: &str| parse_time(text);
    let cut = time("2026-10-20T12:00:00.5Z");
    for (at, compactions, want) in [
        (None, 0, 4),
        (cut, 0, 2),
        (cut, 2, 2),
        (cut, 3, 1),
        // What the clock tells stays.
        (cut, 4, 1),
        (time("2026-10-20T11:59:58Z"), 0, 0),
        (time("2027-01-01T00:00:00Z"), 1, 4),
    ] {
        std::fs::write(&path, sections.concat()).expect("the summaries");
        cut_summaries(&path, at, compactions).expect("a cut");
        let got = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(got, sections[..want].concat(), "{at:?} after {compactions}");
    }
    cut_summaries(&dir.path().join("none.md"), cut, 1).expect("no summaries");
}

#[test]
fn change_records_of_the_removed_runs_go_but_the_coming_run_s_stay() {
    let record = |token: &str| -> String {
        if token == "?" {
            return "not a record".to_owned();
        }
        let (message, before) = match token.strip_suffix('+') {
            Some(message) => (message, true),
            None => (token, false),
        };
        let mut record = serde_json::json!({"message": message, "tree": format!("tree-{token}"), "at": "2026-10-20T12:00:00Z"});
        if before {
            record["before"] = Value::Bool(true);
        }
        record.to_string()
    };
    let lines = |records: &str| -> String {
        records
            .split_whitespace()
            .map(|token| record(token) + "\n")
            .collect()
    };
    for (name, records, keep, next, want) in [
        (
            "of the discarded runs",
            "m1+ m1 m2+ m2 m3+ m3",
            "m1",
            "",
            "m1+ m1",
        ),
        (
            "but the coming run's",
            "m1+ m1 m2+ m2 m3+ m3 n+",
            "m1",
            "n",
            "m1+ m1 n+",
        ),
        ("when it took none", "m1+ m1 m2+ m2", "m1", "n", "m1+ m1"),
        ("after a rewind to the start", "m1+ m1 n+", "", "n", "n+"),
        ("what is not a record", "m1+ ? m2+", "m1", "", "m1+ ?"),
        ("none when all are dropped", "m1+ m1", "", "", ""),
        (
            "all when none are dropped",
            "m1+ m1 n+",
            "m1",
            "n",
            "m1+ m1 n+",
        ),
    ] {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join(".changes").join("changed.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).expect("a directory");
        std::fs::write(&path, lines(records)).expect("the records");
        let keep: HashSet<String> = keep.split_whitespace().map(str::to_owned).collect();
        prune_change_records(dir.path(), "changed", next, &keep).expect("pruned");
        let got = std::fs::read_to_string(&path).unwrap_or_else(|_| "(none)".to_owned());
        let want = if want.is_empty() {
            "(none)".to_owned()
        } else {
            lines(want)
        };
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn the_notes_a_message_of_the_harness_shows_are_read_whole_or_not_at_all() {
    let shown = format!(
        "{NOTES_UPDATE_PREFIX} They are in x.notes.md, and read now:\n<notes>\nkeep this\n</notes>"
    );
    assert_eq!(notes_shown_by(&shown).as_deref(), Some("keep this"));
    let emptied = format!("{NOTES_UPDATE_PREFIX} The file x.notes.md is empty now.");
    assert_eq!(notes_shown_by(&emptied).as_deref(), Some(""));
    assert_eq!(notes_shown_by("[harness] something else"), None);
    let item = |notes: &str| Item {
        recorded_at: None,
        data: Data::Response {
            compacted: Some(Some(notes.to_owned())),
            arguments: Vec::new(),
        },
    };
    assert_eq!(notes_shown_in(&item("short")).as_deref(), Some("short"));
    assert_eq!(
        notes_shown_in(&item(&format!("head\n[… 3 KB {NOTES_LEFT_OUT} …]\ntail"))),
        None
    );
}
