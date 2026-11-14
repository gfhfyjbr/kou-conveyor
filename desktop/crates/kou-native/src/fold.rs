//! What a run's output becomes, line by line: the transcript it adds up to,
//! the updates the thread takes for it, and how the run ended.
//!
//! This is the whole of the mapping from the runner's events to the thread's
//! updates, kept apart from the process that produces the lines, so that
//! recorded output maps the same way as a live run's.

use crate::{
    emit::Emitter,
    text::{clean, headline},
    transcript::{Body, Cut, Event, Transcript},
};
use kou_thread::{Level, Stop, Update};
use std::path::Path;

/// What is known of a run that goes on, for when it ends.
#[derive(Debug, Default)]
struct Ending {
    /// The user asked for the stop.
    cancelled: bool,
    /// What the runner said goes wrong.
    failure: Option<String>,
    /// The last line of its diagnostics that is no report of its own.
    diagnostic: String,
    /// The run compacts instead of answering a prompt.
    compaction: bool,
    /// Entries the transcript had when the run began.
    mark: usize,
}

/// A session's conversation as the runner tells it, and what the thread was
/// told of it.
#[derive(Debug)]
pub(crate) struct Fold {
    /// The conversation so far.
    pub transcript: Transcript,
    /// What the thread was told.
    pub emitter: Emitter,
    /// The model chosen for the session's next prompts.
    pub chosen: Option<String>,
    /// The model the thread was last told is in force.
    pub announced: Option<String>,
    /// Set while a run this session can stop goes on.
    ending: Option<Ending>,
}

impl Fold {
    /// The conversation of a session of the project at `cwd`, empty so far.
    pub(crate) fn new(cwd: &Path, chosen: Option<String>) -> Self {
        Self {
            transcript: Transcript::new(),
            emitter: Emitter::new(cwd),
            chosen,
            announced: None,
            ending: None,
        }
    }

    /// A run begins: the thread's turn opens.
    pub(crate) fn begin(&mut self, compaction: bool, sink: &mut dyn FnMut(Update)) {
        self.ending = Some(Ending {
            compaction,
            mark: self.transcript.entries.len(),
            ..Ending::default()
        });
        self.emitter.start_turn(sink);
    }

    /// A run that was going on already is taken up: its turn is open.
    pub(crate) fn join(&mut self) {
        self.ending = Some(Ending::default());
    }

    /// Notes that the user asked for the run's stop. `false` when it was
    /// asked for already, or no run goes on.
    pub(crate) fn cancel(&mut self) -> bool {
        match &mut self.ending {
            Some(ending) if !ending.cancelled => {
                ending.cancelled = true;
                true
            }
            _ => false,
        }
    }

    /// Folds one line of a run's output in and tells the thread. Returns the
    /// exit status a detached runner's last line states.
    pub(crate) fn line(
        &mut self,
        text: &str,
        stderr: bool,
        sink: &mut dyn FnMut(Update),
    ) -> Option<i32> {
        if stderr {
            self.diagnostic(text, sink);
            return None;
        }
        let mut exit = None;
        for event in self.transcript.apply(text) {
            match event {
                Event::Error(message) => {
                    if let Some(ending) = &mut self.ending {
                        ending.failure = Some(message);
                    }
                }
                Event::Exit(code) => exit = Some(code),
                Event::Entry(index) if self.own_stop(index) => {}
                event => {
                    if let Event::Turn { model, compaction } = &event
                        && !compaction
                        && !model.is_empty()
                        && self.chosen.is_none()
                        && self.announced.as_deref() != Some(model)
                    {
                        // A session without a choice goes on with the model
                        // its last prompt ran with.
                        self.announced = Some(model.clone());
                        sink(Update::Model(model.clone()));
                    }
                    self.emitter.event(&mut self.transcript, event, sink);
                }
            }
        }
        exit
    }

    /// Whether the entry is the notice of a stop this session asked for,
    /// which the end of the turn says already.
    fn own_stop(&self, index: usize) -> bool {
        self.ending.as_ref().is_some_and(|ending| ending.cancelled)
            && matches!(
                &self.transcript.entries[index].body,
                Body::Notice { text, .. } if text.starts_with("Run stopped: ")
            )
    }

    /// What the runner says beside its events: retries and warnings are for
    /// the user, the rest is kept for when the run fails.
    fn diagnostic(&mut self, line: &str, sink: &mut dyn FnMut(Update)) {
        let line = clean(line);
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if let Some(retry) = parse_retry(line) {
            self.emitter.notice(Level::Warning, &retry, sink);
        } else if let Some(warning) = line.strip_prefix("warn> ") {
            self.emitter.notice(Level::Warning, warning, sink);
        } else if [
            "plugin error> ",
            "skill error> ",
            "steering message rejected: ",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
        {
            self.emitter.notice(Level::Warning, line, sink);
        } else if ["plugin> ", "skill> ", "verify> ", "compaction> "]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        {
            self.emitter.notice(Level::Info, line, sink);
        } else {
            log::debug!("runner: {line}");
            if let Some(ending) = &mut self.ending {
                line.clone_into(&mut ending.diagnostic);
            }
        }
    }

    /// The run ended with this exit status (`None` when nothing says how):
    /// what it left open is settled and the thread's turn closes. Returns how
    /// the turn ended.
    pub(crate) fn finish(&mut self, code: Option<i32>, sink: &mut dyn FnMut(Update)) -> Stop {
        let ending = self.ending.take().unwrap_or_default();
        let stop = if ending.cancelled || code == Some(130) {
            Stop::Cancelled
        } else if code == Some(0) {
            match self.transcript.last_cut {
                Some(Cut::Limit) => Stop::Limit,
                Some(Cut::Refused) => Stop::Refused,
                None => Stop::Finished,
            }
        } else {
            let reason = ending.failure.unwrap_or_else(|| {
                let how = match code {
                    Some(code) => format!("Runner exited: exit status {code}"),
                    None => "Runner exited without saying how it ended".to_owned(),
                };
                if ending.diagnostic.is_empty() {
                    how
                } else {
                    format!("{how}: {}", ending.diagnostic)
                }
            });
            Stop::Failed(reason)
        };
        let entries = &self.transcript.entries;
        let compacted = entries[ending.mark.min(entries.len())..]
            .iter()
            .any(|entry| entry.id.starts_with("compaction:"));
        if ending.compaction && stop == Stop::Finished && !compacted {
            self.emitter.notice(
                Level::Info,
                "Nothing to compact: the agent has not answered since the last compaction.",
                sink,
            );
        }
        let open = self.transcript.finish_run();
        self.emitter.settle(&mut self.transcript, &open);
        self.emitter.end_turn(stop.clone(), sink);
        self.emitter.tell_usage(&self.transcript, sink);
        stop
    }

    /// A run of another front-end, followed in the session's file, is over:
    /// the session says how it went. Returns whether it did not finish.
    pub(crate) fn finish_other(&mut self, sink: &mut dyn FnMut(Update)) -> bool {
        self.ending = None;
        let interrupted = self.transcript.interrupted();
        let open = self.transcript.finish_run();
        self.emitter.settle(&mut self.transcript, &open);
        let stop = if interrupted {
            Stop::Cancelled
        } else {
            Stop::Finished
        };
        self.emitter.end_turn(stop, sink);
        self.emitter.tell_usage(&self.transcript, sink);
        interrupted
    }
}

/// Reads the line that says a model request failed and is tried again, and
/// says so for people: the next attempt, and the gist of why the last failed.
fn parse_retry(line: &str) -> Option<String> {
    let rest = line.strip_prefix("retry> attempt ")?;
    let (attempt, rest) = rest.split_once(" of ")?;
    let (attempts, rest) = rest.split_once(" failed, trying again in ")?;
    let (_, reason) = rest.split_once(": ")?;
    let (attempt, attempts): (u32, u32) = (attempt.parse().ok()?, attempts.parse().ok()?);
    // The end of the reason says what went wrong without the layers around it.
    let gist = match reason.rsplit_once(": ") {
        Some((_, last)) if !last.trim().is_empty() => last,
        _ => reason,
    };
    Some(format!(
        "Model request failed, retrying ({} of {attempts}): {}",
        attempt + 1,
        headline(gist, 80)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retry_reads_as_what_the_run_is_doing() {
        assert_eq!(
            parse_retry(
                "retry> attempt 2 of 5 failed, trying again in 4s: post responses: dial tcp: connection refused"
            )
            .as_deref(),
            Some("Model request failed, retrying (3 of 5): connection refused")
        );
        assert_eq!(parse_retry("warn> images left out"), None);
    }

    #[test]
    fn a_response_that_starts_over_says_so_and_streams_again() {
        let mut updates = Vec::new();
        let mut fold = Fold::new(Path::new("/w"), Some("m".into()));
        fold.begin(false, &mut |update| updates.push(update));
        let lines = [
            r#"{"Sequence":1,"Kind":"turn","Data":{"ID":"t1","Type":"regular","Model":"m"}}"#,
            r#"{"type":"delta","turn":"t1","index":0,"kind":"text","text":"Half an","reset":false}"#,
            r#"{"type":"delta","turn":"t1","reset":true}"#,
            r#"{"type":"delta","turn":"t1","index":0,"kind":"reasoning","text":"Again.","reset":false}"#,
            r#"{"type":"delta","turn":"t1","index":1,"kind":"text","text":"Whole.","reset":false}"#,
            r#"{"Sequence":2,"Kind":"model_response","Data":{"TurnID":"t1","Response":{"Stop":"max_output_tokens","Output":[{"Type":"reasoning","Data":{"Summary":["Again."]}},{"Type":"message","Data":{"Role":"assistant","Text":"Whole."}},{"Type":"message","Data":{"Role":"assistant","Text":"And more."}}],"Usage":{"InputTokens":5,"OutputTokens":3}}}}"#,
        ];
        for line in lines {
            fold.line(line, false, &mut |update| updates.push(update));
        }
        assert_eq!(
            fold.finish(Some(0), &mut |update| updates.push(update)),
            Stop::Limit
        );
        assert_eq!(
            updates,
            [
                Update::TurnStarted,
                Update::AssistantChunk("Half an".into()),
                Update::Notice(
                    Level::Warning,
                    "The response started over: what it wrote above is void.".into()
                ),
                Update::ThoughtChunk("Again.".into()),
                Update::AssistantChunk("Whole.".into()),
                Update::Usage(kou_thread::Usage {
                    context_used: Some(5),
                    context_limit: None,
                    input_tokens: Some(5),
                    output_tokens: Some(3),
                    cost_usd: None,
                }),
                // A message that did not stream arrives whole, as a paragraph
                // of its own after the one that did.
                Update::AssistantChunk("\n\nAnd more.".into()),
                Update::Notice(
                    Level::Warning,
                    "Response cut off at the output token limit".into()
                ),
                Update::TurnEnded(Stop::Limit),
            ]
        );
    }

    #[test]
    fn a_run_that_fails_says_why_in_the_runner_s_words_or_by_its_exit() {
        let mut updates = Vec::new();
        let mut fold = Fold::new(Path::new("/w"), None);
        fold.begin(false, &mut |update| updates.push(update));
        fold.line("warn> two images were left out", true, &mut |u| {
            updates.push(u)
        });
        fold.line(
            "retry> attempt 1 of 3 failed, trying again in 2s: status 503: overloaded",
            true,
            &mut |u| updates.push(u),
        );
        fold.line("kou-conveyor-runner: boom", true, &mut |u| updates.push(u));
        fold.line(
            r#"{"type":"error","message":"run coordinator: boom"}"#,
            false,
            &mut |u| {
                updates.push(u);
            },
        );
        assert_eq!(
            fold.finish(Some(1), &mut |update| updates.push(update)),
            Stop::Failed("run coordinator: boom".into())
        );
        assert_eq!(
            updates[..4],
            [
                Update::TurnStarted,
                Update::Notice(Level::Warning, "two images were left out".into()),
                Update::Notice(
                    Level::Warning,
                    "Model request failed, retrying (2 of 3): overloaded".into()
                ),
                Update::TurnEnded(Stop::Failed("run coordinator: boom".into())),
            ]
        );

        // Without a word from the runner, its exit and last diagnostic say it.
        let mut fold = Fold::new(Path::new("/w"), None);
        fold.begin(false, &mut |_| {});
        fold.line("panic: nil map", true, &mut |_| {});
        assert_eq!(
            fold.finish(Some(2), &mut |_| {}),
            Stop::Failed("Runner exited: exit status 2: panic: nil map".into())
        );
        let mut fold = Fold::new(Path::new("/w"), None);
        fold.begin(false, &mut |_| {});
        assert!(fold.cancel());
        assert!(!fold.cancel(), "only the first stop counts");
        assert_eq!(fold.finish(None, &mut |_| {}), Stop::Cancelled);
    }
}
