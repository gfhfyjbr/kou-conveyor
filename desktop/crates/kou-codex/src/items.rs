use base64::{Engine as _, engine::general_purpose::STANDARD};
use kou_thread::{
    Block, Level, Location, ToolCall, ToolContent, ToolKind, ToolPatch, ToolStatus, Update,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub(crate) fn input(blocks: &[Block]) -> Vec<Value> {
    fn encode(block: &Block) -> Value {
        match block {
            Block::Text(text) => json!({"type": "text", "text": text, "text_elements": []}),
            Block::Image { mime, data } => {
                let url = format!("data:{mime};base64,{}", STANDARD.encode(data));
                json!({"type": "image", "url": url})
            }
            Block::File { path, text } => {
                let mut reference = format!("@{}", path.display());
                if let Some(text) = text {
                    reference.push('\n');
                    reference.push_str(text);
                }
                json!({"type": "text", "text": reference, "text_elements": []})
            }
        }
    }
    blocks.iter().map(encode).collect()
}

fn image(url: &str) -> Option<Block> {
    let (mime, bytes) = url.strip_prefix("data:")?.split_once(";base64,")?;
    Some(Block::Image {
        mime: mime.into(),
        data: STANDARD.decode(bytes).ok()?.into(),
    })
}

fn block(value: &Value) -> Option<Block> {
    match value["type"].as_str()? {
        "text" => Some(Block::Text(value["text"].as_str()?.into())),
        "localImage" | "mention" | "skill" => Some(Block::File {
            path: value["path"].as_str()?.into(),
            text: None,
        }),
        "image" => Some(
            value["url"]
                .as_str()
                .and_then(image)
                .unwrap_or_else(|| Block::Text("[Image]".into())),
        ),
        _ => None,
    }
}

/// Per-turn state shared by live items and the snapshot of a resumed turn.
#[derive(Default)]
pub(crate) struct Items {
    partial: HashMap<String, String>,
    // A resume snapshot can overtake deltas already queued on the wire.
    // Reconcile these open items against their next full snapshot.
    snapshot_only: HashSet<String>,
    completed: HashSet<String>,
    users: HashSet<String>,
    pub suppress_first_user: bool,
    last_message: Option<String>,
}

impl Items {
    pub fn delta(&mut self, method: &str, params: &Value) -> Option<Update> {
        let id = params["itemId"].as_str()?;
        let more = params["delta"].as_str()?;
        if self.completed.contains(id) || self.snapshot_only.contains(id) || more.is_empty() {
            return None;
        }
        match method {
            "item/agentMessage/delta" | "item/plan/delta" => {
                self.partial.entry(id.into()).or_default().push_str(more);
                Some(self.words(id, more))
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                self.partial.entry(id.into()).or_default().push_str(more);
                self.last_message = None;
                Some(Update::ThoughtChunk(more.into()))
            }
            "item/commandExecution/outputDelta" => {
                let output = self.partial.entry(id.into()).or_default();
                output.push_str(more);
                Some(Update::ToolPatch(ToolPatch {
                    id: id.into(),
                    content: Some(vec![ToolContent::Terminal {
                        id: id.into(),
                        output: output.clone(),
                        exit: None,
                    }]),
                    ..Default::default()
                }))
            }
            _ => None,
        }
    }

    fn words(&mut self, id: &str, text: &str) -> Update {
        let separate = self
            .last_message
            .as_deref()
            .is_some_and(|previous| previous != id);
        self.last_message = Some(id.into());
        Update::AssistantChunk(if separate {
            format!("\n\n{text}")
        } else {
            text.into()
        })
    }

    pub fn restore(&mut self, turn: &str, item: &Value, active: bool, cwd: &Path) -> Vec<Update> {
        let done = !active
            || item["completedAtMs"].is_number()
            || item["status"]
                .as_str()
                .is_some_and(|status| status != "inProgress");
        if !done && let Some(id) = item["id"].as_str() {
            let kind = item["type"].as_str();
            let text = match kind {
                Some("agentMessage" | "plan") => item["text"].as_str().unwrap_or_default().into(),
                Some("reasoning") => reasoning(item),
                _ => String::new(),
            };
            if !text.is_empty() {
                self.snapshot_only.insert(id.into());
                self.partial.insert(id.into(), text.clone());
                return vec![if kind == Some("reasoning") {
                    self.last_message = None;
                    Update::ThoughtChunk(text)
                } else {
                    self.words(id, &text)
                }];
            }
        }
        self.item(turn, item, done, cwd)
    }

    pub fn item(&mut self, turn: &str, item: &Value, done: bool, cwd: &Path) -> Vec<Update> {
        let Some(id) = item["id"].as_str() else {
            return vec![];
        };
        if self.completed.contains(id) {
            return vec![];
        }
        if done {
            self.completed.insert(id.into());
        }
        let kind = item["type"].as_str().unwrap_or_default();
        if kind == "userMessage" {
            let first = self.users.is_empty();
            if !self.users.insert(id.into()) || (first && self.suppress_first_user) {
                return vec![];
            }
            self.last_message = None;
            let mut updates: Vec<_> = item["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(block)
                .map(Update::UserChunk)
                .collect();
            if !updates.is_empty() {
                // Codex forks at turn boundaries. Keep a distinct ID for a
                // message steered into a turn, so an edit cannot omit earlier work.
                updates.push(Update::PromptId(if first {
                    turn.into()
                } else {
                    format!("{turn}/{id}")
                }));
            }
            return updates;
        }
        let previous = if done {
            self.partial.remove(id).unwrap_or_default()
        } else {
            String::new()
        };
        let update = match kind {
            "agentMessage" | "plan" => {
                // A completed item contains the whole message, including any
                // prefix already displayed from deltas.
                let whole = item["text"].as_str().unwrap_or_default();
                let rest = whole.strip_prefix(&previous).unwrap_or("");
                if !done || rest.is_empty() {
                    return vec![];
                }
                self.words(id, rest)
            }
            "reasoning" => {
                if !done {
                    return vec![];
                }
                let whole = reasoning(item);
                let rest = whole.strip_prefix(&previous).unwrap_or("");
                if rest.is_empty() {
                    return vec![];
                }
                self.last_message = None;
                Update::ThoughtChunk(rest.into())
            }
            "contextCompaction" if done => {
                Update::Notice(Level::Info, "Codex compacted the conversation.".into())
            }
            _ => {
                let Some(call) = tool(item, done, &previous, cwd) else {
                    return vec![];
                };
                self.last_message = None;
                Update::Tool(call)
            }
        };
        vec![update]
    }
}

fn reasoning(item: &Value) -> String {
    let summary = strings(&item["summary"]);
    if summary.is_empty() {
        strings(&item["content"]).join("\n\n")
    } else {
        summary.join("\n\n")
    }
}

fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn tool(item: &Value, done: bool, streamed: &str, cwd: &Path) -> Option<ToolCall> {
    let id = item["id"].as_str()?;
    let mut call = ToolCall {
        id: id.into(),
        status: match item["status"].as_str() {
            Some("failed" | "declined") => ToolStatus::Failed,
            Some("interrupted") => ToolStatus::Cancelled,
            Some("inProgress") => ToolStatus::Running,
            _ if done => ToolStatus::Done,
            _ => ToolStatus::Running,
        },
        ..Default::default()
    };
    match item["type"].as_str()? {
        "commandExecution" => {
            let command = item["command"].as_str().unwrap_or("Command");
            call.kind = ToolKind::Execute;
            call.title = command.into();
            call.raw_input = Some(json!({"command":command,"cwd":item["cwd"]}));
            call.content.push(ToolContent::Terminal {
                id: id.into(),
                output: item["aggregatedOutput"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(streamed)
                    .into(),
                exit: item["exitCode"].as_i64().and_then(|n| n.try_into().ok()),
            });
        }
        "fileChange" => {
            call.kind = ToolKind::Edit;
            for change in item["changes"].as_array().into_iter().flatten() {
                if let Some(path) = change["path"].as_str() {
                    call.locations.push(Location {
                        path: cwd.join(path),
                        line: None,
                    });
                    if let Some(diff) = change["diff"].as_str() {
                        call.content
                            .push(ToolContent::Text(format!("{path}\n```diff\n{diff}\n```")));
                    }
                }
            }
            call.title = match &call.locations[..] {
                [location] => format!(
                    "Edit {}",
                    location
                        .path
                        .strip_prefix(cwd)
                        .unwrap_or(&location.path)
                        .display()
                ),
                paths => format!("Edit {} files", paths.len()),
            };
            call.raw_input = Some(item["changes"].clone());
        }
        "mcpToolCall" | "dynamicToolCall" => {
            let tool = item["tool"].as_str().unwrap_or("Tool");
            call.title = item["server"]
                .as_str()
                .map_or_else(|| tool.into(), |server| format!("{server} / {tool}"));
            call.raw_input = Some(item["arguments"].clone());
            let output = item
                .get("result")
                .filter(|v| !v.is_null())
                .or_else(|| item.get("contentItems"));
            if let Some(output) = output {
                call.raw_output = Some(output.clone());
                for part in output
                    .get("content")
                    .unwrap_or(output)
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if let Some(text) = part["text"].as_str() {
                        call.content.push(ToolContent::Text(text.into()));
                    }
                }
            }
            if let Some(error) = item["error"]["message"].as_str() {
                call.content.push(ToolContent::Text(error.into()));
                call.status = ToolStatus::Failed;
            }
        }
        "collabAgentToolCall" => {
            let kind = item["tool"].as_str().unwrap_or("Agent");
            call.title = item["prompt"].as_str().unwrap_or(kind).into();
            if kind == "spawnAgent" {
                call.agent = Some("Codex".into());
            }
            call.raw_input = Some(item.clone());
            for state in item["agentsStates"]
                .as_object()
                .into_iter()
                .flat_map(|v| v.values())
            {
                if let Some(message) = state["message"].as_str() {
                    call.content.push(ToolContent::Text(message.into()));
                }
            }
        }
        "webSearch" => {
            call.kind = ToolKind::Search;
            call.title = format!("Search {}", item["query"].as_str().unwrap_or("the web"));
        }
        "imageView" => {
            call.kind = ToolKind::Read;
            call.title = format!("View {}", item["path"].as_str().unwrap_or("image"));
        }
        "imageGeneration" => {
            call.title = "Generate image".into();
        }
        _ => return None,
    }
    Some(call)
}
