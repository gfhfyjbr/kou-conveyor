//! An edit of a prompt already sent: Claude Code started anew on a copy of
//! the conversation that ends before that prompt, and the edited prompt
//! sent there.

use super::{
    super::{
        kept,
        link::{self, Link, Resume},
    },
    Before, GRACE, LEAVE, Move, Pending, Session,
    wire::{self, text_of},
};
use crate::records;
use kou_thread::{Block, Stop, Update};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader},
    path::Path,
    time::Duration,
};

impl Session {
    /// Starts an edit of `prompt`: Claude Code is started anew on a copy of
    /// the conversation that ends before it, or on a new one for the first
    /// prompt, and `blocks` run there. An error is why nothing changed.
    pub(super) fn rewind(
        &mut self,
        prompt: Option<String>,
        blocks: Vec<Block>,
    ) -> Result<(), String> {
        if self.moving.is_some() {
            return Err("The last edit is still on its way.".into());
        }
        if !self.ready {
            return Err("Claude Code is starting: edit once it is ready.".into());
        }
        if self.turn.is_some() || !self.queue.is_empty() {
            return Err("Claude Code is working: stop it or let it finish, then edit.".into());
        }
        if wire::prompt_content(&blocks).is_empty() {
            return Err("The edited prompt is empty.".into());
        }
        let uuid = uuid::Uuid::new_v4().to_string();
        let Some(prompt) = prompt else {
            // Nothing the session holds is replaced: the prompt simply runs.
            self.send(Update::Rewound(Ok(uuid.clone())));
            self.write_prompt(blocks, Some(uuid));
            return Ok(());
        };
        let before = self.before(&prompt)?;
        let resume = match &before {
            Before::Nothing => Resume::New,
            Before::At {
                session: Some(session),
                entry,
            } => Resume::Fork {
                session: session.clone(),
                message: entry.clone(),
            },
            Before::At { session: None, .. } | Before::Unknown => {
                return Err(
                    "Claude Code cannot go back to before this prompt: what came before it was not seen here."
                        .into(),
                );
            }
        };
        let model = self.model.as_deref().filter(|model| *model != "default");
        let args = link::arguments(&self.options, &resume, model, false);
        let id = self.next_link;
        self.next_link += 1;
        let started = match &self.options.keep {
            None => Link::start(id, &self.options, &args, &self.sender),
            // Kept, in a host of its own at the next number.
            Some(keeper) => {
                let n = self.next_host;
                self.next_host += 1;
                kept::start(keeper, n, &self.options, &args).and_then(|attached| {
                    let socket = kept::keep_at(keeper, n).socket;
                    Link::kept(id, attached, &self.sender, &self.program, socket)
                })
            }
        };
        let mut link = started
            .map_err(|error| format!("Claude Code could not be started again: {error:#}"))?;
        let request = self.request_id();
        self.pending.insert(request.clone(), (id, Pending::Move));
        link.write(&wire::control_request(&request, wire::initialize()));
        self.moving = Some(Move {
            link,
            blocks,
            uuid,
            before,
            greeting: None,
            early: Vec::new(),
        });
        Ok(())
    }

    /// What came before prompt `prompt`: as the conversation's record says
    /// (the entry the prompt follows), else as it was seen here.
    pub(super) fn before(&self, prompt: &str) -> Result<Before, String> {
        if let Some(home) = &self.home {
            for session in self.sessions.iter().rev() {
                let Some(record) = records::transcript::record(home, &self.options.cwd, session)
                else {
                    continue;
                };
                match parent_in_record(&record, prompt) {
                    Some(Some(entry)) => {
                        return Ok(Before::At {
                            session: Some(session.clone()),
                            entry,
                        });
                    }
                    Some(None) => return Ok(Before::Nothing),
                    None => {}
                }
            }
        }
        match self.prompts.get(prompt) {
            Some(Before::At {
                session: None,
                entry,
            }) => Ok(Before::At {
                session: self.session.clone(),
                entry: entry.clone(),
            }),
            Some(before) => Ok(before.clone()),
            None => {
                Err("That prompt was not sent or shown on this connection to Claude Code.".into())
            }
        }
    }

    /// The process an edit is to run in said something before it was taken
    /// up: its answer to `initialize`, or what is kept for later, of which
    /// the edited prompt's echo takes it up.
    pub(super) fn on_moving_message(&mut self, message: Value) {
        let response = message.get("response").unwrap_or(&Value::Null);
        let answers_move = text_of(&message, "type") == Some("control_response")
            && text_of(response, "request_id")
                .and_then(|id| self.pending.get(id))
                .is_some_and(|(_, pending)| matches!(pending, Pending::Move));
        if answers_move {
            if let Some(id) = text_of(response, "request_id") {
                self.pending.remove(id);
            }
            if text_of(response, "subtype") == Some("error") {
                let error = text_of(response, "error").unwrap_or("it did not say why");
                return self.move_failed(format!("Claude Code did not start: {error}"));
            }
            let answer = response.get("response").cloned().unwrap_or(Value::Null);
            return self.greeted(answer);
        }
        let Some(moving) = &mut self.moving else {
            return;
        };
        let echo = moving.greeting.is_some()
            && text_of(&message, "type") == Some("user")
            && message.get("isReplay").and_then(Value::as_bool) == Some(true)
            && text_of(&message, "uuid") == Some(moving.uuid.as_str());
        moving.early.push(message);
        if echo {
            self.moved();
        }
    }

    /// The edit's process answered `initialize`: it is set up as the
    /// session stands (the mode and the level of effort in force), and the
    /// edited prompt is written to it.
    fn greeted(&mut self, answer: Value) {
        let Some(moving) = &self.moving else {
            return;
        };
        let link = moving.link.id;
        let mut requests = Vec::new();
        // It starts in its own mode: the one in force goes on in force.
        let its_mode = text_of(&answer, "current_permission_mode").unwrap_or("default");
        if let Some(mode) = self.mode.clone().filter(|mode| mode != its_mode) {
            requests.push((
                json!({"subtype": "set_permission_mode", "mode": mode}),
                Pending::SetMode(mode),
            ));
        }
        // (The model went with its command line.)
        requests.push(match self.pinned_effort.clone() {
            Some(level) => (wire::set_effort(&level), Pending::SetEffort(level)),
            None => (wire::get_settings(), Pending::Settings(None)),
        });
        let note = (moving.before == Before::Nothing)
            .then(|| self.options.preface.clone())
            .flatten();
        let prompt = wire::user_message(&moving.blocks, note.as_deref(), &moving.uuid, "");
        for (request, pending) in requests {
            let id = self.request_id();
            self.pending.insert(id.clone(), (link, pending));
            if let Some(moving) = &mut self.moving {
                moving.link.write(&wire::control_request(&id, request));
            }
        }
        if let Some(moving) = &mut self.moving {
            moving.link.write(&prompt);
            moving.greeting = Some(answer);
        }
    }

    /// The edit's process took the edited prompt: the conversation goes on
    /// in it, and the process before it is let go of.
    fn moved(&mut self) {
        let Some(moving) = self.moving.take() else {
            return;
        };
        let answer = moving.greeting.unwrap_or(Value::Null);
        let old = std::mem::replace(&mut self.link, moving.link);
        old.let_go(LEAVE, GRACE);
        // What ran in the background of the old process went with it.
        let behind: Vec<String> = self
            .calls
            .iter()
            .filter(|(_, call)| call.background && !call.ended)
            .map(|(id, _)| id.clone())
            .collect();
        for id in behind {
            self.mark_ended(&id, None);
        }
        self.calls.clear();
        self.tasks.clear();
        self.tasks_list = records::tools::Tasks::default();
        self.streams.clear();
        self.sent.clear();
        self.pending.retain(|_, (link, _)| *link == self.link.id);
        self.last_chunk = None;
        self.last_said = None;
        self.draining = false;
        self.last_entry = None;
        // The conversation it holds is known by its id once it says it.
        self.session = None;
        // (The app's note went with the edited prompt, where it was due.)
        self.fresh = moving.before == Before::Nothing;
        self.preface_due = false;
        self.models = wire::models(&answer);
        self.announce();
        let (commands, arguments) = wire::commands(answer.get("commands"));
        self.send(Update::Commands(commands));
        if arguments != self.arguments {
            self.arguments = arguments;
            self.send(Update::Offers(self.offers()));
        }
        self.prompts.insert(moving.uuid.clone(), moving.before);
        self.send(Update::Rewound(Ok(moving.uuid.clone())));
        self.prompt_written(moving.uuid, false);
        for message in moving.early {
            self.on_message(message);
        }
        self.next_prompt();
    }

    /// The edit's process did not come up, or died before it took the
    /// edited prompt: the conversation goes on where it was. `reason` is
    /// why, to which what it said of its failure is added.
    pub(super) fn move_failed(&mut self, reason: String) {
        let Some(moving) = self.moving.take() else {
            return;
        };
        moving.link.stop(Duration::ZERO);
        let id = moving.link.id;
        self.pending.retain(|_, (link, _)| *link != id);
        let said: Vec<String> = moving
            .early
            .iter()
            .filter(|message| {
                text_of(message, "type") == Some("result")
                    && message.get("is_error").and_then(Value::as_bool) == Some(true)
            })
            .filter_map(|result| match wire::stop(result, false) {
                Stop::Failed(why) => Some(why),
                _ => None,
            })
            .collect();
        let mut reason = reason;
        if !said.is_empty() {
            reason = format!("{}\n{reason}", said.join("\n"));
        }
        self.send(Update::Rewound(Err(format!(
            "Claude Code could not go back to before that prompt. {reason}"
        ))));
        self.next_prompt();
    }
}

/// The entry a conversation's record has before entry `uuid`: `None` when
/// the record has no such entry, `Some(None)` when it is the first.
fn parent_in_record(record: &Path, uuid: &str) -> Option<Option<String>> {
    let file = std::fs::File::open(record).ok()?;
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else {
            break;
        };
        if !line.contains(uuid) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if text_of(&entry, "uuid") == Some(uuid) {
            return Some(text_of(&entry, "parentUuid").map(str::to_owned));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_before_a_prompt_is_read_from_the_record() {
        let folder = tempfile::tempdir().unwrap();
        let record = folder.path().join("s.jsonl");
        std::fs::write(
            &record,
            [
                r#"{"type":"user","uuid":"p1","parentUuid":null}"#,
                "not json, with p2 in it",
                r#"{"type":"assistant","uuid":"a1","parentUuid":"p1"}"#,
                r#"{"type":"user","uuid":"p2","parentUuid":"a1"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(parent_in_record(&record, "p1"), Some(None));
        assert_eq!(parent_in_record(&record, "p2"), Some(Some("a1".into())));
        assert_eq!(parent_in_record(&record, "p3"), None);
        assert_eq!(parent_in_record(&folder.path().join("none"), "p1"), None);
    }
}
