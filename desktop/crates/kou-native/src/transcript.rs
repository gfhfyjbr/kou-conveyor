//! Folds the runner's output and the records of a session file into the
//! conversation they add up to, as the Go cockpits' transcript does: prompts,
//! answers, reasoning, tool calls in their latest state, and notices.
//!
//! The fold keeps entries rather than emitting updates, because a stored
//! session is replayed as its final state (one update per tool call), while a
//! live run is shown as it changes. [`crate::emit`] turns either into updates.

use crate::{
    text::{approximate_tokens, clean, clean_stream, headline},
    wire,
};
use kou_thread::Level;
use serde_json::value::RawValue;
use std::collections::{HashMap, HashSet};

/// The idempotency of the operation the harness checks finished work with.
const VERIFICATION_MARKER: &str = r#"{"harness":"verification"}"#;

/// The most a running command's output is kept while it streams; the call's
/// outcome replaces it, so only the tail matters.
const LIVE_OUTPUT_LIMIT: usize = 64 * 1024;

/// One block of the conversation.
#[derive(Debug)]
pub(crate) struct Entry {
    /// Derived from persisted identifiers, so a record seen twice lands on
    /// the same entry.
    pub id: String,
    /// What the entry is.
    pub body: Body,
}

/// What an entry holds.
#[derive(Debug)]
pub(crate) enum Body {
    /// A message of the user's.
    User(UserMessage),
    /// Text the model wrote. `streamed` says the text reached the thread
    /// already, piece by piece.
    Assistant { text: String, streamed: bool },
    /// Reasoning the provider summarized.
    Thought { text: String, streamed: bool },
    /// A tool call.
    Tool(Box<Tool>),
    /// Something about the session itself.
    Notice { level: Level, text: String },
}

/// A message of the user's as the session recorded it.
#[derive(Debug, Default)]
pub(crate) struct UserMessage {
    /// The ID the runner recorded it under.
    pub message_id: String,
    /// Its text.
    pub text: String,
    /// Its images: label and URL (a data URL holds the bytes).
    pub images: Vec<(String, String)>,
    /// The files it linked: path and what the model saw of each.
    pub files: Vec<(String, String)>,
    /// Sent while the agent worked, and read after its tool calls.
    pub forced: bool,
}

/// Where a tool call stands, as the cockpits fold operation statuses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ToolState {
    /// The model made the call; nothing ran yet.
    #[default]
    Queued,
    /// An operation of the call runs.
    Running,
    /// Every operation completed.
    Done,
    /// The call or an operation failed.
    Failed,
    /// An operation was canceled.
    Canceled,
}

impl ToolState {
    /// Whether the call will not change any more.
    pub(crate) fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Canceled)
    }
}

/// A tool call and what its operations came to.
#[derive(Debug, Default)]
pub(crate) struct Tool {
    /// The model's ID for the call.
    pub call_id: String,
    /// The tool's name; empty when only a status was seen.
    pub name: String,
    /// The arguments as the model wrote them.
    pub arguments: String,
    /// The call's own error, when it never became an operation.
    pub call_error: String,
    /// What each operation came to, in order.
    pub outcomes: Vec<Outcome>,
    /// What a running command wrote so far, until its outcome replaces it.
    pub live: String,
    /// The operation `live` is of.
    pub live_operation: String,
    /// See [`ToolState`].
    pub state: ToolState,
}

/// What one operation of a tool call came to.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// The operation's ID.
    pub id: String,
    /// `ready`, `awaiting`, `canceling`, `completed`, `failed` or `canceled`.
    pub status: String,
    /// The harness's own check of the work, not a call of the model's.
    pub verify: bool,
    /// What the operation did.
    pub detail: Detail,
}

/// What an operation did, by its type.
#[derive(Debug, Default)]
pub(crate) enum Detail {
    /// A state that is missing, unknown or unreadable.
    #[default]
    Unread,
    /// A command.
    Shell {
        /// The command line.
        command: String,
        /// How it ended, once it has.
        result: Option<wire::ShellResult>,
        /// Why it could not run.
        error: String,
    },
    /// A read, an edit, a write, a patch or a search of the transcript.
    File(Box<wire::FileState>),
    /// A picture the agent looked at.
    Image {
        /// The file.
        path: String,
        /// Its size and type, once read.
        described: String,
        /// Why it could not be read.
        error: String,
    },
    /// A skill the agent loaded.
    Skill {
        /// The skill's file.
        path: String,
        /// Why it could not be loaded.
        error: String,
    },
    /// A script whose calls are tool calls.
    Code(Box<wire::CodeState>),
    /// A plugin tool's job.
    Job {
        /// What it returned.
        result: String,
        /// Why it failed.
        error: String,
    },
}

/// Tokens and money a session used so far.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Totals {
    /// Input tokens over the session.
    pub input: u64,
    /// Output tokens over the session.
    pub output: u64,
    /// Input tokens of the latest request: the context in use.
    pub context: u64,
    /// What the providers said the requests cost, when they said.
    pub cost: Option<f64>,
}

/// What a line changed.
#[derive(Debug, PartialEq)]
pub(crate) enum Event {
    /// The entry at this index is new or changed.
    Entry(usize),
    /// A piece of the answer or of the reasoning streamed. `starts_item`
    /// marks the first piece of an output item.
    Delta {
        thought: bool,
        text: String,
        starts_item: bool,
    },
    /// What streamed of a response is void: the response starts over.
    Reset,
    /// A request to the model began.
    Turn { model: String, compaction: bool },
    /// The totals changed.
    Usage,
    /// The runner said why the run fails.
    Error(String),
    /// A detached runner's last line: its exit status.
    Exit(i32),
}

/// How the latest answer ended, when not simply complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cut {
    /// It ran into the output token limit.
    Limit,
    /// The model refused.
    Refused,
}

/// What streams of one output item of a response, until the response is
/// recorded. The text is kept for a front-end that joins the run midway.
#[derive(Debug)]
struct Preview {
    index: i64,
    thought: bool,
    text: String,
}

/// The conversation a session's records add up to.
#[derive(Debug, Default)]
pub(crate) struct Transcript {
    /// The conversation, in order.
    pub entries: Vec<Entry>,
    /// See [`Totals`].
    pub usage: Totals,
    /// The model the latest regular turn went to.
    pub last_model: Option<String>,
    /// Whether the model answered since the conversation was last compacted.
    pub compactable: bool,
    /// How the latest answer was cut short, if it was.
    pub last_cut: Option<Cut>,
    /// Records that could not be decoded.
    pub skipped: usize,
    index: HashMap<String, usize>,
    operations: HashMap<String, String>,
    compactions: HashSet<String>,
    previews: HashMap<String, Vec<Preview>>,
}

impl Transcript {
    /// An empty conversation.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Folds one line in and says what it changed. An undecodable line is
    /// counted and otherwise ignored.
    pub(crate) fn apply(&mut self, line: &str) -> Vec<Event> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        match self.apply_line(line) {
            Some(events) => events,
            None => {
                self.skipped += 1;
                Vec::new()
            }
        }
    }

    fn apply_line(&mut self, text: &str) -> Option<Vec<Event>> {
        let line: wire::Line = serde_json::from_str(text).ok()?;
        match line.kind.as_deref() {
            None | Some("") => {
                let item = wire::Item {
                    sequence: line.sequence.unwrap_or_default(),
                    kind: line.item_kind?,
                    data: line.item_data,
                };
                self.apply_item(&item, None)
            }
            Some("item") => {
                let record: wire::ItemRecord = serde_json::from_str(line.data?.get()).ok()?;
                self.apply_item(&record.item, record.operations.as_deref())
            }
            Some("operation") => {
                let record: wire::OperationRecord = serde_json::from_str(line.data?.get()).ok()?;
                Some(self.apply_operation(&record.operation))
            }
            Some("error") => Some(vec![Event::Error(clean(
                line.message.as_deref().unwrap_or_default(),
            ))]),
            Some("exit") => Some(vec![Event::Exit(line.code?)]),
            Some("delta") => Some(self.apply_delta(&line)),
            Some("output") => Some(self.apply_output(&line)),
            Some("session" | "checkpoint") => Some(Vec::new()),
            Some(_) => None,
        }
    }

    fn apply_item(
        &mut self,
        item: &wire::Item,
        beside: Option<&[&RawValue]>,
    ) -> Option<Vec<Event>> {
        match item.kind.as_str() {
            "fork" => {
                let fork: wire::Fork = wire::decode(item.data)?;
                Some(vec![self.put(
                    format!("fork:{}", item.sequence),
                    Body::Notice {
                        level: Level::Info,
                        text: format!("Forked from session {}", clean(&fork.parent_id)),
                    },
                )])
            }
            "input" => {
                let input: wire::Input = wire::decode(item.data)?;
                Some(self.apply_input(item.sequence, &input))
            }
            "turn" => {
                let turn: wire::Turn = wire::decode(item.data)?;
                let compaction = turn.kind == "compaction";
                let mut events = Vec::new();
                // A turn starts once the one before has its response or will
                // have none: what streamed of a response that never came is void.
                let stale = self.previews.keys().any(|id| *id != turn.id);
                self.previews.retain(|id, _| *id == turn.id);
                if stale {
                    events.push(Event::Reset);
                }
                if compaction {
                    self.compactions.insert(turn.id.clone());
                } else if !turn.model.is_empty() {
                    self.last_model = Some(turn.model.clone());
                }
                events.push(Event::Turn {
                    model: turn.model,
                    compaction,
                });
                Some(events)
            }
            "model_response" => {
                let response: wire::ModelResponse = wire::decode(item.data)?;
                Some(self.apply_response(item.sequence, &response))
            }
            "tool_call_status" => {
                let status: wire::ToolCallStatus = wire::decode(item.data)?;
                let index = self.tool(&status.call_id);
                let Body::Tool(tool) = &mut self.entries[index].body else {
                    return None;
                };
                tool.call_error = clean(&status.status.error);
                // A session file keeps a status's operations beside the item;
                // one it repeats by ID alone it already gave whole.
                let own = status.operations.unwrap_or_default();
                let operations = if own.is_empty() {
                    beside.unwrap_or_default()
                } else {
                    own.as_slice()
                };
                for raw in operations {
                    if raw.get().starts_with('"') {
                        continue;
                    }
                    let Ok(operation) = serde_json::from_str::<wire::Operation>(raw.get()) else {
                        continue;
                    };
                    self.operations
                        .insert(operation.id.clone(), status.call_id.clone());
                    merge_outcome(&mut tool.outcomes, outcome_of(&operation));
                }
                tool.refresh();
                Some(vec![Event::Entry(index)])
            }
            _ => None,
        }
    }

    fn apply_input(&mut self, sequence: u64, input: &wire::Input) -> Vec<Event> {
        match input.kind.as_str() {
            "control" => {
                let Some(control) = wire::decode::<wire::Control>(input.payload) else {
                    return Vec::new();
                };
                let reason = control.reason.trim();
                if control.mode != "hard" || reason.is_empty() {
                    return Vec::new();
                }
                vec![self.put(
                    format!("stop:{sequence}"),
                    Body::Notice {
                        level: Level::Info,
                        text: format!("Run stopped: {}", clean(reason)),
                    },
                )]
            }
            "external" => {
                let mut message = decode_payload(input.payload);
                message.message_id = input.id.clone();
                message.forced = input.delivery == "after_tools";
                vec![self.put(format!("input:{}", input.id), Body::User(message))]
            }
            _ => Vec::new(),
        }
    }

    fn apply_response(&mut self, sequence: u64, data: &wire::ModelResponse) -> Vec<Event> {
        let response = &data.response;
        let usage = &response.usage;
        let input = u64::try_from(usage.input_tokens).unwrap_or_default();
        let output = u64::try_from(usage.output_tokens).unwrap_or_default();
        self.usage.input += input;
        self.usage.output += output;
        self.usage.context = input;
        if let Some(cost) = usage.raw.and_then(reported_cost) {
            *self.usage.cost.get_or_insert(0.0) += cost;
        }
        let mut events = vec![Event::Usage];
        let outputs = response.output.as_deref().unwrap_or_default();
        let mut previews = self.previews.remove(&data.turn_id).unwrap_or_default();

        if self.compactions.contains(&data.turn_id) {
            let summary = summary_of(outputs);
            let (level, text) = if summary.is_empty() {
                (
                    Level::Warning,
                    "Context not compacted: the model wrote no summary".to_owned(),
                )
            } else {
                // The conversation is now about the size of the summary, until
                // the next turn reports it.
                let reasoning = u64::try_from(usage.reasoning_tokens).unwrap_or_default();
                self.usage.context = output
                    .saturating_sub(reasoning)
                    .max(summary.len() as u64 / 4);
                self.compactable = false;
                let mut text = "Context compacted".to_owned();
                if input > 0 {
                    text.push_str(&format!(" from {} tokens", approximate_tokens(input)));
                }
                (Level::Info, text)
            };
            events.push(self.put(
                format!("compaction:{sequence}"),
                Body::Notice { level, text },
            ));
        } else {
            self.compactable |= !outputs.is_empty();
            for (position, item) in outputs.iter().enumerate() {
                let id = format!("output:{sequence}.{position}");
                match item.kind.as_str() {
                    "message" => {
                        let Some(message) = wire::decode::<wire::OutputMessage>(item.data) else {
                            continue;
                        };
                        let text = clean(&message.text);
                        if !matches!(message.role.as_str(), "" | "assistant")
                            || text.trim().is_empty()
                        {
                            continue;
                        }
                        let streamed = take_preview(&mut previews, false);
                        events.push(self.put(id, Body::Assistant { text, streamed }));
                    }
                    "reasoning" => {
                        let Some(reasoning) = wire::decode::<wire::Reasoning>(item.data) else {
                            continue;
                        };
                        let text = clean(&reasoning.summary.unwrap_or_default().join("\n\n"));
                        if text.trim().is_empty() {
                            continue;
                        }
                        let streamed = take_preview(&mut previews, true);
                        events.push(self.put(id, Body::Thought { text, streamed }));
                    }
                    "tool_call" => {
                        let Some(call) = wire::decode::<wire::ToolCall>(item.data) else {
                            continue;
                        };
                        let index = self.tool(&call.call_id);
                        if let Body::Tool(tool) = &mut self.entries[index].body {
                            tool.name = clean(&call.name);
                            tool.arguments = call.arguments;
                        }
                        events.push(Event::Entry(index));
                    }
                    _ => {}
                }
            }
        }
        if let Some(failure) = &response.failure {
            let text = if failure.code.is_empty() {
                clean(&failure.message)
            } else {
                clean(&format!("{}: {}", failure.code, failure.message))
            };
            events.push(self.put(
                format!("failure:{sequence}"),
                Body::Notice {
                    level: Level::Error,
                    text,
                },
            ));
        }
        self.last_cut = match response.stop.as_str() {
            "max_output_tokens" => Some(Cut::Limit),
            "refused" => Some(Cut::Refused),
            _ => None,
        };
        if let Some(cut) = self.last_cut {
            let text = match cut {
                Cut::Limit => "Response cut off at the output token limit",
                Cut::Refused => "The model refused to respond",
            };
            events.push(self.put(
                format!("cut:{sequence}"),
                Body::Notice {
                    level: Level::Warning,
                    text: text.to_owned(),
                },
            ));
        }
        events
    }

    fn apply_operation(&mut self, operation: &wire::Operation) -> Vec<Event> {
        let Some(call) = self.operations.get(&operation.id) else {
            return Vec::new();
        };
        let Some(&index) = self.index.get(&format!("tool:{call}")) else {
            return Vec::new();
        };
        let Body::Tool(tool) = &mut self.entries[index].body else {
            return Vec::new();
        };
        merge_outcome(&mut tool.outcomes, outcome_of(operation));
        tool.refresh();
        vec![Event::Entry(index)]
    }

    fn apply_delta(&mut self, line: &wire::Line) -> Vec<Event> {
        let Some(turn) = line.turn.as_deref().filter(|turn| !turn.is_empty()) else {
            return Vec::new();
        };
        if line.reset {
            let had = self.previews.remove(turn).is_some_and(|p| !p.is_empty());
            return if had { vec![Event::Reset] } else { Vec::new() };
        }
        // A compaction's summary is no part of the conversation: its notice
        // says how it went.
        if self.compactions.contains(turn) {
            return Vec::new();
        }
        let thought = match line.delta_kind.as_deref() {
            Some("text") => false,
            Some("reasoning") => true,
            _ => return Vec::new(),
        };
        let text = clean_stream(line.text.as_deref().unwrap_or_default());
        if text.is_empty() {
            return Vec::new();
        }
        let index = line.index.unwrap_or_default();
        let previews = self.previews.entry(turn.to_owned()).or_default();
        let known = previews
            .iter_mut()
            .find(|preview| preview.index == index && preview.thought == thought);
        let starts_item = known.is_none();
        match known {
            Some(preview) => preview.text.push_str(&text),
            None => previews.push(Preview {
                index,
                thought,
                text: text.clone(),
            }),
        }
        vec![Event::Delta {
            thought,
            text,
            starts_item,
        }]
    }

    fn apply_output(&mut self, line: &wire::Line) -> Vec<Event> {
        let Some(call) = line.call.as_deref() else {
            return Vec::new();
        };
        let Some(&index) = self.index.get(&format!("tool:{call}")) else {
            return Vec::new();
        };
        let Body::Tool(tool) = &mut self.entries[index].body else {
            return Vec::new();
        };
        // The outcome of a call supersedes what it wrote as it ran.
        if tool.state.terminal() {
            return Vec::new();
        }
        let text = line.text.as_deref().unwrap_or_default();
        let skipped = line.skipped.unwrap_or_default();
        if text.is_empty() && skipped <= 0 {
            return Vec::new();
        }
        if skipped > 0 {
            tool.live
                .push_str(&format!("\n[{skipped} bytes not shown]\n"));
        }
        tool.live.push_str(text);
        if tool.live.len() > LIVE_OUTPUT_LIMIT {
            let mut cut = tool.live.len() - LIVE_OUTPUT_LIMIT;
            while !tool.live.is_char_boundary(cut) {
                cut += 1;
            }
            tool.live.drain(..cut);
        }
        if let Some(operation) = &line.operation {
            tool.live_operation.clone_from(operation);
        }
        vec![Event::Entry(index)]
    }

    /// The index of a tool call's entry, made when a status arrives before
    /// (or without) the call itself.
    fn tool(&mut self, call_id: &str) -> usize {
        let id = format!("tool:{call_id}");
        if let Some(&index) = self.index.get(&id) {
            return index;
        }
        self.entries.push(Entry {
            id: id.clone(),
            body: Body::Tool(Box::new(Tool {
                call_id: call_id.to_owned(),
                ..Tool::default()
            })),
        });
        self.index.insert(id, self.entries.len() - 1);
        self.entries.len() - 1
    }

    /// Appends an entry, or replaces the one with the same ID in place.
    fn put(&mut self, id: String, body: Body) -> Event {
        if let Some(&index) = self.index.get(&id) {
            self.entries[index].body = body;
            return Event::Entry(index);
        }
        self.entries.push(Entry {
            id: id.clone(),
            body,
        });
        self.index.insert(id, self.entries.len() - 1);
        Event::Entry(self.entries.len() - 1)
    }

    /// Adds a notice that no record stands for.
    pub(crate) fn note(&mut self, level: Level, text: String) -> usize {
        let id = format!("note:{}", self.entries.len());
        let Event::Entry(index) = self.put(id, Body::Notice { level, text }) else {
            unreachable!("put answers with the entry it made");
        };
        index
    }

    /// Settles the transcript when a run ends: calls still open will never
    /// finish. Returns the entries it changed.
    pub(crate) fn finish_run(&mut self) -> Vec<usize> {
        self.previews.clear();
        let mut changed = Vec::new();
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if let Body::Tool(tool) = &mut entry.body
                && !tool.state.terminal()
            {
                tool.state = ToolState::Canceled;
                changed.push(index);
            }
        }
        changed
    }

    /// Whether the last run did not finish: the last prompt was never
    /// answered, tools were left running, or the agent stopped between tool
    /// calls without a final answer.
    pub(crate) fn interrupted(&self) -> bool {
        let open = self
            .entries
            .iter()
            .any(|entry| matches!(&entry.body, Body::Tool(tool) if !tool.state.terminal()));
        if open {
            return true;
        }
        for entry in self.entries.iter().rev() {
            match entry.body {
                Body::Assistant { .. } => return false,
                Body::User(_) | Body::Tool(_) => return true,
                Body::Thought { .. } | Body::Notice { .. } => {}
            }
        }
        false
    }

    /// The first prompt, on one line.
    pub(crate) fn title(&self) -> Option<String> {
        self.entries.iter().find_map(|entry| match &entry.body {
            Body::User(message) => {
                Some(headline(&message.text, 80)).filter(|title| !title.is_empty())
            }
            _ => None,
        })
    }

    /// What streamed of responses that are not recorded yet, in order: the
    /// text a front-end that joins a run midway has missed. `true` marks
    /// reasoning.
    pub(crate) fn streaming(&self) -> Vec<(bool, &str)> {
        self.previews
            .values()
            .flatten()
            .map(|preview| (preview.thought, preview.text.as_str()))
            .collect()
    }

    /// Forgets which texts streamed: for a run joined midway, whose output
    /// was folded without anyone watching it stream.
    pub(crate) fn forget_streamed(&mut self) {
        for entry in &mut self.entries {
            if let Body::Assistant { streamed, .. } | Body::Thought { streamed, .. } =
                &mut entry.body
            {
                *streamed = false;
            }
        }
    }

    /// Marks the calls a later run left behind as canceled: a call still open
    /// when the user's next prompt starts a run, or when `ended` says no run
    /// goes on, will never finish.
    pub(crate) fn settle_abandoned(&mut self, ended: bool) {
        let mut open: Vec<usize> = Vec::new();
        let mut abandoned: Vec<usize> = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            match &entry.body {
                Body::Tool(tool) if !tool.state.terminal() => open.push(index),
                Body::User(message) if !message.forced => abandoned.append(&mut open),
                _ => {}
            }
        }
        if ended {
            abandoned.append(&mut open);
        }
        for index in abandoned {
            if let Body::Tool(tool) = &mut self.entries[index].body {
                tool.state = ToolState::Canceled;
            }
        }
    }
}

impl Tool {
    /// Recomputes the call's state from its operations, as the cockpits do.
    fn refresh(&mut self) {
        let mut state = if self.call_error.is_empty() {
            ToolState::Done
        } else {
            ToolState::Failed
        };
        for outcome in &self.outcomes {
            match outcome.status.as_str() {
                "completed" => {}
                "failed" => state = ToolState::Failed,
                "canceled" | "canceling" => {
                    if state != ToolState::Failed {
                        state = ToolState::Canceled;
                    }
                }
                _ => {
                    if state == ToolState::Done {
                        state = ToolState::Running;
                    }
                }
            }
            // A script that threw failed, though its operation completed.
            if let Detail::Code(code) = &outcome.detail
                && code.result.as_ref().is_some_and(|r| !r.error.is_empty())
                && state == ToolState::Done
            {
                state = ToolState::Failed;
            }
        }
        self.state = state;
        // A command's result supersedes what it wrote as it ran; one that
        // ended without a result, stopped, keeps what was seen of it.
        let superseded = self.outcomes.iter().any(|outcome| {
            matches!(
                &outcome.detail,
                Detail::Shell {
                    result: Some(_),
                    ..
                }
            )
        });
        if superseded {
            self.live = String::new();
        }
    }

    /// Whether the harness made the call itself, to check finished work.
    pub(crate) fn verification(&self) -> bool {
        self.name.is_empty() && self.outcomes.iter().any(|outcome| outcome.verify)
    }

    /// Drops what a finished call holds once it was shown: outputs run to
    /// megabytes, and a long session has thousands of calls.
    pub(crate) fn release(&mut self) {
        if self.state.terminal() {
            self.arguments = String::new();
            for outcome in &mut self.outcomes {
                outcome.detail = Detail::Unread;
            }
        }
    }
}

fn merge_outcome(outcomes: &mut Vec<Outcome>, outcome: Outcome) {
    match outcomes.iter_mut().find(|known| known.id == outcome.id) {
        Some(known) => *known = outcome,
        None => outcomes.push(outcome),
    }
}

/// Reads what an operation came to from its state.
fn outcome_of(operation: &wire::Operation) -> Outcome {
    let state = operation.state.map(RawValue::get);
    let detail = match (operation.kind.as_str(), state) {
        (_, None) => Detail::Unread,
        ("shell", Some(state)) => serde_json::from_str::<wire::ShellState>(state)
            .map(|state| Detail::Shell {
                command: state.input.command,
                result: state.result,
                error: clean(&state.terminal_error),
            })
            .unwrap_or_default(),
        ("file", Some(state)) => serde_json::from_str::<wire::FileState>(state)
            .map(|state| Detail::File(Box::new(state)))
            .unwrap_or_default(),
        ("view_image", Some(state)) => serde_json::from_str::<wire::ViewImageState>(state)
            .map(|state| {
                let (described, error) = match &state.result {
                    Some(result) if !result.error.is_empty() => {
                        (String::new(), clean(&result.error))
                    }
                    Some(result) => (
                        clean(&format!(
                            "{}×{} {}",
                            result.width, result.height, result.mime
                        )),
                        String::new(),
                    ),
                    None => (String::new(), String::new()),
                };
                Detail::Image {
                    path: state.path,
                    described,
                    error,
                }
            })
            .unwrap_or_default(),
        ("skill_use", Some(state)) => serde_json::from_str::<wire::SkillUseState>(state)
            .map(|state| Detail::Skill {
                path: state.path,
                error: clean(&state.terminal_error),
            })
            .unwrap_or_default(),
        ("code", Some(state)) => serde_json::from_str::<wire::CodeState>(state)
            .map(|state| Detail::Code(Box::new(state)))
            .unwrap_or_default(),
        ("remote_job", Some(state)) => serde_json::from_str::<wire::RemoteJobState>(state)
            .map(|state| Detail::Job {
                result: state.terminal_result,
                error: clean(&state.terminal_error),
            })
            .unwrap_or_default(),
        _ => Detail::Unread,
    };
    Outcome {
        id: operation.id.clone(),
        status: operation.status.clone(),
        verify: operation
            .idempotency
            .is_some_and(|raw| raw.get() == VERIFICATION_MARKER),
        detail,
    }
}

/// Marks the first unused preview of a kind as replaced by the recorded text.
fn take_preview(previews: &mut Vec<Preview>, thought: bool) -> bool {
    match previews
        .iter()
        .position(|preview| preview.thought == thought)
    {
        Some(position) => {
            previews.remove(position);
            true
        }
        None => false,
    }
}

/// Reads the payload of a user's message in either form: a JSON string of its
/// text, or an object with its images and files. A payload no reader knows is
/// its text.
fn decode_payload(payload: Option<&RawValue>) -> UserMessage {
    let Some(raw) = payload.map(RawValue::get) else {
        return UserMessage::default();
    };
    if raw.starts_with('"') {
        if let Ok(text) = serde_json::from_str::<String>(raw) {
            return UserMessage {
                text: clean(&text),
                ..UserMessage::default()
            };
        }
    } else if raw.starts_with('{')
        && let Ok(message) = serde_json::from_str::<wire::Message>(raw)
        // The object form carries images or files; anything else is no
        // message a reader knows.
        && (!message.images.is_empty() || !message.files.is_empty())
    {
        return UserMessage {
            text: clean(&message.text),
            images: message
                .images
                .into_iter()
                .map(|image| (clean(&image.label), image.url))
                .collect(),
            files: message
                .files
                .into_iter()
                .map(|file| (file.path, file.content))
                .collect(),
            ..UserMessage::default()
        };
    }
    UserMessage {
        text: clean(raw),
        ..UserMessage::default()
    }
}

/// The summary a compaction's answer holds: its `<summary>` block without the
/// review before it, or the whole answer of a model that used neither.
fn summary_of(outputs: &[wire::OutputItem]) -> String {
    let mut parts = Vec::new();
    for item in outputs {
        if item.kind != "message" {
            continue;
        }
        let Some(message) = wire::decode::<wire::OutputMessage>(item.data) else {
            continue;
        };
        if matches!(message.role.as_str(), "" | "assistant") && !message.text.trim().is_empty() {
            parts.push(message.text.trim().to_owned());
        }
    }
    let mut text = parts.join("\n\n");
    for tag in ["review", "analysis"] {
        let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
        while let Some(start) = text.find(&open) {
            let Some(length) = text[start..].find(&close) else {
                break;
            };
            text.replace_range(start..start + length + close.len(), "");
        }
    }
    if let Some((_, rest)) = text.split_once("<summary>") {
        return match rest.split_once("</summary>") {
            Some((summary, _)) => summary.trim().to_owned(),
            None => rest.trim().to_owned(),
        };
    }
    // An answer cut off inside its review never reached a summary.
    if text.contains("<review>") || text.contains("<analysis>") {
        return String::new();
    }
    text.trim().to_owned()
}

/// The cost a provider reported with its usage, as OpenRouter does.
fn reported_cost(raw: &RawValue) -> Option<f64> {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(default)]
        cost: Option<f64>,
    }
    serde_json::from_str::<Raw>(raw.get()).ok()?.cost
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(sequence: u64, kind: &str, data: &str) -> String {
        format!(
            r#"{{"Sequence":{sequence},"RecordedAt":"2026-01-01T00:00:00Z","Kind":"{kind}","Data":{data}}}"#
        )
    }

    #[test]
    fn a_response_replaces_what_streamed_of_it_and_keeps_the_rest() {
        let mut transcript = Transcript::new();
        transcript.apply(&item(
            1,
            "turn",
            r#"{"ID":"t1","Type":"regular","Model":"m"}"#,
        ));
        let delta = |kind: &str, index: u32, text: &str| {
            format!(
                r#"{{"type":"delta","turn":"t1","index":{index},"kind":"{kind}","text":"{text}"}}"#
            )
        };
        assert_eq!(
            transcript.apply(&delta("text", 1, "Hel")),
            [Event::Delta {
                thought: false,
                text: "Hel".into(),
                starts_item: true
            }]
        );
        assert_eq!(
            transcript.apply(&delta("text", 1, "lo")),
            [Event::Delta {
                thought: false,
                text: "lo".into(),
                starts_item: false
            }]
        );
        let events = transcript.apply(&item(
            2,
            "model_response",
            r#"{"TurnID":"t1","Response":{"Stop":"complete","Output":[
                {"Type":"reasoning","Data":{"Summary":["why"]}},
                {"Type":"message","Data":{"Role":"assistant","Text":"Hello"}},
                {"Type":"tool_call","Data":{"CallID":"c1","Name":"Bash","Arguments":"{\"command\":\"ls\"}"}}
            ],"Usage":{"InputTokens":10,"OutputTokens":3,"Raw":{"cost":0.5}}}}"#,
        ));
        assert_eq!(
            events,
            [
                Event::Usage,
                Event::Entry(0),
                Event::Entry(1),
                Event::Entry(2)
            ]
        );
        assert!(matches!(
            &transcript.entries[0].body,
            Body::Thought {
                streamed: false,
                ..
            }
        ));
        assert!(matches!(
            &transcript.entries[1].body,
            Body::Assistant { streamed: true, .. }
        ));
        assert_eq!(transcript.usage.context, 10);
        assert_eq!(transcript.usage.cost, Some(0.5));
        assert_eq!(transcript.last_model.as_deref(), Some("m"));
        assert!(transcript.interrupted(), "a tool call was left open");
    }

    #[test]
    fn a_status_folds_its_operations_into_the_call_s_state() {
        let mut transcript = Transcript::new();
        let status = |status: &str, result: &str| {
            item(
                3,
                "tool_call_status",
                &format!(
                    r#"{{"CallID":"c1","Status":{{"Error":""}},"Operations":[{{"ID":"op1","Type":"shell","Status":"{status}","State":{{"Input":{{"Command":"ls"}}{result}}}}}]}}"#
                ),
            )
        };
        transcript.apply(&status("awaiting", ""));
        let Body::Tool(tool) = &transcript.entries[0].body else {
            panic!("a tool entry");
        };
        assert_eq!(tool.state, ToolState::Running);

        let output =
            r#"{"type":"output","call":"c1","operation":"op1","stream":"out","text":"a\n"}"#;
        assert_eq!(transcript.apply(output), [Event::Entry(0)]);

        transcript.apply(&status(
            "completed",
            r#","Result":{"Out":"a\n","Err":"","ExitCode":0}"#,
        ));
        let Body::Tool(tool) = &transcript.entries[0].body else {
            panic!("a tool entry");
        };
        assert_eq!(tool.state, ToolState::Done);
        assert!(tool.live.is_empty(), "the outcome supersedes what streamed");
        assert_eq!(
            transcript.apply(output),
            [],
            "nothing streams after the end"
        );
        assert!(!transcript.interrupted() || transcript.entries.len() == 1);
    }

    #[test]
    fn a_compaction_is_a_notice_and_its_summary_does_not_stream() {
        let mut transcript = Transcript::new();
        transcript.apply(&item(1, "turn", r#"{"ID":"t9","Type":"compaction"}"#));
        assert_eq!(
            transcript
                .apply(r#"{"type":"delta","turn":"t9","index":0,"kind":"text","text":"sum"}"#),
            []
        );
        transcript.apply(&item(
            2,
            "model_response",
            r#"{"TurnID":"t9","Response":{"Stop":"complete","Output":[{"Type":"message","Data":{"Role":"assistant","Text":"<review>long</review><summary>short</summary>"}}],"Usage":{"InputTokens":811600,"OutputTokens":40}}}"#,
        ));
        let Body::Notice { level, text } = &transcript.entries[0].body else {
            panic!("a notice");
        };
        assert_eq!(
            (*level, text.as_str()),
            (Level::Info, "Context compacted from 812k tokens")
        );
        assert_eq!(transcript.usage.context, 40);
    }

    #[test]
    fn a_session_file_record_brings_its_operations_beside_the_item() {
        let mut transcript = Transcript::new();
        let record = r#"{"type":"item","data":{"Item":{"Sequence":7,"RecordedAt":"2026-01-01T00:00:00Z","Kind":"tool_call_status","Data":{"CallID":"c2","Status":{"Error":""}}},"Operations":[{"ID":"op2","Type":"file","Status":"awaiting","State":{"Action":"read","Path":"/w/a.rs"}}]}}"#;
        assert_eq!(transcript.apply(record), [Event::Entry(0)]);
        let progress = r#"{"type":"operation","data":{"Operation":{"ID":"op2","Type":"file","Status":"completed","State":{"Action":"read","Path":"/w/a.rs","Result":{"Text":"     1\tfn main() {}"}}}}}"#;
        assert_eq!(transcript.apply(progress), [Event::Entry(0)]);
        let Body::Tool(tool) = &transcript.entries[0].body else {
            panic!("a tool entry");
        };
        assert_eq!(tool.state, ToolState::Done);
        assert_eq!(transcript.apply("not json"), []);
        assert_eq!(transcript.skipped, 1);
    }

    #[test]
    fn prompts_come_in_both_payload_forms_and_a_stop_says_why() {
        let mut transcript = Transcript::new();
        transcript.apply(&item(
            1,
            "input",
            r#"{"ID":"m1","Kind":"external","Payload":"hello\u001b[1m there"}"#,
        ));
        transcript.apply(&item(
            2,
            "input",
            r#"{"ID":"m2","Kind":"external","Delivery":"after_tools","Payload":{"Text":"see [Image 1]","Images":[{"Label":"[Image 1]","URL":"data:image/png;base64,AAAA"}],"Files":[{"Label":"$a.rs","Path":"a.rs","Content":"fn a() {}"}]}}"#,
        ));
        transcript.apply(&item(
            3,
            "input",
            r#"{"ID":"c","Kind":"control","Payload":{"Mode":"hard","Reason":"the user stopped it"}}"#,
        ));
        let Body::User(first) = &transcript.entries[0].body else {
            panic!("a prompt");
        };
        assert_eq!(first.text, "hello there");
        let Body::User(second) = &transcript.entries[1].body else {
            panic!("a prompt");
        };
        assert!(second.forced);
        assert_eq!(second.images.len(), 1);
        assert_eq!(second.files[0].0, "a.rs");
        assert!(matches!(
            &transcript.entries[2].body,
            Body::Notice { text, .. } if text == "Run stopped: the user stopped it"
        ));
        assert_eq!(transcript.title().as_deref(), Some("hello there"));
    }
}
