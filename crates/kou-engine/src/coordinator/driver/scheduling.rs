use super::*;

impl Coordinator {
    pub(super) async fn schedule(&mut self) -> Result<Vec<ToolCallStatus>> {
        let keys: Vec<_> = self
            .history
            .state
            .calls
            .iter()
            .filter(|(_, call)| call.status.is_none())
            .map(|(key, _)| key.clone())
            .collect();
        let mut statuses = Vec::new();
        for key in keys {
            let item = self.history.state.calls[&key].item.clone();
            let LlmData::ToolCall(call) = &item.data else {
                unreachable!()
            };
            let mut submission = Submission::default();
            let status = if self.registry.available(&call.name) {
                self.registry.translate(call, &mut submission)
            } else {
                CallStatus {
                    error: self.registry.unavailable(&call.name),
                    ..CallStatus::default()
                }
            };
            for operation in &submission.operations {
                operation.validate().map_err(Error)?;
            }
            let recorded = ToolCallStatus {
                turn_id: key.turn.clone(),
                call_id: key.call.clone(),
                status,
                operations: submission.operations,
            };
            self.record_status(recorded.clone()).await?;
            statuses.push(recorded);
        }
        if requires_model(&statuses) && !self.hard_stop() {
            self.live.call_model = true;
        }
        Ok(statuses)
    }
    pub(super) async fn record_status(&mut self, status: ToolCallStatus) -> Result<()> {
        let key = call_key(&status.turn_id, &status.call_id);
        self.record(ItemData::ToolCallStatus(status)).await?;
        if !self.history.state.calls.contains_key(&key) {
            self.finished(&key);
        }
        Ok(())
    }
    pub(super) async fn reconcile(&mut self) -> Result<()> {
        let keys: Vec<_> = self.history.state.calls.keys().cloned().collect();
        for key in keys {
            let call = &self.history.state.calls[&key];
            let Some(status) = call.status.clone() else {
                return Err(Error(format!(
                    "reconcile untranslated tool call {:?} in turn {:?}",
                    key.call, key.turn
                )));
            };
            let mut operations = Vec::new();
            let mut complete = true;
            for id in &call.operations {
                match self.history.state.operations.get(id) {
                    Some(operation) => operations.push(operation.clone()),
                    None => {
                        complete = false;
                        break;
                    }
                }
            }
            if complete && self.history.call_terminal(&key) {
                self.record_status(ToolCallStatus {
                    turn_id: key.turn,
                    call_id: key.call,
                    status,
                    operations: operations.iter().map(|value| (**value).clone()).collect(),
                })
                .await?;
            } else {
                let detail = self.history.tools.running(call.call(), &operations);
                if self
                    .live
                    .running_detail
                    .get(&key)
                    .is_none_or(|previous| *previous != detail)
                {
                    let output = if detail.is_empty() {
                        Vec::new()
                    } else {
                        vec![ToolResultOutput {
                            kind: "text".into(),
                            value: detail.clone(),
                        }]
                    };
                    self.history.context.refresh_tool_result(&key.call, output);
                    self.live.running_detail.insert(key, detail);
                }
            }
        }
        Ok(())
    }
    pub(super) fn dispatch(&mut self, operation: Operation) -> Result<()> {
        if operation.terminal() {
            return Ok(());
        }
        if self.live.cancellation_requested {
            return Err(Error(
                "cannot dispatch operation after stop cancellation was requested".into(),
            ));
        }
        self.operations.add(operation)
    }
}
