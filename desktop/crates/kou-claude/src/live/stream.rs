//! An answer as Claude Code streams it (`stream_event`, asked for with
//! `--include-partial-messages`), and the whole `assistant` messages that
//! follow each part of it: what of those was said already.
//!
//! Each block of an answer streams as `content_block_start`, deltas and
//! `content_block_stop`; then the same block comes again whole, in an
//! `assistant` message of the same `message.id`. Text that streamed is said
//! as it comes; the whole message then adds only what did not stream.

use std::collections::HashMap;

/// The kinds of text an answer's blocks hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// What the model answers.
    Text,
    /// What it thinks first.
    Thinking,
}

/// What a block is while it streams.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Open {
    /// Text of this kind.
    Text(Kind),
    /// The input of the tool call of this id.
    Tool(String),
    /// Anything else, which nothing here shows.
    Other,
}

/// Names a block of an answer, so that two blocks of the same kind one
/// after the other can be set apart.
pub(crate) type Key = (String, u64);

/// One answer's stream: Claude Code's own, or one subagent's.
#[derive(Debug, Default)]
pub(crate) struct Stream {
    /// The message streaming now.
    message: String,
    /// A message's start was seen. (A session found again comes in the
    /// middle of one: what of it was written before is not known, and its
    /// blocks are shown whole when its whole message comes.)
    started: bool,
    /// Its blocks, by their index.
    open: HashMap<u64, Open>,
    /// Text that streamed and that its whole message has not said yet.
    streamed: Vec<Streamed>,
}

#[derive(Debug)]
struct Streamed {
    message: String,
    index: u64,
    kind: Kind,
    text: String,
}

/// More streamed than this and never said again whole is dropped.
const KEPT: usize = 64;

impl Stream {
    /// A message begins to stream.
    pub fn start_message(&mut self, id: &str) {
        self.message = id.to_owned();
        self.started = true;
        self.open.clear();
    }

    /// Whether the start of the message streaming now was seen.
    pub fn started(&self) -> bool {
        self.started
    }

    /// A block of the streaming message begins.
    pub fn start_block(&mut self, index: u64, open: Open) {
        self.open.insert(index, open);
    }

    /// What block `index` of the streaming message is.
    pub fn block(&self, index: u64) -> Option<&Open> {
        self.open.get(&index)
    }

    /// Block `index` of the streaming message is over.
    pub fn stop_block(&mut self, index: u64) -> Option<Open> {
        self.open.remove(&index)
    }

    /// Text streamed into block `index`: kept until its whole message comes,
    /// and the key of the block it is part of.
    pub fn streamed(&mut self, index: u64, kind: Kind, text: &str) -> Key {
        let key = (self.message.clone(), index);
        match self
            .streamed
            .iter_mut()
            .find(|seen| seen.message == key.0 && seen.index == index && seen.kind == kind)
        {
            Some(seen) => seen.text.push_str(text),
            None => {
                if self.streamed.len() == KEPT {
                    self.streamed.remove(0);
                }
                self.streamed.push(Streamed {
                    message: key.0.clone(),
                    index,
                    kind,
                    text: text.to_owned(),
                });
            }
        }
        key
    }

    /// What of `text`, a block of kind `kind` of the whole message `message`,
    /// did not stream, and the key of the streamed block it goes on, if it
    /// goes on one.
    pub fn remainder(&mut self, message: &str, kind: Kind, text: &str) -> (String, Option<Key>) {
        let found = self.streamed.iter().position(|seen| {
            seen.message == message
                && seen.kind == kind
                && !seen.text.is_empty()
                && text.starts_with(&seen.text)
        });
        match found {
            Some(at) => {
                let seen = self.streamed.remove(at);
                (
                    text[seen.text.len()..].to_owned(),
                    Some((seen.message, seen.index)),
                )
            }
            None => (text.to_owned(), None),
        }
    }

    /// Nothing of what streamed is awaited any more.
    pub fn clear(&mut self) {
        self.open.clear();
        self.streamed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_message_adds_only_what_did_not_stream() {
        let mut stream = Stream::default();
        stream.start_message("m1");
        stream.start_block(0, Open::Text(Kind::Thinking));
        assert_eq!(
            stream.streamed(0, Kind::Thinking, "Let me "),
            ("m1".into(), 0)
        );
        stream.streamed(0, Kind::Thinking, "think.");
        stream.start_block(1, Open::Text(Kind::Text));
        stream.streamed(1, Kind::Text, "Hel");
        // The whole thinking block, then the text block with more than streamed.
        assert_eq!(
            stream.remainder("m1", Kind::Thinking, "Let me think."),
            (String::new(), Some(("m1".into(), 0)))
        );
        assert_eq!(
            stream.remainder("m1", Kind::Text, "Hello."),
            ("lo.".into(), Some(("m1".into(), 1)))
        );
        // Said once only, and nothing of another message is taken for it.
        assert_eq!(
            stream.remainder("m1", Kind::Text, "Hello."),
            ("Hello.".into(), None)
        );
        stream.streamed(1, Kind::Text, "x");
        assert_eq!(stream.remainder("m2", Kind::Text, "x"), ("x".into(), None));
    }
}
