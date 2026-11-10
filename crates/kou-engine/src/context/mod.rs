use kou_domain::{
    ControlMessage, Input, Json, LlmData, LlmItem, Message, Reasoning, Response, Settings,
    ToolCall, ToolResult, ToolResultOutput,
};
use kou_provider::{Model, SharedRequest, Tool};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

mod checkpoint;
mod compaction;
mod image_config;
mod images;
mod input;
mod linked;
mod memory;
mod prune;
#[cfg(test)]
mod tests;
pub use compaction::{clip, summary};
pub use input::{ExternalMessage, LinkedFile, decode_message, external_payload};
pub use memory::{
    Memory, NOTES_LIMIT, NOTES_UPDATE_PREFIX, notes_shown_by, shown_notes, shows_whole_notes,
};

pub const TOOL_CALL_RUNNING: &str = "Tool call is still running. Its result arrives in a later turn: continue with independent work, or end your turn to wait for it.";
const PREAMBLE: &str = include_str!("../../../../harness/contextbuilder/prompts/preamble.md");
const SKILL_PREAMBLE: &str =
    include_str!("../../../../harness/contextbuilder/prompts/skill-preamble.md");

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);
pub type Result<T> = std::result::Result<T, Error>;
type Item = Arc<LlmItem>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Compacted {
    pub message: String,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: String,
    pub manual: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Change {
    pub kind: String,
    pub source: String,
    pub reason: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Report {
    pub changes: Vec<Change>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Built {
    pub request: SharedRequest,
    pub report: Report,
    pub estimated_tokens: i64,
    pub pending_tokens: i64,
    pub compactable: bool,
}

pub type PrunePriority = Arc<dyn Fn(&str) -> bool + Send + Sync>;
#[derive(Clone, Default)]
pub struct PruneOptions {
    pub budget: i64,
    pub keep: usize,
    pub bytes: usize,
    pub first: Option<PrunePriority>,
    pub thinking_turns: usize,
}
#[derive(Clone, Copy, Default)]
pub struct MaskOptions {
    pub results: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct UserInput {
    item: Option<Item>,
    answered: String,
}

pub struct Builder {
    model: Model,
    tools: Vec<Tool>,
    preamble: String,
    system_prompt: String,
    committed: Vec<Item>,
    staged: Vec<Item>,
    recorded_staged: Option<Vec<Item>>,
    recorded_prefix_delta: i64,
    checkpoint_compacted: i64,
    calls: BTreeMap<String, Item>,
    staged_inputs: Vec<UserInput>,
    unanswered: Vec<UserInput>,
    answered: Vec<String>,
    answered_omitted: i64,
    compactable: bool,
    compacted: i64,
    usage: i64,
    usage_mark: usize,
    resumed: bool,
    tool_tokens: i64,
    transcript: String,
    memory: Memory,
    touched: Vec<memory::TouchedFile>,
    failed_calls: HashSet<String>,
    shown_notes: Option<String>,
    pruning: PruneOptions,
    masking: MaskOptions,
    pruned: usize,
    masked: usize,
    thinking_left_out: usize,
    images_left_out: usize,
}
impl Default for Builder {
    fn default() -> Self {
        Self::new(&[])
    }
}
impl Builder {
    pub fn new(skills: &[Skill]) -> Self {
        let mut value = Self {
            model: Model::default(),
            tools: Vec::new(),
            preamble: preamble_with(skills),
            system_prompt: String::new(),
            committed: vec![user_message("")],
            staged: Vec::new(),
            recorded_staged: None,
            recorded_prefix_delta: 0,
            checkpoint_compacted: 0,
            calls: BTreeMap::new(),
            staged_inputs: Vec::new(),
            unanswered: Vec::new(),
            answered: Vec::new(),
            answered_omitted: 0,
            compactable: false,
            compacted: 0,
            usage: 0,
            usage_mark: 0,
            resumed: false,
            tool_tokens: 0,
            transcript: String::new(),
            memory: Memory::default(),
            touched: Vec::new(),
            failed_calls: HashSet::new(),
            shown_notes: None,
            pruning: PruneOptions::default(),
            masking: MaskOptions::default(),
            pruned: 0,
            masked: 0,
            thinking_left_out: 0,
            images_left_out: 0,
        };
        value.set_system_prompt("");
        value
    }
    pub fn set_model(&mut self, model: Model) {
        self.model = model;
    }
    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.system_prompt = prompt.to_owned();
        self.committed[0] = item(LlmData::Message(Message {
            role: "system".into(),
            text: format!("{}\n\n{}", self.preamble, prompt).trim().into(),
            ..Message::default()
        }));
    }
    pub fn set_skills(&mut self, skills: &[Skill]) {
        self.preamble = preamble_with(skills);
        self.set_system_prompt(&self.system_prompt.clone());
    }
    pub fn set_tools(&mut self, tools: Vec<Tool>) {
        self.tool_tokens = tools.iter().map(compaction::estimate_tool).sum();
        self.tools = tools;
    }
    pub fn add_tool(&mut self, tool: Tool) {
        self.tool_tokens += compaction::estimate_tool(&tool);
        self.tools.push(tool);
    }
    pub fn set_pruning(&mut self, options: PruneOptions) {
        self.pruning = options;
    }
    pub fn set_masking(&mut self, options: MaskOptions) {
        self.masking = options;
    }
    pub fn set_memory(&mut self, memory: Memory) {
        self.memory = memory;
    }
    pub fn set_transcript(&mut self, path: String) {
        self.transcript = path;
    }
    pub fn resumed(&mut self) {
        self.resumed = true;
    }

    pub fn add_external_input(&mut self, input: &Input) -> Result<()> {
        if input.kind != "external" {
            return Err(Error(format!(
                "external input {:?} has input kind {:?}",
                input.id, input.kind
            )));
        }
        let message = decode_message(&input.payload)
            .map_err(|error| Error(format!("decode external input {:?}: {error}", input.id)))?;
        let answered = linked::answered_text(&message);
        let value = item(LlmData::Message(Message {
            role: "user".into(),
            text: linked::linked_text(&message.text, &message.files),
            images: (!message.images.is_empty()).then_some(message.images),
            ..Message::default()
        }));
        self.stage(value.clone());
        self.staged_inputs.push(UserInput {
            item: Some(value),
            answered,
        });
        Ok(())
    }
    pub fn add_control(&mut self, control: &ControlMessage) -> Result<()> {
        match control.mode.as_str() {
            "settings" => {
                let settings: Settings = control
                    .parameters
                    .decode()
                    .map_err(|error| Error(error.to_string()))?;
                self.model.reasoning_effort = settings.reasoning_effort;
            }
            "heartbeat" => {
                self.stage(user_message(&control.reason));
                if let Some(notes) = notes_shown_by(&control.reason) {
                    self.shown_notes = Some(notes.to_owned());
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn add_response(&mut self, response: Response) {
        let output = response.output.unwrap_or_default();
        if !output.is_empty() {
            self.compactable = true;
        }
        for value in output {
            let value = Arc::new(value);
            if let LlmData::ToolCall(call) = &value.data {
                self.calls.insert(call.call_id.clone(), value.clone());
            }
            self.committed.push(value);
        }
        self.answered.extend(
            self.unanswered
                .drain(..)
                .map(|value| clip(value.answered.trim(), 12_000)),
        );
        // Once an older prompt no longer fits, newer answers cannot make it fit again.
        let (kept, omitted) = compaction::kept_inputs(&self.answered);
        self.answered = kept;
        self.answered_omitted += omitted as i64;
        self.usage = response
            .usage
            .input_tokens
            .wrapping_add(response.usage.output_tokens);
        self.usage_mark = if self.usage > 0 {
            self.committed.len()
        } else {
            self.usage = 0;
            0
        };
        self.resumed = false;
    }
    pub fn add_reasoning(&mut self, reasoning: Reasoning) {
        self.stage(item(LlmData::Reasoning(reasoning)));
    }
    fn stage(&mut self, value: Item) {
        if let Some(recorded) = &mut self.recorded_staged {
            recorded.push(value.clone());
        }
        self.staged.push(value);
    }
    pub fn add_tool_result(&mut self, call_id: &str, output: Vec<ToolResultOutput>, running: bool) {
        let output = if running {
            running_payload(output)
        } else {
            output
        };
        let value = item(LlmData::ToolResult(ToolResult {
            call_id: call_id.into(),
            output,
        }));
        with_tool_result(&mut self.staged, value.clone());
        if let Some(recorded) = &mut self.recorded_staged {
            with_tool_result(recorded, value);
        }
    }
    pub fn refresh_tool_result(&mut self, call_id: &str, output: Vec<ToolResultOutput>) {
        self.recorded_staged
            .get_or_insert_with(|| self.staged.clone());
        with_tool_result(
            &mut self.staged,
            item(LlmData::ToolResult(ToolResult {
                call_id: call_id.into(),
                output: running_payload(output),
            })),
        );
    }
    pub fn commit(&mut self) {
        memory::remember_failures(&self.staged, &mut self.failed_calls);
        if let Some(recorded) = self.recorded_staged.take() {
            self.recorded_prefix_delta += recorded.len() as i64 - self.staged.len() as i64;
        }
        self.committed.append(&mut self.staged);
        self.unanswered.append(&mut self.staged_inputs);
    }
    pub fn build(&mut self) -> Built {
        self.bound_images();
        self.mask();
        self.prune();
        let mut input: Vec<Item> = self.committed.iter().chain(&self.staged).cloned().collect();
        self.detach_results(&mut input, false);
        let mut changes = Vec::new();
        let mut change = |kind: &str, source: &str, reason: String| {
            changes.push(Change {
                kind: kind.into(),
                source: source.into(),
                reason,
            })
        };
        if self.compacted > 0 {
            change(
                "compacted",
                "conversation",
                format!("a summary replaced {} earlier items", self.compacted),
            );
        }
        if self.masked > 0 {
            change(
                "omitted",
                "tool results",
                format!(
                    "{} tool results beyond the latest {} were masked to save context",
                    self.masked, self.masking.results
                ),
            );
        }
        if self.pruned > 0 {
            change(
                "truncated",
                "tool results",
                format!(
                    "{} older tool results were pruned to save context",
                    self.pruned
                ),
            );
        }
        if self.thinking_left_out > 0 {
            change(
                "omitted",
                "thinking",
                format!(
                    "{} thinking blocks of older turns were left out to save context",
                    self.thinking_left_out
                ),
            );
        }
        if self.images_left_out > 0 {
            change(
                "omitted",
                "images",
                format!(
                    "{} older images were left out to keep the requests within the provider's limits",
                    self.images_left_out
                ),
            );
        }
        Built {
            request: SharedRequest {
                model: self.model.clone(),
                input,
                tools: self.tools.clone(),
            },
            report: Report { changes },
            estimated_tokens: self.estimate(),
            pending_tokens: self
                .unanswered
                .iter()
                .chain(&self.staged_inputs)
                .filter_map(|value| value.item.as_ref())
                .map(|item| compaction::estimate_item(item))
                .sum(),
            compactable: self.compactable,
        }
    }
    fn uncount(&mut self, saved: i64) {
        if self.usage > 0 {
            self.usage = self.usage.saturating_sub(saved).max(0);
        }
    }
    pub(crate) fn recorded_call(&self, id: &str) -> Option<Arc<LlmItem>> {
        self.calls.get(id).cloned()
    }
    fn call(&self, id: &str) -> Option<&ToolCall> {
        self.calls.get(id).and_then(|item| {
            if let LlmData::ToolCall(call) = &item.data {
                Some(call)
            } else {
                None
            }
        })
    }
}
fn item(data: LlmData) -> Item {
    Arc::new(LlmItem {
        provider_id: String::new(),
        model: String::new(),
        data,
    })
}
fn with_data(source: &LlmItem, data: LlmData) -> Item {
    Arc::new(LlmItem {
        provider_id: source.provider_id.clone(),
        model: source.model.clone(),
        data,
    })
}
fn user_message(text: &str) -> Item {
    item(LlmData::Message(Message {
        role: "user".into(),
        text: text.into(),
        ..Message::default()
    }))
}
fn is_running(output: &[ToolResultOutput]) -> bool {
    output.len() == 1 && output[0].kind == "text" && output[0].value.starts_with(TOOL_CALL_RUNNING)
}
fn running_payload(output: Vec<ToolResultOutput>) -> Vec<ToolResultOutput> {
    let mut text = TOOL_CALL_RUNNING.to_owned();
    if let Some(part) = output
        .iter()
        .find(|part| part.kind == "text" && !part.value.trim().is_empty())
    {
        text.push('\n');
        text.push_str(part.value.trim());
    }
    vec![ToolResultOutput {
        kind: "text".into(),
        value: text,
    }]
}
fn with_tool_result(items: &mut Vec<Item>, value: Item) {
    let LlmData::ToolResult(next) = &value.data else {
        unreachable!()
    };
    items.retain(|item| !matches!(&item.data, LlmData::ToolResult(result) if result.call_id == next.call_id && is_running(&result.output)));
    items.push(value);
}
fn preamble_with(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return PREAMBLE.trim().into();
    }
    fn xml(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&#34;")
            .replace('\'', "&#39;")
            .replace('\t', "&#x9;")
            .replace('\n', "&#xA;")
            .replace('\r', "&#xD;")
    }
    let mut parts = vec![PREAMBLE.trim().to_owned(), SKILL_PREAMBLE.trim().to_owned()];
    let mut encoded = String::new();
    let mut manual = Vec::new();
    for skill in skills {
        if skill.manual {
            manual.push(skill.name.as_str());
            continue;
        }
        encoded.push_str(&format!(
            "<skill><name>{}</name><description>{}</description><location>{}</location></skill>",
            xml(&skill.name),
            xml(&skill.description),
            xml(&skill.path)
        ));
    }
    if !encoded.is_empty() {
        parts.push(format!("<available_skills>{encoded}</available_skills>"));
    }
    if !manual.is_empty() {
        parts.push(format!(
            "Load these skills with SkillUse only when the user asks for one of them by name: {}.",
            manual.join(", ")
        ));
    }
    parts.join("\n\n")
}
