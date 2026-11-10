use super::*;
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Checkpoint {
    #[serde(deserialize_with = "input::zero")]
    version: i64,
    #[serde(deserialize_with = "input::zero")]
    committed: Vec<Item>,
    #[serde(deserialize_with = "input::zero")]
    staged: Vec<Item>,
    #[serde(deserialize_with = "input::zero")]
    calls: BTreeMap<String, ToolCall>,
    #[serde(deserialize_with = "input::zero")]
    staged_inputs: Vec<UserInput>,
    #[serde(deserialize_with = "input::zero")]
    unanswered: Vec<UserInput>,
    #[serde(deserialize_with = "input::zero")]
    answered: Vec<String>,
    #[serde(deserialize_with = "input::zero")]
    answered_omitted: i64,
    #[serde(deserialize_with = "input::zero")]
    compactable: bool,
    #[serde(deserialize_with = "input::zero")]
    compacted: i64,
    #[serde(deserialize_with = "input::zero")]
    touched: Vec<memory::TouchedFile>,
    shown_notes: Option<String>,
    #[serde(deserialize_with = "input::zero")]
    reasoning_effort: String,
}
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct CheckpointView<'a> {
    version: i64,
    committed: &'a [Item],
    staged: &'a [Item],
    calls: BTreeMap<&'a str, &'a ToolCall>,
    staged_inputs: &'a [UserInput],
    unanswered: &'a [UserInput],
    answered: &'a [String],
    answered_omitted: i64,
    compactable: bool,
    compacted: i64,
    touched: &'a [memory::TouchedFile],
    shown_notes: &'a Option<String>,
    reasoning_effort: &'a str,
}
impl Builder {
    pub fn checkpoint(&self, live_calls: &[String]) -> Result<Json> {
        if self.compactable
            || self.usage != 0
            || self.usage_mark != 0
            || self.pruned != 0
            || self.masked != 0
            || self.thinking_left_out != 0
            || self.images_left_out != 0
        {
            return Err(Error("context checkpoint must follow compaction".into()));
        }
        let mut calls = BTreeMap::new();
        let mut keep = |id: &str| {
            if let Some(call) = self.call(id) {
                calls.insert(call.call_id.as_str(), call);
            }
        };
        for id in live_calls {
            keep(id);
        }
        for item in self.committed.iter().chain(&self.staged) {
            match &item.data {
                LlmData::ToolCall(call) => keep(&call.call_id),
                LlmData::ToolResult(result) => keep(&result.call_id),
                _ => {}
            }
        }
        let value = CheckpointView {
            version: 1,
            committed: &self.committed[1..],
            staged: self.recorded_staged.as_deref().unwrap_or(&self.staged),
            calls,
            staged_inputs: &self.staged_inputs,
            unanswered: &self.unanswered,
            answered: &self.answered,
            answered_omitted: self.answered_omitted,
            compactable: self.compactable,
            compacted: self.checkpoint_compacted,
            touched: &self.touched,
            shown_notes: &self.shown_notes,
            reasoning_effort: &self.model.reasoning_effort,
        };
        let encoded = serde_json::to_string(&value).map_err(|error| Error(error.to_string()))?;
        Json::parse(&encoded).map_err(|error| Error(error.to_string()))
    }
    pub fn restore_checkpoint(&mut self, encoded: &Json) -> Result<()> {
        let raw = encoded.as_str().unwrap_or("null").trim_start();
        if !raw.starts_with('{') && raw != "null" {
            return Err(Error(
                "decode context checkpoint: expected object or null".into(),
            ));
        }
        let value: Checkpoint = if raw == "null" {
            Checkpoint::default()
        } else {
            encoded
                .decode()
                .map_err(|error| Error(format!("decode context checkpoint: {error}")))?
        };
        if value.version != 1
            || value.compacted < 0
            || value.compactable
            || value.committed.is_empty()
            || value.touched.len() > 20
            || value.answered_omitted < 0
        {
            return Err(Error("unsupported or invalid context checkpoint".into()));
        }
        if !matches!(&value.committed[0].data, LlmData::Message(message) if message.role == "user" && !message.text.is_empty())
        {
            return Err(Error(
                "context checkpoint has no compacted user summary".into(),
            ));
        }
        let (kept, omitted) = compaction::kept_inputs(&value.answered);
        if omitted != 0 || kept.len() != value.answered.len() {
            return Err(Error(
                "context checkpoint exceeds the earlier-prompt budget".into(),
            ));
        }
        let mut committed = vec![self.committed[0].clone()];
        committed.extend(value.committed);
        self.committed = committed;
        self.staged = value.staged;
        self.calls = value
            .calls
            .into_iter()
            .map(|(id, call)| (id, item(LlmData::ToolCall(call))))
            .collect();
        self.staged_inputs = value.staged_inputs;
        self.unanswered = value.unanswered;
        self.answered = value.answered;
        self.answered_omitted = value.answered_omitted;
        self.compactable = value.compactable;
        self.compacted = value.compacted;
        self.checkpoint_compacted = value.compacted;
        self.recorded_prefix_delta = 0;
        self.failed_calls.clear();
        self.recorded_staged = None;
        self.touched = value.touched;
        self.shown_notes = value.shown_notes;
        self.model.reasoning_effort = value.reasoning_effort;
        self.pruned = 0;
        self.masked = 0;
        self.thinking_left_out = 0;
        self.images_left_out = 0;
        self.usage = 0;
        self.usage_mark = 0;
        Ok(())
    }
}
