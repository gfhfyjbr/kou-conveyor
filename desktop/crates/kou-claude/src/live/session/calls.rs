//! The calls Claude Code and its subagents make, and the leave it asks to
//! make one. A call is converted as the records' replay converts it
//! ([`records::tools`]), so that a conversation looks the same live and
//! read back.

use super::{
    Asked, CANCELLED, Call, Session,
    wire::{self, text_of},
};
use crate::records;
use kou_thread::{PlanStep, ToolCall, ToolPatch, Update};
use serde_json::{Value, json};

impl Session {
    /// A call begins to stream: shown at once, its input to come. A call of
    /// the plan's tools (Claude Code's own: a subagent's to-dos are its
    /// calls) is not a call but the plan.
    pub(super) fn call_begins(&mut self, id: &str, name: &str, parent: Option<String>) {
        if self.calls.contains_key(id) {
            return;
        }
        let plan = parent.is_none() && records::tools::is_plan(name);
        let call = Call {
            name: name.to_owned(),
            input: json!({}),
            parent,
            plan,
            shown: !plan,
            ..Call::default()
        };
        if !plan {
            let made = self.made(id, &call);
            self.announce_call(made);
        }
        self.calls.insert(id.to_owned(), call);
    }

    /// A call's input is whole: the plan it states, or the call shown with
    /// all of its input.
    pub(super) fn call_complete(&mut self, id: &str, input: Value) {
        let Some(call) = self.calls.get_mut(id) else {
            return;
        };
        call.input = input;
        call.complete = true;
        let shown = std::mem::replace(&mut call.shown, !call.plan);
        if call.plan {
            // The to-do tool says the plan in its input; the task tools'
            // results change it (see `on_user`).
            if let Some(steps) = records::tools::plan(&call.name, &call.input) {
                self.plan(steps);
            }
            return;
        }
        let made = self.made(id, &self.calls[id]);
        if !shown {
            return self.announce_call(made);
        }
        self.send(Update::ToolPatch(ToolPatch {
            id: id.to_owned(),
            title: Some(made.title),
            kind: Some(made.kind),
            content: Some(made.content),
            locations: Some(made.locations),
            raw_input: made.raw_input,
            agent: made.agent,
            parent: made.parent,
            ..ToolPatch::default()
        }));
    }

    /// More of a streaming call's input is whole: the call shown says what
    /// that tells (a file's path, a command) before the rest has come.
    pub(super) fn call_grows(&mut self, id: &str, input: &Value) {
        let Some(call) = self.calls.get(id) else {
            return;
        };
        if call.complete || call.plan || !call.shown {
            return;
        }
        let made = records::tools::within(
            &self.options.cwd,
            records::tools::call(id, &call.name, input, call.parent.as_deref()),
        );
        self.send(Update::ToolPatch(ToolPatch {
            id: id.to_owned(),
            title: Some(made.title),
            kind: Some(made.kind),
            content: Some(made.content),
            locations: Some(made.locations),
            raw_input: made.raw_input,
            ..ToolPatch::default()
        }));
    }

    /// A call in a whole message: new, or one whose input streamed.
    pub(super) fn whole_call(
        &mut self,
        id: &str,
        name: &str,
        input: Value,
        parent: Option<String>,
    ) {
        match self.calls.get(id) {
            Some(call) if call.complete && call.input == input => {}
            Some(_) => self.call_complete(id, input),
            None => {
                let plan = parent.is_none() && records::tools::is_plan(name);
                self.calls.insert(
                    id.to_owned(),
                    Call {
                        name: name.to_owned(),
                        input: json!({}),
                        parent,
                        plan,
                        ..Call::default()
                    },
                );
                self.call_complete(id, input);
            }
        }
    }

    /// The result of a tool the API ran itself: a patch of its call, as a
    /// tool's result in a `user` message is. Its failure is in the block's
    /// content (an error of its own kind), which the records read.
    pub(super) fn server_result(&mut self, block: &Value) {
        let Some(id) = text_of(block, "tool_use_id") else {
            return;
        };
        let Some(call) = self.calls.get(id) else {
            log::debug!(
                "{} gave the result of a call never made: {id}",
                self.program
            );
            return;
        };
        let content = block.get("content").cloned().unwrap_or(Value::Null);
        let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
        let mut patch = records::tools::result(id, &call.name, &call.input, &content, None, failed);
        if let Some(title) = patch.title.take() {
            patch.title = Some(records::tools::relative(&self.options.cwd, &title));
        }
        self.send(Update::ToolPatch(patch));
    }

    /// Call `id` as the conversation shows it.
    pub(super) fn made(&self, id: &str, call: &Call) -> ToolCall {
        let made = records::tools::call(id, &call.name, &call.input, call.parent.as_deref());
        records::tools::within(&self.options.cwd, made)
    }

    pub(super) fn announce_call(&mut self, call: ToolCall) {
        if call.parent.is_none() {
            self.last_chunk = None;
        }
        self.send(Update::Tool(call));
    }

    pub(super) fn plan(&mut self, steps: Vec<PlanStep>) {
        self.last_chunk = None;
        self.send(Update::Plan(steps));
    }

    pub(super) fn on_task_started(&mut self, message: &Value) {
        let ambient = ["skip_transcript", "ambient"]
            .iter()
            .any(|key| message.get(*key).and_then(Value::as_bool) == Some(true));
        let (Some(task), Some(call)) =
            (text_of(message, "task_id"), text_of(message, "tool_use_id"))
        else {
            return;
        };
        if ambient {
            return;
        }
        let call = call.to_owned();
        self.tasks.insert(task.to_owned(), call.clone());
        if message.get("is_backgrounded").and_then(Value::as_bool) == Some(true) {
            self.mark_background(&call);
        }
    }

    pub(super) fn on_task_updated(&mut self, message: &Value) {
        let Some(call) = text_of(message, "task_id")
            .and_then(|task| self.tasks.get(task))
            .cloned()
        else {
            return;
        };
        let patch = message.get("patch").unwrap_or(&Value::Null);
        if patch.get("is_backgrounded").and_then(Value::as_bool) == Some(true) {
            self.mark_background(&call);
        }
        if matches!(
            text_of(patch, "status"),
            Some("completed" | "failed" | "killed" | "stopped")
        ) {
            self.mark_ended(&call, None);
        }
    }

    pub(super) fn on_task_notification(&mut self, message: &Value) {
        let call = text_of(message, "tool_use_id")
            .map(str::to_owned)
            .or_else(|| text_of(message, "task_id").and_then(|task| self.tasks.get(task).cloned()));
        if let Some(task) = text_of(message, "task_id") {
            self.tasks.remove(task);
        }
        let tokens = message
            .pointer("/usage/total_tokens")
            .and_then(Value::as_u64);
        if let Some(call) = call {
            self.mark_ended(&call, tokens);
        }
    }

    /// What call `id` started runs on in the background.
    pub(super) fn mark_background(&mut self, id: &str) {
        let Some(call) = self.calls.get_mut(id) else {
            return;
        };
        if call.background {
            return;
        }
        call.background = true;
        self.send(Update::ToolPatch(ToolPatch {
            id: id.to_owned(),
            background: Some(true),
            ..ToolPatch::default()
        }));
    }

    /// What call `id` left running in the background has ended.
    pub(super) fn mark_ended(&mut self, id: &str, tokens: Option<u64>) {
        let Some(call) = self.calls.get_mut(id) else {
            return;
        };
        if !call.background || (call.ended && tokens.is_none()) {
            return;
        }
        // (A task can end twice over: its update, then its notification
        // with what it used.)
        let ended = !std::mem::replace(&mut call.ended, true);
        self.send(Update::ToolPatch(ToolPatch {
            id: id.to_owned(),
            ended: ended.then_some(true),
            tokens,
            ..ToolPatch::default()
        }));
    }

    // Leave to use a tool.

    pub(super) fn on_permission_request(&mut self, id: String, request: &Value) {
        let tool_use = text_of(request, "tool_use_id")
            .unwrap_or_default()
            .to_owned();
        let name = text_of(request, "tool_name").unwrap_or_default().to_owned();
        let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
        if self.stopped() {
            // The user already said stop: there is nothing left to ask.
            self.link
                .write(&wire::success(&id, wire::deny(&tool_use, CANCELLED)));
            return;
        }
        // Asked with no turn open: it opens one of Claude Code's own, in
        // which the answer goes on and which can end.
        self.own_turn_starts();
        self.last_chunk = None;
        let parent = self
            .calls
            .get(&tool_use)
            .and_then(|call| call.parent.clone())
            .or_else(|| {
                text_of(request, "agent_id").and_then(|task| self.tasks.get(task).cloned())
            });
        let call = self.calls.entry(tool_use.clone()).or_insert_with(|| Call {
            name: name.clone(),
            parent: parent.clone(),
            complete: true,
            ..Call::default()
        });
        call.shown = true;
        let tool = records::tools::within(
            &self.options.cwd,
            records::tools::call(&tool_use, &name, &input, parent.as_deref()),
        );
        if name == "AskUserQuestion" {
            return self.ask_questions(id, tool_use, input, tool);
        }
        let suggestions: Vec<Value> = request
            .get("permission_suggestions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let lasting = request
            .get("suppress_always_allow_rule")
            .and_then(Value::as_bool)
            != Some(true);
        let options = wire::permission_options(&suggestions, lasting);
        let asked = Asked::Leave {
            tool_use,
            input,
            suggestions,
        };
        self.open(id, asked, tool, options, Vec::new());
    }
}
