//! A session found again: Claude Code kept by a host the session had left
//! (see `kept`), taken up where it stands as the host's `#hello` says it,
//! without greeting it again. The greeting the host kept gives the models
//! and commands; its latest `init` and `status` lines the model, the mode
//! and the conversation's id; its `state` whether Claude Code is at work;
//! its `pending` what Claude Code asked and nobody answered. What was said
//! comes from the conversation's record.

use super::{Call, Pending, Session, Turn, wire};
use crate::{host::Hello, records};
use kou_thread::{Entry, Thread, ToolStatus, Update};
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    path::Path,
};

impl Session {
    /// Take up the session the host of the link keeps, as `hello` says it
    /// stands.
    pub(super) fn take_up(&mut self, hello: Hello) {
        let greeting = hello
            .greeting
            .as_ref()
            .and_then(|line| line.pointer("/response/response"))
            .cloned()
            .unwrap_or(Value::Null);
        let init = hello.init.clone().unwrap_or(Value::Null);
        let latest: Vec<&Value> = [&hello.status, &hello.init, &hello.state]
            .into_iter()
            .flatten()
            .collect();
        // The conversation the kept Claude Code holds, which counts over
        // the one the session was asked to go on with.
        let session = latest
            .iter()
            .find_map(|line| wire::text_of(line, "session_id"))
            .map(str::to_owned)
            .or_else(|| self.options.resume.clone());
        self.models = wire::models(&greeting);
        self.model = wire::current_model(&self.models, self.options.model.as_deref());
        if let Some(running) = wire::text_of(&init, "model")
            && let Some(row) = self
                .models
                .iter()
                .find(|row| row.choice.id == running || row.resolved.as_deref() == Some(running))
        {
            self.model = Some(row.choice.id.clone());
        }
        // A mode changed since the last `init` is said in `status`.
        let mode = latest
            .iter()
            .find_map(|line| wire::text_of(line, "permissionMode"))
            .or_else(|| wire::text_of(&greeting, "current_permission_mode"))
            .unwrap_or("default");
        self.mode = Some(mode.to_owned());
        if let Some(effort) = wire::effort_field(&init) {
            self.effort = effort;
        }
        if let Some(capabilities) = init.get("capabilities").and_then(Value::as_array) {
            self.capabilities = capabilities
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
        let state = hello
            .state
            .as_ref()
            .and_then(|line| wire::text_of(line, "state"));
        self.states = state.is_some();
        let working = state.is_some_and(|state| state != "idle");
        self.fresh = false;

        // What was said so far, from the record: what has no end in it yet
        // is left at work while Claude Code is.
        if let Some(session) = session {
            self.session = Some(session.clone());
            self.sessions.push(session.clone());
            let record = self
                .home
                .as_deref()
                .and_then(|home| records::transcript::record(home, &self.options.cwd, &session));
            if let Some(record) = record {
                self.said = true;
                let live = working || !hello.pending.is_empty();
                let updates = if live {
                    records::transcript::replay_live(&record)
                } else {
                    records::transcript::replay(&record)
                };
                if live {
                    self.seed_calls(&record, &updates);
                }
                for update in updates {
                    if let Update::Title(title) = &update {
                        self.title = Some(title.clone());
                    }
                    self.send(update);
                }
            }
        }

        self.ready = true;
        self.announce();
        let (commands, arguments) = wire::commands(greeting.get("commands"));
        self.arguments = arguments;
        self.send(Update::Commands(commands));
        self.send(Update::Offers(self.offers()));
        if working {
            // Its turn is open, and ends as any turn does, when Claude Code
            // says it is idle.
            self.turn = Some(Turn {
                ran: true,
                answered: hello.answered,
                ..Turn::default()
            });
            self.send(Update::TurnStarted);
        }
        self.request(wire::get_settings(), Pending::Settings(None));
        self.note_title();
        // What it asked while nobody was there is asked now.
        for request in hello.pending {
            self.on_message(request);
        }
        self.next_prompt();
    }

    /// The calls a live replay leaves at work, known as the calls made on
    /// this connection are: what comes of them is shown as it would have
    /// been, and what runs on in the background is heard of when it ends.
    /// (A subagent's calls are its own record's, and are not looked for.)
    fn seed_calls(&mut self, record: &Path, updates: &[Update]) {
        let mut thread = Thread::new();
        for update in updates {
            thread.apply(update.clone());
        }
        let open: Vec<_> = thread
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool(call)
                    if matches!(call.status, ToolStatus::Pending | ToolStatus::Running)
                        || (call.work.background && !call.work.ended) =>
                {
                    Some(call.clone())
                }
                _ => None,
            })
            .collect();
        if open.is_empty() {
            return;
        }
        let ids: Vec<&str> = open.iter().map(|call| call.id.as_str()).collect();
        let mut made = calls_in_record(record, &ids);
        for call in open {
            let (name, input) = made
                .remove(&call.id)
                .unwrap_or_else(|| (String::new(), call.raw_input.clone().unwrap_or_default()));
            self.calls.insert(
                call.id.clone(),
                Call {
                    name,
                    input,
                    parent: call.parent.clone(),
                    complete: true,
                    shown: true,
                    background: call.work.background,
                    ..Call::default()
                },
            );
        }
    }
}

/// The tool and input of each call of `ids` that a `tool_use` block of the
/// record makes.
fn calls_in_record(record: &Path, ids: &[&str]) -> HashMap<String, (String, Value)> {
    let mut found = HashMap::new();
    let Ok(file) = std::fs::File::open(record) else {
        return found;
    };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else {
            break;
        };
        if !line.contains("tool_use") || !ids.iter().any(|id| line.contains(id)) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let blocks = entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for block in blocks {
            let kind = wire::text_of(block, "type").unwrap_or_default();
            let (Some(id), Some(name)) = (wire::text_of(block, "id"), wire::text_of(block, "name"))
            else {
                continue;
            };
            if kind.ends_with("tool_use") && ids.contains(&id) {
                let input = block.get("input").cloned().unwrap_or_default();
                found.insert(id.to_owned(), (name.to_owned(), input));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_calls_still_open_are_found_in_the_record() {
        let folder = tempfile::tempdir().unwrap();
        let record = folder.path().join("s.jsonl");
        let lines = [
            json!({"type": "assistant", "message": {"content": [
                {"type": "text", "text": "Reading."},
                {"type": "tool_use", "id": "read-1", "name": "Read", "input": {"file_path": "/a"}},
            ]}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "server_tool_use", "id": "srv-1", "name": "web_search", "input": {"query": "x"}},
            ]}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "other", "name": "Bash", "input": {}},
            ]}}),
        ];
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(&record, text.join("\n") + "\nnot json, read-1\n").unwrap();
        let found = calls_in_record(&record, &["read-1", "srv-1", "gone"]);
        assert_eq!(found.len(), 2);
        assert_eq!(found["read-1"], ("Read".into(), json!({"file_path": "/a"})));
        assert_eq!(found["srv-1"].0, "web_search");
    }
}
