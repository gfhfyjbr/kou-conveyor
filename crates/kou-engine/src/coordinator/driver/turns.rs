use super::*;

impl Coordinator {
    pub(super) async fn start_turn(&mut self, answer_wanted: bool) -> Result<()> {
        if self.hard_stop() {
            return Ok(());
        }
        if answer_wanted {
            self.hooks.before_turn(&mut self.history.context);
        }
        let built = self.history.context.build();
        let compact = self.compaction_due(&built);
        if compact.is_none() && !answer_wanted {
            return Ok(());
        }
        let mut notes = Vec::new();
        if compact.is_none() {
            if let Some(stop) = self.history.state.stopped.take() {
                let at = self.hooks.stopped_at(stop.at);
                let at = if at.is_empty() {
                    at
                } else {
                    format!(" at {at}")
                };
                notes.push(format!("{HARNESS} The previous run was stopped{at}: {}. The tool calls it was making were canceled and the commands they ran were ended; what they started that outlives them (servers, containers, detached jobs) may or may not still run. Check what they left before relying on it.", stop.reason.strip_suffix('.').unwrap_or(&stop.reason)));
            }
            if !self.live.notes_checked {
                self.live.notes_checked = true;
                if let Some(note) = self.history.context.notes_update() {
                    notes.push(note);
                }
            }
            if !self.config.compaction_reminder.is_empty()
                && !self.history.state.reminded
                && built.compactable
                && self.auto_compaction_allowed()
                && built.estimated_tokens
                    >= self.config.auto_compact_tokens
                        - 30_000.min(self.config.auto_compact_tokens / 5)
            {
                notes.push(format!("{REMINDER} {}", self.config.compaction_reminder));
            }
        }
        for note in notes {
            self.nudge(note).await?;
        }
        let kind = if compact.is_some() {
            "compaction"
        } else {
            "regular"
        };
        let turn = Turn {
            id: uuid::Uuid::new_v4().to_string(),
            previous_turn_id: self.history.state.current_turn.clone(),
            kind: kind.into(),
            model: built.request.model.id,
        };
        self.record(ItemData::Turn(turn)).await?;
        let (built, sent) = match compact {
            Some((focus, budget)) => {
                let built = self.history.context.build_compaction(&focus, budget);
                let sent = Sent {
                    estimate: built.estimated_tokens,
                    focus,
                    budget,
                    retries: 0,
                };
                (built, sent)
            }
            None => {
                let built = self.history.context.build();
                let sent = Sent {
                    estimate: built.estimated_tokens,
                    ..Sent::default()
                };
                (built, sent)
            }
        };
        if kind == "regular" {
            self.live.call_model = false;
        }
        self.start_request(built, sent);
        Ok(())
    }
    fn start_request(&mut self, built: Built, sent: Sent) {
        self.flight = None;
        self.live.sent = sent;
        let cancellation = CancellationToken::new();
        let future = self.model.clone().respond(
            built.request,
            self.journal.session_id().to_owned(),
            cancellation.clone(),
        );
        self.flight = Some(Flight {
            future,
            cancellation,
        });
    }
    fn compaction_due(&mut self, built: &Built) -> Option<(String, i64)> {
        let requested = self.live.compact_request.take();
        if !built.compactable {
            self.live.overflow_budget = 0;
            return None;
        }
        let budget = summary_room(self.config.context_window);
        if let Some(focus) = requested {
            return Some((focus, budget));
        }
        if self.live.overflow_budget > 0 {
            let budget = std::mem::take(&mut self.live.overflow_budget);
            return Some((String::new(), budget));
        }
        if !self.history.state.compacted
            && self.auto_compaction_allowed()
            && built.estimated_tokens >= self.config.auto_compact_tokens
        {
            Some((String::new(), budget))
        } else {
            None
        }
    }
    fn auto_compaction_allowed(&self) -> bool {
        self.config.auto_compact_tokens > 0
            && self.history.state.compaction_failures < 3
            && (!self.history.refilled_rapidly() || self.history.state.rapid_refills < 3)
    }
    pub(super) async fn response(
        &mut self,
        result: ModelResult,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        self.flight = None;
        check_cancel(cancellation)?;
        let turn = self.history.state.current_turn.clone();
        let compaction = self.history.state.compaction_turns.contains(&turn);
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                if self.recover_overflow(&error, compaction).await? {
                    return Ok(());
                }
                if self.wait_for_quota(&error, compaction) {
                    return Ok(());
                }
                return Err(Error(format!("call model for turn {turn:?}: {error}")));
            }
        };
        self.live.quota_waits = 0;
        let stop = response.stop.clone();
        let failure = response.failure.clone();
        let summary = if compaction {
            crate::context::summary(&response)
        } else {
            String::new()
        };
        self.record(ItemData::ModelResponse(ModelResponse {
            turn_id: turn,
            response,
            compacted: None,
        }))
        .await?;
        if compaction
            && !summary.is_empty()
            && self.journal.supports_checkpoints()
            && let Some(state) = self.history.checkpoint()?
        {
            self.journal.checkpoint(state).await?;
        }
        let statuses = self.schedule().await?;
        if compaction {
            self.compacted_live(&summary).await?;
        } else {
            self.count_turn().await?;
            self.live.step_calls.clear();
            self.live.step_calls.extend(
                statuses
                    .iter()
                    .map(|status| call_key(&status.turn_id, &status.call_id))
                    .filter(|key| self.history.state.calls.contains_key(key)),
            );
            self.answer_stop(&stop, failure.as_ref(), statuses.len())
                .await?;
        }
        for status in &statuses {
            for operation in &status.operations {
                self.dispatch(operation.clone())?;
            }
        }
        if !self.live.call_model && !statuses.is_empty() {
            self.start_grace(&statuses);
        }
        Ok(())
    }
    async fn recover_overflow(&mut self, error: &ModelError, compaction: bool) -> Result<bool> {
        if self.config.no_recovery || self.hard_stop() {
            return Ok(false);
        }
        let ModelFailure::ContextOverflow { tokens, limit } = error.kind else {
            return Ok(false);
        };
        if compaction {
            if self.live.sent.retries >= 3 {
                return Ok(false);
            }
            let base = if self.live.sent.budget > 0 {
                self.live.sent.budget
            } else {
                self.live.sent.estimate
            };
            let mut sent = Sent {
                estimate: self.live.sent.estimate,
                focus: self.live.sent.focus.clone(),
                budget: overflow_budget(
                    base,
                    self.live.sent.estimate,
                    tokens,
                    limit,
                    self.config.context_window,
                ),
                retries: self.live.sent.retries + 1,
            };
            let built = self
                .history
                .context
                .build_compaction(&sent.focus, sent.budget);
            sent.estimate = built.estimated_tokens;
            self.start_request(built, sent);
            return Ok(true);
        }
        let built = self.history.context.build();
        if !built.compactable || !self.auto_compaction_allowed() {
            return Ok(false);
        }
        self.live.overflow_budget = overflow_budget(
            summary_room(self.config.context_window),
            self.live.sent.estimate,
            tokens,
            limit,
            self.config.context_window,
        );
        self.live.call_model = true;
        self.start_turn(false).await?;
        Ok(true)
    }
    fn wait_for_quota(&mut self, error: &ModelError, compaction: bool) -> bool {
        let ModelFailure::RateLimited(mut delay) = error.kind else {
            return false;
        };
        if self.config.no_recovery || self.hard_stop() || self.live.quota_waits >= 2 {
            return false;
        }
        self.live.quota_waits += 1;
        if delay.is_zero() {
            delay = Duration::from_secs(300);
        }
        delay = delay.min(Duration::from_secs(6 * 3600));
        self.hooks.quota_retry(self.live.quota_waits, delay, error);
        self.live.quota_wait = after(delay);
        if compaction {
            self.live.compact_request = Some(self.live.sent.focus.clone());
        } else {
            self.live.call_model = true;
        }
        true
    }
}
fn summary_room(window: i64) -> i64 {
    if window <= 0 {
        0
    } else {
        window.wrapping_sub(20_000.min(window.wrapping_mul(15) / 100))
    }
}
fn overflow_budget(mut base: i64, estimate: i64, tokens: i64, mut limit: i64, window: i64) -> i64 {
    if limit <= 0 {
        limit = window;
    }
    if base <= 0 {
        base = if limit > 0 {
            summary_room(limit)
        } else {
            estimate
        };
    }
    let mut budget = base.wrapping_mul(85) / 100;
    if tokens > 0 && limit > 0 && estimate > 0 {
        let room = summary_room(limit);
        let room = room.wrapping_sub(1000.min((room / 10).max(1)));
        budget = budget.min(estimate.wrapping_mul(room) / tokens);
    }
    if budget >= base {
        budget = base.wrapping_sub(1000.min((base / 10).max(1)));
    }
    budget.max(1)
}
