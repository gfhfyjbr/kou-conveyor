//! The reducer: a thread is what its updates add up to.

use crate::{
    AgentNote, AgentWork, Block, Capabilities, Choice, Level, Offers, PermissionOption, PlanStep,
    Provider, Stop, ToolCall, ToolPatch, ToolStatus, Update, Usage,
};
use std::collections::{HashMap, VecDeque};

/// One item of the conversation, in display order.
// A tool call is several times the size of the other variants, and stays
// inline: entries live in one vector, most of a working thread is tool calls,
// and a box would cost an allocation per call to save space in the rest.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// What the user sent.
    User(Vec<Block>),
    /// The assistant's answer so far, as markdown.
    Assistant(String),
    /// The assistant's reasoning so far.
    Thought(String),
    /// A tool call in its latest state.
    Tool(ToolCall),
    /// The agent's plan in its latest state.
    Plan(Vec<PlanStep>),
    /// A message from the backend about the session itself.
    Notice(Level, String),
}

/// What the thread is doing, for the pane header and the rail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ThreadState {
    /// The backend has not said it is ready.
    #[default]
    Starting,
    /// Waiting for a prompt.
    Idle,
    /// A turn is running.
    Working,
    /// A turn is waiting for the user to answer a permission request.
    Blocked,
    /// The backend is gone; `Some` is why, when it failed.
    Closed(Option<String>),
}

/// What an update changed, so a view redraws only that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// A new entry at this index.
    Appended(usize),
    /// The entry at this index changed.
    Updated(usize),
    /// The entries from this index on are gone (see [`Thread::rewind`]); the
    /// changes after it append what stands there now.
    Truncated(usize),
    /// Something outside the entries: state, title, mode, model, usage, commands, permissions.
    Meta,
}

/// A prompt edited with [`Thread::rewind`], on its way to the backend.
#[derive(Clone, Debug, PartialEq)]
pub struct Rewinding {
    /// The prompt the session goes back to before, for [`crate::Request::Rewind`].
    pub prompt: Option<String>,
    /// What changed in the thread.
    pub changes: Vec<Change>,
}

/// What a rewind on its way replaced, put back if it fails.
#[derive(Clone, Debug)]
struct Undo {
    /// Where the edited prompt stands: the entries from here on were replaced.
    at: usize,
    entries: Vec<Entry>,
    tools: HashMap<String, usize>,
    plan: Option<usize>,
    open: Open,
    prompts: HashMap<usize, String>,
    unnamed: VecDeque<Option<usize>>,
    shown_user: Option<usize>,
}

/// A permission request the user has not answered.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingPermission {
    /// The id to answer with.
    pub request: u64,
    /// Index of the tool call's entry.
    pub entry: usize,
    /// The answers on offer.
    pub options: Vec<PermissionOption>,
    /// What the call asks the user, when it is one that asks
    /// ([`crate::PermissionRequest::questions`]).
    pub questions: Vec<crate::Question>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Open {
    #[default]
    Nothing,
    User,
    Assistant,
    Thought,
}

/// A conversation with one agent session.
#[derive(Clone, Debug, Default)]
pub struct Thread {
    entries: Vec<Entry>,
    state: ThreadState,
    open: Open,
    /// The entry that is open, while one is: the agent's own message goes
    /// on in it though what its subagents do is entered after it.
    open_at: usize,
    tools: HashMap<String, usize>,
    plan: Option<usize>,
    /// How many entries there were when the last turn began.
    turn_from: usize,
    permissions: Vec<PendingPermission>,
    /// The question that blocks was asked while no turn ran: answered, it
    /// leaves the thread as it found it.
    asked_idle: bool,
    /// The backend's IDs of the prompts of user entries, by entry, as far as
    /// the backend named them (see [`Thread::prompt_id`]).
    prompts: HashMap<usize, String>,
    /// The entries of the prompts the interface sent that the backend has not
    /// named yet, oldest first; `None` for one a rewind took away since.
    unnamed: VecDeque<Option<usize>>,
    /// The user entry the backend's chunks opened last.
    shown_user: Option<usize>,
    /// Whether the backend said what it offers: until then a prompt sent may
    /// still be named.
    offered: bool,
    /// What a rewind on its way replaced.
    undo: Option<Undo>,
    /// The backend's session id, once ready.
    pub session: Option<String>,
    /// The session's title, when the backend named it.
    pub title: Option<String>,
    /// What prompts may contain.
    pub capabilities: Capabilities,
    /// Modes on offer.
    pub modes: Vec<Choice>,
    /// The mode in force.
    pub mode: Option<String>,
    pub access: crate::Access,
    /// Models on offer.
    pub models: Vec<Choice>,
    /// The model in force.
    pub model: Option<String>,
    /// How hard the model in force may be asked to think, lowest first;
    /// none when the backend offers no such choice.
    pub efforts: Vec<Choice>,
    /// The level in force.
    pub effort: Option<String>,
    /// Who serves the models on offer, when the backend says.
    pub providers: Vec<Provider>,
    /// Slash commands on offer.
    pub commands: Vec<Choice>,
    /// What the backend answers beyond the commands every backend takes.
    pub offers: Offers,
    /// The latest totals.
    pub usage: Usage,
    /// Why the last turn ended.
    pub last_stop: Option<Stop>,
}

impl Thread {
    /// An empty thread waiting for its backend.
    pub fn new() -> Self {
        Self::default()
    }

    /// The conversation so far.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// What the thread is doing.
    pub fn state(&self) -> &ThreadState {
        &self.state
    }

    /// The first entry of the turn that runs, while one does: what stands
    /// from there on is of that turn, a message of the user's that the turn
    /// took in among it.
    pub fn turn_from(&self) -> Option<usize> {
        matches!(self.state, ThreadState::Working | ThreadState::Blocked).then_some(self.turn_from)
    }

    /// The entry of the tool call the backend knows as `id`.
    pub fn tool_entry(&self, id: &str) -> Option<usize> {
        self.tools.get(id).copied()
    }

    /// Requests waiting for the user, oldest first.
    pub fn permissions(&self) -> &[PendingPermission] {
        &self.permissions
    }

    /// Record a prompt the user just sent, before the backend says anything about it.
    pub fn push_user(&mut self, blocks: Vec<Block>) -> Change {
        self.open = Open::Nothing;
        let change = self.push(Entry::User(blocks));
        // A backend that rewinds names it once it takes it (`PromptTaken`).
        if !self.offered || self.offers.rewind {
            self.unnamed.push_back(Some(self.entries.len() - 1));
        }
        change
    }

    /// The ID the backend knows the prompt of user entry `index` by, for
    /// [`crate::Request::Rewind`]; `None` while it has not named it, and for
    /// a prompt that never reached the session.
    pub fn prompt_id(&self, index: usize) -> Option<&str> {
        self.prompts.get(&index).map(String::as_str)
    }

    /// The user entry whose edit is on its way to the backend, if one is.
    pub fn rewinding(&self) -> Option<usize> {
        self.undo.as_ref().map(|undo| undo.at)
    }

    /// Edits the prompt of user entry `index`: it and every entry after it
    /// make way for `blocks`, shown at once as the prompt that runs from
    /// there, as [`push_user`](Self::push_user) shows one. Returns the prompt
    /// the session goes back to before, for the [`crate::Request::Rewind`]
    /// that runs the edit: the first prompt from `index` on that the backend
    /// named, `None` when none of them reached the session. The thread keeps
    /// what it replaced until [`Update::Rewound`] says how the rewind went,
    /// and puts it back if it failed.
    ///
    /// `None`, and nothing changes, when `index` is no user entry or another
    /// rewind is on its way.
    pub fn rewind(&mut self, index: usize, blocks: Vec<Block>) -> Option<Rewinding> {
        if self.undo.is_some() || !matches!(self.entries.get(index), Some(Entry::User(_))) {
            return None;
        }
        let prompt = (index..self.entries.len()).find_map(|at| self.prompts.get(&at).cloned());
        let undo = Undo {
            at: index,
            entries: self.entries[index..].to_vec(),
            tools: self.tools.clone(),
            plan: self.plan,
            open: self.open,
            prompts: self.prompts.clone(),
            unnamed: self.unnamed.clone(),
            shown_user: self.shown_user,
        };
        self.entries.truncate(index);
        self.tools.retain(|_, at| *at < index);
        self.plan = self.plan.filter(|at| *at < index);
        self.prompts.retain(|at, _| *at < index);
        // Their names may still come: they name nothing any more.
        for slot in &mut self.unnamed {
            if slot.is_some_and(|at| at >= index) {
                *slot = None;
            }
        }
        self.shown_user = self.shown_user.filter(|at| *at < index);
        self.permissions.retain(|pending| pending.entry < index);
        self.open = Open::Nothing;
        let mut changes = vec![Change::Truncated(index)];
        changes.push(self.push(Entry::User(blocks)));
        self.undo = Some(undo);
        Some(Rewinding { prompt, changes })
    }

    /// Puts back what a rewind that failed replaced. What came after the
    /// edited prompt in the meantime stays, after it.
    fn restore(&mut self, undo: Undo) -> Vec<Change> {
        let later = if self.entries.len() > undo.at + 1 {
            self.entries.split_off(undo.at + 1)
        } else {
            Vec::new()
        };
        self.entries.truncate(undo.at);
        let mut changes = vec![Change::Truncated(undo.at)];
        for entry in undo.entries {
            changes.push(self.push(entry));
        }
        self.tools = undo.tools;
        self.plan = undo.plan;
        self.open = undo.open;
        self.open_at = self.entries.len().saturating_sub(1);
        self.prompts = undo.prompts;
        self.unnamed = undo.unnamed;
        self.shown_user = undo.shown_user;
        for entry in later {
            let at = self.entries.len();
            match &entry {
                Entry::Tool(call) => {
                    self.tools.insert(call.id.clone(), at);
                }
                Entry::Plan(_) => self.plan = Some(at),
                _ => {}
            }
            changes.push(self.push(entry));
        }
        changes.push(Change::Meta);
        changes
    }

    /// The user answered a request: drop it, and go back to working if none remain.
    pub fn answered(&mut self, request: u64) -> Option<Change> {
        let before = self.permissions.len();
        self.permissions
            .retain(|pending| pending.request != request);
        if self.permissions.len() == before {
            return None;
        }
        if self.permissions.is_empty() && self.state == ThreadState::Blocked {
            // Back to what the question interrupted. One asked while no
            // turn ran ends no turn when it is answered: "working" would
            // then never end, with prompts queued behind it for good. Work
            // that follows the answer says so itself (a turn begins).
            self.state = if std::mem::take(&mut self.asked_idle) {
                ThreadState::Idle
            } else {
                ThreadState::Working
            };
        }
        Some(Change::Meta)
    }

    /// Fold one update in and say what changed.
    pub fn apply(&mut self, update: Update) -> Vec<Change> {
        match update {
            Update::Ready {
                session,
                capabilities,
                modes,
                mode,
                models,
                model,
            } => {
                self.session = Some(session);
                self.capabilities = capabilities;
                self.modes = modes;
                self.mode = mode;
                self.models = models;
                self.model = model;
                if self.state == ThreadState::Starting {
                    self.state = ThreadState::Idle;
                }
                vec![Change::Meta]
            }
            Update::UserChunk(block) => {
                if self.open == Open::User
                    && let Some(Entry::User(blocks)) = self.entries.get_mut(self.open_at)
                {
                    match (blocks.last_mut(), block) {
                        (Some(Block::Text(text)), Block::Text(more)) => text.push_str(&more),
                        (_, block) => blocks.push(block),
                    }
                    return vec![Change::Updated(self.open_at)];
                }
                self.open = Open::User;
                let change = self.push(Entry::User(vec![block]));
                self.open_at = self.entries.len() - 1;
                self.shown_user = Some(self.entries.len() - 1);
                vec![change]
            }
            Update::AssistantChunk(more) => {
                if self.open == Open::Assistant
                    && let Some(Entry::Assistant(text)) = self.entries.get_mut(self.open_at)
                {
                    text.push_str(&more);
                    return vec![Change::Updated(self.open_at)];
                }
                self.open = Open::Assistant;
                let change = self.push(Entry::Assistant(more));
                self.open_at = self.entries.len() - 1;
                vec![change]
            }
            Update::ThoughtChunk(more) => {
                if self.open == Open::Thought
                    && let Some(Entry::Thought(text)) = self.entries.get_mut(self.open_at)
                {
                    text.push_str(&more);
                    return vec![Change::Updated(self.open_at)];
                }
                self.open = Open::Thought;
                let change = self.push(Entry::Thought(more));
                self.open_at = self.entries.len() - 1;
                vec![change]
            }
            Update::AgentSaid { call, text } => self.agent_said(&call, text),
            Update::Tool(call) => vec![self.upsert_tool(call)],
            Update::ToolPatch(patch) => vec![self.patch_tool(patch)],
            Update::Plan(steps) => {
                if let Some(index) = self.plan {
                    self.entries[index] = Entry::Plan(steps);
                    return vec![Change::Updated(index)];
                }
                self.open = Open::Nothing;
                let change = self.push(Entry::Plan(steps));
                self.plan = Some(self.entries.len() - 1);
                vec![change]
            }
            Update::Permission(request) => {
                let mut call = request.tool;
                call.status = ToolStatus::Pending;
                let change = self.upsert_tool(call.clone());
                let entry = self.tools[&call.id];
                self.permissions
                    .retain(|pending| pending.request != request.id);
                self.permissions.push(PendingPermission {
                    request: request.id,
                    entry,
                    options: request.options,
                    questions: request.questions,
                });
                if self.state != ThreadState::Blocked {
                    self.asked_idle = self.state == ThreadState::Idle;
                }
                self.state = ThreadState::Blocked;
                vec![change, Change::Meta]
            }
            // Taken back, a request is gone as an answered one is.
            Update::Withdrawn(request) => self.answered(request).into_iter().collect(),
            Update::Commands(commands) => {
                self.commands = commands;
                vec![Change::Meta]
            }
            Update::Mode(mode) => {
                self.mode = Some(mode);
                vec![Change::Meta]
            }
            Update::Access(access) => {
                self.access = access;
                vec![Change::Meta]
            }
            Update::Model(model) => {
                self.model = Some(model);
                vec![Change::Meta]
            }
            Update::Efforts { levels, current } => {
                self.efforts = levels;
                self.effort = current;
                vec![Change::Meta]
            }
            Update::Providers(providers) => {
                self.providers = providers;
                vec![Change::Meta]
            }
            Update::Title(title) => {
                self.title = Some(crate::plain_title(&title).to_owned());
                vec![Change::Meta]
            }
            Update::Usage(usage) => {
                self.usage = usage;
                vec![Change::Meta]
            }
            Update::TurnStarted => {
                self.open = Open::Nothing;
                self.plan = None;
                self.turn_from = self.entries.len();
                self.asked_idle = false;
                self.state = ThreadState::Working;
                vec![Change::Meta]
            }
            Update::TurnEnded(stop) => {
                let mut changes = vec![Change::Meta];
                self.open = Open::Nothing;
                self.permissions.clear();
                // A call still open when the turn is over will never finish.
                for (index, entry) in self.entries.iter_mut().enumerate() {
                    if let Entry::Tool(call) = entry
                        && matches!(call.status, ToolStatus::Pending | ToolStatus::Running)
                    {
                        call.status = ToolStatus::Cancelled;
                        changes.push(Change::Updated(index));
                    }
                }
                if let Stop::Failed(reason) = &stop {
                    changes.push(self.push(Entry::Notice(Level::Error, reason.clone())));
                }
                self.last_stop = Some(stop);
                if !matches!(self.state, ThreadState::Closed(_)) {
                    self.state = ThreadState::Idle;
                }
                changes
            }
            Update::Notice(level, text) => {
                self.open = Open::Nothing;
                vec![self.push(Entry::Notice(level, text))]
            }
            Update::Closed(reason) => {
                self.open = Open::Nothing;
                self.permissions.clear();
                self.state = ThreadState::Closed(reason);
                // What the agent left running went with it (its commands
                // are ended when its session closes): nothing is in the
                // background of an agent that is gone.
                let mut changes = vec![Change::Meta];
                for (index, entry) in self.entries.iter_mut().enumerate() {
                    if let Entry::Tool(call) = entry
                        && call.work.background
                        && !call.work.ended
                    {
                        call.work.ended = true;
                        changes.push(Change::Updated(index));
                    }
                }
                changes
            }
            Update::Offers(offers) => {
                self.offered = true;
                if !offers.rewind {
                    // Nothing will name the prompts sent so far.
                    self.unnamed.clear();
                }
                self.offers = offers;
                vec![Change::Meta]
            }
            Update::PromptTaken(id) => {
                if let Some(Some(at)) = self.unnamed.pop_front()
                    && let Some(id) = id
                {
                    self.prompts.entry(at).or_insert(id);
                }
                Vec::new()
            }
            Update::PromptId(id) => {
                // An entry that holds two messages is known by the first.
                if let Some(at) = self.shown_user {
                    self.prompts.entry(at).or_insert(id);
                }
                Vec::new()
            }
            Update::Rewound(result) => match (result, self.undo.take()) {
                (Ok(id), Some(undo)) => {
                    self.prompts.insert(undo.at, id);
                    vec![Change::Meta]
                }
                (Err(_), Some(undo)) => self.restore(undo),
                // No rewind was on its way: nothing to settle.
                (_, None) => Vec::new(),
            },
        }
    }

    fn push(&mut self, entry: Entry) -> Change {
        self.entries.push(entry);
        Change::Appended(self.entries.len() - 1)
    }

    fn upsert_tool(&mut self, mut call: ToolCall) -> Change {
        // A call of the agent's own ends the message it was writing. One a
        // subagent makes does not: it is that subagent's, working beside
        // the agent, and the agent's message goes on after it. (Taken for a
        // break, it cut an answer in two wherever a subagent did something
        // while the answer was being written.)
        let nested = call.parent.is_some()
            || self.tools.get(&call.id).is_some_and(|&index| {
                matches!(&self.entries[index], Entry::Tool(known) if known.parent.is_some())
            });
        if !nested {
            self.open = Open::Nothing;
        }
        if let Some(&index) = self.tools.get(&call.id) {
            // Announced again, it is the same call: what the thread has
            // gathered of its subagent since is kept.
            if let Entry::Tool(known) = &mut self.entries[index] {
                let gathered = std::mem::take(&mut known.work);
                call.work.steps = gathered.steps;
                call.work.said = gathered.said;
                call.work.background |= gathered.background;
                call.work.ended |= gathered.ended;
                call.work.tokens = call.work.tokens.or(gathered.tokens);
                if call.agent.as_deref() == Some("") && known.agent.is_some() {
                    call.agent = known.agent.take();
                }
                if call.parent.is_none() {
                    call.parent = known.parent.take();
                }
            }
            self.entries[index] = Entry::Tool(call);
            return Change::Updated(index);
        }
        self.tools.insert(call.id.clone(), self.entries.len());
        if let Some(parent) = call.parent.clone() {
            self.count_step(&parent);
        }
        self.push(Entry::Tool(call))
    }

    /// One more call made by the subagent that call `launch` started.
    fn count_step(&mut self, launch: &str) {
        if let Some(&index) = self.tools.get(launch)
            && let Entry::Tool(call) = &mut self.entries[index]
        {
            call.work.steps += 1;
        }
    }

    /// More of what the subagent that call `launch` started says: added to
    /// what it said last, unless it has made a call since.
    fn agent_said(&mut self, launch: &str, text: String) -> Vec<Change> {
        let Some(&index) = self.tools.get(launch) else {
            // Words of a subagent whose call was never announced: nothing
            // of the conversation's.
            return Vec::new();
        };
        let Entry::Tool(call) = &mut self.entries[index] else {
            unreachable!("the tool index points at tool entries only");
        };
        let after = call.work.steps;
        match call.work.said.last_mut() {
            Some(note) if note.after == after => note.text.push_str(&text),
            _ => call.work.said.push(AgentNote { after, text }),
        }
        vec![Change::Updated(index)]
    }

    fn patch_tool(&mut self, patch: ToolPatch) -> Change {
        let Some(&index) = self.tools.get(&patch.id) else {
            // Some agents report progress on a call they never announced.
            return self.upsert_tool(ToolCall {
                id: patch.id,
                title: patch.title.unwrap_or_default(),
                kind: patch.kind.unwrap_or_default(),
                status: patch.status.unwrap_or_default(),
                content: patch.content.unwrap_or_default(),
                locations: patch.locations.unwrap_or_default(),
                raw_input: patch.raw_input,
                raw_output: patch.raw_output,
                agent: patch.agent,
                parent: patch.parent,
                work: AgentWork {
                    background: patch.background.unwrap_or_default(),
                    ended: patch.ended.unwrap_or_default(),
                    tokens: patch.tokens,
                    ..AgentWork::default()
                },
            });
        };
        let Entry::Tool(call) = &mut self.entries[index] else {
            unreachable!("the tool index points at tool entries only");
        };
        // A call first said to be a subagent's by a later report is counted
        // as one of that subagent's then.
        let adopted = match (&call.parent, &patch.parent) {
            (None, Some(parent)) => Some(parent.clone()),
            _ => None,
        };
        if let Some(title) = patch.title {
            call.title = title;
        }
        if let Some(kind) = patch.kind {
            call.kind = kind;
        }
        if let Some(status) = patch.status {
            call.status = status;
        }
        if let Some(content) = patch.content {
            call.content = content;
        }
        if let Some(locations) = patch.locations {
            call.locations = locations;
        }
        if patch.raw_input.is_some() {
            call.raw_input = patch.raw_input;
        }
        if patch.raw_output.is_some() {
            call.raw_output = patch.raw_output;
        }
        // The kind is said with the call's arguments; a later report that
        // names the tool again without them does not take it away.
        match patch.agent {
            Some(kind) if !kind.is_empty() || call.agent.is_none() => call.agent = Some(kind),
            _ => {}
        }
        if patch.parent.is_some() {
            call.parent = patch.parent;
        }
        if let Some(background) = patch.background {
            call.work.background = background;
        }
        if let Some(ended) = patch.ended {
            call.work.ended = ended;
        }
        if patch.tokens.is_some() {
            call.work.tokens = patch.tokens;
        }
        if let Some(parent) = adopted {
            self.count_step(&parent);
        }
        Change::Updated(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PermissionKind, PermissionRequest, ToolContent, ToolKind};

    fn ready() -> Update {
        Update::Ready {
            session: "s1".into(),
            capabilities: Capabilities::default(),
            modes: vec![],
            mode: None,
            models: vec![],
            model: None,
        }
    }

    #[test]
    fn a_subagent_s_words_and_calls_are_kept_with_the_call_that_started_it() {
        let mut thread = Thread::new();
        thread.apply(ready());
        thread.push_user(vec![Block::Text("check".into())]);
        thread.apply(Update::TurnStarted);
        let launch = ToolCall {
            agent: Some("Explore".into()),
            ..tool("a1", ToolStatus::Running)
        };
        thread.apply(Update::Tool(launch));
        let said = |text: &str| Update::AgentSaid {
            call: "a1".into(),
            text: text.into(),
        };
        let its = |id: &str| ToolCall {
            parent: Some("a1".into()),
            ..tool(id, ToolStatus::Running)
        };
        // Its words change its call's entry, and are no entry of their own.
        assert_eq!(thread.apply(said("Looking ")), [Change::Updated(1)]);
        assert_eq!(thread.apply(said("around.")), [Change::Updated(1)]);
        thread.apply(Update::Tool(its("c1")));
        // A call announced again is the same call, counted once.
        thread.apply(Update::Tool(its("c1")));
        thread.apply(said("Found it."));
        // One first said to be the subagent's by a later report counts then.
        thread.apply(Update::Tool(tool("c2", ToolStatus::Running)));
        thread.apply(Update::ToolPatch(ToolPatch {
            id: "c2".into(),
            parent: Some("a1".into()),
            ..ToolPatch::default()
        }));
        // A report that names the tool again without its arguments keeps
        // the kind, and one that says more of the subagent adds to it.
        thread.apply(Update::ToolPatch(ToolPatch {
            id: "a1".into(),
            agent: Some(String::new()),
            background: Some(true),
            tokens: Some(900),
            ..ToolPatch::default()
        }));
        // Announced again, the call keeps what was gathered of its subagent.
        thread.apply(Update::Tool(ToolCall {
            agent: Some(String::new()),
            ..tool("a1", ToolStatus::Done)
        }));
        let Entry::Tool(call) = &thread.entries()[1] else {
            panic!("the call");
        };
        assert_eq!(call.agent.as_deref(), Some("Explore"));
        assert_eq!(call.status, ToolStatus::Done);
        assert_eq!(
            call.work,
            AgentWork {
                background: true,
                ended: false,
                steps: 2,
                said: vec![
                    AgentNote {
                        after: 0,
                        text: "Looking around.".into()
                    },
                    AgentNote {
                        after: 1,
                        text: "Found it.".into()
                    },
                ],
                tokens: Some(900),
            }
        );
        // Words for a call the thread never heard of are nobody's.
        assert_eq!(
            thread.apply(Update::AgentSaid {
                call: "gone".into(),
                text: "?".into()
            }),
            []
        );
        assert_eq!(thread.entries().len(), 4);
    }

    fn tool(id: &str, status: ToolStatus) -> ToolCall {
        ToolCall {
            id: id.into(),
            title: format!("Run {id}"),
            kind: ToolKind::Execute,
            status,
            ..ToolCall::default()
        }
    }

    #[test]
    fn chunks_of_one_kind_grow_one_entry_and_a_tool_call_splits_them() {
        let mut thread = Thread::new();
        thread.apply(ready());
        assert_eq!(thread.state(), &ThreadState::Idle);
        thread.push_user(vec![Block::Text("hi".into())]);
        thread.apply(Update::TurnStarted);
        assert_eq!(
            thread.apply(Update::ThoughtChunk("hm".into())),
            [Change::Appended(1)]
        );
        assert_eq!(
            thread.apply(Update::ThoughtChunk("m".into())),
            [Change::Updated(1)]
        );
        thread.apply(Update::AssistantChunk("He".into()));
        thread.apply(Update::AssistantChunk("llo".into()));
        thread.apply(Update::Tool(tool("t1", ToolStatus::Running)));
        thread.apply(Update::AssistantChunk("Done".into()));
        assert_eq!(
            thread.entries(),
            [
                Entry::User(vec![Block::Text("hi".into())]),
                Entry::Thought("hmm".into()),
                Entry::Assistant("Hello".into()),
                Entry::Tool(tool("t1", ToolStatus::Running)),
                Entry::Assistant("Done".into()),
            ]
        );
    }

    #[test]
    fn providers_replace_the_grouping_of_the_models_on_offer() {
        let mut thread = Thread::new();
        thread.apply(ready());
        assert!(thread.providers.is_empty());
        let provider = |id: &str, models: &[&str]| Provider {
            id: id.into(),
            name: id.to_uppercase(),
            models: models.iter().map(|model| (*model).to_owned()).collect(),
        };
        assert_eq!(
            thread.apply(Update::Providers(vec![provider("a", &["a1", "a2"])])),
            [Change::Meta]
        );
        thread.apply(Update::Providers(vec![provider("b", &["b1"])]));
        assert_eq!(thread.providers, [provider("b", &["b1"])]);
    }

    #[test]
    fn a_patch_changes_only_what_it_names_and_may_arrive_before_its_call() {
        let mut thread = Thread::new();
        thread.apply(Update::Tool(tool("t1", ToolStatus::Pending)));
        let changes = thread.apply(Update::ToolPatch(ToolPatch {
            id: "t1".into(),
            status: Some(ToolStatus::Done),
            content: Some(vec![ToolContent::Text("ok".into())]),
            ..ToolPatch::default()
        }));
        assert_eq!(changes, [Change::Updated(0)]);
        let Entry::Tool(call) = &thread.entries()[0] else {
            panic!("a tool entry");
        };
        assert_eq!(
            (call.title.as_str(), call.status, call.content.len()),
            ("Run t1", ToolStatus::Done, 1)
        );

        let changes = thread.apply(Update::ToolPatch(ToolPatch {
            id: "t2".into(),
            title: Some("Read x".into()),
            ..ToolPatch::default()
        }));
        assert_eq!(changes, [Change::Appended(1)]);
    }

    #[test]
    fn a_permission_blocks_until_answered_and_a_finished_turn_clears_it() {
        let mut thread = Thread::new();
        thread.apply(ready());
        thread.apply(Update::TurnStarted);
        let request = PermissionRequest {
            questions: Vec::new(),
            id: 7,
            tool: tool("t1", ToolStatus::Running),
            options: vec![PermissionOption {
                id: "allow".into(),
                label: "Allow".into(),
                kind: PermissionKind::AllowOnce,
            }],
        };
        thread.apply(Update::Permission(request.clone()));
        assert_eq!(thread.state(), &ThreadState::Blocked);
        assert_eq!(thread.permissions()[0].entry, 0);
        assert_eq!(thread.answered(8), None);
        assert_eq!(thread.answered(7), Some(Change::Meta));
        assert_eq!(thread.state(), &ThreadState::Working);

        thread.apply(Update::Permission(request));
        let changes = thread.apply(Update::TurnEnded(Stop::Cancelled));
        assert!(thread.permissions().is_empty());
        assert_eq!(thread.state(), &ThreadState::Idle);
        assert!(changes.contains(&Change::Updated(0)));
        let Entry::Tool(call) = &thread.entries()[0] else {
            panic!("a tool entry");
        };
        assert_eq!(call.status, ToolStatus::Cancelled);
    }

    #[test]
    fn a_plan_is_one_entry_per_turn_and_a_failed_turn_says_why() {
        let mut thread = Thread::new();
        thread.apply(Update::TurnStarted);
        let step = |text: &str| PlanStep {
            text: text.into(),
            status: crate::PlanStatus::Pending,
        };
        thread.apply(Update::Plan(vec![step("a")]));
        assert_eq!(
            thread.apply(Update::Plan(vec![step("a"), step("b")])),
            [Change::Updated(0)]
        );
        thread.apply(Update::TurnEnded(Stop::Failed("rate limited".into())));
        assert_eq!(
            thread.entries().last(),
            Some(&Entry::Notice(Level::Error, "rate limited".into()))
        );
        thread.apply(Update::TurnStarted);
        assert_eq!(
            thread.apply(Update::Plan(vec![step("c")])),
            [Change::Appended(2)]
        );
        thread.apply(Update::Closed(Some("exited".into())));
        assert_eq!(thread.state(), &ThreadState::Closed(Some("exited".into())));
        thread.apply(Update::TurnEnded(Stop::Cancelled));
        assert_eq!(thread.state(), &ThreadState::Closed(Some("exited".into())));
    }

    fn rewinds() -> Update {
        Update::Offers(Offers {
            rewind: true,
            ..Offers::default()
        })
    }

    fn text(text: &str) -> Vec<Block> {
        vec![Block::Text(text.into())]
    }

    /// A conversation of a replayed prompt, its answer and a call, then a
    /// prompt of the interface's and its answer.
    fn conversation() -> Thread {
        let mut thread = Thread::new();
        thread.apply(ready());
        thread.apply(rewinds());
        thread.apply(Update::UserChunk(Block::Text("first".into())));
        thread.apply(Update::PromptId("p1".into()));
        thread.apply(Update::TurnStarted);
        thread.apply(Update::AssistantChunk("one".into()));
        thread.apply(Update::Tool(tool("t1", ToolStatus::Done)));
        thread.apply(Update::TurnEnded(Stop::Finished));
        thread.push_user(text("second"));
        thread.apply(Update::PromptTaken(Some("p2".into())));
        thread.apply(Update::TurnStarted);
        thread.apply(Update::AssistantChunk("two".into()));
        thread.apply(Update::TurnEnded(Stop::Finished));
        thread
    }

    #[test]
    fn prompts_are_named_as_the_backend_takes_them_and_shows_them() {
        let thread = conversation();
        assert_eq!(thread.prompt_id(0), Some("p1"));
        assert_eq!(thread.prompt_id(3), Some("p2"));
        assert_eq!(thread.prompt_id(1), None, "an answer is no prompt");

        // A prompt the backend did not take as one stays unnamed, and the
        // names after it go to their own prompts.
        let mut thread = conversation();
        thread.push_user(text("/model x"));
        thread.push_user(text("third"));
        thread.apply(Update::PromptTaken(None));
        thread.apply(Update::PromptTaken(Some("p3".into())));
        assert_eq!(
            (thread.prompt_id(5), thread.prompt_id(6)),
            (None, Some("p3"))
        );

        // Two messages shown as one entry are known by the first.
        let mut thread = Thread::new();
        thread.apply(rewinds());
        thread.apply(Update::UserChunk(Block::Text("a".into())));
        thread.apply(Update::PromptId("a".into()));
        thread.apply(Update::UserChunk(Block::Text("\n\nb".into())));
        thread.apply(Update::PromptId("b".into()));
        assert_eq!(thread.entries().len(), 1);
        assert_eq!(thread.prompt_id(0), Some("a"));

        // A backend that does not rewind names nothing, and the prompts sent
        // before it said so wait for no name.
        let mut thread = Thread::new();
        thread.push_user(text("early"));
        thread.apply(Update::Offers(Offers::default()));
        thread.push_user(text("later"));
        thread.apply(Update::PromptTaken(Some("stray".into())));
        assert_eq!((thread.prompt_id(0), thread.prompt_id(1)), (None, None));
    }

    #[test]
    fn a_rewind_replaces_the_prompt_and_what_followed_until_the_backend_says_how_it_went() {
        let mut thread = conversation();
        let before = thread.entries().to_vec();
        let rewinding = thread.rewind(0, text("first, edited")).expect("a prompt");
        assert_eq!(rewinding.prompt.as_deref(), Some("p1"));
        assert_eq!(
            rewinding.changes,
            [Change::Truncated(0), Change::Appended(0)]
        );
        assert_eq!(thread.entries(), [Entry::User(text("first, edited"))]);
        assert_eq!(thread.rewinding(), Some(0));
        assert_eq!(thread.prompt_id(0), None, "not named until it runs");
        // One rewind at a time, and only of a prompt.
        assert_eq!(thread.rewind(0, text("again")), None);

        // It runs: the edited prompt is named and the turn goes on as any.
        assert_eq!(
            thread.apply(Update::Rewound(Ok("p1b".into()))),
            [Change::Meta]
        );
        assert_eq!(thread.rewinding(), None);
        assert_eq!(thread.prompt_id(0), Some("p1b"));
        thread.apply(Update::TurnStarted);
        thread.apply(Update::Tool(tool("t1", ToolStatus::Running)));
        assert_eq!(
            thread.entries(),
            [
                Entry::User(text("first, edited")),
                Entry::Tool(tool("t1", ToolStatus::Running)),
            ],
            "a call of the old run is not the new one's"
        );
        assert_ne!(thread.entries(), before.as_slice());
    }

    #[test]
    fn a_failed_rewind_puts_back_what_it_replaced() {
        let mut thread = conversation();
        let before = thread.entries().to_vec();
        thread.rewind(3, text("second, edited")).expect("a prompt");
        assert_eq!(thread.entries().len(), 4);
        // Something said meanwhile stays, after what comes back.
        thread.apply(Update::Notice(Level::Warning, "slow disk".into()));
        let changes = thread.apply(Update::Rewound(Err("the prompt is gone".into())));
        assert_eq!(
            changes,
            [
                Change::Truncated(3),
                Change::Appended(3),
                Change::Appended(4),
                Change::Appended(5),
                Change::Meta,
            ]
        );
        let mut expected = before.clone();
        expected.push(Entry::Notice(Level::Warning, "slow disk".into()));
        assert_eq!(thread.entries(), expected.as_slice());
        assert_eq!(thread.rewinding(), None);
        assert_eq!(thread.prompt_id(3), Some("p2"), "its name came back too");
        // The tool call is found again where it was.
        assert_eq!(
            thread.apply(Update::ToolPatch(ToolPatch {
                id: "t1".into(),
                title: Some("Read".into()),
                ..ToolPatch::default()
            })),
            [Change::Updated(2)]
        );
        // A word on a rewind nobody asked for changes nothing.
        assert!(thread.apply(Update::Rewound(Ok("x".into()))).is_empty());
    }

    #[test]
    fn a_rewind_goes_back_before_the_first_prompt_that_reached_the_session() {
        let mut thread = conversation();
        // The last prompt never reached the session: its run did not start.
        thread.push_user(text("third"));
        thread.apply(Update::PromptTaken(None));
        thread.apply(Update::TurnEnded(Stop::Failed("no runner".into())));
        let rewinding = thread.rewind(5, text("third, again")).expect("a prompt");
        assert_eq!(rewinding.prompt, None, "nothing of the session goes");
        thread.apply(Update::Rewound(Err("busy".into())));
        // From an earlier prompt, the first one named after it.
        let mut thread = conversation();
        thread.push_user(text("unnamed"));
        let at = thread.entries().len() - 1;
        let rewinding = thread.rewind(at, text("x")).expect("a prompt");
        assert_eq!(rewinding.prompt, None);
        // Its name, when it comes, names nothing.
        thread.apply(Update::PromptTaken(Some("late".into())));
        assert_eq!(thread.prompt_id(at), None);
    }
    #[test]
    fn a_request_taken_back_unblocks_as_an_answered_one_does() {
        use crate::{Question, QuestionOption};
        let mut thread = Thread::default();
        thread.apply(Update::TurnStarted);
        let asks = |id: u64| {
            Update::Permission(PermissionRequest {
                id,
                tool: tool(&format!("q{id}"), ToolStatus::Pending),
                options: Vec::new(),
                questions: vec![Question {
                    header: Some("Store".into()),
                    text: "Which store should it use?".into(),
                    options: vec![
                        QuestionOption {
                            label: "SQLite".into(),
                            description: None,
                        },
                        QuestionOption {
                            label: "Postgres".into(),
                            description: Some("Needs a server".into()),
                        },
                    ],
                    multiple: false,
                    free: true,
                }],
            })
        };
        thread.apply(asks(1));
        thread.apply(asks(2));
        assert_eq!(thread.state(), &ThreadState::Blocked);
        // What a call asks is kept with the request, for the interface.
        assert_eq!(thread.permissions()[0].questions[0].options.len(), 2);
        // One taken back leaves the other open; the last one unblocks.
        assert_eq!(thread.apply(Update::Withdrawn(1)), [Change::Meta]);
        assert_eq!(thread.state(), &ThreadState::Blocked);
        assert_eq!(thread.permissions().len(), 1);
        thread.apply(Update::Withdrawn(2));
        assert_eq!(thread.state(), &ThreadState::Working);
        // One that is not open changes nothing.
        assert!(thread.apply(Update::Withdrawn(9)).is_empty());
    }

    #[test]
    fn the_levels_of_effort_on_offer_replace_those_said_before() {
        let level = |id: &str| Choice {
            id: id.into(),
            name: id.into(),
            description: None,
        };
        let mut thread = Thread::default();
        assert!(thread.efforts.is_empty() && thread.effort.is_none());
        thread.apply(Update::Efforts {
            levels: vec![level("low"), level("high")],
            current: Some("high".into()),
        });
        assert_eq!(thread.efforts.len(), 2);
        assert_eq!(thread.effort.as_deref(), Some("high"));
        // A model with no such choice offers none.
        thread.apply(Update::Efforts {
            levels: Vec::new(),
            current: None,
        });
        assert!(thread.efforts.is_empty() && thread.effort.is_none());
    }

    #[test]
    fn a_question_asked_while_no_turn_runs_leaves_the_thread_idle_once_answered() {
        let mut thread = Thread::default();
        let ask = |id: u64| {
            Update::Permission(PermissionRequest {
                questions: Vec::new(),
                id,
                tool: tool("p1", ToolStatus::Pending),
                options: Vec::new(),
            })
        };
        // A turn has been and gone. No turn is open: answered, there is
        // none to wait the end of.
        thread.apply(Update::TurnStarted);
        thread.apply(Update::TurnEnded(Stop::Finished));
        assert_eq!(thread.state(), &ThreadState::Idle);
        thread.apply(ask(1));
        assert_eq!(thread.state(), &ThreadState::Blocked);
        thread.answered(1);
        assert_eq!(thread.state(), &ThreadState::Idle);
        // In a turn, the answer lets the turn go on.
        thread.apply(Update::TurnStarted);
        thread.apply(ask(2));
        thread.answered(2);
        assert_eq!(thread.state(), &ThreadState::Working);
    }

    #[test]
    fn what_an_agent_left_running_is_over_when_the_agent_is_gone() {
        let mut thread = Thread::default();
        thread.apply(Update::TurnStarted);
        let mut call = tool("b1", ToolStatus::Done);
        call.work.background = true;
        thread.apply(Update::Tool(call));
        thread.apply(Update::TurnEnded(Stop::Finished));
        let running = |thread: &Thread| match &thread.entries()[0] {
            Entry::Tool(call) => call.work.background && !call.work.ended,
            _ => false,
        };
        assert!(running(&thread));
        let changes = thread.apply(Update::Closed(Some("gone".into())));
        assert!(!running(&thread));
        assert!(changes.contains(&Change::Updated(0)), "{changes:?}");
    }
    #[test]
    fn what_a_subagent_does_does_not_cut_the_agent_s_message_in_two() {
        let mut thread = Thread::default();
        thread.apply(Update::TurnStarted);
        let launch = ToolCall {
            agent: Some("Explore".into()),
            ..tool("a1", ToolStatus::Done)
        };
        thread.apply(Update::Tool(launch));
        thread.apply(Update::AssistantChunk("The first one made the JSON".into()));
        // While the answer is written, the subagent makes a call and says
        // something, and a call of its is reported on.
        let step = ToolCall {
            parent: Some("a1".into()),
            ..tool("s1", ToolStatus::Running)
        };
        thread.apply(Update::Tool(step.clone()));
        thread.apply(Update::AgentSaid {
            call: "a1".into(),
            text: "Reading.".into(),
        });
        thread.apply(Update::Tool(ToolCall {
            status: ToolStatus::Done,
            ..step
        }));
        let changes = thread.apply(Update::AssistantChunk(" protocol.".into()));
        // The same message goes on, where it stands.
        assert_eq!(changes, [Change::Updated(1)]);
        let said: Vec<&str> = thread
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                Entry::Assistant(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(said, ["The first one made the JSON protocol."]);
        // A call of the agent's own is a break, as it was.
        thread.apply(Update::Tool(tool("t1", ToolStatus::Done)));
        thread.apply(Update::AssistantChunk("Next.".into()));
        assert_eq!(
            thread
                .entries()
                .iter()
                .filter(|entry| matches!(entry, Entry::Assistant(_)))
                .count(),
            2
        );
    }
}
