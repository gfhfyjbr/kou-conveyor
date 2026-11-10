use super::*;

impl History {
    /// A body-free diagnostic projection. It includes terminal operation headers
    /// omitted from a recovery checkpoint, and never builds or prunes context.
    pub fn inspect(&self) -> impl Serialize + '_ {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct View<'a> {
            available_inputs: i64,
            delivered_inputs: i64,
            current_turn_inputs: i64,
            turns_since_compaction: i64,
            compaction_failures: i64,
            rapid_refills: i64,
            verifications: i64,
            heartbeats: i64,
            file_changes: i64,
            compacted: bool,
            compaction_asked: bool,
            reminded: bool,
            work_since_verification: bool,
            #[serde(rename = "currentTurnID")]
            current_turn: &'a str,
            current_turn_type: &'a str,
            history_work: bool,
            history_verifications: i64,
            history_stop: Option<&'a str>,
            calls: BTreeMap<String, &'a str>,
            operations: BTreeMap<&'a str, &'a str>,
        }
        let state = &self.state;
        View {
            available_inputs: state.available_inputs,
            delivered_inputs: state.delivered_inputs,
            current_turn_inputs: state.current_turn_inputs,
            turns_since_compaction: state.turns_since_compaction,
            compaction_failures: state.compaction_failures,
            rapid_refills: state.rapid_refills,
            verifications: state.verifications,
            heartbeats: state.heartbeats,
            file_changes: state.file_changes,
            compacted: state.compacted,
            compaction_asked: state.compaction_asked,
            reminded: state.reminded,
            work_since_verification: state.work_since_verification,
            current_turn: &state.current_turn,
            current_turn_type: &state.current_turn_type,
            history_work: self.history.work_since_verification,
            history_verifications: self.history.verifications,
            history_stop: self
                .history
                .stopped
                .as_ref()
                .map(|stop| stop.reason.as_str()),
            calls: state
                .calls
                .iter()
                .map(|(key, call)| {
                    (
                        format!("{}/{}", key.turn, key.call),
                        call.call().name.as_str(),
                    )
                })
                .collect(),
            operations: state
                .operations
                .iter()
                .map(|(id, operation)| (id.as_str(), operation.status.as_str()))
                .collect(),
        }
    }
}
