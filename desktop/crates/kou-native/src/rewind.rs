//! Editing a prompt that was already sent rewinds its session, as the Go
//! cockpits do it (`cmd/internal/cockpit/rewind.go`, `memory.go`): the
//! session file goes back to how it was before the prompt's run began, and
//! the edited prompt runs from there under the same session ID. What remains
//! is a state the session really was in, so the runner resumes it like any
//! other.
//!
//! The prompt's run and everything after it leave the session, with the
//! output their commands left behind. Commands started before the prompt
//! keep the outcome they reached later, so resuming never runs one twice.
//! The model's notes and the summaries of the session's compactions go back
//! too. Files the agent changed in the workspace stay as they are.
//!
//! A session file runs to hundreds of megabytes, so it is read a record at a
//! time, and what remains of it is copied from it, not held.
//!
//! The caller holds the session's lock (see [`crate::runner::lock_session`]).

use crate::{
    runner::{session_path, valid_session_id},
    text::{format_time, parse_time},
};
use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::{
    collections::HashSet,
    fs::File,
    io::{BufReader, BufWriter, Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
    time::SystemTime,
};

/// A prompt the session does not hold: the runner never recorded it, or the
/// session was rewound past it elsewhere (`cockpit.ErrPromptGone`).
#[derive(Debug)]
pub(crate) struct PromptGone;

impl std::fmt::Display for PromptGone {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the prompt is no longer in the session")
    }
}

impl std::error::Error for PromptGone {}

/// What a session keeps beside its log (`memory.go`).
const MEMORY_SUFFIX: &str = ".memory.jsonl";
const NOTES_SUFFIX: &str = ".notes.md";
const SUMMARIES_SUFFIX: &str = ".summaries.md";
/// The beginning of the harness's message that shows the notes as they are
/// now (`contextbuilder.NotesUpdatePrefix`).
const NOTES_UPDATE_PREFIX: &str =
    "[harness] Your notes changed since the conversation last showed them.";
/// What marks notes too long to be shown whole (`contextbuilder.notesLeftOut`).
const NOTES_LEFT_OUT: &str = "from the middle of the notes are left out here";
/// How much of the log is copied at a time.
const COPY_BUFFER: usize = 1 << 20;

fn sibling(dir: &Path, id: &str, suffix: &str) -> PathBuf {
    dir.join(format!("{id}{suffix}"))
}

/// Takes session `id` of the sessions in `dir` back to how it was before the
/// prompt with `message_id` was sent. `next` is the message ID of the prompt
/// the caller runs in the rewound session, `""` for none. Fails with
/// [`PromptGone`] when the session holds no such prompt, and then changes
/// nothing; so does any other failure before the log is replaced.
pub(crate) fn rewind_session(dir: &Path, id: &str, message_id: &str, next: &str) -> Result<()> {
    if !valid_session_id(id) {
        bail!("invalid session ID {id:?}");
    }
    let path = session_path(dir, id);
    let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
    let cut = cut_log(BufReader::new(&file), id, message_id)?;
    // The notes and the summaries go back before the log is cut, so that a
    // failure leaves the session as it was.
    restore_memory(dir, id, message_id, &cut).context("rewind session memory")?;
    // A new file rather than a truncated one: readers see the old history or
    // the new one.
    let temp = write_temp(dir, |out| cut.remains(&mut file, out, SystemTime::now()))
        .context("rewind session")?;
    drop(file);
    rename_temp(&temp, &path).context("rewind session")?;
    // The raw output of the commands the removed runs started.
    for operation in &cut.dropped {
        if valid_session_id(operation) {
            let _ = std::fs::remove_dir_all(dir.join("operations").join(id).join(operation));
        }
    }
    // Best effort: the session is rewound already.
    if let Err(error) = prune_memory(dir, id, &cut.messages) {
        log::debug!("the memory snapshots of the removed runs stay: {error:#}");
    }
    if let Err(error) = prune_change_records(dir, id, next, &cut.messages) {
        log::debug!("the change records of the removed runs stay: {error:#}");
    }
    Ok(())
}

/// Where a record is in a session log, its newline included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    at: u64,
    size: u64,
}

/// A fork item: whose history the items before it are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct Fork {
    #[serde(rename = "ParentID")]
    parent_id: String,
    #[serde(rename = "PreviousTurnID")]
    previous_turn_id: String,
}

/// A session log cut before the run of a prompt (`logCut`).
#[derive(Debug, Default)]
pub(crate) struct LogCut {
    /// How long the start of the log that remains is.
    keep: u64,
    /// What is added back after it: a fork item, the `sequence`-th item of
    /// the log; else the checkpoints at these places of the log.
    fork: Option<Fork>,
    sequence: u64,
    checkpoints: Vec<Span>,
    /// The operations only the removed part started.
    dropped: Vec<String>,
    /// One of the model's tool calls cut off names the session's notes.
    notes_named: bool,
    /// The IDs of the prompts that remain.
    messages: HashSet<String>,
    /// When the first record cut off was recorded.
    at: Option<SystemTime>,
    /// How many compactions the part cut off made.
    compactions: usize,
    /// The notes as the conversation that remains last showed them whole,
    /// and when.
    shown: String,
    shown_at: Option<SystemTime>,
}

impl LogCut {
    /// Writes the log that remains of `log`: the records before the cut, then
    /// what is added back, made as of `now`.
    fn remains(&self, log: &mut File, out: &mut dyn std::io::Write, now: SystemTime) -> Result<()> {
        log.seek(SeekFrom::Start(0))?;
        copy_exactly(log, self.keep, out)?;
        if let Some(fork) = &self.fork {
            let line = format!(
                "{{\"type\":\"item\",\"data\":{{\"Item\":{{\"Sequence\":{},\"RecordedAt\":{},\"Kind\":\"fork\",\"Data\":{{\"ParentID\":{},\"PreviousTurnID\":{}}}}}}}}}\n",
                self.sequence,
                Value::from(format_time(now)),
                Value::from(fork.parent_id.as_str()),
                Value::from(fork.previous_turn_id.as_str()),
            );
            out.write_all(line.as_bytes())?;
        }
        for span in &self.checkpoints {
            log.seek(SeekFrom::Start(span.at))?;
            copy_exactly(log, span.size, out)?;
        }
        Ok(())
    }
}

/// Copies `size` bytes of `from`, all of them.
fn copy_exactly(from: &mut File, size: u64, out: &mut dyn std::io::Write) -> Result<()> {
    let copied = std::io::copy(&mut from.take(size), out)?;
    if copied != size {
        bail!("the session file is shorter than it was read");
    }
    Ok(())
}

/// One record of a session file, decoded as far as a cut needs it.
#[derive(Debug, Default)]
struct LogRecord {
    item: Option<Item>,
    /// The operations a tool call status carries, or the one a checkpoint saves.
    operations: Vec<String>,
    /// An operation record.
    checkpoint: bool,
    resume_checkpoint: bool,
}

#[derive(Debug)]
struct Item {
    recorded_at: Option<SystemTime>,
    data: Data,
}

#[derive(Debug)]
enum Data {
    /// A message of the user's.
    Prompt(String),
    /// A control message of the harness: its mode and reason, `None` when it
    /// does not decode as one.
    Control(Option<(String, String)>),
    Turn(String),
    Fork(Fork),
    /// Where a tool call stands, with the operations it started.
    Status,
    /// A model's response: what its compaction showed of the notes (outer
    /// `Some` for a compaction's), and its tool calls' arguments.
    Response {
        compacted: Option<Option<String>>,
        arguments: Vec<String>,
    },
    Other,
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(borrow, default)]
    data: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct ItemRecord<'a> {
    #[serde(rename = "Item", borrow)]
    item: RawItem<'a>,
    #[serde(rename = "Operations", borrow, default)]
    operations: Option<Vec<&'a RawValue>>,
}

#[derive(Deserialize)]
struct RawItem<'a> {
    #[serde(rename = "RecordedAt", default)]
    recorded_at: String,
    #[serde(rename = "Kind", default)]
    kind: String,
    #[serde(rename = "Data", borrow, default)]
    data: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct OperationRecord {
    #[serde(rename = "Operation")]
    operation: OperationId,
}

#[derive(Deserialize)]
struct OperationId {
    #[serde(rename = "ID", default)]
    id: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Input<'a> {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Kind")]
    kind: String,
    #[serde(rename = "Payload", borrow)]
    payload: Option<&'a RawValue>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Response {
    #[serde(rename = "Response")]
    response: Outputs,
    #[serde(rename = "Compacted")]
    compacted: Option<Compacted>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Outputs {
    #[serde(rename = "Output")]
    output: Option<Vec<Output>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Output {
    #[serde(rename = "Type")]
    kind: String,
    #[serde(rename = "Data")]
    data: Option<Value>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Compacted {
    #[serde(rename = "Notes")]
    notes: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Id {
    #[serde(rename = "ID")]
    id: String,
}

/// Decodes one record of a session file, as `decodeLogRecord` does.
fn decode_record(line: &[u8]) -> Result<LogRecord, String> {
    let envelope: Envelope = serde_json::from_slice(line).map_err(|error| error.to_string())?;
    let data = envelope.data.map_or("", RawValue::get);
    let mut record = LogRecord::default();
    match envelope.kind.as_str() {
        "session" => {}
        "checkpoint" => record.resume_checkpoint = true,
        "item" => {
            let value: ItemRecord =
                serde_json::from_str(data).map_err(|error| error.to_string())?;
            for operation in value.operations.unwrap_or_default() {
                // Whole, or by ID alone when the file has it already.
                let id = match serde_json::from_str::<String>(operation.get()) {
                    Ok(id) => id,
                    Err(_) => {
                        serde_json::from_str::<Id>(operation.get())
                            .map_err(|error| error.to_string())?
                            .id
                    }
                };
                record.operations.push(id);
            }
            record.item = Some(Item {
                recorded_at: parse_time(&value.item.recorded_at),
                data: item_data(
                    &value.item.kind,
                    value.item.data.map_or("null", RawValue::get),
                ),
            });
        }
        "operation" => {
            let value: OperationRecord =
                serde_json::from_str(data).map_err(|error| error.to_string())?;
            record.checkpoint = true;
            record.operations.push(value.operation.id);
        }
        other => return Err(format!("unsupported record type {other:?}")),
    }
    Ok(record)
}

/// What an item of `kind` holds, as far as a cut looks.
fn item_data(kind: &str, data: &str) -> Data {
    match kind {
        "input" => {
            let Ok(input) = serde_json::from_str::<Input>(data) else {
                return Data::Other;
            };
            match input.kind.as_str() {
                "external" => Data::Prompt(input.id),
                "control" => Data::Control(control(input.payload.map_or("", RawValue::get))),
                _ => Data::Other,
            }
        }
        "turn" => serde_json::from_str::<Id>(data).map_or(Data::Other, |turn| Data::Turn(turn.id)),
        "fork" => serde_json::from_str::<Fork>(data).map_or(Data::Other, Data::Fork),
        "tool_call_status" => Data::Status,
        "model_response" => {
            let Ok(response) = serde_json::from_str::<Response>(data) else {
                return Data::Other;
            };
            let arguments = response
                .response
                .output
                .unwrap_or_default()
                .into_iter()
                .filter(|output| output.kind == "tool_call")
                .filter_map(|output| {
                    output
                        .data
                        .as_ref()?
                        .get("Arguments")?
                        .as_str()
                        .map(str::to_owned)
                })
                .collect();
            Data::Response {
                compacted: response.compacted.map(|compacted| compacted.notes),
                arguments,
            }
        }
        _ => Data::Other,
    }
}

/// A control message's mode and reason, as `DecodeControlMessage` reads it:
/// nothing but these and the parameters of a change of settings.
fn control(payload: &str) -> Option<(String, String)> {
    let Value::Object(fields) = serde_json::from_str::<Value>(payload).ok()? else {
        return None;
    };
    let text = |name: &str| match fields.get(name) {
        None => Some(String::new()),
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => None,
    };
    let (mode, reason) = (text("Mode")?, text("Reason")?);
    if fields
        .keys()
        .any(|name| !matches!(name.as_str(), "Mode" | "Reason" | "Parameters"))
        || mode != "settings" && fields.contains_key("Parameters")
    {
        return None;
    }
    Some((mode, reason))
}

/// Cuts the log of session `id` before the run that sent the prompt with
/// `message_id`, reading it a record at a time (`cutLog`).
///
/// A run starts by recording its settings, then its prompt, so the cut falls
/// before those settings: what remains is the log as the previous run left
/// it. Two things are added back. Operation checkpoints the removed part wrote
/// for operations started before the cut keep their latest outcome. And when
/// the cut falls in history inherited from another session, a fork item marks
/// what remains as inherited again, as the store requires.
fn cut_log(mut log: impl std::io::BufRead, id: &str, message_id: &str) -> Result<LogCut> {
    let mut cutter = Cutter {
        message_id,
        notes_file: format!("{id}{NOTES_SUFFIX}"),
        ..Cutter::default()
    };
    let mut at = 0u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        if log.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        // Only whole records count: the store discards a record whose write
        // was cut short.
        if line.last() != Some(&b'\n') {
            break;
        }
        let size = line.len() as u64;
        cutter.add(&line[..line.len() - 1], Span { at, size })?;
        at += size;
    }
    if !cutter.found {
        return Err(PromptGone.into());
    }
    let mut cut = cutter.cut;
    if let Some(fork) = cutter.fork
        && cutter.since_fork > 0
    {
        // The kept items after the last kept fork came from this fork's
        // parent: without a fork after them, the store would read them as
        // the session's own and reject their tool calls, whose operations
        // stayed with the parent. Nothing kept is the session's own to
        // checkpoint then.
        cut.fork = Some(Fork {
            parent_id: fork.parent_id,
            previous_turn_id: cutter.latest,
        });
        cut.sequence = cutter.items + 1;
        cut.checkpoints.clear();
    }
    Ok(cut)
}

/// Cuts a session log as [`cut_log`] reads it (`logCutter`).
#[derive(Default)]
struct Cutter<'a> {
    message_id: &'a str,
    notes_file: String,
    records: usize,
    /// The prompt was read: the records from the cut on are cut off.
    found: bool,
    /// The settings read last, which remain unless the prompt comes next.
    pending: Vec<(LogRecord, Span)>,
    cut: LogCut,
    items: u64,
    since_fork: u64,
    latest: String,
    /// The operations the kept log started, and those it can still
    /// checkpoint: started after its last fork.
    started: HashSet<String>,
    owned: HashSet<String>,
    /// The first fork cut off.
    fork: Option<Fork>,
}

impl Cutter<'_> {
    fn add(&mut self, line: &[u8], span: Span) -> Result<()> {
        let number = self.records + 1;
        let record = decode_record(line)
            .map_err(|error| anyhow!("read session record {number}: {error}"))?;
        let header = record.item.is_none() && !record.checkpoint && !record.resume_checkpoint;
        if header != (self.records == 0) {
            bail!(
                "read session record {number}: the session header must come first, and only once"
            );
        }
        self.records += 1;
        if self.found {
            self.cut_off(&record, span);
        } else if is_prompt(&record, self.message_id) {
            self.found = true;
            let pending = std::mem::take(&mut self.pending);
            let first = pending.first().map_or(span, |(_, span)| *span);
            self.cut.keep = first.at;
            self.cut.at = match pending.first() {
                Some((settings, _)) => settings.item.as_ref().and_then(|item| item.recorded_at),
                None => record.item.as_ref().and_then(|item| item.recorded_at),
            };
            for (settings, span) in &pending {
                self.cut_off(settings, *span);
            }
            self.cut_off(&record, span);
        } else if is_settings(&record) {
            self.pending.push((record, span));
        } else {
            for (settings, _) in std::mem::take(&mut self.pending) {
                self.remain(&settings);
            }
            self.remain(&record);
        }
        Ok(())
    }

    /// Takes in a record before the cut.
    fn remain(&mut self, record: &LogRecord) {
        let Some(item) = &record.item else {
            return;
        };
        self.items += 1;
        self.since_fork += 1;
        match &item.data {
            Data::Turn(id) => id.clone_into(&mut self.latest),
            Data::Fork(_) => {
                self.since_fork = 0;
                self.owned.clear();
            }
            Data::Prompt(id) => {
                self.cut.messages.insert(id.clone());
            }
            Data::Status => {
                for operation in &record.operations {
                    self.started.insert(operation.clone());
                    self.owned.insert(operation.clone());
                }
            }
            _ => {}
        }
        if let Some(notes) = notes_shown_in(item) {
            self.cut.shown = notes;
            self.cut.shown_at = item.recorded_at;
        }
    }

    /// Takes in a record from the cut on.
    fn cut_off(&mut self, record: &LogRecord, span: Span) {
        if record.checkpoint {
            if self.owned.contains(&record.operations[0]) {
                self.cut.checkpoints.push(span);
            }
            return;
        }
        let Some(item) = &record.item else {
            return;
        };
        for operation in &record.operations {
            if self.started.insert(operation.clone()) {
                self.cut.dropped.push(operation.clone());
            }
        }
        match &item.data {
            Data::Response {
                compacted,
                arguments,
            } => {
                if compacted.is_some() {
                    self.cut.compactions += 1;
                }
                if arguments
                    .iter()
                    .any(|arguments| arguments.contains(&self.notes_file))
                {
                    self.cut.notes_named = true;
                }
            }
            Data::Fork(fork) => {
                self.fork.get_or_insert_with(|| fork.clone());
            }
            _ => {}
        }
    }
}

fn is_prompt(record: &LogRecord, message_id: &str) -> bool {
    matches!(&record.item, Some(Item { data: Data::Prompt(id), .. }) if id == message_id)
}

/// The settings a run records before its prompt.
fn is_settings(record: &LogRecord) -> bool {
    matches!(
        &record.item,
        Some(Item { data: Data::Control(Some((mode, _))), .. }) if mode == "settings"
    )
}

/// The notes an item showed the model whole: the message of a compaction, or
/// one of the harness that they changed (`notesShownIn`).
fn notes_shown_in(item: &Item) -> Option<String> {
    let notes = match &item.data {
        Data::Response {
            compacted: Some(Some(notes)),
            ..
        } => notes.clone(),
        Data::Control(Some((mode, reason))) if mode == "heartbeat" => notes_shown_by(reason)?,
        _ => return None,
    };
    (!notes.contains(NOTES_LEFT_OUT)).then_some(notes)
}

/// The notes a message of the harness that they changed shows
/// (`contextbuilder.NotesShownBy`); `None` for any other message.
fn notes_shown_by(message: &str) -> Option<String> {
    if !message.starts_with(NOTES_UPDATE_PREFIX) {
        return None;
    }
    const OPEN: &str = "\n<notes>\n";
    let start = message.find(OPEN);
    let end = message.rfind("\n</notes>");
    Some(match (start, end) {
        (Some(start), Some(end)) if end >= start + OPEN.len() => {
            message[start + OPEN.len()..end].to_owned()
        }
        _ => String::new(),
    })
}

/// One line of a session's memory snapshots: the notes and the length of the
/// summaries a prompt's run started from, with the line as it was written.
#[derive(Debug)]
struct Snapshot {
    message: String,
    notes: String,
    summaries: u64,
    at: Option<SystemTime>,
    line: String,
}

/// Reads a session's memory snapshots, skipping a line that is none.
fn read_snapshots(dir: &Path, id: &str) -> Result<Vec<Snapshot>> {
    #[derive(Deserialize)]
    struct Line {
        #[serde(default)]
        message: String,
        #[serde(default)]
        notes: String,
        #[serde(default)]
        summaries: u64,
        #[serde(default)]
        at: Option<String>,
    }
    let data = match std::fs::read_to_string(sibling(dir, id, MEMORY_SUFFIX)) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    Ok(data
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let read: Line = serde_json::from_str(line).ok()?;
            // A time that is no time spoils the line; none is the zero time.
            let at = match &read.at {
                Some(at) => Some(parse_time(at)?),
                None => None,
            };
            Some(Snapshot {
                at,
                message: read.message,
                notes: read.notes,
                summaries: read.summaries,
                line: line.to_owned(),
            })
        })
        .collect())
}

/// Takes a session's notes and summaries back to how they were before the
/// run of the prompt with `message_id`: to the snapshot the run recorded as
/// it started, else as far as what remains of the log tells (`restoreMemory`).
fn restore_memory(dir: &Path, id: &str, message_id: &str, cut: &LogCut) -> Result<()> {
    let snapshots = read_snapshots(dir, id)?;
    let (notes, summaries) = (
        sibling(dir, id, NOTES_SUFFIX),
        sibling(dir, id, SUMMARIES_SUFFIX),
    );
    if let Some(snapshot) = snapshots
        .iter()
        .find(|snapshot| snapshot.message == message_id)
    {
        write_notes(&notes, &snapshot.notes).context("restore notes")?;
        // The summaries only ever grow: cut back to the length it recorded.
        truncate_file(&summaries, snapshot.summaries).context("restore summaries")?;
        return Ok(());
    }
    cut_summaries(&summaries, cut.at, cut.compactions).context("restore summaries")?;
    if let Some(before) = notes_before(&snapshots, cut) {
        write_notes(&notes, &before).context("restore notes")?;
    }
    Ok(())
}

/// What a session's notes were where `cut` cut its log, with no snapshot
/// taken there; `None` when they are as they were then (`notesBefore`).
///
/// The model changes its notes with tool calls, which name the file: when no
/// call cut off names it, the notes are as they were. Else they go back to the
/// latest state what remains tells of: the notes a run that remains started
/// from, or what the conversation last showed of them, when it showed them
/// whole; and to none when it tells of none.
fn notes_before(snapshots: &[Snapshot], cut: &LogCut) -> Option<String> {
    if !cut.notes_named {
        return None;
    }
    let (mut notes, mut at) = (String::new(), None);
    for snapshot in snapshots {
        if cut.messages.contains(&snapshot.message) && snapshot.at < cut.at && snapshot.at >= at {
            snapshot.notes.clone_into(&mut notes);
            at = snapshot.at;
        }
    }
    if cut.shown_at.is_some() && cut.shown_at > at {
        // What a message shows of the notes is trimmed.
        cut.shown.clone_into(&mut notes);
        if !notes.is_empty() {
            notes.push('\n');
        }
    }
    Some(notes)
}

/// Makes the notes file hold `notes`, removing it for none.
fn write_notes(path: &Path, notes: &str) -> Result<()> {
    match std::fs::read(path) {
        Ok(current) if current == notes.as_bytes() => Ok(()),
        _ if notes.is_empty() => remove_file(path),
        _ => replace_file(path, notes.as_bytes()),
    }
}

/// Removes a file that may not be there.
fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// Removes from a session's summaries those of the compactions a rewind cut
/// off the log at `at` (`cutSummaries`): the sections the runner appended for
/// them, the last of the file. Their headers tell the time to the second: the
/// sections of a later second than the cut go, those of an earlier one stay,
/// and of those of its second, as many go from the end as the part cut off
/// made compactions.
fn cut_summaries(path: &Path, at: Option<SystemTime>, compactions: usize) -> Result<()> {
    let Some(at) = at else {
        return Ok(());
    };
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let at = whole_second(at);
    const HEADER: &[u8] = b"## Compaction at ";
    let mut sections: Vec<(usize, SystemTime)> = Vec::new();
    let mut from = 0;
    while let Some(found) = memchr::memmem::find(&data[from..], HEADER) {
        let start = from + found;
        from = start + HEADER.len();
        // A section starts the file, or the line after the blank one that
        // ends the section before.
        if start > 0 && !data[..start].ends_with(b"\n\n") {
            continue;
        }
        let stamp = String::from_utf8_lossy(&data[from..data.len().min(from + 64)]);
        let stamp = stamp.split('\n').next().unwrap_or_default();
        let stamp = stamp.split(',').next().unwrap_or_default();
        if let Some(time) = parse_time(stamp) {
            sections.push((start, time));
        }
    }
    let count = sections.len();
    let later = sections
        .iter()
        .position(|(_, time)| *time > at)
        .unwrap_or(count);
    let mut same = later;
    while same > 0 && sections[same - 1].1 == at {
        same -= 1;
    }
    let cut = same.max(later.min(count.saturating_sub(compactions)));
    if cut == count {
        return Ok(());
    }
    truncate_file(path, sections[cut].0 as u64)
}

/// A time with its fraction of a second dropped (`time.Truncate(time.Second)`).
fn whole_second(time: SystemTime) -> SystemTime {
    let since = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(since.as_secs())
}

/// Cuts the file to `size`, removing it when that is nothing. A file that
/// short already, or gone, is left alone.
fn truncate_file(path: &Path, size: u64) -> Result<()> {
    if size == 0 {
        return remove_file(path);
    }
    let length = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if length <= size {
        return Ok(());
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_len(size)?;
    Ok(())
}

/// Drops the snapshots of the runs a rewind removed, keeping those whose
/// prompt remains (`pruneMemory`).
fn prune_memory(dir: &Path, id: &str, keep: &HashSet<String>) -> Result<()> {
    let snapshots = read_snapshots(dir, id)?;
    if snapshots.is_empty() {
        return Ok(());
    }
    let kept: Vec<&Snapshot> = snapshots
        .iter()
        .filter(|snapshot| keep.contains(&snapshot.message))
        .collect();
    if kept.len() == snapshots.len() {
        return Ok(());
    }
    let path = sibling(dir, id, MEMORY_SUFFIX);
    if kept.is_empty() {
        return remove_file(&path);
    }
    let mut out = String::new();
    for snapshot in kept {
        out.push_str(&snapshot.line);
        out.push('\n');
    }
    replace_file(&path, out.as_bytes())
}

/// Drops the workspace change records of the prompts a rewind removed,
/// keeping those whose prompt remains, and those at the end that the coming
/// run's prompt `next` took as it started (`pruneChangeRecords`).
fn prune_change_records(dir: &Path, id: &str, next: &str, keep: &HashSet<String>) -> Result<()> {
    #[derive(Deserialize)]
    struct Record {
        #[serde(default)]
        message: String,
        #[serde(default)]
        before: bool,
        #[serde(default)]
        at: Option<String>,
    }
    let path = dir.join(".changes").join(format!("{id}.jsonl"));
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let lines: Vec<(&str, Option<Record>)> = data
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let record = serde_json::from_str::<Record>(line).ok().filter(|record| {
                record
                    .at
                    .as_deref()
                    .is_none_or(|at| parse_time(at).is_some())
            });
            (line, record)
        })
        .collect();
    let mut coming = lines.len();
    for (at, (_, record)) in lines.iter().enumerate().rev() {
        let Some(record) = record
            .as_ref()
            .filter(|record| !next.is_empty() && record.message == next)
        else {
            break;
        };
        if record.before {
            coming = at;
            break;
        }
    }
    let mut out = String::new();
    let mut changed = false;
    for (at, (line, record)) in lines.iter().enumerate() {
        if at < coming
            && let Some(record) = record
            && !keep.contains(&record.message)
        {
            changed = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    match (changed, out.is_empty()) {
        (false, _) => Ok(()),
        (true, true) => remove_file(&path),
        (true, false) => replace_file(&path, out.as_bytes()),
    }
}

/// Atomically replaces the file at `path` with `data`.
fn replace_file(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let temp = write_temp(dir, |out| Ok(out.write_all(data)?))?;
    rename_temp(&temp, path)
}

/// Writes a new file in `dir` with `write`, to replace a file of `dir` with
/// ([`rename_temp`]), and returns its path.
fn write_temp(
    dir: &Path,
    write: impl FnOnce(&mut dyn std::io::Write) -> Result<()>,
) -> Result<PathBuf> {
    let (path, file) = (0..100)
        .find_map(|_| {
            let path = dir.join(format!(".rewind-{}.tmp", uuid::Uuid::new_v4().simple()));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            options.open(&path).ok().map(|file| (path, file))
        })
        .ok_or_else(|| anyhow!("cannot make a file in {}", dir.display()))?;
    let written = (|| -> Result<()> {
        let mut out = BufWriter::with_capacity(COPY_BUFFER, &file);
        write(&mut out)?;
        out.flush()?;
        drop(out);
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

/// Atomically replaces the file at `path` with the one [`write_temp`] wrote.
fn rename_temp(temp: &Path, path: &Path) -> Result<()> {
    if let Err(error) = std::fs::rename(temp, path) {
        let _ = std::fs::remove_file(temp);
        return Err(error.into());
    }
    // The rename is what matters; persisting it is best effort.
    if let Some(dir) = path.parent()
        && let Ok(dir) = File::open(dir)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests;
