//! The sessions of a project, for a rail: what each is called, when the user
//! last wrote to it, how many messages they sent, the model that answered
//! last, and whether a run holds it right now.
//!
//! Session files run to tens of megabytes, mostly tool output, so a listing
//! does not decode them: it reads the first bytes of each record, which say
//! what the record is, and skips to the next line. A file only grows while it
//! is the same file, so what was read of it is kept for the process and only
//! what a session gained is read the next time.

use crate::{
    connection::Meta,
    runner::{session_busy, session_dir, valid_session_id},
    text::{clean, headline, parse_time},
};
use anyhow::{Context as _, Result};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::SystemTime,
};

const SESSION_SUFFIX: &str = ".session.jsonl";
/// How much of a record is looked at: enough for what it is and, for the
/// first prompt, a title's worth of its text.
const HEAD: usize = 2048;
/// How every item record begins, as the runner writes them.
const ITEM_PREFIX: &[u8] = br#"{"type":"item","data":{"Item":{"Sequence":"#;

/// A stored session, as a list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSummary {
    /// The session's ID, to resume it with.
    pub id: String,
    /// The title the user gave it, else the one a model made of its first
    /// prompt, else that prompt on one line. Empty for a session without a
    /// prompt.
    pub title: String,
    /// When the user last wrote to the session, or when its file last changed
    /// for a session without a prompt. The agent at work does not move a
    /// session in the list, as in the Go cockpits.
    pub updated: SystemTime,
    /// The model that answered the latest prompt, when the session says.
    pub model: Option<String>,
    /// Whether a run, of any front-end, holds the session right now.
    pub running: bool,
    /// How many messages the user sent.
    pub messages: usize,
}

/// What was read of a session file so far.
#[derive(Clone, Debug, Default)]
struct Scan {
    /// The file the scan is of; a rewound session is another file.
    identity: Option<(u64, u64)>,
    /// The end of the last whole record read.
    offset: u64,
    messages: usize,
    prompted: Option<SystemTime>,
    model: Option<String>,
    title: Option<String>,
}

/// Lists the sessions of a project, the one the user wrote to last first.
///
/// The first call in a process reads every session file once, a few at a
/// time (about half a second for two gigabytes of sessions); later calls read
/// only what the files gained, which makes them cheap enough for a rail that
/// opens.
pub fn sessions(project: &Path) -> Result<Vec<SessionSummary>> {
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, Scan>>> = OnceLock::new();
    let directory = session_dir(project);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("list sessions"),
    };
    struct Found {
        id: String,
        path: PathBuf,
        modified: SystemTime,
        length: u64,
        scan: Scan,
    }
    let known = KNOWN.get_or_init(Mutex::default);
    let mut found = Vec::new();
    {
        let known = known.lock().ok();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|name| name.strip_suffix(SESSION_SUFFIX))
            else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() || !valid_session_id(id) {
                continue;
            }
            let path = entry.path();
            let identity = identity_of(&metadata);
            let scan = known
                .as_ref()
                .and_then(|known| known.get(&path))
                .filter(|scan| scan.identity == identity && scan.offset <= metadata.len())
                .cloned()
                .unwrap_or(Scan {
                    identity,
                    ..Scan::default()
                });
            found.push(Found {
                id: id.to_owned(),
                path,
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                length: metadata.len(),
                scan,
            });
        }
    }
    // The files that gained something are read side by side: the disk and
    // the page cache serve several readers faster than one.
    let behind: Vec<Mutex<&mut Found>> = found
        .iter_mut()
        .filter(|found| found.scan.offset < found.length)
        .map(Mutex::new)
        .collect();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .min(8)
        .min(behind.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(found) = behind.get(index) else {
                        return;
                    };
                    let Ok(mut found) = found.lock() else {
                        return;
                    };
                    let found = &mut **found;
                    if let Err(error) = found.scan.read(&found.path) {
                        log::debug!("{}: {error}", found.path.display());
                    }
                }
            });
        }
    });
    drop(behind);
    if let Ok(mut known) = known.lock() {
        for found in &found {
            known.insert(found.path.clone(), found.scan.clone());
        }
    }
    let mut summaries: Vec<SessionSummary> = found
        .into_iter()
        .map(|found| {
            let meta = Meta::load(&directory, &found.id);
            SessionSummary {
                title: meta
                    .shown_title()
                    .map(str::to_owned)
                    .or(found.scan.title)
                    .unwrap_or_default(),
                updated: found.scan.prompted.unwrap_or(found.modified),
                model: found.scan.model,
                running: session_busy(&directory, &found.id),
                messages: found.scan.messages,
                id: found.id,
            }
        })
        .collect();
    summaries.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    Ok(summaries)
}

#[cfg(unix)]
fn identity_of(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity_of(_metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

impl Scan {
    /// Reads the records the file gained since the last read. A last line
    /// without a newline, a record the runner is still writing, waits.
    fn read(&mut self, path: &Path) -> std::io::Result<()> {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buffer = vec![0; 1 << 20];
        let mut head = Vec::with_capacity(HEAD);
        let mut pending = 0u64;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                return Ok(());
            }
            let mut data = &buffer[..read];
            while !data.is_empty() {
                let end = memchr::memchr(b'\n', data);
                let line = &data[..end.unwrap_or(data.len())];
                let room = HEAD - head.len();
                head.extend_from_slice(&line[..line.len().min(room)]);
                match end {
                    Some(end) => {
                        self.record(&head);
                        self.offset += pending + end as u64 + 1;
                        pending = 0;
                        head.clear();
                        data = &data[end + 1..];
                    }
                    None => {
                        pending += data.len() as u64;
                        data = &[];
                    }
                }
            }
        }
    }

    /// Takes in one record by its first bytes.
    fn record(&mut self, head: &[u8]) {
        let Some(rest) = head.strip_prefix(ITEM_PREFIX) else {
            return;
        };
        let Some(kind) = string_after(rest, br#""Kind":""#) else {
            return;
        };
        match kind.as_slice() {
            b"input" => {
                // The input's own kind follows its ID.
                let Some(data) = after(rest, br#""Data":{"#) else {
                    return;
                };
                if string_after(data, br#""Kind":""#).as_deref() != Some(b"external".as_slice()) {
                    return;
                }
                self.messages += 1;
                let recorded = string_after(rest, br#""RecordedAt":""#)
                    .and_then(|at| parse_time(&String::from_utf8_lossy(&at)));
                if recorded > self.prompted {
                    self.prompted = recorded;
                }
                if self.title.is_none() {
                    self.title = after(data, br#""Payload":"#)
                        .and_then(payload_text)
                        .map(|text| headline(&clean(&text), 80))
                        .filter(|title| !title.is_empty());
                }
            }
            b"turn" => {
                let Some(data) = after(rest, br#""Data":{"#) else {
                    return;
                };
                if string_after(data, br#""Type":""#).as_deref() != Some(b"regular".as_slice()) {
                    return;
                }
                if let Some(model) = string_after(data, br#""Model":""#)
                    .map(|model| String::from_utf8_lossy(&model).into_owned())
                    .filter(|model| !model.is_empty())
                {
                    self.model = Some(model);
                }
            }
            _ => {}
        }
    }
}

/// What follows the first `needle` in `data`.
fn after<'a>(data: &'a [u8], needle: &[u8]) -> Option<&'a [u8]> {
    let at = memchr::memmem::find(data, needle)?;
    Some(&data[at + needle.len()..])
}

/// The plain string that follows the first `needle`, up to its closing
/// quote; `None` when the bytes end before it does.
fn string_after(data: &[u8], needle: &[u8]) -> Option<Vec<u8>> {
    let rest = after(data, needle)?;
    let end = memchr::memchr(b'"', rest)?;
    Some(rest[..end].to_vec())
}

/// The text of a prompt's payload, as far as the bytes at hand go: a JSON
/// string, or the `Text` of a message that brought images or files.
fn payload_text(payload: &[u8]) -> Option<String> {
    let string = match payload.first()? {
        b'"' => &payload[1..],
        b'{' => after(payload, br#""Text":""#)?,
        _ => return None,
    };
    // The string may be cut short: it is closed where it ends, without the
    // escape or the character the cut went through.
    let mut end = 0;
    let mut closed = false;
    while end < string.len() {
        match string[end] {
            b'"' => {
                closed = true;
                break;
            }
            b'\\' => {
                let length = if string.get(end + 1) == Some(&b'u') {
                    6
                } else {
                    2
                };
                if end + length > string.len() {
                    break;
                }
                end += length;
            }
            _ => end += 1,
        }
    }
    let mut cut = &string[..end];
    if !closed {
        // A surrogate pair cut in two is no character, nor is half a UTF-8 one.
        while let Some(at) = cut.len().checked_sub(6) {
            if cut[at..].starts_with(b"\\ud8")
                || cut[at..].starts_with(b"\\ud9")
                || cut[at..].starts_with(b"\\uda")
                || cut[at..].starts_with(b"\\udb")
                || cut[at..].starts_with(b"\\uD8")
                || cut[at..].starts_with(b"\\uD9")
                || cut[at..].starts_with(b"\\uDA")
                || cut[at..].starts_with(b"\\uDB")
            {
                cut = &cut[..at];
            } else {
                break;
            }
        }
        cut = match std::str::from_utf8(cut) {
            Ok(_) => cut,
            Err(error) => &cut[..error.valid_up_to()],
        };
    }
    let mut quoted = Vec::with_capacity(cut.len() + 2);
    quoted.push(b'"');
    quoted.extend_from_slice(cut);
    quoted.push(b'"');
    serde_json::from_slice(&quoted).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Body, Transcript};
    use std::io::Write as _;

    fn record(sequence: u32, at: &str, kind: &str, data: &str) -> String {
        format!(
            r#"{{"type":"item","data":{{"Item":{{"Sequence":{sequence},"RecordedAt":"{at}","Kind":"{kind}","Data":{data}}}}}}}"#
        ) + "\n"
    }

    fn write_session(project: &Path, id: &str, records: &[String]) -> PathBuf {
        let directory = session_dir(project);
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join(format!("{id}{SESSION_SUFFIX}"));
        std::fs::write(&path, records.concat()).expect("a session");
        path
    }

    #[test]
    fn a_listing_counts_prompts_and_reads_only_what_a_session_gained() {
        let project = tempfile::tempdir().expect("a directory");
        assert_eq!(sessions(project.path()).expect("no sessions"), []);

        let padding = "x".repeat(3 << 20);
        let older = write_session(
            project.path(),
            "aaaa",
            &[
                "{\"type\":\"session\",\"data\":{\"Version\":2}}\n".to_owned(),
                record(
                    1,
                    "2026-01-01T10:00:00Z",
                    "input",
                    r#"{"ID":"c","Kind":"control","Payload":{"Mode":"settings"}}"#,
                ),
                record(
                    2,
                    "2026-01-01T10:00:01Z",
                    "input",
                    r#"{"ID":"m1","Kind":"external","Payload":"Fix the\n  build \"now\" — please"}"#,
                ),
                record(
                    3,
                    "2026-01-01T10:00:02Z",
                    "turn",
                    r#"{"ID":"t1","PreviousTurnID":"","Type":"regular","Model":"claude-opus-5"}"#,
                ),
                // A tool output that quotes a record does not count as one.
                record(
                    4,
                    "2026-01-01T10:00:03Z",
                    "model_response",
                    &format!(
                        r#"{{"TurnID":"t1","Response":{{"Output":[{{"Type":"message","Data":{{"Text":"{padding} \"Kind\":\"input\",\"Data\":{{\"ID\":\"x\",\"Kind\":\"external\""}}}}]}}}}"#
                    ),
                ),
                record(
                    5,
                    "2026-01-01T10:00:04Z",
                    "turn",
                    r#"{"ID":"t2","PreviousTurnID":"t1","Type":"compaction","Model":"summarizer"}"#,
                ),
            ],
        );
        write_session(
            project.path(),
            "bbbb",
            &[
                record(
                    1,
                    "2026-01-02T09:00:00Z",
                    "input",
                    r#"{"ID":"m1","Kind":"external","Payload":{"Text":"Look at [Image 1]","Images":[{"Label":"[Image 1]","URL":"data:image/png;base64,AAAA"}]}}"#,
                ),
                record(
                    2,
                    "2026-01-02T09:00:05Z",
                    "input",
                    r#"{"ID":"m2","Kind":"external","Delivery":"after_tools","Payload":"and this"}"#,
                ),
            ],
        );
        std::fs::create_dir_all(session_dir(project.path()).join(".meta")).expect("a directory");
        std::fs::write(
            session_dir(project.path()).join(".meta/bbbb.json"),
            r#"{"auto_title":"Screenshot review"}"#,
        )
        .expect("meta");
        std::fs::write(
            session_dir(project.path()).join("notes.md"),
            "not a session",
        )
        .expect("a file");

        let listed = sessions(project.path()).expect("sessions");
        assert_eq!(listed.len(), 2);
        assert_eq!(
            (
                listed[0].id.as_str(),
                listed[0].title.as_str(),
                listed[0].messages
            ),
            ("bbbb", "Screenshot review", 2)
        );
        assert_eq!(
            listed[0].updated,
            parse_time("2026-01-02T09:00:05Z").expect("a time")
        );
        assert_eq!(listed[0].model, None);
        assert_eq!(
            (
                listed[1].id.as_str(),
                listed[1].title.as_str(),
                listed[1].messages
            ),
            ("aaaa", "Fix the build \"now\" \u{2014} please", 1)
        );
        assert_eq!(listed[1].model.as_deref(), Some("claude-opus-5"));
        assert!(!listed[1].running);

        // The session gains a prompt, and a record still being written.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&older)
            .expect("the session");
        file.write_all(
            record(
                6,
                "2026-01-03T08:00:00Z",
                "input",
                r#"{"ID":"m9","Kind":"external","Payload":"again"}"#,
            )
            .as_bytes(),
        )
        .expect("written");
        file.write_all(br#"{"type":"item","data":{"Item":{"Sequence":7,"RecordedAt":"2026-01-04T00:00:00Z","Kind":"input","Data":{"ID":"m10","Kind":"external","Payl"#)
            .expect("written");
        let listed = sessions(project.path()).expect("sessions");
        assert_eq!((listed[0].id.as_str(), listed[0].messages), ("aaaa", 2));
        assert_eq!(
            listed[0].updated,
            parse_time("2026-01-03T08:00:00Z").expect("a time")
        );

        // A rewound session is another file, read from its start.
        std::fs::remove_file(&older).expect("removed");
        write_session(
            project.path(),
            "aaaa",
            &[record(
                1,
                "2026-01-01T10:00:01Z",
                "input",
                r#"{"ID":"m1","Kind":"external","Payload":"Rewound"}"#,
            )],
        );
        let listed = sessions(project.path()).expect("sessions");
        assert_eq!(
            (
                listed[1].id.as_str(),
                listed[1].title.as_str(),
                listed[1].messages
            ),
            ("aaaa", "Rewound", 1)
        );
    }

    #[test]
    fn a_title_survives_a_payload_cut_anywhere() {
        assert_eq!(
            payload_text(br#""whole" and more"#).as_deref(),
            Some("whole")
        );
        assert_eq!(payload_text(br#""cut \"he"#).as_deref(), Some("cut \"he"));
        assert_eq!(payload_text(br#""cut \u00"#).as_deref(), Some("cut "));
        assert_eq!(payload_text(b"\"cut \\").as_deref(), Some("cut "));
        assert_eq!(payload_text(br#""pair \ud83d"#).as_deref(), Some("pair "));
        assert_eq!(
            payload_text(&"\"half п".as_bytes()[..7]).as_deref(),
            Some("half ")
        );
        assert_eq!(
            payload_text(br#"{"Text":"with images","Images":[]}"#).as_deref(),
            Some("with images")
        );
        assert_eq!(payload_text(b"42"), None);
    }

    /// The repository this crate lives in keeps real sessions; what the fast
    /// listing says of them must be what a full reading of the files says.
    #[test]
    fn the_listing_agrees_with_a_full_reading_of_the_repository_s_sessions() {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let directory = session_dir(&project);
        if !directory.is_dir() {
            eprintln!("skipped: {} has no sessions", directory.display());
            return;
        }
        let started = std::time::Instant::now();
        let listed = sessions(&project).expect("sessions");
        let first = started.elapsed();
        let started = std::time::Instant::now();
        let again = sessions(&project).expect("sessions");
        let second = started.elapsed();
        eprintln!(
            "{} sessions: first listing {first:?}, second {second:?}",
            listed.len()
        );
        // `KOU_NATIVE_PRINT=1 cargo test ... -- --nocapture` shows the list.
        if std::env::var_os("KOU_NATIVE_PRINT").is_some() {
            for summary in &listed {
                eprintln!(
                    "{} {:>4} {} {:<22} {}",
                    crate::text::format_time(summary.updated),
                    summary.messages,
                    if summary.running {
                        "running"
                    } else {
                        "       "
                    },
                    summary.model.as_deref().unwrap_or("-"),
                    headline(&summary.title, 60)
                );
            }
        }
        assert_eq!(
            listed.iter().map(|s| &s.id).collect::<Vec<_>>(),
            again.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
        assert!(
            listed
                .windows(2)
                .all(|pair| pair[0].updated >= pair[1].updated)
        );
        assert!(
            second < std::time::Duration::from_millis(500),
            "a second listing reads nothing again, yet took {second:?}"
        );

        // The smallest sessions are read whole and compared.
        let mut sizes: Vec<(u64, &SessionSummary)> = listed
            .iter()
            .filter_map(|summary| {
                let path = directory.join(format!("{}{SESSION_SUFFIX}", summary.id));
                Some((std::fs::metadata(path).ok()?.len(), summary))
            })
            .collect();
        sizes.sort_by_key(|(size, _)| *size);
        let mut compared = 0;
        for (size, summary) in sizes.into_iter().take(40) {
            if summary.running || size > 8 << 20 {
                continue;
            }
            let path = directory.join(format!("{}{SESSION_SUFFIX}", summary.id));
            let mut transcript = Transcript::new();
            for line in std::fs::read_to_string(path).expect("a session").lines() {
                transcript.apply(line);
            }
            let prompts = transcript
                .entries
                .iter()
                .filter(|entry| matches!(entry.body, Body::User(_)))
                .count();
            assert_eq!(summary.messages, prompts, "messages of {}", summary.id);
            assert_eq!(
                summary.model, transcript.last_model,
                "model of {}",
                summary.id
            );
            if Meta::load(&directory, &summary.id).shown_title().is_none() {
                assert_eq!(
                    Some(summary.title.as_str()).filter(|title| !title.is_empty()),
                    transcript.title().as_deref(),
                    "title of {}",
                    summary.id
                );
            }
            compared += 1;
        }
        eprintln!("{compared} sessions compared with a full reading");
        assert!(compared > 0 || listed.is_empty());
    }
}
