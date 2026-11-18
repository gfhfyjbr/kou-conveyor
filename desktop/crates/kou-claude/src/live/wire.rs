//! The messages written to Claude Code, and what is read out of its answers,
//! in the shapes of the Agent SDK's types (`sdk.d.ts`): `SDKUserMessage`,
//! `SDKControlRequest` and `SDKControlResponse`, `PermissionResult`,
//! `SDKControlInitializeResponse`, `SDKResultMessage`.

use base64::Engine as _;
use kou_thread::{Block, Choice, PermissionKind, PermissionOption, Stop};
use serde_json::{Value, json};
use std::path::Path;

/// What marks the app's own note in a prompt (see [`preface`]): a record
/// read back shows it, and whoever reads it can leave it out.
pub(crate) const PREFACE_OPEN: &str = "<kou-desktop>";
pub(crate) const PREFACE_CLOSE: &str = "</kou-desktop>";

/// `text` as a note of the app's that goes before a first prompt: marked,
/// and said not to be the user's.
pub(crate) fn preface(text: &str) -> String {
    format!(
        "{PREFACE_OPEN}\nA note from the app this session runs in, not from the user:\n{}\n{PREFACE_CLOSE}",
        text.trim()
    )
}

/// A prompt (or a message into a turn at work) as Claude Code reads it: a
/// `user` message whose blocks are the prompt's, after the app's note when
/// one is due. `uuid` names it, in the echo and in the conversation's record.
pub(crate) fn user_message(
    blocks: &[Block],
    note: Option<&str>,
    uuid: &str,
    session: &str,
) -> Value {
    let mut content = Vec::new();
    if let Some(note) = note {
        content.push(json!({"type": "text", "text": preface(note)}));
    }
    content.extend(prompt_content(blocks));
    json!({
        "type": "user",
        "message": {"role": "user", "content": content},
        "parent_tool_use_id": null,
        "session_id": session,
        "uuid": uuid,
        // Typed by a person: an absent origin counts as no one's, which some
        // of Claude Code's gates refuse.
        "origin": {"kind": "human"},
    })
}

/// A prompt's blocks as content blocks, the way the ACP adapter passed
/// them: pictures as base64 images, a file as a link to it, and the text of
/// a file that came with it in a `<context>` block after everything else.
pub(crate) fn prompt_content(blocks: &[Block]) -> Vec<Value> {
    let mut content = Vec::new();
    let mut context = Vec::new();
    for block in blocks {
        match block {
            Block::Text(text) => content.push(json!({"type": "text", "text": text})),
            Block::Image { mime, data } => content.push(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": mime,
                    "data": base64::engine::general_purpose::STANDARD.encode(data),
                },
            })),
            Block::File { path, text } => {
                let uri = file_uri(path);
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                content.push(json!({"type": "text", "text": format!("[@{name}]({uri})")}));
                if let Some(text) = text {
                    context.push(json!({
                        "type": "text",
                        "text": format!("\n<context ref=\"{uri}\">\n{text}\n</context>"),
                    }));
                }
            }
        }
    }
    content.extend(context);
    content
}

/// The `file://` URI of an absolute path, percent-encoded.
fn file_uri(path: &Path) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let text = path.to_string_lossy();
    let mut uri = String::with_capacity(text.len() + 8);
    uri.push_str("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(byte as char)
            }
            _ => {
                uri.push('%');
                uri.push(HEX[(byte >> 4) as usize] as char);
                uri.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    uri
}

/// A request of ours to Claude Code.
pub(crate) fn control_request(id: &str, request: Value) -> Value {
    json!({"type": "control_request", "request_id": id, "request": request})
}

/// The answer to a request of Claude Code's that was handled.
pub(crate) fn success(id: &str, response: Value) -> Value {
    json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": response},
    })
}

/// The answer to a request of Claude Code's that was not.
pub(crate) fn failure(id: &str, error: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {"subtype": "error", "request_id": id, "error": error},
    })
}

/// The greeting. A subagent's words are passed on only when asked for:
/// without them a subagent is its calls alone.
pub(crate) fn initialize() -> Value {
    json!({"subtype": "initialize", "forwardSubagentText": true})
}

/// A model of the initialize answer's `models` (`ModelInfo`): the choice
/// offered, the model id it stands for when that differs, and what it can
/// be asked for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Model {
    pub choice: Choice,
    pub resolved: Option<String>,
    /// The levels of effort it takes, lowest first; none when it has no
    /// such choice (`supportsEffort`, `supportedEffortLevels`).
    pub efforts: Vec<String>,
    /// It can run in the `auto` mode (`supportsAutoMode`).
    pub auto: bool,
}

/// The models an `initialize` answer offers, in its order.
pub(crate) fn models(answer: &Value) -> Vec<Model> {
    let Some(rows) = answer.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let id = text_of(row, "value").filter(|id| !id.is_empty())?;
            let name = text_of(row, "displayName").unwrap_or(id);
            let flag = |key: &str| row.get(key).and_then(Value::as_bool) == Some(true);
            let mut efforts: Vec<String> = if flag("supportsEffort") {
                row.get("supportedEffortLevels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|level| !level.is_empty())
                    .map(str::to_owned)
                    .collect()
            } else {
                Vec::new()
            };
            efforts.sort_by_key(|level| effort_rank(level));
            Some(Model {
                choice: Choice {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    description: text_of(row, "description").map(str::to_owned),
                },
                resolved: text_of(row, "resolvedModel").map(str::to_owned),
                efforts,
                auto: flag("supportsAutoMode"),
            })
        })
        .collect()
}

/// The levels of effort Claude Code knows (`EffortLevel`), lowest first.
const EFFORTS: [(&str, &str); 5] = [
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "Extra high"),
    ("max", "Max"),
];

/// Where a level stands among the known ones; one not known comes last.
fn effort_rank(level: &str) -> usize {
    EFFORTS
        .iter()
        .position(|(id, _)| *id == level)
        .unwrap_or(EFFORTS.len())
}

/// Levels of effort as the choices offered.
pub(crate) fn effort_choices(levels: &[String]) -> Vec<Choice> {
    levels
        .iter()
        .map(|level| {
            let name = match EFFORTS.iter().find(|(id, _)| id == level) {
                Some((_, name)) => (*name).to_owned(),
                None => {
                    let mut chars = level.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().chain(chars).collect())
                        .unwrap_or_default()
                }
            };
            Choice {
                id: level.clone(),
                name,
                description: None,
            }
        })
        .collect()
}

/// The request that asks what settings are in force (`get_settings`).
pub(crate) fn get_settings() -> Value {
    json!({"subtype": "get_settings"})
}

/// The request that sets the level of effort for the session
/// (`apply_flag_settings`, as `/effort` and the SDK's `applyFlagSettings`).
pub(crate) fn set_effort(level: &str) -> Value {
    json!({"subtype": "apply_flag_settings", "settings": {"effortLevel": level}})
}

/// The level of effort a `get_settings` answer says is in force
/// (`applied.effort`): `Some(None)` when none is (a model without the
/// choice), `None` when the answer does not say.
pub(crate) fn applied_effort(answer: &Value) -> Option<Option<String>> {
    effort_field(answer.get("applied")?)
}

/// The level of effort at `effort` of `value` (as `system/init` says it):
/// `Some(None)` for `null`, `None` when it is not there.
pub(crate) fn effort_field(value: &Value) -> Option<Option<String>> {
    match value.get("effort")? {
        Value::Null => Some(None),
        Value::String(level) if !level.is_empty() => Some(Some(level.clone())),
        _ => None,
    }
}

/// The model a conversation starts with: the one it was started with,
/// else Claude Code's own default where it offers it, else its first.
pub(crate) fn current_model(models: &[Model], started_with: Option<&str>) -> Option<String> {
    if let Some(model) = started_with {
        return Some(model.to_owned());
    }
    models
        .iter()
        .find(|model| model.choice.id == "default")
        .or(models.first())
        .map(|model| model.choice.id.clone())
}

/// The slash commands of an `initialize` answer's `commands` or of a
/// `commands_changed`, and what each takes after its name.
pub(crate) fn commands(list: Option<&Value>) -> (Vec<Choice>, Vec<(String, String)>) {
    let mut choices = Vec::new();
    let mut arguments = Vec::new();
    for command in list.and_then(Value::as_array).into_iter().flatten() {
        let Some(name) = text_of(command, "name") else {
            continue;
        };
        choices.push(Choice {
            id: name.to_owned(),
            name: name.to_owned(),
            description: text_of(command, "description").map(str::to_owned),
        });
        if let Some(hint) = text_of(command, "argumentHint") {
            arguments.push((name.to_owned(), hint.to_owned()));
        }
    }
    (choices, arguments)
}

/// Claude Code's permission modes, named and ordered as the ACP adapter
/// has them (`buildAvailableModes`); `auto` only where it can be taken.
pub(crate) fn modes(auto: bool) -> Vec<Choice> {
    let mode = |id: &str, name: &str, description: &str| Choice {
        id: id.into(),
        name: name.into(),
        description: Some(description.into()),
    };
    let mut modes = vec![
        mode("default", "Manual", "Always ask before making changes"),
        mode(
            "acceptEdits",
            "Accept edits",
            "Automatically accept all file edits",
        ),
        mode("plan", "Plan", "Create a plan before making changes"),
    ];
    if auto {
        modes.push(mode("auto", "Auto", "Claude handles permission decisions"));
    }
    modes.push(mode(
        "bypassPermissions",
        "Bypass permissions",
        "Accepts all permissions",
    ));
    modes
}

/// The id of the option that allows a call once.
pub(crate) const ALLOW: &str = "allow";
/// The id of the option that refuses it.
pub(crate) const REJECT: &str = "reject";
/// What the id of an option that allows it and remembers the rule Claude
/// Code suggested at this index begins with.
const ALWAYS: &str = "always-";

/// The answers to a `can_use_tool`: allow once; allow and remember, one for
/// each of the rules Claude Code suggests (none where it says a lasting
/// rule must not be offered); refuse.
pub(crate) fn permission_options(suggestions: &[Value], lasting: bool) -> Vec<PermissionOption> {
    let mut options = vec![PermissionOption {
        id: ALLOW.into(),
        label: "Yes".into(),
        kind: PermissionKind::AllowOnce,
    }];
    if lasting {
        for (index, suggestion) in suggestions.iter().enumerate() {
            options.push(PermissionOption {
                id: format!("{ALWAYS}{index}"),
                label: describe_update(suggestion),
                kind: PermissionKind::AllowAlways,
            });
        }
    }
    options.push(PermissionOption {
        id: REJECT.into(),
        label: "No".into(),
        kind: PermissionKind::RejectOnce,
    });
    options
}

/// The suggestion an option of [`permission_options`] remembers, by its id.
pub(crate) fn suggestion_of(option: &str) -> Option<usize> {
    option.strip_prefix(ALWAYS)?.parse().ok()
}

/// What remembering a `PermissionUpdate` means, for a person.
fn describe_update(update: &Value) -> String {
    let session = if text_of(update, "destination") == Some("session") {
        " in this session"
    } else {
        ""
    };
    match text_of(update, "type") {
        Some("addRules" | "replaceRules") => {
            let rules: Vec<String> = update
                .get("rules")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|rule| {
                    let tool = text_of(rule, "toolName")?;
                    Some(match text_of(rule, "ruleContent") {
                        Some(content) => format!("{tool}({content})"),
                        None => tool.to_owned(),
                    })
                })
                .collect();
            if rules.is_empty() {
                return format!("Yes, and remember this{session}");
            }
            format!("Yes, and don't ask again for {}{session}", rules.join(", "))
        }
        Some("setMode") => {
            let mode = text_of(update, "mode").unwrap_or_default();
            let name = modes(true)
                .into_iter()
                .find(|choice| choice.id == mode)
                .map_or_else(|| mode.to_owned(), |choice| choice.name);
            format!("Yes, and switch to {name}")
        }
        Some("addDirectories") => {
            let folders: Vec<&str> = update
                .get("directories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            format!("Yes, and allow access to {}{session}", folders.join(", "))
        }
        _ => format!("Yes, and remember this{session}"),
    }
}

/// `PermissionResult` allowing call `tool_use` with its own input, and
/// remembering `update` when the user chose "always".
pub(crate) fn allow(input: &Value, tool_use: &str, update: Option<&Value>) -> Value {
    let mut answer = json!({
        "behavior": "allow",
        "updatedInput": input,
        "toolUseID": tool_use,
        "decisionClassification": "user_temporary",
    });
    if let Some(update) = update {
        answer["updatedPermissions"] = json!([update]);
        answer["decisionClassification"] = json!("user_permanent");
    }
    answer
}

/// `PermissionResult` refusing call `tool_use`, with the reason the model reads.
pub(crate) fn deny(tool_use: &str, message: &str) -> Value {
    json!({
        "behavior": "deny",
        "message": message,
        "toolUseID": tool_use,
        "decisionClassification": "user_reject",
    })
}

/// Why a cycle that ended with `result` stopped. `cancelled`: the user
/// asked it to.
pub(crate) fn stop(result: &Value, cancelled: bool) -> Stop {
    if cancelled
        || matches!(
            text_of(result, "terminal_reason"),
            Some("aborted_streaming" | "aborted_tools")
        )
    {
        return Stop::Cancelled;
    }
    let failed = result.get("is_error").and_then(Value::as_bool) == Some(true);
    match text_of(result, "subtype") {
        Some("success") if failed => Stop::Failed(
            text_of(result, "result")
                .filter(|text| !text.trim().is_empty())
                .unwrap_or("Claude Code ended the turn with an error")
                .to_owned(),
        ),
        Some("success") => match text_of(result, "stop_reason") {
            Some("max_tokens") => Stop::Limit,
            Some("refusal") => Stop::Refused,
            _ => Stop::Finished,
        },
        Some("error_max_turns" | "error_max_budget_usd") => Stop::Limit,
        subtype => {
            let errors: Vec<&str> = result
                .get("errors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|error| !error.trim().is_empty())
                .collect();
            Stop::Failed(if errors.is_empty() {
                format!(
                    "Claude Code stopped with an error ({})",
                    subtype.unwrap_or("of no known kind")
                )
            } else {
                errors.join("\n")
            })
        }
    }
}

/// The string at `key` of `value`, when there is one and it is not empty.
pub(crate) fn text_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_prompt_s_blocks_go_as_the_adapter_passed_them() {
        let blocks = vec![
            Block::Text("Look at ".into()),
            Block::File {
                path: "/work/my notes.md".into(),
                text: Some("# Notes".into()),
            },
            Block::File {
                path: "/work/a.rs".into(),
                text: None,
            },
            Block::Image {
                mime: "image/png".into(),
                data: Arc::from(&b"png"[..]),
            },
        ];
        let message = user_message(&blocks, Some("Hello from the app."), "u-1", "s-1");
        assert_eq!(
            message,
            json!({
                "type": "user",
                "message": {"role": "user", "content": [
                    {"type": "text", "text": "<kou-desktop>\nA note from the app this session runs in, not from the user:\nHello from the app.\n</kou-desktop>"},
                    {"type": "text", "text": "Look at "},
                    {"type": "text", "text": "[@my notes.md](file:///work/my%20notes.md)"},
                    {"type": "text", "text": "[@a.rs](file:///work/a.rs)"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "cG5n"}},
                    {"type": "text", "text": "\n<context ref=\"file:///work/my%20notes.md\">\n# Notes\n</context>"},
                ]},
                "parent_tool_use_id": null,
                "session_id": "s-1",
                "uuid": "u-1",
                "origin": {"kind": "human"},
            })
        );
    }

    #[test]
    fn suggested_rules_become_options_said_in_words() {
        let suggestions = [
            json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "npm test:*"}], "behavior": "allow", "destination": "localSettings"}),
            json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"}),
            json!({"type": "addDirectories", "directories": ["/tmp/x"], "destination": "session"}),
        ];
        let labels: Vec<(String, String)> = permission_options(&suggestions, true)
            .into_iter()
            .map(|option| (option.id, option.label))
            .collect();
        let pair = |id: &str, label: &str| (id.to_owned(), label.to_owned());
        assert_eq!(
            labels,
            [
                pair("allow", "Yes"),
                pair("always-0", "Yes, and don't ask again for Bash(npm test:*)"),
                pair("always-1", "Yes, and switch to Accept edits"),
                pair(
                    "always-2",
                    "Yes, and allow access to /tmp/x in this session"
                ),
                pair("reject", "No"),
            ]
        );
        // A lasting rule Claude Code says must not be offered is not.
        assert_eq!(permission_options(&suggestions, false).len(), 2);
        assert_eq!(suggestion_of("always-2"), Some(2));
        assert_eq!(suggestion_of("allow"), None);
    }

    #[test]
    fn a_result_says_why_its_cycle_stopped() {
        let result = |value: Value| stop(&value, false);
        assert_eq!(
            result(json!({"subtype": "success", "is_error": false, "stop_reason": "end_turn"})),
            Stop::Finished
        );
        assert_eq!(
            result(json!({"subtype": "success", "is_error": false, "stop_reason": "max_tokens"})),
            Stop::Limit
        );
        assert_eq!(
            result(json!({"subtype": "success", "is_error": false, "stop_reason": "refusal"})),
            Stop::Refused
        );
        assert_eq!(
            result(json!({"subtype": "success", "is_error": true, "result": "Invalid API key"})),
            Stop::Failed("Invalid API key".into())
        );
        assert_eq!(
            result(json!({"subtype": "error_max_turns", "is_error": true, "errors": []})),
            Stop::Limit
        );
        assert_eq!(
            result(
                json!({"subtype": "error_during_execution", "is_error": true, "errors": ["boom"]})
            ),
            Stop::Failed("boom".into())
        );
        assert_eq!(
            result(
                json!({"subtype": "error_during_execution", "terminal_reason": "aborted_streaming"})
            ),
            Stop::Cancelled
        );
        assert_eq!(
            stop(&json!({"subtype": "success", "is_error": false}), true),
            Stop::Cancelled
        );
    }

    #[test]
    fn the_models_offered_and_the_one_in_force() {
        let answer = json!({"models": [
            {"value": "default", "displayName": "Default (recommended)", "description": "Opus", "resolvedModel": "claude-opus-9"},
            {"value": "sonnet", "displayName": "Sonnet", "description": ""},
            {"displayName": "no id"},
        ]});
        let offered = models(&answer);
        assert_eq!(offered.len(), 2);
        assert_eq!(offered[0].resolved.as_deref(), Some("claude-opus-9"));
        assert_eq!(current_model(&offered, None).as_deref(), Some("default"));
        assert_eq!(
            current_model(&offered, Some("haiku")).as_deref(),
            Some("haiku")
        );
        assert_eq!(current_model(&[], None), None);
    }
}
