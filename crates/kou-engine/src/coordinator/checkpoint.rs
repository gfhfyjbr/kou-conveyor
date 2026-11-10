use super::*;
use kou_domain::Json;

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Checkpoint {
    #[serde(deserialize_with = "zero")]
    version: i64,
    builder: Json,
    #[serde(rename = "CurrentTurnID", deserialize_with = "zero")]
    current_turn: String,
    #[serde(deserialize_with = "zero")]
    current_turn_type: String,
    turn_models: Option<BTreeMap<String, String>>,
    compaction_turns: Option<BTreeMap<String, Json>>,
    #[serde(deserialize_with = "zero")]
    tool_calls: Vec<SavedCall>,
    #[serde(deserialize_with = "zero")]
    operations: Vec<Arc<Operation>>,
    #[serde(deserialize_with = "zero")]
    available_inputs: i64,
    #[serde(deserialize_with = "zero")]
    delivered_inputs: i64,
    #[serde(deserialize_with = "zero")]
    current_turn_inputs: i64,
    #[serde(deserialize_with = "zero")]
    compacted: bool,
    #[serde(deserialize_with = "zero")]
    compaction_asked: bool,
    #[serde(deserialize_with = "zero")]
    turns_since_compaction: i64,
    #[serde(deserialize_with = "zero")]
    compaction_failures: i64,
    #[serde(deserialize_with = "zero")]
    rapid_refills: i64,
    #[serde(deserialize_with = "zero")]
    reminded: bool,
    #[serde(deserialize_with = "zero")]
    history: ReplayHistory,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct SavedCall {
    #[serde(deserialize_with = "zero")]
    turn: String,
    #[serde(deserialize_with = "zero")]
    call: ToolCall,
    #[serde(deserialize_with = "zero")]
    operations: Vec<String>,
    status: Option<CallStatus>,
}
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SavedCallView<'a> {
    turn: &'a str,
    call: &'a ToolCall,
    operations: Vec<&'a str>,
    status: &'a Option<CallStatus>,
}
fn zero<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(
    de: D,
) -> std::result::Result<T, D::Error> {
    Ok(Option::<T>::deserialize(de)?.unwrap_or_default())
}

impl History {
    /// None means this boundary has no replay-stable stop timestamp and must use
    /// full item replay. The caller writes Some through SessionWriter's envelope.
    pub fn checkpoint(&self) -> Result<Option<Json>> {
        if self
            .history
            .stopped
            .as_ref()
            .is_some_and(|stop| stop.at == Timestamp::default())
        {
            return Ok(None);
        }
        let mut needed = BTreeSet::new();
        let mut live_calls = Vec::new();
        let mut calls = Vec::new();
        for (key, value) in &self.state.calls {
            live_calls.push(key.call.clone());
            needed.extend(value.operations.iter().cloned());
            calls.push(SavedCallView {
                turn: &key.turn,
                call: value.call(),
                status: &value.status,
                operations: value.operations.iter().map(String::as_str).collect(),
            });
        }
        // The wire serializer streams borrowed payloads; only active call headers
        // are staged here. Finished operation bodies are not checkpointed.
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct View<'a> {
            version: i64,
            builder: Json,
            #[serde(rename = "CurrentTurnID")]
            current_turn: &'a str,
            current_turn_type: &'a str,
            turn_models: &'a BTreeMap<String, String>,
            compaction_turns: BTreeMap<&'a str, BTreeMap<String, String>>,
            tool_calls: Vec<SavedCallView<'a>>,
            operations: Vec<&'a Arc<Operation>>,
            available_inputs: i64,
            delivered_inputs: i64,
            current_turn_inputs: i64,
            compacted: bool,
            compaction_asked: bool,
            turns_since_compaction: i64,
            compaction_failures: i64,
            rapid_refills: i64,
            reminded: bool,
            history: &'a ReplayHistory,
        }
        let state = &self.state;
        let value = View {
            version: 1,
            builder: self.context.checkpoint(&live_calls)?,
            current_turn: &state.current_turn,
            current_turn_type: &state.current_turn_type,
            turn_models: &state.turn_models,
            compaction_turns: state
                .compaction_turns
                .iter()
                .map(|turn| (turn.as_str(), BTreeMap::new()))
                .collect(),
            tool_calls: calls,
            operations: state
                .operations
                .values()
                .filter(|operation| needed.contains(&operation.id) || !operation.terminal())
                .collect(),
            available_inputs: state.available_inputs,
            delivered_inputs: state.delivered_inputs,
            current_turn_inputs: state.current_turn_inputs,
            compacted: state.compacted,
            compaction_asked: state.compaction_asked,
            turns_since_compaction: state.turns_since_compaction,
            compaction_failures: state.compaction_failures,
            rapid_refills: state.rapid_refills,
            reminded: state.reminded,
            history: &self.history,
        };
        let text = serde_json::to_string(&value).map_err(|error| Error(error.to_string()))?;
        Json::parse(&text)
            .map(Some)
            .map_err(|error| Error(error.to_string()))
    }
    /// A false result leaves both the context and the coordinator untouched, so
    /// store recovery can fall back to its complete item archive.
    pub fn restore_checkpoint(&mut self, encoded: &Json) -> bool {
        let Some(raw) = encoded.as_str() else {
            return false;
        };
        if !raw.trim_start().starts_with('{') {
            return false;
        }
        let Ok(value) = encoded.decode::<Checkpoint>() else {
            return false;
        };
        if value.version != 1
            || value.current_turn.is_empty()
            || value.turn_models.is_none()
            || value.compaction_turns.is_none()
            || value.current_turn_type != "compaction"
            || value.available_inputs < 0
            || value.delivered_inputs < 0
            || value.current_turn_inputs < 0
            || value.delivered_inputs > value.available_inputs
            || value.current_turn_inputs > value.available_inputs
        {
            return false;
        }
        let mut calls = BTreeMap::new();
        for call in value.tool_calls {
            if call.turn.is_empty() || call.call.call_id.is_empty() {
                return false;
            }
            if call.status.as_ref().is_some_and(|status| {
                status.error.is_empty()
                    || !status.waiting().is_empty()
                    || !call.operations.is_empty()
            }) && !self.tools.available(&call.call.name)
            {
                return false;
            }
            let key = call_key(&call.turn, &call.call.call_id);
            calls.insert(
                key,
                ActiveCall {
                    item: Arc::new(LlmItem {
                        model: String::new(),
                        provider_id: String::new(),
                        data: LlmData::ToolCall(call.call),
                    }),
                    status: call.status,
                    operations: call.operations.into_iter().collect(),
                },
            );
        }
        if self.context.restore_checkpoint(&value.builder).is_err() {
            return false;
        }
        for (key, value) in &mut calls {
            if let Some(shared) = self.context.recorded_call(&key.call)
                && shared.data == value.item.data
            {
                value.item = shared;
            }
        }
        let state = &mut self.state;
        state.current_turn = value.current_turn;
        state.current_turn_type = value.current_turn_type;
        state.turn_models = value.turn_models.unwrap();
        state.compaction_turns = value.compaction_turns.unwrap().into_keys().collect();
        state.calls = calls;
        for operation in value.operations {
            state.operations.insert(operation.id.clone(), operation);
        }
        state.available_inputs = value.available_inputs;
        state.delivered_inputs = value.delivered_inputs;
        state.current_turn_inputs = value.current_turn_inputs;
        state.compacted = value.compacted;
        state.compaction_asked = value.compaction_asked;
        state.turns_since_compaction = value.turns_since_compaction;
        state.compaction_failures = value.compaction_failures;
        state.rapid_refills = value.rapid_refills;
        state.reminded = value.reminded;
        state.work_since_verification = value.history.work_since_verification;
        state.verifications = value.history.verifications;
        if let Some(stop) = &value.history.stopped {
            state.stopped = Some(stop.clone());
        }
        self.history = value.history;
        true
    }
}
