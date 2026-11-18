//! What the host remembers of the lines that pass through it: the few things
//! a client that comes back needs and Claude Code's record on disk does not
//! hold. Each line is parsed as JSON just to look at it; one that does not
//! parse changes nothing.

use serde::Serialize;
use serde_json::{Value, value::RawValue};
mod codex;

/// See the module documentation. Lines are kept whole, as they were written.
#[derive(Default)]
pub(super) struct Memory {
    codex: Option<codex::Memory>,
    /// The `request_id` of the client's last `initialize`.
    greeting_id: Option<Value>,
    /// The program's `control_response` to it.
    greeting: Option<Box<RawValue>>,
    /// The latest `system` lines of subtype `init`, `status` and
    /// `session_state_changed`.
    init: Option<Box<RawValue>>,
    status: Option<Box<RawValue>>,
    state: Option<Box<RawValue>>,
    /// What `state` said: `idle`, `running`...
    state_word: Option<String>,
    /// The program's `control_request`s still unanswered, by `request_id`,
    /// in the order they came.
    pending: Vec<(Value, Box<RawValue>)>,
    /// The program has written a `result` since the client last wrote a
    /// `user` line and since the state last changed.
    answered: bool,
}

/// What `#hello` carries.
#[derive(Serialize)]
struct Hello<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    codex: Option<&'a super::CodexState>,
    version: u32,
    pid: u32,
    started: u64,
    fresh: bool,
    answered: bool,
    greeting: Option<&'a RawValue>,
    init: Option<&'a RawValue>,
    status: Option<&'a RawValue>,
    state: Option<&'a RawValue>,
    pending: Vec<&'a RawValue>,
}

impl Memory {
    /// Take note of a line of the program's standard output.
    pub fn note_program(&mut self, line: &str) {
        let Some(message) = parse(line) else {
            return;
        };
        if let Some(codex) = &mut self.codex {
            codex.server(&message, line, &mut self.pending);
            return;
        }
        match text(&message["type"]) {
            Some("control_response") => {
                let id = &message["response"]["request_id"];
                if !id.is_null() && self.greeting_id.as_ref() == Some(id) {
                    self.greeting = whole(line);
                }
            }
            Some("system") => match text(&message["subtype"]) {
                Some("init") => self.init = whole(line),
                Some("status") => self.status = whole(line),
                Some("session_state_changed") => {
                    self.state = whole(line);
                    self.state_word = text(&message["state"]).map(str::to_owned);
                    // A state said anew: idle, the turn is over; anything
                    // else, a result that comes is a new answer.
                    self.answered = false;
                }
                _ => {}
            },
            Some("result") => self.answered = true,
            Some("control_request") => {
                let id = &message["request_id"];
                if let (false, Some(line)) = (id.is_null(), whole(line)) {
                    self.pending.retain(|(known, _)| known != id);
                    self.pending.push((id.clone(), line));
                }
            }
            Some("control_cancel_request") => self.settle(&message["request_id"]),
            _ => {}
        }
    }

    /// Take note of a line a client wrote to the program.
    pub fn note_client(&mut self, line: &str) {
        let Some(message) = parse(line) else {
            return;
        };
        if message["method"] == "initialize" {
            self.codex.get_or_insert_with(Default::default);
        }
        if let Some(codex) = &mut self.codex {
            codex.client(&message, &mut self.pending);
            return;
        }
        match text(&message["type"]) {
            Some("user") => self.answered = false,
            Some("control_request")
                if text(&message["request"]["subtype"]) == Some("initialize") =>
            {
                let id = &message["request_id"];
                if !id.is_null() {
                    self.greeting_id = Some(id.clone());
                }
            }
            Some("control_response") => self.settle(&message["response"]["request_id"]),
            _ => {}
        }
    }

    /// Nothing is owed: the state is idle or was never said, and no request
    /// waits for an answer.
    pub fn quiet(&self) -> bool {
        if let Some(codex) = &self.codex {
            return self.pending.is_empty() && codex.state.turn.is_none();
        }
        self.pending.is_empty() && self.state_word.as_deref().is_none_or(|word| word == "idle")
    }

    /// The JSON of `#hello`, for a program of process id `pid` started at
    /// `started` (seconds since the epoch); `fresh` for the host's first
    /// client.
    pub fn hello(&self, pid: u32, started: u64, fresh: bool) -> String {
        let hello = Hello {
            codex: self.codex.as_ref().map(|m| &m.state),
            version: super::VERSION,
            pid,
            started,
            fresh,
            answered: self.answered,
            greeting: self.greeting.as_deref(),
            init: self.init.as_deref(),
            status: self.status.as_deref(),
            state: self.state.as_deref(),
            pending: self.pending.iter().map(|(_, line)| &**line).collect(),
        };
        serde_json::to_string(&hello).expect("lines that parsed serialize")
    }

    fn settle(&mut self, id: &Value) {
        if !id.is_null() {
            self.pending.retain(|(known, _)| known != id);
        }
    }
}

fn parse(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line)
        .ok()
        .filter(Value::is_object)
}

/// The line as it was, to be put whole into `#hello`.
fn whole(line: &str) -> Option<Box<RawValue>> {
    RawValue::from_string(line.trim().to_owned()).ok()
}

fn text(value: &Value) -> Option<&str> {
    value.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hello(memory: &Memory) -> Value {
        serde_json::from_str(&memory.hello(42, 1000, false)).unwrap()
    }

    #[test]
    fn the_greeting_is_the_answer_to_the_client_s_initialize_kept_as_it_was_written() {
        let mut memory = Memory::default();
        // An answer to some other request is not the greeting.
        memory.note_program(
            r#"{"type":"control_response","response":{"request_id":"r-0","subtype":"success"}}"#,
        );
        assert_eq!(hello(&memory)["greeting"], Value::Null);
        memory.note_client(
            r#"{"type":"control_request","request_id":"r-1","request":{"subtype":"initialize"}}"#,
        );
        memory.note_client(
            r#"{"type":"control_request","request_id":"r-2","request":{"subtype":"set_model"}}"#,
        );
        memory.note_program(
            r#"{"type":"control_response","response":{"request_id":"r-2","subtype":"success"}}"#,
        );
        let greeting = r#"{"type":"control_response", "response":{"request_id":"r-1","subtype":"success","response":{"n":1.50}}}"#;
        memory.note_program(greeting);
        let said = memory.hello(42, 1000, true);
        // Whole, with its own spacing and numbers.
        assert!(said.contains(greeting), "{said}");
        assert_eq!(
            hello(&memory),
            json!({
                "version": 1, "pid": 42, "started": 1000, "fresh": false, "answered": false,
                "greeting": serde_json::from_str::<Value>(greeting).unwrap(),
                "init": null, "status": null, "state": null, "pending": [],
            })
        );
    }

    #[test]
    fn the_latest_init_status_and_state_are_kept() {
        let mut memory = Memory::default();
        memory.note_program(r#"{"type":"system","subtype":"init","model":"a"}"#);
        memory.note_program(r#"{"type":"system","subtype":"init","model":"b"}"#);
        memory.note_program(r#"{"type":"system","subtype":"status","permissionMode":"plan"}"#);
        memory.note_program(
            r#"{"type":"system","subtype":"session_state_changed","state":"running"}"#,
        );
        memory.note_program(r#"{"type":"system","subtype":"task_started"}"#);
        memory.note_program("not JSON at all");
        memory.note_program("[1, 2]");
        let said = hello(&memory);
        assert_eq!(said["init"]["model"], "b");
        assert_eq!(said["status"]["permissionMode"], "plan");
        assert_eq!(said["state"]["state"], "running");
        assert!(!memory.quiet());
        memory
            .note_program(r#"{"type":"system","subtype":"session_state_changed","state":"idle"}"#);
        assert_eq!(hello(&memory)["state"]["state"], "idle");
        assert!(memory.quiet());
    }

    #[test]
    fn a_request_is_pending_until_the_client_answers_it_or_the_program_takes_it_back() {
        let mut memory = Memory::default();
        assert!(memory.quiet());
        let ask = |id: &str| {
            format!(
                r#"{{"type":"control_request","request_id":"{id}","request":{{"subtype":"can_use_tool"}}}}"#
            )
        };
        for id in ["p-1", "p-2", "p-3"] {
            memory.note_program(&ask(id));
        }
        assert!(!memory.quiet());
        memory.note_client(
            r#"{"type":"control_response","response":{"request_id":"p-2","subtype":"success"}}"#,
        );
        memory.note_program(r#"{"type":"control_cancel_request","request_id":"p-1"}"#);
        // The program's answers to the client's requests settle nothing.
        memory.note_program(r#"{"type":"control_response","response":{"request_id":"p-3"}}"#);
        assert_eq!(
            hello(&memory)["pending"],
            json!([serde_json::from_str::<Value>(&ask("p-3")).unwrap()])
        );
        memory.note_client(r#"{"type":"control_response","response":{"request_id":"p-3"}}"#);
        assert_eq!(hello(&memory)["pending"], json!([]));
        assert!(memory.quiet());
    }

    #[test]
    fn answered_is_a_result_since_the_last_prompt_and_the_last_change_of_state() {
        let mut memory = Memory::default();
        let state = |word: &str| {
            format!(r#"{{"type":"system","subtype":"session_state_changed","state":"{word}"}}"#)
        };
        let result = r#"{"type":"result","subtype":"success"}"#;
        let answered = |memory: &Memory| hello(memory)["answered"].as_bool().unwrap();
        memory.note_client(r#"{"type":"user","message":{"role":"user","content":"hi"}}"#);
        memory.note_program(&state("running"));
        assert!(!answered(&memory));
        memory.note_program(result);
        assert!(answered(&memory));
        // A prompt joins the turn: its answer is still to come.
        memory.note_client(r#"{"type":"user","message":{"role":"user","content":"and?"}}"#);
        assert!(!answered(&memory));
        memory.note_program(result);
        assert!(answered(&memory));
        memory.note_program(&state("idle"));
        assert!(!answered(&memory));
        // Claude Code going on by itself.
        memory.note_program(&state("running"));
        memory.note_program(result);
        assert!(answered(&memory));
        memory.note_program(&state("running"));
        assert!(!answered(&memory));
    }
}
