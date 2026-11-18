//! A record's lines, read, and the conversation among them: a record is a
//! tree of messages (each names the one before it, `parentUuid`), and what
//! the conversation is now is one path through it. A rewound or forked
//! conversation leaves its abandoned branch in the same file.
//!
//! The path is chosen as the Agent SDK chooses it when it reads a session
//! back (`getSessionMessages`): from the newest message of the main thread
//! that nothing follows, back to the root; across a compaction, whose
//! boundary names the message before it (`logicalParentUuid`), as Claude's
//! ACP adapter reads it; and with the blocks of one answer that Claude Code
//! wrote as siblings rather than one after another (calls made at once) put
//! back beside the rest of that answer.

use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

/// One line of a record that is part of the tree: a message, an
/// attachment, a note of the system's.
#[derive(Debug)]
pub(crate) struct Record {
    /// `user`, `assistant`, `system`, `attachment`, `progress`.
    pub kind: String,
    pub subtype: Option<String>,
    pub uuid: String,
    /// The line before it in the conversation: `parentUuid`, or for a
    /// compaction's boundary the message before the compaction.
    pub parent: Option<String>,
    /// A subagent's line (`isSidechain`).
    pub sidechain: bool,
    /// A line of an agent team's (`teamName`), no part of the main thread.
    pub team: bool,
    /// Said to the model, not by anyone in the conversation (`isMeta`).
    pub meta: bool,
    /// The summary a compaction left for the model (`isCompactSummary`).
    pub compact_summary: bool,
    /// Shown in Claude Code's own transcript view only.
    pub transcript_only: bool,
    /// An answer that is an API error Claude Code wrote in the model's place.
    pub api_error: bool,
    /// What went wrong with the answer, as Claude Code names it (`error`).
    pub error: Option<String>,
    pub timestamp: String,
    /// The kind of an attachment (`attachment.type`).
    pub attachment_kind: Option<String>,
    /// The id of the API message an assistant line is a block of.
    pub message_id: Option<String>,
    pub cwd: Option<String>,
    pub message: Option<Box<RawValue>>,
    pub tool_use_result: Option<Box<RawValue>>,
    pub attachment: Option<Box<RawValue>>,
}

impl Record {
    /// A message of the user's or of the model's.
    pub fn is_message(&self) -> bool {
        matches!(self.kind.as_str(), "user" | "assistant")
    }

    /// Its message, read.
    pub fn message(&self) -> Option<Value> {
        serde_json::from_str(self.message.as_ref()?.get()).ok()
    }

    /// Its message's content: a string or blocks.
    pub fn content(&self) -> Value {
        self.message()
            .and_then(|mut message| message.get_mut("content").map(Value::take))
            .unwrap_or(Value::Null)
    }

    pub fn attachment(&self) -> Option<Value> {
        serde_json::from_str(self.attachment.as_ref()?.get()).ok()
    }

    pub fn tool_use_result(&self) -> Option<Value> {
        serde_json::from_str(self.tool_use_result.as_ref()?.get()).ok()
    }
}

/// A record file, read.
#[derive(Debug, Default)]
pub(crate) struct Transcript {
    pub records: Vec<Record>,
    /// The name the conversation was given last (`custom-title`).
    pub custom_title: Option<String>,
    /// The name Claude Code made up for it last (`ai-title`).
    pub ai_title: Option<String>,
}

/// A line's fields, each as it was written: a field of an unexpected type
/// is then simply not understood, and never makes the line unreadable.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fields<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<&'a RawValue>,
    #[serde(borrow)]
    subtype: Option<&'a RawValue>,
    #[serde(borrow)]
    uuid: Option<&'a RawValue>,
    #[serde(borrow)]
    parent_uuid: Option<&'a RawValue>,
    #[serde(borrow)]
    logical_parent_uuid: Option<&'a RawValue>,
    #[serde(borrow)]
    is_sidechain: Option<&'a RawValue>,
    #[serde(borrow)]
    team_name: Option<&'a RawValue>,
    #[serde(borrow)]
    is_meta: Option<&'a RawValue>,
    #[serde(borrow)]
    is_compact_summary: Option<&'a RawValue>,
    #[serde(borrow)]
    is_visible_in_transcript_only: Option<&'a RawValue>,
    #[serde(borrow)]
    is_api_error_message: Option<&'a RawValue>,
    #[serde(borrow)]
    error: Option<&'a RawValue>,
    #[serde(borrow)]
    timestamp: Option<&'a RawValue>,
    #[serde(borrow)]
    cwd: Option<&'a RawValue>,
    #[serde(borrow)]
    message: Option<&'a RawValue>,
    #[serde(borrow)]
    tool_use_result: Option<&'a RawValue>,
    #[serde(borrow)]
    attachment: Option<&'a RawValue>,
    #[serde(borrow)]
    custom_title: Option<&'a RawValue>,
    #[serde(borrow)]
    ai_title: Option<&'a RawValue>,
}

fn string(raw: Option<&RawValue>) -> Option<String> {
    serde_json::from_str::<String>(raw?.get()).ok()
}

fn flag(raw: Option<&RawValue>) -> bool {
    raw.and_then(|raw| serde_json::from_str::<bool>(raw.get()).ok()) == Some(true)
}

/// Just what is needed of a message to group the blocks of one answer.
#[derive(Deserialize)]
struct MessageId {
    id: Option<String>,
}

#[derive(Deserialize)]
struct AttachmentKind {
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// The file at `path`, read: every line that can be read, the others (one
/// half written, one that is not JSON) skipped.
pub(crate) fn read(path: &Path) -> Transcript {
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes),
        Err(error) => {
            log::debug!("could not read the record {}: {error}", path.display());
            Transcript::default()
        }
    }
}

pub(crate) fn parse(bytes: &[u8]) -> Transcript {
    let mut transcript = Transcript::default();
    for line in bytes.split(|byte| *byte == b'\n') {
        let line = line.trim_ascii();
        if line.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(line);
        let fields: Fields = match serde_json::from_str(&text) {
            Ok(fields) => fields,
            Err(error) => {
                log::debug!("a line of a record is not one: {error}");
                continue;
            }
        };
        let Some(kind) = string(fields.kind) else {
            continue;
        };
        match kind.as_str() {
            "user" | "assistant" | "system" | "attachment" | "progress" => {}
            "custom-title" => {
                if let Some(title) =
                    string(fields.custom_title).filter(|title| !title.trim().is_empty())
                {
                    transcript.custom_title = Some(title);
                }
                continue;
            }
            "ai-title" => {
                if let Some(title) =
                    string(fields.ai_title).filter(|title| !title.trim().is_empty())
                {
                    transcript.ai_title = Some(title);
                }
                continue;
            }
            _ => continue,
        }
        let Some(uuid) = string(fields.uuid).filter(|uuid| !uuid.is_empty()) else {
            continue;
        };
        let subtype = string(fields.subtype);
        let boundary = kind == "system" && subtype.as_deref() == Some("compact_boundary");
        // A compaction's boundary starts a new tree; what it compacted is
        // the message it names, and stays in the conversation.
        let parent = match string(fields.logical_parent_uuid) {
            Some(logical) if boundary => Some(logical),
            _ => string(fields.parent_uuid),
        }
        .filter(|parent| !parent.is_empty());
        let message_id = (kind == "assistant")
            .then(|| {
                serde_json::from_str::<MessageId>(fields.message?.get())
                    .ok()?
                    .id
            })
            .flatten();
        let attachment_kind = fields
            .attachment
            .and_then(|raw| serde_json::from_str::<AttachmentKind>(raw.get()).ok())
            .and_then(|attachment| attachment.kind);
        transcript.records.push(Record {
            kind,
            subtype,
            uuid,
            parent,
            sidechain: flag(fields.is_sidechain),
            team: fields.team_name.is_some_and(|raw| raw.get() != "null"),
            meta: flag(fields.is_meta),
            compact_summary: flag(fields.is_compact_summary),
            transcript_only: flag(fields.is_visible_in_transcript_only),
            api_error: flag(fields.is_api_error_message),
            error: string(fields.error),
            timestamp: string(fields.timestamp).unwrap_or_default(),
            attachment_kind,
            message_id,
            cwd: string(fields.cwd),
            message: fields.message.map(RawValue::to_owned),
            tool_use_result: fields.tool_use_result.map(RawValue::to_owned),
            attachment: fields.attachment.map(RawValue::to_owned),
        });
    }
    follow_compactions(&mut transcript.records);
    transcript
}

/// A compaction's boundary that names no line of the file is hung after the
/// message written before it. Not every boundary names one that was written:
/// a compaction in the middle of a turn names a message still being made,
/// and some name none at all. What they compacted is still what stands
/// before them in the file.
fn follow_compactions(records: &mut [Record]) {
    let known: HashSet<String> = records.iter().map(|record| record.uuid.clone()).collect();
    let mut before: Option<String> = None;
    for record in records.iter_mut() {
        let boundary =
            record.kind == "system" && record.subtype.as_deref() == Some("compact_boundary");
        if boundary
            && record
                .parent
                .as_ref()
                .is_none_or(|parent| !known.contains(parent))
        {
            record.parent = before.clone();
        }
        if record.is_message() && main_thread(record) {
            before = Some(record.uuid.clone());
        }
    }
}

/// The lines by their ids: a line written again under the same id stands
/// for it, as the SDK takes it.
struct Tree<'a> {
    records: &'a [Record],
    by_uuid: HashMap<&'a str, usize>,
}

impl<'a> Tree<'a> {
    fn new(records: &'a [Record]) -> Self {
        let mut by_uuid = HashMap::with_capacity(records.len());
        for (index, record) in records.iter().enumerate() {
            by_uuid.insert(record.uuid.as_str(), index);
        }
        Tree { records, by_uuid }
    }

    /// The lines that stand, in the file's order.
    fn standing(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.records.len())
            .filter(|&index| self.by_uuid[self.records[index].uuid.as_str()] == index)
    }

    fn parent(&self, index: usize) -> Option<usize> {
        self.by_uuid
            .get(self.records[index].parent.as_deref()?)
            .copied()
    }

    /// From `leaf` back to the root, then turned round: the root first.
    fn path_to(&self, leaf: usize) -> Vec<usize> {
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        let mut at = Some(leaf);
        while let Some(index) = at {
            if !seen.insert(index) {
                break;
            }
            path.push(index);
            at = self.parent(index);
        }
        path.reverse();
        path
    }
}

/// Whether a line is of the main thread: not a subagent's, not a team's,
/// not a note of progress.
fn main_thread(record: &Record) -> bool {
    !record.sidechain
        && !record.team
        && record.kind != "progress"
        && record.attachment_kind.as_deref() != Some("fork_briefing")
}

/// The main conversation of a record, as the lines to show, in order.
pub(crate) fn conversation(records: &[Record]) -> Vec<usize> {
    let tree = Tree::new(records);
    let Some(leaf) = leaf(&tree) else {
        return Vec::new();
    };
    let path = tree.path_to(leaf);
    let mut path = with_siblings(&tree, path);
    path.extend(trailing(&tree, &path, leaf));
    path
}

/// The last message of the conversation: of the main thread's lines that
/// nothing follows, the newest in the file, walked back to a message.
fn leaf(tree: &Tree) -> Option<usize> {
    let records = tree.records;
    let mut followed: HashSet<&str> = HashSet::new();
    for index in tree.standing() {
        let record = &records[index];
        if main_thread(record)
            && let Some(parent) = &record.parent
        {
            followed.insert(parent.as_str());
        }
    }
    let mut ends: Vec<usize> = tree
        .standing()
        .filter(|&index| {
            let record = &records[index];
            main_thread(record) && !followed.contains(record.uuid.as_str())
        })
        .collect();
    ends.sort_by(|a, b| b.cmp(a));
    let mut dead = HashSet::new();
    for end in ends {
        // A walk that finds no message leaves its lines dead for the next.
        let mut at = Some(end);
        while let Some(index) = at {
            if !dead.insert(index) {
                break;
            }
            if records[index].is_message() {
                return Some(index);
            }
            at = tree.parent(index);
        }
    }
    // No line of the main thread leads to a message: the newest message
    // that ends a branch, of the main thread if one does.
    let mut parents: HashSet<&str> = HashSet::new();
    for index in tree.standing() {
        if let Some(parent) = &records[index].parent {
            parents.insert(parent.as_str());
        }
    }
    let mut found = Vec::new();
    for end in tree
        .standing()
        .filter(|&index| !parents.contains(records[index].uuid.as_str()))
    {
        let mut seen = HashSet::new();
        let mut at = Some(end);
        while let Some(index) = at {
            if !seen.insert(index) {
                break;
            }
            if records[index].is_message() {
                found.push(index);
                break;
            }
            at = tree.parent(index);
        }
    }
    let plain = found
        .iter()
        .copied()
        .filter(|&index| {
            let record = &records[index];
            !record.sidechain && !record.team && !record.meta
        })
        .max();
    plain.or_else(|| found.into_iter().max())
}

/// `path` with the blocks of each of its answers that are not on it: Claude
/// Code writes the blocks of one API message as lines of their own, and
/// calls made at once may hang off the same line instead of following one
/// another. They are put after the answer's last line on the path.
fn with_siblings(tree: &Tree, path: Vec<usize>) -> Vec<usize> {
    let records = tree.records;
    let on_path: HashSet<usize> = path.iter().copied().collect();
    let mut last_of: HashMap<&str, usize> = HashMap::new();
    for (place, &index) in path.iter().enumerate() {
        if let Some(id) = &records[index].message_id {
            last_of.insert(id.as_str(), place);
        }
    }
    if last_of.is_empty() {
        return path;
    }
    let mut extra: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in tree.standing() {
        let record = &records[index];
        if on_path.contains(&index) || record.kind != "assistant" {
            continue;
        }
        let Some(&place) = record.message_id.as_deref().and_then(|id| last_of.get(id)) else {
            continue;
        };
        if record.sidechain != records[path[place]].sidechain {
            continue;
        }
        extra.entry(place).or_default().push(index);
    }
    if extra.is_empty() {
        return path;
    }
    let mut out = Vec::with_capacity(path.len());
    for (place, index) in path.into_iter().enumerate() {
        out.push(index);
        if let Some(mut more) = extra.remove(&place) {
            more.sort_by(|a, b| {
                records[*a]
                    .timestamp
                    .cmp(&records[*b].timestamp)
                    .then(a.cmp(b))
            });
            out.extend(more);
        }
    }
    out
}

/// Messages the user sent after the last one that were taken in as queued
/// prompts (`queued_command` attachments) with no answer after them yet:
/// they follow the last message, not on the path to it.
fn trailing(tree: &Tree, path: &[usize], leaf: usize) -> Vec<usize> {
    let records = tree.records;
    let on_path: HashSet<usize> = path.iter().copied().collect();
    // What leads to a message of another branch belongs to that branch.
    let mut elsewhere: HashSet<usize> = HashSet::new();
    for index in tree.standing() {
        let record = &records[index];
        if !record.is_message() || !main_thread(record) || on_path.contains(&index) {
            continue;
        }
        let mut at = tree.parent(index);
        while let Some(above) = at {
            if on_path.contains(&above) || !elsewhere.insert(above) {
                break;
            }
            at = tree.parent(above);
        }
    }
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    for index in tree.standing() {
        let record = &records[index];
        if record.is_message() {
            continue;
        }
        if let Some(parent) = &record.parent {
            children.entry(parent.as_str()).or_default().push(index);
        }
    }
    let mut found = Vec::new();
    let mut seen: HashSet<usize> = on_path.clone();
    let mut stack = vec![leaf];
    while let Some(index) = stack.pop() {
        if index != leaf {
            if !seen.insert(index) {
                continue;
            }
            if records[index].attachment_kind.as_deref() == Some("queued_command")
                && !elsewhere.contains(&index)
            {
                found.push(index);
            }
        }
        let mut next = children
            .get(records[index].uuid.as_str())
            .cloned()
            .unwrap_or_default();
        next.sort_by(|a, b| records[*b].timestamp.cmp(&records[*a].timestamp));
        stack.extend(next);
    }
    found
}

/// The path of a subagent's own record: from its last message back to its
/// first, as the SDK reads a subagent's transcript.
pub(crate) fn agent_path(records: &[Record]) -> Vec<usize> {
    let tree = Tree::new(records);
    let last = (0..records.len()).rev().find(|&index| {
        records[index].is_message() && tree.by_uuid[records[index].uuid.as_str()] == index
    });
    last.map(|leaf| tree.path_to(leaf)).unwrap_or_default()
}

/// The subagents' lines that a record of an older Claude Code keeps among
/// its own (`isSidechain`): each subagent's path, by the prompt it was
/// given, in the order they were started.
pub(crate) fn sidechains(records: &[Record]) -> HashMap<String, Vec<Vec<usize>>> {
    let tree = Tree::new(records);
    let mut roots: HashMap<usize, usize> = HashMap::new();
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in tree.standing() {
        if !records[index].sidechain {
            continue;
        }
        let mut walked = Vec::new();
        let mut seen = HashSet::new();
        let mut at = index;
        let root = loop {
            if let Some(&root) = roots.get(&at) {
                break root;
            }
            walked.push(at);
            seen.insert(at);
            match tree.parent(at) {
                Some(above) if records[above].sidechain && !seen.contains(&above) => at = above,
                _ => break at,
            }
        };
        for walked in walked {
            roots.insert(walked, root);
        }
        groups.entry(root).or_default().push(index);
    }
    let mut chains: Vec<(usize, String, Vec<usize>)> = Vec::new();
    for (root, members) in groups {
        let Some(&last) = members
            .iter()
            .filter(|&&index| records[index].is_message())
            .max()
        else {
            continue;
        };
        let path = tree.path_to(last);
        let Some(prompt) = path
            .iter()
            .find(|&&index| records[index].kind == "user")
            .map(|&index| crate::records::user::text_of(&records[index].content()))
        else {
            continue;
        };
        chains.push((root, prompt.trim().to_owned(), path));
    }
    chains.sort_by_key(|(root, ..)| *root);
    let mut by_prompt: HashMap<String, Vec<Vec<usize>>> = HashMap::new();
    for (_, prompt, path) in chains {
        by_prompt.entry(prompt).or_default().push(path);
    }
    by_prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(value: Value) -> String {
        value.to_string()
    }

    fn file(lines: &[Value]) -> Transcript {
        let text = lines
            .iter()
            .map(|value| line(value.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        parse(text.as_bytes())
    }

    fn uuids(transcript: &Transcript, path: &[usize]) -> Vec<String> {
        path.iter()
            .map(|&index| transcript.records[index].uuid.clone())
            .collect()
    }

    fn user(uuid: &str, parent: Option<&str>) -> Value {
        json!({"type": "user", "uuid": uuid, "parentUuid": parent,
            "message": {"role": "user", "content": uuid}})
    }

    fn assistant(uuid: &str, parent: &str, id: &str) -> Value {
        json!({"type": "assistant", "uuid": uuid, "parentUuid": parent,
            "message": {"id": id, "role": "assistant", "content": []}})
    }

    #[test]
    fn the_conversation_is_the_path_to_the_newest_message_and_not_an_abandoned_branch() {
        let transcript = file(&[
            user("u1", None),
            assistant("a1", "u1", "m1"),
            user("u2", Some("a1")),
            assistant("a2", "u2", "m2"),
            // Rewound to before u2: a new prompt from a1.
            user("u3", Some("a1")),
            assistant("a3", "u3", "m3"),
            json!({"type": "system", "subtype": "turn_duration", "uuid": "s1", "parentUuid": "a3"}),
        ]);
        let path = conversation(&transcript.records);
        assert_eq!(uuids(&transcript, &path), ["u1", "a1", "u3", "a3"]);
    }

    #[test]
    fn a_compaction_keeps_what_came_before_it() {
        let transcript = file(&[
            user("u1", None),
            assistant("a1", "u1", "m1"),
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "b",
                "parentUuid": null, "logicalParentUuid": "a1"}),
            json!({"type": "user", "uuid": "sum", "parentUuid": "b", "isCompactSummary": true,
                "message": {"role": "user", "content": "summary"}}),
            user("u2", Some("sum")),
        ]);
        let path = conversation(&transcript.records);
        assert_eq!(uuids(&transcript, &path), ["u1", "a1", "b", "sum", "u2"]);
    }

    #[test]
    fn a_compaction_that_names_no_line_keeps_what_was_written_before_it() {
        // One boundary names a message that was never written (a compaction
        // in the middle of a turn), the next names none at all.
        let transcript = file(&[
            user("u1", None),
            assistant("a1", "u1", "m1"),
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "b1",
                "parentUuid": null, "logicalParentUuid": "never-written"}),
            user("u2", Some("b1")),
            assistant("a2", "u2", "m2"),
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "b2",
                "parentUuid": null}),
            user("u3", Some("b2")),
            assistant("a3", "u3", "m3"),
        ]);
        let path = conversation(&transcript.records);
        assert_eq!(
            uuids(&transcript, &path),
            ["u1", "a1", "b1", "u2", "a2", "b2", "u3", "a3"]
        );
        // A record that begins with a boundary has nothing before it.
        let transcript = file(&[
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "b",
                "parentUuid": null, "logicalParentUuid": "elsewhere"}),
            user("u1", Some("b")),
        ]);
        let path = conversation(&transcript.records);
        assert_eq!(uuids(&transcript, &path), ["b", "u1"]);
    }

    #[test]
    fn blocks_of_one_answer_written_side_by_side_are_put_back_together() {
        let transcript = file(&[
            user("u1", None),
            assistant("t1", "u1", "m1"),
            // A second call of the same answer hangs off the first's line.
            assistant("t2", "t1", "m1"),
            json!({"type": "user", "uuid": "r1", "parentUuid": "t1",
                "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "x"}]}}),
            assistant("a2", "r1", "m2"),
        ]);
        let path = conversation(&transcript.records);
        assert_eq!(uuids(&transcript, &path), ["u1", "t1", "t2", "r1", "a2"]);
    }

    #[test]
    fn a_line_that_cannot_be_read_is_skipped_and_the_rest_is_read() {
        let text = format!(
            "{}\nnot json at all\n\n{{\"type\":\"user\",\"uuid\":\"u2\",\"parentUuid\":\"u1\",\"message\":{{\"content\":\"x\"}}}}\n{{\"type\":\"assistant\",\"uuid\":\"a",
            line(user("u1", None))
        );
        let transcript = parse(text.as_bytes());
        assert_eq!(transcript.records.len(), 2);
        let path = conversation(&transcript.records);
        assert_eq!(uuids(&transcript, &path), ["u1", "u2"]);
        assert!(conversation(&parse(b"").records).is_empty());
        assert!(conversation(&parse(b"\x00\xff garbage").records).is_empty());
    }

    #[test]
    fn a_subagent_s_lines_are_no_part_of_the_main_thread() {
        let transcript = file(&[
            user("u1", None),
            assistant("a1", "u1", "m1"),
            json!({"type": "user", "uuid": "s1", "parentUuid": null, "isSidechain": true,
                "message": {"role": "user", "content": "Look around"}}),
            json!({"type": "assistant", "uuid": "s2", "parentUuid": "s1", "isSidechain": true,
                "message": {"id": "sm", "role": "assistant", "content": []}}),
        ]);
        assert_eq!(
            uuids(&transcript, &conversation(&transcript.records)),
            ["u1", "a1"]
        );
        let chains = sidechains(&transcript.records);
        let path = &chains["Look around"][0];
        assert_eq!(uuids(&transcript, path), ["s1", "s2"]);
    }
}
