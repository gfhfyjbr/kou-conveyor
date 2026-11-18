use super::{super::CodexState, whole};
use serde_json::{Value, value::RawValue};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Memory {
    pub state: CodexState,
    // Client RPCs which may change the pane's thread. A rejected fork or
    // turn must leave the previous conversation selected on reconnection.
    watching: HashMap<String, Option<String>>,
    bytes: usize,
    last_finished: Option<String>,
}

type Pending = Vec<(Value, Box<RawValue>)>;

impl Memory {
    pub fn client(&mut self, message: &Value, pending: &mut Pending) {
        let id = &message["id"];
        match message["method"].as_str() {
            Some("initialized") => self.state.initialized = true,
            Some("thread/start" | "thread/resume") if self.state.thread.is_none() => {
                self.watching.insert(id.to_string(), None);
            }
            Some("turn/start") => {
                self.watching.insert(
                    id.to_string(),
                    message["params"]["threadId"].as_str().map(str::to_owned),
                );
            }
            None => pending.retain(|(wire, _)| wire != id),
            _ => {}
        }
    }

    pub fn server(&mut self, message: &Value, line: &str, pending: &mut Pending) {
        let p = &message["params"];
        let id = &message["id"];
        let turn = p["turn"]["id"].as_str();
        let thread = p["threadId"].as_str();
        let our_thread = thread.is_some_and(|thread| {
            self.state.thread.as_deref() == Some(thread)
                || self
                    .watching
                    .values()
                    .flatten()
                    .any(|candidate| candidate == thread)
        });
        match message["method"].as_str() {
            Some(_) if !id.is_null() => {
                pending.retain(|(wire, _)| wire != id);
                if let Some(raw) = whole(line) {
                    pending.push((id.clone(), raw));
                }
            }
            Some("serverRequest/resolved") => pending.retain(|(wire, _)| *wire != p["requestId"]),
            Some("turn/started") if our_thread => {
                self.state.turn = turn.map(str::to_owned);
                self.state.events = Some(Vec::new());
                self.bytes = 0;
            }
            Some("turn/completed") if our_thread && turn == self.state.turn.as_deref() => {
                self.last_finished = self.state.turn.take();
                self.state.events = None;
                pending.retain(|(_, raw)| {
                    super::parse(raw.get()).is_some_and(|request| {
                        let params = &request["params"];
                        params["isBlocking"] == false || params["turnId"].as_str() != turn
                    })
                });
            }
            Some(method)
                if our_thread
                    && self.state.turn.is_some()
                    && p["turnId"].as_str() == self.state.turn.as_deref()
                    && (method.starts_with("item/")
                        || matches!(method, "turn/plan/updated" | "thread/tokenUsage/updated")) =>
            {
                // Bound what a keeper retains and sends in its greeting. A
                // larger turn can still be hydrated from the app server.
                if let Some(events) = &mut self.state.events {
                    self.bytes += line.len();
                    if self.bytes <= 4 * 1024 * 1024 {
                        events.push(message.clone());
                    } else {
                        self.state.events = None;
                    }
                }
            }
            None => {
                if let Some(candidate) = self.watching.remove(&id.to_string())
                    && let Some(result) = message.get("result")
                {
                    if let Some(thread) = candidate {
                        self.state.thread = Some(thread);
                        let turn = result["turn"]["id"].as_str();
                        if turn != self.last_finished.as_deref() {
                            self.state.turn = turn.map(str::to_owned);
                        }
                    } else {
                        self.state.thread = result["thread"]["id"].as_str().map(str::to_owned);
                    }
                }
            }
            _ => {}
        }
    }
}
