//! Turns the folded transcript into the updates a thread takes: a stored
//! session as its final state, a live run as it changes.
//!
//! A thread grows the entry that is open when a chunk of the same kind
//! arrives, so the emitter mirrors which entry is open and separates two
//! messages that would otherwise run together.

use crate::{
    present::present,
    transcript::{Body, Cut, Event, Transcript, UserMessage},
};
use base64::Engine;
use kou_thread::{Block, Level, Stop, ToolCall, ToolPatch, Update, Usage};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

/// The chunk a thread would append to, as far as the backend can tell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Open {
    #[default]
    Nothing,
    User,
    Assistant,
    Thought,
}

/// What the thread was told so far.
#[derive(Debug, Default)]
pub(crate) struct Emitter {
    cwd: PathBuf,
    /// Entries that reached the thread, by index.
    shown: Vec<bool>,
    /// Tool calls as the thread has them; `None` once a call is settled.
    tools: HashMap<usize, Option<ToolCall>>,
    open: Open,
    /// Whether a turn is open in the thread.
    pub turn_open: bool,
    /// Message IDs of prompts the interface shows itself when it sends them.
    own: HashSet<String>,
    /// The context window of the model in force, when known.
    pub context_limit: Option<u64>,
    /// The totals the thread has.
    told_usage: Option<Usage>,
    /// The IDs the thread knows its prompts by, in the order it was told
    /// them: the interface's as the session took them, the others as they
    /// were shown.
    pub named: Vec<String>,
}

impl Emitter {
    /// An emitter for a session of the project at `cwd`.
    pub(crate) fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_owned(),
            ..Self::default()
        }
    }

    /// Notes the ID the thread knows a prompt of the interface's by.
    pub(crate) fn taken(&mut self, message_id: &str) {
        self.named.push(message_id.to_owned());
    }

    /// The session was rewound to `transcript`, the beginning of the one the
    /// thread was told, and the thread let go of its prompts from `prompt`
    /// on: what the thread has is all told, and what is left open of it will
    /// not change.
    pub(crate) fn rewound(&mut self, transcript: &mut Transcript, prompt: Option<&str>) {
        self.shown = vec![true; transcript.entries.len()];
        self.tools = (0..transcript.entries.len())
            .filter(|index| matches!(transcript.entries[*index].body, Body::Tool(_)))
            .map(|index| (index, None))
            .collect();
        for entry in &mut transcript.entries {
            if let Body::Tool(tool) = &mut entry.body {
                tool.release();
            }
        }
        self.open = Open::Nothing;
        self.turn_open = false;
        self.let_go(prompt);
    }

    /// The thread let go of its prompts from `prompt` on.
    pub(crate) fn let_go(&mut self, prompt: Option<&str>) {
        if let Some(at) = prompt.and_then(|prompt| self.named.iter().position(|id| id == prompt)) {
            self.named.truncate(at);
        }
    }

    /// Notes a prompt the interface already shows: its record is not echoed.
    pub(crate) fn sent(&mut self, message_id: &str) {
        self.own.insert(message_id.to_owned());
        // The interface closed whatever chunk was open when it showed it.
        self.open = Open::Nothing;
    }

    /// Emits a stored conversation as its final state, with a turn around
    /// what each prompt set off. `live` leaves the last turn open: a run
    /// still goes on. Returns whether the last run did not finish.
    pub(crate) fn replay(
        &mut self,
        transcript: &mut Transcript,
        live: bool,
        sink: &mut dyn FnMut(Update),
    ) -> bool {
        // Calls an earlier run left open say nothing of the last one.
        transcript.settle_abandoned(false);
        let interrupted = transcript.interrupted();
        if !live {
            transcript.settle_abandoned(true);
        }
        for index in 0..transcript.entries.len() {
            match &transcript.entries[index].body {
                Body::User(message) if !message.forced => self.end_turn(Stop::Finished, sink),
                Body::Assistant { .. } | Body::Thought { .. } | Body::Tool(_) => {
                    self.start_turn(sink);
                }
                _ => {}
            }
            self.show(transcript, index, sink);
        }
        if live {
            self.start_turn(sink);
            // What streamed of a response that is not recorded yet.
            let streaming: Vec<(bool, String)> = transcript
                .streaming()
                .into_iter()
                .map(|(thought, text)| (thought, text.to_owned()))
                .collect();
            for (thought, text) in streaming {
                self.chunk(thought, text, true, sink);
            }
        } else {
            let stop = match transcript.last_cut {
                _ if interrupted => Stop::Cancelled,
                Some(Cut::Limit) => Stop::Limit,
                Some(Cut::Refused) => Stop::Refused,
                None => Stop::Finished,
            };
            self.end_turn(stop, sink);
        }
        interrupted
    }

    /// Emits what one event of a live run changed. Events that are not about
    /// the conversation are the caller's.
    pub(crate) fn event(
        &mut self,
        transcript: &mut Transcript,
        event: Event,
        sink: &mut dyn FnMut(Update),
    ) {
        match event {
            Event::Entry(index) => self.show(transcript, index, sink),
            Event::Delta {
                thought,
                text,
                starts_item,
            } => self.chunk(thought, text, starts_item, sink),
            Event::Reset => self.notice(
                Level::Warning,
                "The response started over: what it wrote above is void.",
                sink,
            ),
            Event::Turn { compaction, .. } => {
                if compaction {
                    self.notice(Level::Info, "Compacting context", sink);
                }
            }
            Event::Usage => self.tell_usage(transcript, sink),
            Event::Error(_) | Event::Exit(_) => {}
        }
    }

    /// Tells the thread the totals, when they are not the ones it has.
    pub(crate) fn tell_usage(&mut self, transcript: &Transcript, sink: &mut dyn FnMut(Update)) {
        let totals = transcript.usage;
        let usage = Usage {
            context_used: Some(totals.context),
            context_limit: self.context_limit,
            input_tokens: Some(totals.input),
            output_tokens: Some(totals.output),
            cost_usd: totals.cost,
        };
        if self.told_usage != Some(usage) {
            self.told_usage = Some(usage);
            sink(Update::Usage(usage));
        }
    }

    /// Says something about the session itself.
    pub(crate) fn notice(&mut self, level: Level, text: &str, sink: &mut dyn FnMut(Update)) {
        self.open = Open::Nothing;
        sink(Update::Notice(level, text.to_owned()));
    }

    /// Opens a turn in the thread, unless one is open.
    pub(crate) fn start_turn(&mut self, sink: &mut dyn FnMut(Update)) {
        if !self.turn_open {
            self.turn_open = true;
            self.open = Open::Nothing;
            sink(Update::TurnStarted);
        }
    }

    /// Closes the thread's turn, if one is open.
    pub(crate) fn end_turn(&mut self, stop: Stop, sink: &mut dyn FnMut(Update)) {
        if self.turn_open {
            self.turn_open = false;
            self.open = Open::Nothing;
            sink(Update::TurnEnded(stop));
        }
    }

    /// Tells the thread of the calls a finished run left open: the thread
    /// marks them cancelled itself, so only the emitter's copy changes.
    pub(crate) fn settle(&mut self, transcript: &mut Transcript, indices: &[usize]) {
        for &index in indices {
            self.tools.insert(index, None);
            if let Body::Tool(tool) = &mut transcript.entries[index].body {
                tool.release();
            }
        }
    }

    fn chunk(
        &mut self,
        thought: bool,
        mut text: String,
        starts_item: bool,
        sink: &mut dyn FnMut(Update),
    ) {
        let kind = if thought {
            Open::Thought
        } else {
            Open::Assistant
        };
        // The thread appends to the open entry of the same kind: another
        // message needs a paragraph of its own.
        if starts_item && self.open == kind {
            text.insert_str(0, "\n\n");
        }
        self.open = kind;
        sink(if thought {
            Update::ThoughtChunk(text)
        } else {
            Update::AssistantChunk(text)
        });
    }

    fn show(&mut self, transcript: &mut Transcript, index: usize, sink: &mut dyn FnMut(Update)) {
        if self.shown.len() <= index {
            self.shown.resize(index + 1, false);
        }
        let first = !std::mem::replace(&mut self.shown[index], true);
        match &mut transcript.entries[index].body {
            Body::User(message) => {
                if first && !self.own.contains(&message.message_id) {
                    self.user(message, sink);
                }
            }
            Body::Assistant { text, streamed } => {
                if first && !*streamed {
                    self.chunk(false, std::mem::take(text), true, sink);
                }
            }
            Body::Thought { text, streamed } => {
                if first && !*streamed {
                    self.chunk(true, std::mem::take(text), true, sink);
                }
            }
            Body::Notice { level, text } => {
                if first {
                    let (level, text) = (*level, text.clone());
                    self.notice(level, &text, sink);
                }
            }
            Body::Tool(tool) => {
                let call = present(tool, &self.cwd);
                let settled = tool.state.terminal();
                match self.tools.get(&index) {
                    // A settled call does not change any more.
                    Some(None) => return,
                    Some(Some(known)) => {
                        if let Some(patch) = patch_between(known, &call) {
                            sink(Update::ToolPatch(patch));
                        }
                    }
                    None => {
                        self.open = Open::Nothing;
                        sink(Update::Tool(call.clone()));
                    }
                }
                if settled {
                    tool.release();
                    self.tools.insert(index, None);
                } else {
                    self.tools.insert(index, Some(call));
                }
            }
        }
    }

    fn user(&mut self, message: &UserMessage, sink: &mut dyn FnMut(Update)) {
        let mut text = message.text.clone();
        if self.open == Open::User {
            text.insert_str(0, "\n\n");
        }
        self.open = Open::User;
        // An image stands where its label is in the text, as the interface
        // sends it; the label is the image's place, not more text.
        let mut rest = text.as_str();
        let mut unplaced = Vec::new();
        for (label, url) in &message.images {
            let image = decode_data_url(url);
            match (rest.find(label.as_str()), image) {
                (Some(at), Some((mime, data))) => {
                    if at > 0 {
                        sink(Update::UserChunk(Block::Text(rest[..at].to_owned())));
                    }
                    sink(Update::UserChunk(Block::Image {
                        mime,
                        data: data.into(),
                    }));
                    rest = &rest[at + label.len()..];
                }
                (_, image) => unplaced.push((label, url, image)),
            }
        }
        if !rest.is_empty() {
            sink(Update::UserChunk(Block::Text(rest.to_owned())));
        }
        for (label, url, image) in unplaced {
            match image {
                Some((mime, data)) => sink(Update::UserChunk(Block::Image {
                    mime,
                    data: data.into(),
                })),
                // An image the session knows by its address only.
                None => sink(Update::UserChunk(Block::Text(format!("\n{label}: {url}")))),
            }
        }
        for (path, content) in &message.files {
            let path = kou_thread::files::link_target(&self.cwd, path);
            sink(Update::UserChunk(Block::File {
                path,
                text: Some(content.clone()).filter(|content| !content.is_empty()),
            }));
        }
        // What an edit of it names it by.
        if !message.message_id.is_empty() {
            self.named.push(message.message_id.clone());
            sink(Update::PromptId(message.message_id.clone()));
        }
    }
}

/// The fields of a call that changed since the thread last heard of it.
fn patch_between(known: &ToolCall, now: &ToolCall) -> Option<ToolPatch> {
    if known == now {
        return None;
    }
    fn changed<T: PartialEq + Clone>(known: &T, now: &T) -> Option<T> {
        (known != now).then(|| now.clone())
    }
    Some(ToolPatch {
        id: now.id.clone(),
        title: changed(&known.title, &now.title),
        kind: changed(&known.kind, &now.kind),
        status: changed(&known.status, &now.status),
        content: changed(&known.content, &now.content),
        locations: changed(&known.locations, &now.locations),
        raw_input: changed(&known.raw_input, &now.raw_input).flatten(),
        raw_output: changed(&known.raw_output, &now.raw_output).flatten(),
        agent: changed(&known.agent, &now.agent).flatten(),
        parent: changed(&known.parent, &now.parent).flatten(),
        background: changed(&known.work.background, &now.work.background),
        ended: changed(&known.work.ended, &now.work.ended),
        tokens: changed(&known.work.tokens, &now.work.tokens).flatten(),
    })
}

/// The media type and bytes of a `data:` URL in base64.
fn decode_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let (header, data) = url.strip_prefix("data:")?.split_once(',')?;
    let (mime, encoding) = header.split_once(';')?;
    if encoding != "base64" {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some((mime.to_owned(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kou_thread::{Entry, Thread, ThreadState, ToolStatus};

    fn item(sequence: u64, kind: &str, data: &str) -> String {
        format!(r#"{{"Sequence":{sequence},"Kind":"{kind}","Data":{data}}}"#)
    }

    fn stored() -> Vec<String> {
        vec![
            item(
                1,
                "input",
                r#"{"ID":"m1","Kind":"external","Payload":"first"}"#,
            ),
            item(2, "turn", r#"{"ID":"t1","Type":"regular","Model":"m"}"#),
            item(
                3,
                "model_response",
                r#"{"TurnID":"t1","Response":{"Stop":"complete","Output":[{"Type":"message","Data":{"Role":"assistant","Text":"On it."}},{"Type":"message","Data":{"Role":"assistant","Text":"Running."}},{"Type":"tool_call","Data":{"CallID":"c1","Name":"Bash","Arguments":"{\"command\":\"sleep 100\"}"}}]}}"#,
            ),
            item(
                4,
                "tool_call_status",
                r#"{"CallID":"c1","Status":{"Error":""},"Operations":[{"ID":"op1","Type":"shell","Status":"awaiting","State":{"Input":{"Command":"sleep 100"}}}]}"#,
            ),
            item(
                5,
                "input",
                r#"{"ID":"m2","Kind":"external","Payload":"second"}"#,
            ),
            item(6, "turn", r#"{"ID":"t2","Type":"regular","Model":"m"}"#),
            item(
                7,
                "model_response",
                r#"{"TurnID":"t2","Response":{"Stop":"complete","Output":[{"Type":"message","Data":{"Role":"assistant","Text":"Done."}}]}}"#,
            ),
        ]
    }

    #[test]
    fn a_stored_session_replays_as_turns_with_calls_in_their_final_state() {
        let mut transcript = Transcript::new();
        for line in stored() {
            transcript.apply(&line);
        }
        let mut emitter = Emitter::new(Path::new("/w"));
        let mut updates = Vec::new();
        emitter.replay(&mut transcript, false, &mut |update| updates.push(update));

        let mut thread = Thread::new();
        for update in &updates {
            thread.apply(update.clone());
        }
        let kinds: Vec<&str> = updates
            .iter()
            .map(|update| match update {
                Update::UserChunk(_) => "user",
                Update::AssistantChunk(_) => "assistant",
                Update::Tool(_) => "tool",
                Update::TurnStarted => "start",
                Update::TurnEnded(_) => "end",
                // What an edit names each prompt by, after it.
                Update::PromptId(_) => "named",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user",
                "named",
                "start",
                "assistant",
                "assistant",
                "tool",
                "end",
                "user",
                "named",
                "start",
                "assistant",
                "end"
            ]
        );
        assert_eq!(thread.entries().len(), 5);
        assert_eq!(
            thread.entries()[1],
            Entry::Assistant("On it.\n\nRunning.".into()),
            "two messages in a row keep a paragraph each"
        );
        let Entry::Tool(call) = &thread.entries()[2] else {
            panic!("a tool entry");
        };
        assert_eq!(
            call.status,
            ToolStatus::Cancelled,
            "a call the next prompt's run left behind will never finish"
        );
        assert_eq!(thread.entries()[4], Entry::Assistant("Done.".into()));
        assert_eq!(thread.state(), &ThreadState::Idle);
        assert_eq!(thread.last_stop, Some(Stop::Finished));
    }

    #[test]
    fn a_run_joined_midway_keeps_its_turn_open_and_what_streamed() {
        let mut transcript = Transcript::new();
        for line in &stored()[..4] {
            transcript.apply(line);
        }
        transcript.apply(&item(
            5,
            "turn",
            r#"{"ID":"t2","Type":"regular","Model":"m"}"#,
        ));
        transcript
            .apply(r#"{"type":"delta","turn":"t2","index":0,"kind":"text","text":"Half an ans"}"#);
        let mut emitter = Emitter::new(Path::new("/w"));
        let mut thread = Thread::new();
        emitter.replay(&mut transcript, true, &mut |update| {
            thread.apply(update);
        });
        assert_eq!(thread.state(), &ThreadState::Working);
        assert_eq!(
            thread.entries().last(),
            Some(&Entry::Assistant("Half an ans".into()))
        );
        let Entry::Tool(call) = &thread.entries()[2] else {
            panic!("a tool entry");
        };
        assert_eq!(call.status, ToolStatus::Running);

        // The rest streams, the response is recorded, and the call finishes.
        let mut live = |line: &str, thread: &mut Thread| {
            for event in transcript.apply(line) {
                emitter.event(&mut transcript, event, &mut |update| {
                    thread.apply(update);
                });
            }
        };
        live(
            r#"{"type":"delta","turn":"t2","index":0,"kind":"text","text":"wer."}"#,
            &mut thread,
        );
        live(
            &item(
                6,
                "model_response",
                r#"{"TurnID":"t2","Response":{"Stop":"complete","Output":[{"Type":"message","Data":{"Role":"assistant","Text":"Half an answer."}}],"Usage":{"InputTokens":7,"OutputTokens":2}}}"#,
            ),
            &mut thread,
        );
        live(
            &item(
                7,
                "tool_call_status",
                r#"{"CallID":"c1","Status":{"Error":""},"Operations":[{"ID":"op1","Type":"shell","Status":"completed","State":{"Input":{"Command":"sleep 100"},"Result":{"Out":"","Err":"","ExitCode":0}}}]}"#,
            ),
            &mut thread,
        );
        assert_eq!(
            thread.entries().last(),
            Some(&Entry::Assistant("Half an answer.".into())),
            "the recorded text does not repeat what streamed"
        );
        let Entry::Tool(call) = &thread.entries()[2] else {
            panic!("a tool entry");
        };
        assert_eq!(call.status, ToolStatus::Done);
        assert_eq!(thread.usage.context_used, Some(7));
    }

    #[test]
    fn a_prompt_the_interface_sent_is_not_echoed_and_images_keep_their_bytes() {
        let mut transcript = Transcript::new();
        let mut emitter = Emitter::new(Path::new("/w"));
        emitter.sent("m1");
        let mut updates = Vec::new();
        let lines = [
            item(
                1,
                "input",
                r#"{"ID":"m1","Kind":"external","Payload":"mine"}"#,
            ),
            item(
                2,
                "input",
                r#"{"ID":"m2","Kind":"external","Payload":{"Text":"look [Image 1]","Images":[{"Label":"[Image 1]","URL":"data:image/png;base64,iVBORw=="}]}}"#,
            ),
        ];
        for line in lines {
            for event in transcript.apply(&line) {
                emitter.event(&mut transcript, event, &mut |update| updates.push(update));
            }
        }
        // The image stands where its label was.
        assert_eq!(
            updates,
            [
                Update::UserChunk(Block::Text("look ".into())),
                Update::UserChunk(Block::Image {
                    mime: "image/png".into(),
                    data: vec![0x89, b'P', b'N', b'G'].into(),
                }),
                Update::PromptId("m2".into()),
            ]
        );
        assert_eq!(emitter.named, ["m2"], "the interface names its own");
    }
}
