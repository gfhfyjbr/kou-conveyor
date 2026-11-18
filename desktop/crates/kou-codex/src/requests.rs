//! Server-initiated requests. Keep the JSON-RPC id verbatim (number or
//! string), separately from the request and item IDs used by the view.

use crate::rpc::Rpc;
use anyhow::Result;
use kou_thread::{
    Level, PermissionKind, PermissionOption, PermissionRequest, Question, QuestionOption, ToolCall,
    ToolContent, ToolKind, ToolPatch, ToolStatus, Update,
    elicitation::{self, Field},
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Requests {
    next: u64,
    open: HashMap<u64, Open>,
}

struct Open {
    wire_id: Value,
    request: PermissionRequest,
    replies: Vec<(String, Value)>,
    fields: Answers,
    asynchronous: bool,
}

enum Answers {
    None,
    Questions(Vec<String>),
    Form(Vec<Field>),
}

impl Requests {
    pub fn receive(
        &mut self,
        rpc: &Rpc,
        message: &Value,
        updates: &async_channel::Sender<Update>,
    ) -> Result<()> {
        let wire_id = &message["id"];
        if self.open.values().any(|r| r.wire_id == *wire_id) {
            return Ok(());
        }
        let p = &message["params"];
        self.next += 1;
        let id = self.next;
        let mut tool = ToolCall {
            id: p["itemId"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("codex-request-{id}")),
            status: ToolStatus::Pending,
            raw_input: Some(p.clone()),
            ..Default::default()
        };
        if let Some(reason) = p["reason"].as_str() {
            tool.content.push(ToolContent::Text(reason.into()));
        }
        let mut options = Vec::new();
        let mut replies = Vec::new();
        let mut questions = Vec::new();
        let mut option = |key: &str, name: &str, kind, result| {
            options.push(PermissionOption {
                id: key.into(),
                label: name.into(),
                kind,
            });
            replies.push((key.to_owned(), result));
        };
        use PermissionKind::{AllowAlways, AllowOnce, RejectOnce};
        let fields = match message["method"].as_str().unwrap_or("") {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let command = message["method"] == "item/commandExecution/requestApproval";
                tool.kind = if command {
                    ToolKind::Execute
                } else {
                    ToolKind::Edit
                };
                let title = if command {
                    "Run command"
                } else {
                    "Apply file changes"
                };
                tool.title = p["command"].as_str().unwrap_or(title).into();
                let defaults = [json!("accept"), json!("decline")];
                let allowed = p["availableDecisions"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&defaults);
                for decision in allowed {
                    let (key, label, kind) = match decision.as_str() {
                        Some("accept") => ("accept", "Allow once", AllowOnce),
                        Some("acceptForSession") if p["kind"] != "humanCommand" => {
                            ("acceptForSession", "Allow for this session", AllowAlways)
                        }
                        Some("decline") => ("decline", "Decline", RejectOnce),
                        Some("cancel") => ("cancel", "Cancel turn", RejectOnce),
                        _ => continue,
                    };
                    option(key, label, kind, json!({"decision":decision}));
                }
                Answers::None
            }
            "item/permissions/requestApproval" => {
                tool.title = "Grant additional permissions".into();
                tool.content
                    .push(ToolContent::Text(p["permissions"].to_string()));
                let grant = json!({"permissions": p["permissions"], "scope": "turn"});
                option("accept", "Grant for this turn", AllowOnce, grant);
                option(
                    "decline",
                    "Decline",
                    RejectOnce,
                    json!({"permissions": {}, "scope": "turn"}),
                );
                Answers::None
            }
            "item/tool/requestUserInput" => {
                tool.title = "Questions from Codex".into();
                let mut keys = Vec::new();
                for q in p["questions"].as_array().into_iter().flatten() {
                    // The ordinary composer records its answers as plain text.
                    if q["isSecret"] == true {
                        return rpc.error(wire_id, -32602, "Secret input requires a secure field");
                    }
                    if let Some((key, question)) = user_question(q) {
                        keys.push(key);
                        questions.push(question);
                    }
                }
                option("skip", "Skip", RejectOnce, json!({"answers":{}}));
                Answers::Questions(keys)
            }
            "mcpServer/elicitation/request" => {
                tool.title = p["message"].as_str().unwrap_or("MCP server request").into();
                let (fields, form_questions): (Vec<_>, Vec<_>) =
                    elicitation::form(&p["requestedSchema"], None)
                        .into_iter()
                        .unzip();
                questions = form_questions;
                if p["mode"] == "url" {
                    if let Some(url) = p["url"].as_str() {
                        tool.content.push(ToolContent::Text(url.into()));
                    }
                    option(
                        "accept",
                        "I have opened the link",
                        AllowOnce,
                        json!({"action":"accept","content":null,"_meta":null}),
                    );
                } else if questions.is_empty()
                    && p["requestedSchema"]["properties"]
                        .as_object()
                        .is_some_and(Map::is_empty)
                {
                    option(
                        "accept",
                        "Accept",
                        AllowOnce,
                        json!({"action":"accept","content":{},"_meta":null}),
                    );
                }
                option(
                    "decline",
                    "Decline",
                    RejectOnce,
                    json!({"action":"decline","content":null,"_meta":null}),
                );
                Answers::Form(fields)
            }
            _ => return rpc.error(wire_id, -32601, "Unsupported Codex client request"),
        };
        if options.is_empty() && questions.is_empty() {
            return rpc.error(wire_id, -32602, "No supported decision was offered");
        }
        let request = PermissionRequest {
            id,
            tool,
            options,
            questions,
        };
        self.open.insert(
            id,
            Open {
                wire_id: wire_id.clone(),
                request: request.clone(),
                replies,
                fields,
                asynchronous: p["isBlocking"] == false || p["turnId"].is_null(),
            },
        );
        let _ = updates.try_send(Update::Permission(request));
        Ok(())
    }

    pub fn answer(
        &mut self,
        rpc: &Rpc,
        id: u64,
        option: Option<&str>,
        answers: &[Vec<String>],
        updates: &async_channel::Sender<Update>,
    ) -> Result<()> {
        let Some(open) = self.open.get(&id) else {
            return Ok(());
        };
        let result = if let Some(option) = option {
            open.replies
                .iter()
                .find(|(key, _)| key == option)
                .map(|(_, result)| result.clone())
        } else {
            match &open.fields {
                Answers::None => None,
                Answers::Questions(keys) => {
                    let values: Map<_, _> = keys
                        .iter()
                        .zip(answers)
                        .map(|(key, a)| (key.clone(), json!({"answers":a})))
                        .collect();
                    Some(json!({"answers":values}))
                }
                Answers::Form(fields) => match elicitation::form_content(fields, answers) {
                    Ok(content) => Some(json!({"action":"accept","content":content,"_meta":null})),
                    Err(why) => {
                        let _ = updates.try_send(Update::Notice(Level::Warning, why));
                        None
                    }
                },
            }
        };
        let Some(result) = result else {
            let _ = updates.try_send(Update::Permission(open.request.clone()));
            return Ok(());
        };
        rpc.write(json!({"id":open.wire_id,"result":result}))?;
        let status = if option.is_some_and(|o| matches!(o, "decline" | "cancel" | "skip")) {
            ToolStatus::Cancelled
        } else if matches!(open.fields, Answers::None) {
            ToolStatus::Running
        } else {
            ToolStatus::Done
        };
        let _ = updates.try_send(Update::ToolPatch(ToolPatch {
            id: open.request.tool.id.clone(),
            status: Some(status),
            ..Default::default()
        }));
        let _ = updates.try_send(Update::Withdrawn(id));
        self.open.remove(&id);
        Ok(())
    }

    pub fn resolved(&mut self, wire_id: &Value) -> Option<Update> {
        let id = self
            .open
            .iter()
            .find(|(_, r)| r.wire_id == *wire_id)?
            .0
            .to_owned();
        self.open.remove(&id);
        Some(Update::Withdrawn(id))
    }

    pub fn turn_ended(&mut self, updates: &async_channel::Sender<Update>) {
        self.open.retain(|_, open| open.asynchronous);
        // TurnEnded clears the view's requests. Non-blocking questions can
        // still take an answer until serverRequest/resolved withdraws them.
        for open in self.open.values() {
            let _ = updates.try_send(Update::Permission(open.request.clone()));
        }
    }
}

fn user_question(value: &Value) -> Option<(String, Question)> {
    let key = value["id"].as_str()?.to_owned();
    let text = value["question"].as_str()?.to_owned();
    let options: Vec<_> = value["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            Some(QuestionOption {
                label: option["label"].as_str()?.into(),
                description: option["description"].as_str().map(str::to_owned),
            })
        })
        .collect();
    Some((
        key,
        Question {
            text,
            header: value["header"].as_str().map(str::to_owned),
            free: options.is_empty() || value["isOther"] == true,
            multiple: false,
            options,
        },
    ))
}
