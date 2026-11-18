//! A conversation's record: found, listed, and replayed.
//!
//! The list reads each record's head and tail only, as the Agent SDK does
//! (`listSessions`): a folder holds hundreds of records of megabytes each.
//! The replay reads all of one, keeps the conversation it is now (see
//! `chain`), and says it as the updates the chat would have had live, in
//! their final state: history, with no turn open and nothing left running.

use crate::records::{
    chain::{self, Record, Transcript},
    tools::{self, Tasks},
    user::{self, Spoken},
};
use kou_thread::{Block, Level, PlanStep, ToolPatch, ToolStatus, Update};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

/// A conversation Claude Code keeps, as a list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastSession {
    /// The id to go on with it by ([`crate::Options::resume`]).
    pub id: String,
    /// What it is called, in the order Claude Code's own list takes (the
    /// SDK's `listSessions`): the name it was given, else the one Claude
    /// Code made up for it, else its last prompt, else its summary, else
    /// its first prompt, on one line. A record with none of them is no
    /// conversation and is not listed.
    pub title: String,
    /// When it last changed.
    pub updated: Option<SystemTime>,
    /// The folder it works in.
    pub cwd: PathBuf,
}

/// Claude Code's own home when no other is named: `CLAUDE_CONFIG_DIR` when
/// that is set, else `~/.claude`.
pub fn default_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|home| !home.is_empty()) {
        return Some(PathBuf::from(home));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude"))
}

/// The conversations of the folder `cwd` kept under Claude Code's `home`,
/// the latest first.
pub fn sessions(home: &Path, cwd: &Path) -> Vec<PastSession> {
    let wanted = forms_of(cwd);
    let mut found: Vec<PastSession> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (folder, exact) in project_folders(home, &wanted) {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if uuid::Uuid::parse_str(id).is_err() || seen.contains(id) {
                continue;
            }
            let Some(summary) = summary(&path, id, &folder) else {
                continue;
            };
            // A folder's name stands for more than one path: a record that
            // says it ran in another folder that is there is that one's.
            match &summary.ran_in {
                Some(ran) if !same_folder(ran, &wanted) && (!exact || ran.is_dir()) => continue,
                None if !exact => continue,
                _ => {}
            }
            seen.insert(id.to_owned());
            found.push(PastSession {
                id: id.to_owned(),
                title: summary.title,
                updated: summary.updated,
                cwd: summary.ran_in.unwrap_or_else(|| cwd.to_path_buf()),
            });
        }
    }
    found.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    found
}

/// The record of the conversation `session` of `cwd` under `home`, when
/// there is one.
pub fn record(home: &Path, cwd: &Path, session: &str) -> Option<PathBuf> {
    if session.is_empty()
        || session.starts_with('.')
        || session.contains(['/', '\\'])
        || session.contains('\0')
    {
        return None;
    }
    project_folders(home, &forms_of(cwd))
        .into_iter()
        .map(|(folder, _)| folder.join(format!("{session}.jsonl")))
        .find(|path| path.is_file())
}

/// A conversation's record as the updates it would have been live, in
/// order: prompts (each named with [`Update::PromptId`]), answers, thinking,
/// calls and their results, plans, what subagents did under the call that
/// started each. No turn is opened or ended by it: it is history.
///
/// Every call is in its final state: one the record has no result for is
/// [`ToolStatus::Cancelled`], and what was left in the background has ended
/// (its process went with the session that ran it).
pub fn replay(record: &Path) -> Vec<Update> {
    replayed(record, false)
}

/// [`replay`] of a conversation that is still going on, in a Claude Code
/// that lives: what the record has no end for is left as it stands, at
/// work (a call with no result yet, what was left running in the
/// background), for the session to end as it hears of it.
pub fn replay_live(record: &Path) -> Vec<Update> {
    replayed(record, true)
}

fn replayed(record: &Path, live: bool) -> Vec<Update> {
    let transcript = chain::read(record);
    let order = chain::conversation(&transcript.records);
    let results = results(&transcript.records, &order);
    let cwd = order
        .iter()
        .find_map(|&index| transcript.records[index].cwd.as_deref())
        .map(PathBuf::from);
    let mut player = Player::new(&transcript, &results, cwd, record);
    player.play(&transcript.records, &order, &results, None);
    let title = transcript
        .custom_title
        .clone()
        .or_else(|| transcript.ai_title.clone());
    player.finish(title, live)
}

/// What a call's result was: the `tool_result` block's content and whether
/// it is an error, and what Claude Code kept beside it.
struct Outcome {
    content: Value,
    structured: Option<Value>,
    failed: bool,
}

/// The results of a record's calls, by the calls' ids. One on the
/// conversation's path is taken before one off it.
fn results(records: &[Record], order: &[usize]) -> HashMap<String, Outcome> {
    let on_path: HashSet<usize> = order.iter().copied().collect();
    let mut found = HashMap::new();
    for pass in [false, true] {
        for (index, record) in records.iter().enumerate() {
            if on_path.contains(&index) != pass
                || record.kind != "user"
                || !record
                    .message
                    .as_ref()
                    .is_some_and(|message| message.get().contains("\"tool_result\""))
            {
                continue;
            }
            let Value::Array(blocks) = record.content() else {
                continue;
            };
            let answers: Vec<Value> = blocks
                .into_iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
                .collect();
            // What is kept beside the line is of its only result.
            let structured = (answers.len() == 1)
                .then(|| record.tool_use_result())
                .flatten();
            for mut answer in answers {
                let Some(id) = answer.get("tool_use_id").and_then(Value::as_str) else {
                    continue;
                };
                let id = id.to_owned();
                found.insert(
                    id,
                    Outcome {
                        failed: answer.get("is_error") == Some(&Value::Bool(true)),
                        content: answer
                            .get_mut("content")
                            .map(Value::take)
                            .unwrap_or_default(),
                        structured: structured.clone(),
                    },
                );
            }
        }
    }
    found
}

/// What was shown last, for what follows to go on in it or to start anew,
/// as the thread joins what comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Open {
    Nothing,
    User,
    Assistant,
    Thought,
}

/// A call the replay has shown, or kept out of sight (one of the plan's).
struct Known {
    id: String,
    name: String,
    input: Value,
    /// Its [`Update::Tool`] among what was said; `None` when not shown.
    at: Option<usize>,
    resulted: bool,
    background: bool,
    ended: bool,
}

const COMPACTED: &str = "Context compacted";

/// The replay of one record, its subagents' records with it.
struct Player<'a> {
    out: Vec<Update>,
    open: Open,
    calls: Vec<Known>,
    by_id: HashMap<String, usize>,
    tasks: Tasks,
    /// A plan has been shown: later ones change it.
    planned: bool,
    /// The folder the conversation ran in, for the paths in titles.
    cwd: Option<PathBuf>,
    /// Where the subagents' own records are kept, and those found there by
    /// the call that started each, once looked for.
    subagents: PathBuf,
    agents: Option<HashMap<String, PathBuf>>,
    main: &'a Transcript,
    main_results: &'a HashMap<String, Outcome>,
    /// Subagents' lines among the conversation's own, once looked for.
    sidechains: Option<HashMap<String, Vec<Vec<usize>>>>,
    depth: usize,
    /// The call whose subagent spoke last, while it has made no call since.
    said_last: Option<String>,
}

impl<'a> Player<'a> {
    fn new(
        main: &'a Transcript,
        main_results: &'a HashMap<String, Outcome>,
        cwd: Option<PathBuf>,
        record: &Path,
    ) -> Self {
        Player {
            out: Vec::new(),
            open: Open::Nothing,
            calls: Vec::new(),
            by_id: HashMap::new(),
            tasks: Tasks::default(),
            planned: false,
            cwd,
            subagents: record.with_extension("").join("subagents"),
            agents: None,
            main,
            main_results,
            sidechains: None,
            depth: 0,
            said_last: None,
        }
    }

    fn play(
        &mut self,
        records: &[Record],
        order: &[usize],
        results: &HashMap<String, Outcome>,
        agent: Option<&str>,
    ) {
        for &index in order {
            self.line(&records[index], results, agent);
        }
    }

    /// One line of the conversation; `agent` is the call that started the
    /// subagent it is of, when it is a subagent's.
    fn line(&mut self, record: &Record, results: &HashMap<String, Outcome>, agent: Option<&str>) {
        match record.kind.as_str() {
            "user" => self.user_line(record, agent),
            "attachment" => self.attachment_line(record, agent),
            "assistant" => self.assistant_line(record, results, agent),
            "system"
                if agent.is_none() && record.subtype.as_deref() == Some("compact_boundary") =>
            {
                self.notice(Level::Info, COMPACTED.to_owned());
            }
            _ => {}
        }
    }

    fn user_line(&mut self, record: &Record, agent: Option<&str>) {
        let content = record.content();
        for (call, _) in user::notifications(&content) {
            self.work_ended(&call);
        }
        // What a subagent was told is its call's to show.
        if agent.is_some() {
            return;
        }
        if record.compact_summary {
            // The summary is for the model; that there was a compaction is
            // said once.
            let said = matches!(self.out.last(), Some(Update::Notice(Level::Info, text)) if text == COMPACTED);
            if !said {
                self.notice(Level::Info, COMPACTED.to_owned());
            }
            return;
        }
        if record.meta || record.transcript_only {
            return;
        }
        self.spoken(user::spoken(&content), &record.uuid);
    }

    /// A message the user sent while a turn ran, taken into it.
    fn attachment_line(&mut self, record: &Record, agent: Option<&str>) {
        if record.attachment_kind.as_deref() != Some("queued_command") {
            return;
        }
        let Some(attachment) = record.attachment() else {
            return;
        };
        let prompt = attachment.get("prompt").cloned().unwrap_or_default();
        for (call, _) in user::notifications(&prompt) {
            self.work_ended(&call);
        }
        if agent.is_some() || attachment.get("isMeta") == Some(&Value::Bool(true)) {
            return;
        }
        self.spoken(user::spoken(&prompt), &record.uuid);
    }

    fn spoken(&mut self, spoken: Spoken, uuid: &str) {
        match spoken {
            Spoken::Said(blocks) => self.prompt(blocks, uuid),
            Spoken::Interrupted => self.notice(Level::Info, "Stopped by the user".to_owned()),
            Spoken::Nothing => {}
        }
    }

    fn assistant_line(
        &mut self,
        record: &Record,
        results: &HashMap<String, Outcome>,
        agent: Option<&str>,
    ) {
        let content = record.content();
        if record.api_error || record.error.is_some() {
            if agent.is_none() {
                let said = user::text_of(&content);
                let said = said.trim();
                self.notice(
                    Level::Error,
                    if said.is_empty() {
                        "The model's answer failed".to_owned()
                    } else {
                        said.to_owned()
                    },
                );
            }
            return;
        }
        let blocks = match content {
            Value::Array(blocks) => blocks,
            Value::String(text) => vec![serde_json::json!({"type": "text", "text": text})],
            _ => return,
        };
        for block in blocks {
            let kind = block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match kind {
                "text" => {
                    let text = block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if text.trim().is_empty() {
                        continue;
                    }
                    match agent {
                        Some(launch) => self.agent_said(launch, text),
                        None => self.chunk(false, text),
                    }
                }
                "thinking" => {
                    let thought = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    // A subagent's reasoning is not shown, as `kou-acp` has it.
                    if agent.is_none() && !thought.trim().is_empty() {
                        self.chunk(true, thought);
                    }
                }
                "tool_use" | "server_tool_use" | "mcp_tool_use" => {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let input = block.get("input").cloned().unwrap_or_default();
                    self.tool(id, name, input, results, agent);
                }
                // The result of a tool the API ran itself, in the answer.
                kind if kind.ends_with("_tool_result") => {
                    let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let failed = block.get("is_error") == Some(&Value::Bool(true));
                    let outcome = Outcome {
                        content: block.get("content").cloned().unwrap_or_default(),
                        structured: None,
                        failed,
                    };
                    self.settle(id, &outcome);
                }
                _ => {}
            }
        }
    }

    fn tool(
        &mut self,
        id: &str,
        name: &str,
        input: Value,
        results: &HashMap<String, Outcome>,
        agent: Option<&str>,
    ) {
        if id.is_empty() || self.by_id.contains_key(id) {
            return;
        }
        let outcome = results.get(id);
        if tools::is_plan(name) {
            // The task list is the session's, whoever changes it; a
            // subagent's to-dos are its own, shown as its calls.
            let steps = if name == "TodoWrite" {
                agent.is_none().then(|| tools::plan(name, &input)).flatten()
            } else {
                outcome.and_then(|outcome| {
                    self.tasks.apply(
                        name,
                        &input,
                        &outcome.content,
                        outcome.structured.as_ref(),
                        outcome.failed,
                    )
                })
            };
            if agent.is_none() {
                if let Some(steps) = steps {
                    self.plan(steps);
                }
                self.remember(id, name, input, None, false);
                return;
            }
        }
        let mut call = tools::call(id, name, &input, agent);
        if let Some(cwd) = &self.cwd {
            call = tools::within(cwd, call);
        }
        match agent {
            None => self.open = Open::Nothing,
            Some(launch) => {
                if self.said_last.as_deref() == Some(launch) {
                    self.said_last = None;
                }
            }
        }
        let background = call.work.background;
        self.out.push(Update::Tool(call));
        let at = self.out.len() - 1;
        let starts_agent = tools::is_agent(name);
        let prompt = starts_agent
            .then(|| {
                input
                    .get("prompt")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten();
        self.remember(id, name, input, Some(at), background);
        if starts_agent {
            self.subagent(id, prompt.as_deref());
        }
        if let Some(outcome) = outcome {
            self.settle(id, outcome);
        }
    }

    fn remember(
        &mut self,
        id: &str,
        name: &str,
        input: Value,
        at: Option<usize>,
        background: bool,
    ) {
        self.by_id.insert(id.to_owned(), self.calls.len());
        self.calls.push(Known {
            id: id.to_owned(),
            name: name.to_owned(),
            input,
            at,
            resulted: false,
            background,
            ended: false,
        });
    }

    /// The result of the call `id`, when it is shown and has none yet.
    fn settle(&mut self, id: &str, outcome: &Outcome) {
        let Some(&place) = self.by_id.get(id) else {
            return;
        };
        let known = &self.calls[place];
        if known.at.is_none() || known.resulted {
            return;
        }
        let mut patch = tools::result(
            id,
            &known.name,
            &known.input,
            &outcome.content,
            outcome.structured.as_ref(),
            outcome.failed,
        );
        if let Some(cwd) = &self.cwd
            && let Some(title) = patch.title.take()
        {
            patch.title = Some(tools::relative(cwd, &title));
        }
        let known = &mut self.calls[place];
        known.resulted = true;
        if patch.background == Some(true) {
            known.background = true;
        }
        self.out.push(Update::ToolPatch(patch));
    }

    /// What the call `launch` left to run in the background has ended.
    fn work_ended(&mut self, launch: &str) {
        let Some(&place) = self.by_id.get(launch) else {
            return;
        };
        let known = &mut self.calls[place];
        if known.at.is_none() || !known.background || known.ended {
            return;
        }
        known.ended = true;
        self.out.push(Update::ToolPatch(ToolPatch {
            id: launch.to_owned(),
            ended: Some(true),
            ..ToolPatch::default()
        }));
    }

    /// What the subagent that call `launch` started did, under that call:
    /// from its own record beside the conversation's, or from the
    /// conversation's own lines in an older record, found by its prompt.
    fn subagent(&mut self, launch: &str, prompt: Option<&str>) {
        if self.depth >= 8 {
            return;
        }
        if let Some(path) = self.agent_record(launch) {
            let transcript = chain::read(&path);
            let mut order = chain::agent_path(&transcript.records);
            without_fork_context(&transcript.records, &mut order);
            let results = results(&transcript.records, &order);
            self.depth += 1;
            self.play(&transcript.records, &order, &results, Some(launch));
            self.depth -= 1;
            return;
        }
        let Some(prompt) = prompt.map(str::trim) else {
            return;
        };
        let main = self.main;
        let main_results = self.main_results;
        let sidechains = self
            .sidechains
            .get_or_insert_with(|| chain::sidechains(&main.records));
        let Some(paths) = sidechains.get_mut(prompt) else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        let path = paths.remove(0);
        self.depth += 1;
        self.play(&main.records, &path, main_results, Some(launch));
        self.depth -= 1;
    }

    /// The record of the subagent the call `launch` started, kept as
    /// `<session>/subagents/agent-<id>.jsonl` beside the conversation's,
    /// with a `.meta.json` that names the call (`toolUseId`).
    fn agent_record(&mut self, launch: &str) -> Option<PathBuf> {
        let folder = &self.subagents;
        self.agents
            .get_or_insert_with(|| agent_records(folder))
            .get(launch)
            .cloned()
    }

    fn prompt(&mut self, mut blocks: Vec<Block>, uuid: &str) {
        // A message after another with nothing between them is one entry
        // of the thread's: a paragraph of its own in it.
        if self.open == Open::User {
            match blocks.first_mut() {
                Some(Block::Text(text)) => text.insert_str(0, "\n\n"),
                _ => blocks.insert(0, Block::Text("\n\n".to_owned())),
            }
        }
        self.open = Open::User;
        for block in blocks {
            self.out.push(Update::UserChunk(block));
        }
        self.out.push(Update::PromptId(uuid.to_owned()));
    }

    fn chunk(&mut self, thought: bool, text: &str) {
        let kind = if thought {
            Open::Thought
        } else {
            Open::Assistant
        };
        let mut text = text.to_owned();
        // Another block of the same kind is another paragraph.
        if self.open == kind {
            text.insert_str(0, "\n\n");
        }
        self.open = kind;
        self.out.push(if thought {
            Update::ThoughtChunk(text)
        } else {
            Update::AssistantChunk(text)
        });
    }

    fn agent_said(&mut self, launch: &str, text: &str) {
        let mut text = text.to_owned();
        if self.said_last.as_deref() == Some(launch) {
            text.insert_str(0, "\n\n");
        }
        self.said_last = Some(launch.to_owned());
        self.out.push(Update::AgentSaid {
            call: launch.to_owned(),
            text,
        });
    }

    fn plan(&mut self, steps: Vec<PlanStep>) {
        if !self.planned {
            // An empty list where there was none says nothing.
            if steps.is_empty() {
                return;
            }
            self.planned = true;
            self.open = Open::Nothing;
        }
        self.out.push(Update::Plan(steps));
    }

    fn notice(&mut self, level: Level, text: String) {
        self.open = Open::Nothing;
        self.out.push(Update::Notice(level, text));
    }

    /// History is final: a call with no result never got one, and what was
    /// left running went with the process that ran it. Not so (`live`) when
    /// that process still runs: then what is open stays open.
    fn finish(mut self, title: Option<String>, live: bool) -> Vec<Update> {
        let mut ended = Vec::new();
        for known in self.calls.iter().filter(|_| !live) {
            let Some(at) = known.at else {
                continue;
            };
            if !known.resulted
                && let Update::Tool(call) = &mut self.out[at]
            {
                call.status = ToolStatus::Cancelled;
            }
            if known.background && !known.ended {
                ended.push(known.id.clone());
            }
        }
        for id in ended {
            self.out.push(Update::ToolPatch(ToolPatch {
                id,
                ended: Some(true),
                ..ToolPatch::default()
            }));
        }
        if let Some(title) = title
            .map(|title| user::one_line(&title))
            .filter(|title| !title.is_empty())
        {
            self.out.push(Update::Title(title));
        }
        self.out
    }
}

/// The subagents' records under `folder`, by the call that started each.
fn agent_records(folder: &Path) -> HashMap<String, PathBuf> {
    let mut found = HashMap::new();
    let mut folders = vec![(folder.to_path_buf(), 0)];
    while let Some((folder, depth)) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                if depth < 4 {
                    folders.push((path, depth + 1));
                }
                continue;
            }
            let name = entry.file_name();
            let Some(agent) = name
                .to_str()
                .and_then(|name| name.strip_prefix("agent-"))
                .and_then(|name| name.strip_suffix(".meta.json"))
            else {
                continue;
            };
            let record = folder.join(format!("agent-{agent}.jsonl"));
            if !record.is_file() {
                continue;
            }
            let call = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|meta| Some(meta.get("toolUseId")?.as_str()?.to_owned()));
            if let Some(call) = call {
                found.insert(call, record);
            }
        }
    }
    found
}

/// A forked subagent's record starts with a copy of what its parent had
/// said, up to the message that gives it its task: none of that is its own.
fn without_fork_context(records: &[Record], order: &mut Vec<usize>) {
    let directive = order.iter().position(|&index| {
        let record = &records[index];
        record.kind == "user"
            && record.content().as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.starts_with("<fork-boilerplate>"))
                })
            })
    });
    if let Some(directive) = directive {
        order.drain(..=directive);
    }
}

/// The forms a folder's path takes: as given, and as the system resolves it
/// (Claude Code records the folder it finds itself in).
fn forms_of(cwd: &Path) -> Vec<PathBuf> {
    let given = PathBuf::from(cwd.to_string_lossy().trim_end_matches('/'));
    let mut forms = vec![if given.as_os_str().is_empty() {
        cwd.to_path_buf()
    } else {
        given
    }];
    if let Ok(real) = cwd.canonicalize()
        && !forms.contains(&real)
    {
        forms.push(real);
    }
    forms
}

fn same_folder(ran: &Path, wanted: &[PathBuf]) -> bool {
    let ran_trimmed = PathBuf::from(ran.to_string_lossy().trim_end_matches('/'));
    wanted.contains(&ran_trimmed) || ran.canonicalize().is_ok_and(|real| wanted.contains(&real))
}

/// The folder Claude Code keeps a folder's records in, by its path: every
/// character that is not an ASCII letter or digit a dash (one for each
/// UTF-16 unit, as Claude Code's JavaScript counts them).
fn slug(path: &Path) -> String {
    let mut slug = String::new();
    for character in path.to_string_lossy().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else {
            for _ in 0..character.len_utf16() {
                slug.push('-');
            }
        }
    }
    slug
}

/// Longer slugs are cut and given a hash of the path, which differs
/// between the runtimes Claude Code is built with: such a folder is found
/// by its first part, and its records by the folder they say they ran in.
const SLUG_LIMIT: usize = 200;

/// The folders under `home` that may hold the records of the folder whose
/// forms are `wanted`, each with whether its name is the folder's exactly.
fn project_folders(home: &Path, wanted: &[PathBuf]) -> Vec<(PathBuf, bool)> {
    let projects = home.join("projects");
    let mut folders: Vec<(PathBuf, bool)> = Vec::new();
    for path in wanted {
        let slug = slug(path);
        if slug.len() <= SLUG_LIMIT {
            let folder = projects.join(&slug);
            if folder.is_dir() && !folders.iter().any(|(known, _)| *known == folder) {
                folders.push((folder, true));
            }
            continue;
        }
        let prefix = format!("{}-", &slug[..SLUG_LIMIT]);
        let Ok(entries) = fs::read_dir(&projects) else {
            continue;
        };
        for entry in entries.flatten() {
            let folder = entry.path();
            let named = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix));
            if named && folder.is_dir() && !folders.iter().any(|(known, _)| *known == folder) {
                folders.push((folder, false));
            }
        }
    }
    folders
}

/// What the list says of one record.
struct Summary {
    title: String,
    updated: Option<SystemTime>,
    ran_in: Option<PathBuf>,
}

/// How much of a record's head and of its tail is read for the list, as
/// the SDK reads them.
const WINDOW: u64 = 64 * 1024;

/// The list's line for the record at `path`, `None` when it is no
/// conversation: a subagent's side record, an empty file, one with nothing
/// to call it by (no prompt), one carried on in another session.
fn summary(path: &Path, id: &str, folder: &Path) -> Option<Summary> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 {
        return None;
    }
    let (head, tail) = head_and_tail(path, metadata.len())?;
    let first = head.lines().next().unwrap_or_default();
    if first.contains("\"isSidechain\":true") || first.contains("\"isSidechain\": true") {
        return None;
    }
    if let Some(other) = continued_in(&tail)
        && other != id
        && folder.join(format!("{other}.jsonl")).is_file()
    {
        return None;
    }
    let title = named(&head, &tail, folder, id)
        .or_else(|| {
            record_field(&tail, "last-prompt", "lastPrompt")
                .map(|prompt| user::one_line(user::without_preface(&prompt).unwrap_or_default()))
        })
        .filter(|title| !title.is_empty())
        .or_else(|| {
            record_field(&tail, "summary", "summary").map(|summary| user::one_line(&summary))
        })
        .filter(|title| !title.is_empty())
        .or_else(|| first_prompt(&head))?;
    Some(Summary {
        title,
        updated: metadata.modified().ok(),
        ran_in: first_field(&head, "cwd").map(PathBuf::from),
    })
}

/// The name a conversation has in what was read of its record: the one it
/// was given, else the one Claude Code made for it.
fn named(head: &str, tail: &str, folder: &Path, id: &str) -> Option<String> {
    record_field(tail, "custom-title", "customTitle")
        .or_else(|| side_title(folder, id))
        .or_else(|| record_field(head, "custom-title", "customTitle"))
        .or_else(|| record_field(tail, "ai-title", "aiTitle"))
        .or_else(|| record_field(head, "ai-title", "aiTitle"))
        .map(|title| user::one_line(&title))
        .filter(|title| !title.is_empty())
}

/// The name of the conversation kept in `record`: the one it was given,
/// else the one Claude Code made for it; `None` while it has neither. Only
/// the record's head and tail are read, so a live session can ask after
/// every turn however long the conversation is.
pub fn title(record: &Path) -> Option<String> {
    let size = fs::metadata(record).ok()?.len();
    let (head, tail) = head_and_tail(record, size)?;
    let id = record.file_stem()?.to_str()?;
    named(&head, &tail, record.parent()?, id)
}

/// The first `WINDOW` bytes of a file and its last, as text.
fn head_and_tail(path: &Path, size: u64) -> Option<(String, String)> {
    // (A record being written grows meanwhile: what is there is read.)
    let mut file = fs::File::open(path).ok()?;
    let mut buffer = Vec::new();
    file.by_ref().take(WINDOW).read_to_end(&mut buffer).ok()?;
    if buffer.is_empty() {
        return None;
    }
    let head = String::from_utf8_lossy(&buffer).into_owned();
    if size <= WINDOW {
        return Some((head.clone(), head));
    }
    file.seek(SeekFrom::Start(size - WINDOW)).ok()?;
    buffer.clear();
    file.take(WINDOW).read_to_end(&mut buffer).ok()?;
    Some((head, String::from_utf8_lossy(&buffer).into_owned()))
}

/// The field `key` of the last whole record of type `kind` in `text`.
fn record_field(text: &str, kind: &str, key: &str) -> Option<String> {
    let kind_mark = format!("\"{kind}\"");
    let key_mark = format!("\"{key}\"");
    text.lines()
        .rev()
        .filter(|line| line.contains(&kind_mark) && line.contains(&key_mark))
        .find_map(|line| {
            let record: Value = serde_json::from_str(line).ok()?;
            if record.get("type")?.as_str()? != kind {
                return None;
            }
            let value = record.get(key)?.as_str()?.trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
}

/// The name a conversation was given that Claude Code keeps in a file of
/// its own beside the record (`<session>/custom-title.json`).
fn side_title(folder: &Path, id: &str) -> Option<String> {
    let text = fs::read_to_string(folder.join(id).join("custom-title.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let title = value.get("customTitle")?.as_str()?.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// The first string field `key` written in `text`, wherever it is: the SDK
/// reads the folder a session ran in so, from a head that may end mid-line.
fn first_field(text: &str, key: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut first: Option<(usize, String)> = None;
    for pattern in [format!("\"{key}\":\""), format!("\"{key}\": \"")] {
        let Some(at) = text.find(&pattern) else {
            continue;
        };
        if first.as_ref().is_some_and(|(known, _)| *known < at) {
            continue;
        }
        let start = at + pattern.len();
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'"' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        if end >= bytes.len() {
            continue;
        }
        let raw = &text[start..end];
        let value = serde_json::from_str::<String>(&format!("\"{raw}\""))
            .unwrap_or_else(|_| raw.to_owned());
        first = Some((at, value));
    }
    first
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

/// The first prompt of a record's head, else the first slash command, else
/// that it began with a picture or a document.
fn first_prompt(head: &str) -> Option<String> {
    let mut command = None;
    let users = head.lines().filter(|line| {
        (line.contains("\"type\":\"user\"") || line.contains("\"type\": \"user\""))
            && !line.contains("\"tool_result\"")
            && !line.contains("\"isMeta\":true")
            && !line.contains("\"isMeta\": true")
    });
    for line in users.clone() {
        if line.contains("\"isCompactSummary\":true") || line.contains("\"isCompactSummary\": true")
        {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(prompt) = user::prompt_of(&record, &mut command) {
            return Some(prompt);
        }
    }
    if let Some(command) = command.filter(|command| !command.trim().is_empty()) {
        return Some(command);
    }
    for line in users {
        if line.contains("\"type\":\"image\"") || line.contains("\"type\": \"image\"") {
            return Some("Image".to_owned());
        }
        if line.contains("\"type\":\"document\"") || line.contains("\"type\": \"document\"") {
            return Some("Document".to_owned());
        }
    }
    None
}

/// The session a record says the conversation went on in
/// (`continued-in`), unless the conversation went on here after that.
fn continued_in(tail: &str) -> Option<String> {
    const MARK: &str = "\"type\":\"continued-in\"";
    if !tail.contains(MARK) {
        return None;
    }
    for line in tail.lines().rev() {
        let moved = line.contains(MARK);
        let message = line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"");
        if !moved && !message {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // A turn completed here after it, or a prompt, says the
        // conversation went on in this session after all.
        let went_on = match record.get("type").and_then(Value::as_str) {
            Some("continued-in") => {
                let other = record.get("continuedInSessionId")?.as_str()?;
                return uuid::Uuid::parse_str(other)
                    .is_ok()
                    .then(|| other.to_owned());
            }
            Some("assistant") => {
                record.get("isApiErrorMessage") != Some(&Value::Bool(true))
                    && record
                        .get("message")
                        .and_then(|message| message.get("stop_reason"))
                        .is_some_and(Value::is_string)
            }
            Some("user") => user::prompt_of(&record, &mut None).is_some(),
            _ => false,
        };
        if went_on {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests;
