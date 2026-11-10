use super::*;
use crate::context::Compacted;
use kou_domain::{ControlMessage, Item, ItemData, ModelResponse};
use kou_store::{ResumeState, SessionLog};

pub struct Recovery {
    pub used_checkpoint: bool,
    pub external_input_ids: Vec<String>,
}
impl History {
    /// Read one cold record at a time on fallback. A rejected consumer checkpoint
    /// must replay from sequence one, not continue after the envelope's offset.
    pub fn recover(&mut self, log: &SessionLog, mut restored: ResumeState) -> Result<Recovery> {
        log.ensure_current()
            .map_err(|error| Error(format!("load session history: {error}")))?;
        if restored.snapshot.session.id != log.session().id {
            return Err(Error("resume state belongs to a different session".into()));
        }
        let mut used_checkpoint = false;
        if let Some(checkpoint) = restored.checkpoint.take()
            && self.restore_checkpoint(&checkpoint.state)
        {
            used_checkpoint = true;
            for item in checkpoint.items {
                self.apply_recorded(item)?;
            }
        }
        if !used_checkpoint {
            for sequence in 1..=log.item_count() as u64 {
                let after = (sequence - 1) / 256 * 256;
                let item = log.item(sequence).map_err(|error| {
                    Error(format!("load session history after {after}: {error}"))
                })?;
                self.apply_recorded(item)
                    .map_err(|error| Error(format!("load session item {sequence}: {error}")))?;
            }
        }
        self.context.resumed();
        for operation in restored.operations {
            if operation.idempotency.as_str() == Some(r#"{"harness":"verification"}"#)
                && !operation.terminal()
            {
                self.state.verifying = Some(operation.id.clone());
            }
            self.apply_operation(operation);
        }
        log.ensure_current()
            .map_err(|error| Error(format!("load session history: {error}")))?;
        self.state.file_changes = 0;
        Ok(Recovery {
            used_checkpoint,
            external_input_ids: restored.external_input_ids,
        })
    }
    pub fn apply_recorded(&mut self, mut item: Item) -> Result<()> {
        self.apply(&mut item, true)
    }
    pub(super) fn apply(&mut self, item: &mut Item, replay: bool) -> Result<()> {
        match &mut item.data {
            ItemData::Fork(_) => {
                self.state.calls.clear();
                self.state.operations.clear();
            }
            ItemData::Input(input) => {
                input
                    .validate()
                    .map_err(|error| Error(format!("invalid input: {error}")))?;
                match input.kind.as_str() {
                    "external" => {
                        self.context.add_external_input(input).map_err(|error| {
                            Error(format!("add input {:?} to context: {error}", input.id))
                        })?;
                        self.state.available_inputs += 1;
                        self.state.verifications = 0;
                        self.history.verifications = 0;
                        self.state.heartbeats = 0;
                    }
                    "control" => {
                        let control = input.control().map_err(Error)?;
                        self.context.add_control(&control)?;
                        self.remember_control(&control, item.recorded_at);
                    }
                    _ => {}
                }
            }
            ItemData::Turn(turn) => {
                self.state.current_turn = turn.id.clone();
                self.state.current_turn_type = turn.kind.clone();
                if !turn.model.is_empty() {
                    self.state
                        .turn_models
                        .insert(turn.id.clone(), turn.model.clone());
                }
                if turn.kind == "compaction" {
                    self.state.compaction_turns.insert(turn.id.clone());
                    if !self.state.compaction_asked && self.refilled_rapidly() {
                        self.state.rapid_refills += 1;
                    } else {
                        self.state.rapid_refills = 0;
                    }
                } else {
                    self.state.compaction_turns.remove(&turn.id);
                    self.state.stopped = None;
                    self.history.stopped = None;
                }
                self.state.compaction_asked = false;
                self.state.current_turn_inputs = self.state.available_inputs;
                self.context.commit();
            }
            ItemData::ModelResponse(response) => self.recorded_response(response),
            ItemData::ToolCallStatus(status) => self.recorded_status(status, replay)?,
        }
        Ok(())
    }
    pub(super) fn refilled_rapidly(&self) -> bool {
        (0..3).contains(&self.state.turns_since_compaction)
    }
    fn remember_control(&mut self, control: &ControlMessage, at: Timestamp) {
        match control.mode.as_str() {
            "heartbeat" => {
                self.state.available_inputs += 1;
                if control.reason.starts_with("[harness verification]") {
                    self.state.verifications += 1;
                    self.history.verifications += 1;
                }
                if control
                    .reason
                    .starts_with("[harness] The conversation will be compacted soon.")
                {
                    self.state.reminded = true;
                }
            }
            "compact" => self.state.compaction_asked = true,
            "hard" if !control.reason.is_empty() => {
                let stop = RecordedStop {
                    at,
                    reason: control.reason.clone(),
                };
                self.state.stopped = Some(stop.clone());
                self.history.stopped = Some(stop);
            }
            _ => {}
        }
    }
    fn recorded_response(&mut self, recorded: &mut ModelResponse) {
        let turn_id = &recorded.turn_id;
        if self.state.compaction_turns.contains(turn_id) {
            if *turn_id == self.state.current_turn {
                let kept = recorded.compacted.as_ref().map(|value| Compacted {
                    message: value.message.clone(),
                    notes: value.notes.clone(),
                });
                if let Some(made) = self.context.compact(&recorded.response, kept.as_ref()) {
                    if recorded.compacted.is_none() {
                        recorded.compacted = Some(kou_domain::Compacted {
                            message: made.message,
                            notes: made.notes,
                        });
                    }
                    self.state.turns_since_compaction = 0;
                    self.state.compaction_failures = 0;
                    self.state.reminded = false;
                } else {
                    self.state.compaction_failures += 1;
                }
                self.state.compacted = true;
            }
            return;
        }
        let mut response = recorded.response.clone();
        if let Some(model) = self
            .state
            .turn_models
            .get(turn_id)
            .filter(|model| !model.is_empty())
        {
            for item in response.output.iter_mut().flatten() {
                if item.model.is_empty() {
                    item.model = model.clone();
                }
            }
        }
        let ids: Vec<String> = response
            .output
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| {
                if let LlmData::ToolCall(call) = &item.data {
                    Some(call.call_id.clone())
                } else {
                    None
                }
            })
            .collect();
        self.context.add_response(response);
        self.state.compacted = false;
        if self.state.turns_since_compaction >= 0 {
            self.state.turns_since_compaction += 1;
        }
        if self.state.current_turn == *turn_id {
            self.state.delivered_inputs = self.state.current_turn_inputs;
        }
        if !ids.is_empty() {
            self.state.work_since_verification = true;
            self.history.work_since_verification = true;
        }
        for id in ids {
            if let Some(item) = self.context.recorded_call(&id) {
                self.state.calls.insert(
                    call_key(turn_id, &id),
                    ActiveCall {
                        item,
                        status: None,
                        operations: BTreeSet::new(),
                    },
                );
            }
        }
    }
    fn recorded_status(&mut self, status: &kou_domain::ToolCallStatus, replay: bool) -> Result<()> {
        let key = call_key(&status.turn_id, &status.call_id);
        let required = status.status.error.is_empty()
            || !status.status.waiting().is_empty()
            || !status.operations.is_empty();
        if replay
            && required
            && let Some(call) = self.state.calls.get(&key)
            && !self.tools.available(&call.call().name)
        {
            return Err(Error(format!(
                "tool {:?} required by recorded call {:?} is not available",
                call.call().name,
                status.call_id
            )));
        }
        // Harness verification statuses have no model call, but still introduce
        // durable operations which must be restored and canceled normally.
        for operation in &status.operations {
            self.apply_operation(operation.clone());
        }
        if let Some(call) = self.state.calls.get_mut(&key) {
            call.operations
                .extend(status.status.waiting().iter().cloned());
            call.status = Some(status.status.clone());
            self.stage_call_result(&key)?;
        }
        Ok(())
    }
    fn finish_call(&mut self, key: &CallKey) {
        self.state.calls.remove(key);
        self.state.heartbeats = 0;
        self.state.available_inputs += 1;
    }
    pub(super) fn call_terminal(&self, key: &CallKey) -> bool {
        self.state.calls.get(key).is_some_and(|call| {
            call.operations.iter().all(|id| {
                self.state
                    .operations
                    .get(id)
                    .is_some_and(|operation| operation.terminal())
            })
        })
    }
    fn stage_call_result(&mut self, key: &CallKey) -> Result<()> {
        let Some(value) = self.state.calls.get(key) else {
            return Ok(());
        };
        let Some(status) = &value.status else {
            return Ok(());
        };
        if !self.tools.available(&value.call().name) {
            if !status.error.is_empty() && status.waiting().is_empty() {
                self.context.add_tool_result(
                    &key.call,
                    vec![ToolResultOutput {
                        kind: "text".into(),
                        value: status.error.clone(),
                    }],
                    false,
                );
                self.finish_call(key);
            }
            return Ok(());
        }
        let mut operations = Vec::new();
        for id in status.waiting() {
            if !value.operations.contains(id) {
                return Ok(());
            }
            let Some(operation) = self.state.operations.get(id) else {
                return Ok(());
            };
            operations.push(operation.clone());
        }
        let translated = self
            .tools
            .result(value.call(), status, &operations)
            .map_err(|error| {
                Error(format!(
                    "add tool call {:?} result to context: {error}",
                    value.call().call_id
                ))
            })?;
        let running = !self.call_terminal(key);
        let output = if running {
            let detail = self.tools.running(value.call(), &operations);
            if detail.is_empty() {
                Vec::new()
            } else {
                vec![ToolResultOutput {
                    kind: "text".into(),
                    value: detail,
                }]
            }
        } else {
            translated
        };
        self.context.add_tool_result(&key.call, output, running);
        if !running {
            if status.error.is_empty()
                && changes_files(&value.call().name)
                && operations
                    .iter()
                    .all(|operation| operation.status == "completed")
            {
                self.state.file_changes += 1;
            }
            self.finish_call(key);
        }
        Ok(())
    }
}
