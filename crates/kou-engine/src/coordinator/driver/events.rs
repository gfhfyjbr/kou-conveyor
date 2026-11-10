use super::*;
use tokio::sync::mpsc;

impl Coordinator {
    pub(super) async fn next_event(&mut self, cancellation: &CancellationToken) -> Event {
        tokio::select! {
            _ = cancellation.cancelled() => Event::Canceled,
            input = self.channels.inputs.recv() => Event::Input(input),
            operation = self.channels.operations.recv() => Event::Operation(operation),
            response = async {
                match &mut self.flight {
                    Some(flight) => flight.future.as_mut().await,
                    None => pending::<ModelResult>().await,
                }
            } => Event::Response(response),
            _ = tick(self.live.grace) => Event::Grace,
            _ = tick(self.live.tool_wait) => Event::ToolWait,
            _ = tick(self.live.quota_wait) => Event::Quota,
            _ = tick(self.live.heartbeat) => Event::Heartbeat,
            _ = tick(self.live.deadline) => Event::Deadline,
            _ = tick(self.live.report) => Event::Report,
        }
    }
    pub(super) async fn process_events(
        &mut self,
        cancellation: &CancellationToken,
    ) -> Result<bool> {
        let inputs = slurp(&mut self.channels.inputs, cancellation)
            .await
            .map_err(|error| Error(format!("slurp inbox: {error}")))?;
        for input in inputs {
            self.input(input).await?;
        }
        let updates = slurp(&mut self.channels.operations, cancellation)
            .await
            .map_err(|error| Error(format!("slurp operation updates: {error}")))?;
        for update in updates {
            self.update(update.value).await?;
        }
        self.reconcile().await?;
        let mut due = self.live.call_model
            || (self.pending_inputs() > 0
                && self.flight.is_none()
                && self.live.grace_calls.is_empty());
        if !self.live.deferred.is_empty() && !self.hard_stop() && !self.compacting() {
            if due || !self.mid_step() || (self.live.waited_long && self.flight.is_none()) {
                while let Some(input) = self.live.deferred.pop_front() {
                    self.record_input(input.value).await?;
                    self.live.call_model = true;
                }
                self.live.tool_wait = None;
                self.live.waited_long = false;
                due = true;
            } else if self.flight.is_none() && self.live.tool_wait.is_none() {
                self.live.tool_wait = after(self.config.tool_wait_limit);
            }
        }
        Ok(due && self.live.quota_wait.is_none())
    }
    pub(super) async fn input(&mut self, input: Queued<Input>) -> Result<()> {
        if !self.seen_inputs.insert(input.value.id.clone()) {
            return Ok(());
        }
        if input.value.kind == "external" && input.value.delivery == "after_tools" {
            if self.hard_stop() {
                return Ok(());
            }
            if self.mid_step() || self.compacting() || !self.live.deferred.is_empty() {
                self.live.deferred.push_back(input);
                return Ok(());
            }
        }
        self.record_input(input.value).await
    }
    pub(super) async fn record_input(&mut self, input: Input) -> Result<()> {
        let control = if input.kind == "control" {
            Some(input.control().map_err(Error)?)
        } else {
            None
        };
        if control
            .as_ref()
            .is_some_and(|control| control.mode == "compact")
            && self.hard_stop()
        {
            return Ok(());
        }
        let external = input.kind == "external";
        self.record(ItemData::Input(input)).await?;
        if external {
            if !self.hard_stop() {
                self.live.call_model = true;
                self.clear_grace();
            }
            return Ok(());
        }
        if let Some(control) = control {
            match control.mode.as_str() {
                "hard" | "when_idle" => self.accept_stop(control),
                "heartbeat" => {
                    if !self.hard_stop() {
                        self.live.call_model = true;
                        self.clear_grace();
                    }
                }
                "compact" => self.live.compact_request = Some(control.reason),
                "settings" if !self.hard_stop() && self.flight.is_some() => {
                    self.live.call_model = true;
                    self.clear_grace();
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub(super) async fn nudge(&mut self, reason: String) -> Result<()> {
        let payload = serde_json::to_string(&ControlMessage {
            mode: "heartbeat".into(),
            reason,
            parameters: Json::default(),
        })
        .map_err(|error| Error(format!("encode harness message: {error}")))?;
        self.record_input(Input {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "control".into(),
            payload: Json::parse(&payload).map_err(|error| Error(error.to_string()))?,
            delivery: String::new(),
        })
        .await
    }
    pub(super) async fn update(&mut self, operation: Operation) -> Result<()> {
        if self.live.cancellation_requested
            && !operation.terminal()
            && !self.history.state.operations.contains_key(&operation.id)
        {
            return Err(Error(
                "received new nonterminal operation after stop cancellation was requested".into(),
            ));
        }
        self.journal
            .save_operation(operation.clone())
            .await
            .map_err(|error| Error(format!("store operation {:?}: {error}", operation.id)))?;
        self.history.apply_operation(operation.clone());
        self.reconcile().await?;
        if self.history.state.verifying.as_deref() == Some(operation.id.as_str())
            && operation.terminal()
        {
            self.verified(&operation).await?;
        }
        Ok(())
    }
    pub(super) fn handle_stop(&mut self) -> Result<bool> {
        if !self.live.cancellation_requested {
            self.flight = None;
            let ids: Vec<_> = self
                .history
                .operations()
                .filter(|operation| !operation.terminal())
                .map(|operation| operation.id.clone())
                .collect();
            let mut errors = Vec::new();
            for id in ids {
                if let Err(error) = self.operations.cancel(&id, &self.live.stop.reason) {
                    errors.push(format!("cancel operation {id:?}: {error}"));
                }
            }
            if !errors.is_empty() {
                return Err(Error(errors.join("\n")));
            }
            self.live.cancellation_requested = true;
        }
        Ok(self
            .history
            .operations()
            .all(|operation| operation.terminal()))
    }
    pub(super) fn clear_grace(&mut self) {
        self.live.grace_calls.clear();
        self.live.grace = None;
        self.live.grace_started = None;
    }
    pub(super) fn start_grace(&mut self, statuses: &[ToolCallStatus]) {
        self.live.grace_calls.extend(
            statuses
                .iter()
                .map(|status| call_key(&status.turn_id, &status.call_id)),
        );
        self.live.grace_started = Some(Instant::now());
        self.live.grace = after(self.grace_duration());
    }
    fn grace_duration(&self) -> Duration {
        if self.config.tool_grace.is_zero() {
            Duration::from_secs(1)
        } else {
            self.config.tool_grace
        }
    }
    pub(super) fn finished(&mut self, key: &CallKey) {
        self.live.running_detail.remove(key);
        self.live.step_calls.remove(key);
        self.live.grace_calls.remove(key);
        if self.live.grace_calls.is_empty() || self.live.stop.mode == "when_idle" {
            self.clear_grace();
        } else if !self.config.tool_grace_limit.is_zero()
            && let Some(start) = self.live.grace_started
        {
            let left =
                (start + self.config.tool_grace_limit).saturating_duration_since(Instant::now());
            if !left.is_zero() {
                self.live.grace = after(self.grace_duration().min(left));
            }
        }
    }
    pub(super) fn arm_heartbeat(&mut self) {
        if !self.waiting_for_tools() {
            self.live.heartbeat = None;
        } else if self.live.heartbeat.is_none() && !self.config.tool_heartbeat.is_zero() {
            let shift = if self.only_waits() {
                2
            } else {
                self.history.state.heartbeats.min(2) as u32
            };
            self.live.heartbeat_wait = self.config.tool_heartbeat.saturating_mul(1 << shift);
            self.live.heartbeat = after(self.live.heartbeat_wait);
        }
    }
    pub(super) async fn post_heartbeat(&mut self) -> Result<()> {
        let reason = format!(
            "{} passed with tool calls still running. Check progress and continue waiting unless you have other work to do.",
            duration_text(self.live.heartbeat_wait)
        );
        self.record_input(Input {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "control".into(),
            payload: Json::parse(
                &serde_json::to_string(&ControlMessage {
                    mode: "heartbeat".into(),
                    reason,
                    parameters: Json::default(),
                })
                .map_err(|error| Error(error.to_string()))?,
            )
            .map_err(|error| Error(error.to_string()))?,
            delivery: String::new(),
        })
        .await?;
        self.history.state.heartbeats += 1;
        Ok(())
    }
}
async fn slurp<T>(
    receiver: &mut mpsc::Receiver<T>,
    cancellation: &CancellationToken,
) -> Result<Vec<T>> {
    let mut values = Vec::new();
    while values.len() < 100 {
        check_cancel(cancellation)?;
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Error("context canceled".into())),
            value = receiver.recv() => match value { Some(value) => values.push(value), None => break },
            _ = tokio::time::sleep(Duration::from_millis(1)) => break,
        }
    }
    Ok(values)
}
