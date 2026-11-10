use super::*;
use kou_domain::Failure;

impl Coordinator {
    pub(super) async fn answer_stop(
        &mut self,
        stop: &str,
        failure: Option<&Failure>,
        calls: usize,
    ) -> Result<()> {
        if self.hard_stop() || self.config.no_recovery {
            return Ok(());
        }
        if let Some(failure) = failure {
            self.live.failures += 1;
            if self.live.failures > 2 {
                return Err(Error(format!(
                    "the model's response failed {} times in a row: {}: {}",
                    self.live.failures, failure.code, failure.message
                )));
            }
            self.live.call_model = true;
            return Ok(());
        }
        self.live.failures = 0;
        if stop == "max_output_tokens" && calls == 0 {
            self.live.retries = 0;
            if self.live.continues >= 3 {
                return Ok(());
            }
            self.live.continues += 1;
            return self.nudge(format!("{HARNESS} Your previous answer was cut off at the output token limit. Continue exactly from where it stopped, without repeating what you already wrote.")).await;
        }
        if stop == "refused" {
            self.live.continues = 0;
            if self.live.retries >= 1 {
                return Ok(());
            }
            self.live.retries += 1;
            return self.nudge(format!("{HARNESS} The previous response was refused. If the task is complete, say so in a sentence; otherwise carry on with it.")).await;
        }
        self.live.continues = 0;
        self.live.retries = 0;
        if calls == 0 {
            self.verify().await?;
        }
        Ok(())
    }
    pub(super) async fn count_turn(&mut self) -> Result<()> {
        self.live.turns += 1;
        if self.live.wrap_up > 0 && self.history.state.current_turn_inputs >= self.live.wrap_up {
            self.accept_stop(ControlMessage {
                mode: "hard".into(),
                reason: "the run's budget is spent".into(),
                parameters: Json::default(),
            });
        } else if self.config.max_turns > 0 && self.live.turns >= self.config.max_turns {
            self.budget_spent(&format!("{} model turns", self.config.max_turns))
                .await?;
        }
        Ok(())
    }
    pub(super) async fn compacted_live(&mut self, summary: &str) -> Result<()> {
        if summary.is_empty() {
            return Ok(());
        }
        self.live.compactions += 1;
        let file_changes = std::mem::take(&mut self.history.state.file_changes);
        let note = self.hooks.compacted(Compaction {
            summary,
            number: self.live.compactions,
            file_changes,
        });
        let note = note.trim();
        if !note.is_empty()
            && !self.hard_stop()
            && (self.live.call_model || self.pending_inputs() > 0)
        {
            self.nudge(format!("{HARNESS} {note}")).await?;
        }
        if self.config.max_compactions > 0 && self.live.compactions >= self.config.max_compactions {
            self.budget_spent(&format!("{} compactions", self.config.max_compactions))
                .await?;
        }
        Ok(())
    }
    pub(super) async fn budget_spent(&mut self, bound: &str) -> Result<()> {
        if self.live.wrap_up > 0 || self.hard_stop() {
            return Ok(());
        }
        self.nudge(format!("{HARNESS} The run has reached its limit of {bound}. Stop the work here and start nothing new: write the final report for the user, saying what was done, what was not and what is left, and end your turn. The run stops after this answer, and tool calls still running are canceled.")).await?;
        self.live.wrap_up = self.history.state.available_inputs;
        Ok(())
    }
    pub(super) async fn ask_for_report(&mut self) -> Result<()> {
        if self.live.turns == self.live.reported_at || self.live.wrap_up > 0 || self.hard_stop() {
            return Ok(());
        }
        self.live.reported_at = self.live.turns;
        self.nudge(format!("{HARNESS} Progress report: in a few lines, tell the user what you have done since your last report, what you are doing now and what is left, then carry on with the work.")).await
    }
    async fn verify(&mut self) -> Result<()> {
        let state = &self.history.state;
        let limit = if self.config.max_verifications == 0 {
            3
        } else {
            self.config.max_verifications
        };
        let Some(verifier) = &self.verifier else {
            return Ok(());
        };
        if !state.work_since_verification
            || state.verifying.is_some()
            || state.verifications >= limit as i64
            || self.pending_inputs() > 0
            || !self.live.deferred.is_empty()
        {
            return Ok(());
        }
        let Some(spec) = verifier.spec() else {
            return Ok(());
        };
        let id = format!("verification-{}", uuid::Uuid::new_v4());
        let mut check = spec.operation(id.clone());
        check.idempotency = Json::parse(VERIFICATION_MARKER).expect("static marker");
        check.validate().map_err(Error)?;
        self.record_status(ToolCallStatus {
            turn_id: self.history.state.current_turn.clone(),
            call_id: id.clone(),
            status: CallStatus {
                waiting_for: Some(vec![id.clone()]),
                ..CallStatus::default()
            },
            operations: vec![check.clone()],
        })
        .await?;
        self.history.state.work_since_verification = false;
        self.history.state.verifying = Some(id);
        self.dispatch(check)
    }
    pub(super) async fn verified(&mut self, check: &Operation) -> Result<()> {
        self.history.state.verifying = None;
        self.history.state.verifications += 1;
        let Some(verifier) = &self.verifier else {
            return Err(Error(
                "the session has unfinished verification checks, but no verifier is configured"
                    .into(),
            ));
        };
        let (report, passed) = verifier.report(check);
        if !passed && !self.hard_stop() {
            self.nudge(format!("{VERIFICATION} {}", report.trim()))
                .await?;
        }
        Ok(())
    }
}
