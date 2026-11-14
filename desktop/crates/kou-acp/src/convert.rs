//! Between the protocol's wire types and the thread's.

use crate::{terminal::Snapshot, uri};
use agent_client_protocol_schema::v1 as acp;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use kou_thread::{
    Block, Choice, Level, Location, PermissionKind, PermissionOption, PlanStatus, PlanStep, Stop,
    ToolContent, ToolKind, ToolStatus,
};

/// A content block from the agent as markdown, which is all an answer, a
/// thought or a tool's output can carry in the thread.
pub(crate) fn markdown(block: &acp::ContentBlock) -> String {
    match block {
        acp::ContentBlock::Text(text) => text.text.clone(),
        acp::ContentBlock::Image(image) => match &image.uri {
            // A link, not an embed: nothing is fetched until the user asks.
            Some(uri) => format!("[image: {}]({uri})", image.mime_type),
            None => format!("[image: {}]", image.mime_type),
        },
        acp::ContentBlock::Audio(audio) => format!("[audio: {}]", audio.mime_type),
        acp::ContentBlock::ResourceLink(link) => {
            format!(
                "[{}]({})",
                link.title.as_deref().unwrap_or(&link.name),
                link.uri
            )
        }
        acp::ContentBlock::Resource(resource) => match &resource.resource {
            acp::EmbeddedResourceResource::TextResourceContents(text) => {
                let fence = fence(&text.text);
                format!("{}\n{fence}\n{}\n{fence}", text.uri, text.text)
            }
            acp::EmbeddedResourceResource::BlobResourceContents(blob) => {
                format!("[{}]({})", file_name(&blob.uri), blob.uri)
            }
            _ => "[resource]".into(),
        },
        _ => "[content this client cannot show]".into(),
    }
}

/// A content block of a replayed user message.
pub(crate) fn user_block(block: acp::ContentBlock) -> Block {
    match block {
        acp::ContentBlock::Text(text) => Block::Text(text.text),
        acp::ContentBlock::Image(image) => match BASE64.decode(image.data.as_bytes()) {
            Ok(data) => Block::Image {
                mime: image.mime_type,
                data: data.into(),
            },
            Err(_) => Block::Text(format!("[image: {}]", image.mime_type)),
        },
        acp::ContentBlock::ResourceLink(link) => match uri::to_path(&link.uri) {
            Some(path) => Block::File { path, text: None },
            None => Block::Text(markdown(&acp::ContentBlock::ResourceLink(link))),
        },
        acp::ContentBlock::Resource(resource) => {
            let (uri, text) = match &resource.resource {
                acp::EmbeddedResourceResource::TextResourceContents(text) => {
                    (text.uri.as_str(), Some(text.text.clone()))
                }
                acp::EmbeddedResourceResource::BlobResourceContents(blob) => {
                    (blob.uri.as_str(), None)
                }
                _ => ("", None),
            };
            match uri::to_path(uri) {
                Some(path) => Block::File { path, text },
                None => Block::Text(markdown(&acp::ContentBlock::Resource(resource))),
            }
        }
        other => Block::Text(markdown(&other)),
    }
}

/// What a prompt becomes on the wire.
pub(crate) struct Prompt {
    pub content: Vec<acp::ContentBlock>,
    /// What the agent cannot take, for a notice.
    pub left_out: Vec<&'static str>,
    /// The prompt's text, by which an agent's echo of it is recognised.
    pub text: String,
    /// How many blocks other than text were sent.
    pub attachments: usize,
}

/// A prompt, restricted to what the agent said it accepts. Text and resource
/// links are the baseline every agent takes.
pub(crate) fn prompt(blocks: &[Block], accepts: &acp::PromptCapabilities) -> Prompt {
    let mut prompt = Prompt {
        content: Vec::with_capacity(blocks.len()),
        left_out: Vec::new(),
        text: String::new(),
        attachments: 0,
    };
    for block in blocks {
        match block {
            Block::Text(text) => {
                prompt.text.push_str(text);
                prompt.content.push(acp::ContentBlock::from(text.clone()));
                continue;
            }
            Block::Image { mime, data } => {
                if !accepts.image {
                    prompt.left_out.push("an image");
                    continue;
                }
                let image = acp::ImageContent::new(BASE64.encode(data), mime.clone());
                prompt.content.push(acp::ContentBlock::Image(image));
            }
            Block::File {
                path,
                text: Some(text),
            } if accepts.embedded_context => {
                let contents = acp::TextResourceContents::new(text.clone(), uri::from_path(path));
                let resource = acp::EmbeddedResource::new(
                    acp::EmbeddedResourceResource::TextResourceContents(contents),
                );
                prompt.content.push(acp::ContentBlock::Resource(resource));
            }
            // The agent takes no embedded context. If it can read the file
            // itself a link is enough; if not, the text travels inline.
            Block::File {
                path,
                text: Some(text),
            } if !path.exists() => {
                let fence = fence(text);
                let inline = format!("{}\n{fence}\n{text}\n{fence}", path.display());
                prompt.content.push(acp::ContentBlock::from(inline));
                continue;
            }
            Block::File { path, .. } => {
                let uri = uri::from_path(path);
                let link = acp::ResourceLink::new(file_name(&path.to_string_lossy()), uri);
                prompt.content.push(acp::ContentBlock::ResourceLink(link));
            }
        }
        prompt.attachments += 1;
    }
    prompt
}

/// A code fence longer than any run of backticks in `text`.
fn fence(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for character in text.chars() {
        run = if character == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat((longest + 1).max(3))
}

fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

pub(crate) fn tool_kind(kind: acp::ToolKind) -> ToolKind {
    match kind {
        acp::ToolKind::Read => ToolKind::Read,
        acp::ToolKind::Edit => ToolKind::Edit,
        acp::ToolKind::Delete => ToolKind::Delete,
        acp::ToolKind::Move => ToolKind::Move,
        acp::ToolKind::Search => ToolKind::Search,
        acp::ToolKind::Execute => ToolKind::Execute,
        acp::ToolKind::Think => ToolKind::Think,
        acp::ToolKind::Fetch => ToolKind::Fetch,
        // `switch_mode` has no counterpart in the thread.
        _ => ToolKind::Other,
    }
}

pub(crate) fn tool_status(status: acp::ToolCallStatus) -> ToolStatus {
    match status {
        acp::ToolCallStatus::InProgress => ToolStatus::Running,
        acp::ToolCallStatus::Completed => ToolStatus::Done,
        acp::ToolCallStatus::Failed => ToolStatus::Failed,
        _ => ToolStatus::Pending,
    }
}

pub(crate) fn locations(locations: Vec<acp::ToolCallLocation>) -> Vec<Location> {
    locations
        .into_iter()
        .map(|location| Location {
            path: location.path,
            line: location.line,
        })
        .collect()
}

/// What an agent's own metadata on a tool call says of subagents.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Delegation {
    /// The call starts a subagent, of this kind (empty when no kind is
    /// said with it: a report that comes without the call's arguments).
    pub agent: Option<String>,
    /// The call that started the subagent which made this call.
    pub parent: Option<String>,
    /// The subagent was started to run in the background, when that is said.
    pub background: Option<bool>,
    /// The tokens the subagent used in all, when its result says.
    pub tokens: Option<u64>,
}

/// What an agent's own metadata on a tool call says of subagents: whether
/// the call starts one (and of what kind, from its arguments), and which
/// call's subagent made it. Claude's adapter names the tool that was called
/// (`_meta.claudeCode.toolName`: `Agent`, or `Task` in older versions) and,
/// on what a subagent does, the call that started it (`parentToolUseId`).
/// A subagent started in the background is known by the call's arguments
/// (`run_in_background`) or by what the tool answered
/// (`_meta.claudeCode.toolResponse`: `isAsync`, `status: async_launched`),
/// which also totals a finished subagent's tokens.
pub(crate) fn delegation(
    meta: &impl serde::Serialize,
    raw_input: Option<&serde_json::Value>,
) -> Delegation {
    let meta = serde_json::to_value(meta).unwrap_or_default();
    let claude = meta.get("claudeCode");
    let text = |key: &str| {
        claude
            .and_then(|claude| claude.get(key))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
    };
    let agent = matches!(text("toolName"), Some("Agent" | "Task")).then(|| {
        raw_input
            .and_then(|input| input.get("subagent_type"))
            .and_then(|kind| kind.as_str())
            .unwrap_or_default()
            .to_owned()
    });
    let answered = claude.and_then(|claude| claude.get("toolResponse"));
    let asked_for = raw_input
        .and_then(|input| input.get("run_in_background"))
        .and_then(|background| background.as_bool());
    let launched = answered.and_then(|answered| {
        let status = answered.get("status").and_then(|status| status.as_str());
        match answered.get("isAsync").and_then(|flag| flag.as_bool()) {
            Some(flag) => Some(flag),
            None => (status == Some("async_launched")).then_some(true),
        }
    });
    Delegation {
        background: agent
            .is_some()
            .then(|| launched.or(asked_for.filter(|asked| *asked)))
            .flatten(),
        tokens: agent
            .is_some()
            .then(|| answered?.get("totalTokens")?.as_u64())
            .flatten(),
        agent,
        parent: text("parentToolUseId").map(str::to_owned),
    }
}

/// What a report of a tool call says of the terminal the agent shows its
/// command in (the `terminal_output` extension: see
/// [`crate::Options::display_terminals`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Display {
    /// The call's command has a terminal of this id.
    pub info: Option<String>,
    /// More of a terminal's output: its id and the text.
    pub output: Option<(String, String)>,
    /// A terminal's command ended: its id, and its status when known.
    pub exit: Option<(String, Option<i32>)>,
}

/// The terminal a report of a tool call speaks of, from its `_meta`:
/// `terminal_info`, `terminal_output` and `terminal_exit`, each with a
/// `terminal_id`.
pub(crate) fn display(meta: &impl serde::Serialize) -> Display {
    let meta = serde_json::to_value(meta).unwrap_or_default();
    let id = |part: &serde_json::Value| {
        part.get("terminal_id")
            .and_then(|id| id.as_str())
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    };
    Display {
        info: meta.get("terminal_info").and_then(id),
        output: meta.get("terminal_output").and_then(|output| {
            let text = output.get("data").and_then(|data| data.as_str())?;
            Some((id(output)?, text.to_owned()))
        }),
        exit: meta.get("terminal_exit").and_then(|exit| {
            let code = exit
                .get("exit_code")
                .and_then(|code| code.as_i64())
                .and_then(|code| i32::try_from(code).ok());
            Some((id(exit)?, code))
        }),
    }
}

/// The call that started the subagent a chunk of text comes from, when a
/// subagent said it (`_meta.claudeCode.parentToolUseId`): such text is no
/// part of the conversation's own.
pub(crate) fn said_by_subagent(meta: &impl serde::Serialize) -> Option<String> {
    delegation(meta, None).parent
}

/// A tool call's content. `terminal` gives the state of an embedded terminal.
pub(crate) fn tool_content(
    content: Vec<acp::ToolCallContent>,
    terminal: impl Fn(&str) -> Option<Snapshot>,
) -> Vec<ToolContent> {
    content
        .into_iter()
        .filter_map(|content| match content {
            acp::ToolCallContent::Content(content) => Some(match &content.content {
                // A picture a tool gave back is shown as one, when its bytes
                // came with it and read as base64.
                acp::ContentBlock::Image(image) => match BASE64.decode(image.data.as_bytes()) {
                    Ok(data) if !data.is_empty() => ToolContent::Image {
                        mime: image.mime_type.clone(),
                        data: data.into(),
                    },
                    _ => ToolContent::Text(markdown(&content.content)),
                },
                block => ToolContent::Text(markdown(block)),
            }),
            acp::ToolCallContent::Diff(diff) => Some(ToolContent::Diff {
                path: diff.path,
                old: diff.old_text,
                new: diff.new_text,
            }),
            acp::ToolCallContent::Terminal(embedded) => {
                let id = embedded.terminal_id.0.to_string();
                Some(terminal_content(
                    id.clone(),
                    terminal(&id).unwrap_or_default(),
                ))
            }
            _ => None,
        })
        .collect()
}

/// A terminal as tool content.
pub(crate) fn terminal_content(id: String, snapshot: Snapshot) -> ToolContent {
    ToolContent::Terminal {
        id,
        output: snapshot.output,
        exit: snapshot.exit.map(|exit| exit.shell_code()),
    }
}

pub(crate) fn plan(plan: acp::Plan) -> Vec<PlanStep> {
    plan.entries
        .into_iter()
        .map(|entry| PlanStep {
            text: entry.content,
            status: match entry.status {
                acp::PlanEntryStatus::InProgress => PlanStatus::Running,
                acp::PlanEntryStatus::Completed => PlanStatus::Done,
                _ => PlanStatus::Pending,
            },
        })
        .collect()
}

pub(crate) fn permission_options(options: Vec<acp::PermissionOption>) -> Vec<PermissionOption> {
    options
        .into_iter()
        .map(|option| PermissionOption {
            id: option.option_id.0.to_string(),
            label: option.name,
            kind: match option.kind {
                acp::PermissionOptionKind::AllowOnce => PermissionKind::AllowOnce,
                acp::PermissionOptionKind::AllowAlways => PermissionKind::AllowAlways,
                acp::PermissionOptionKind::RejectAlways => PermissionKind::RejectAlways,
                // A kind this client does not know is never shown as an approval.
                _ => PermissionKind::RejectOnce,
            },
        })
        .collect()
}

/// Slash commands. The name is the agent's, without the slash it is typed with.
pub(crate) fn commands(commands: Vec<acp::AvailableCommand>) -> Vec<Choice> {
    commands
        .into_iter()
        .map(|command| Choice {
            id: command.name.clone(),
            name: command.name,
            description: Some(command.description).filter(|text| !text.is_empty()),
        })
        .collect()
}

pub(crate) fn stop(reason: acp::StopReason) -> Stop {
    match reason {
        acp::StopReason::EndTurn => Stop::Finished,
        acp::StopReason::MaxTokens | acp::StopReason::MaxTurnRequests => Stop::Limit,
        acp::StopReason::Refusal => Stop::Refused,
        acp::StopReason::Cancelled => Stop::Cancelled,
        other => Stop::Failed(format!(
            "the agent stopped for a reason this client does not know: {other:?}"
        )),
    }
}

pub(crate) fn notice(notice: acp::Notice) -> (Level, String) {
    let level = match notice.severity {
        acp::NoticeSeverity::Warning => Level::Warning,
        acp::NoticeSeverity::Error => Level::Error,
        _ => Level::Info,
    };
    let text = match notice.description.filter(|text| !text.is_empty()) {
        Some(description) => format!("{}: {description}", notice.title),
        None => notice.title,
    };
    (level, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::Exit;
    use serde_json::json;
    use std::path::PathBuf;

    fn block(value: serde_json::Value) -> acp::ContentBlock {
        serde_json::from_value(value).expect("a content block")
    }

    #[test]
    fn agent_content_becomes_markdown() {
        let cases = [
            (json!({"type": "text", "text": "**hi**"}), "**hi**"),
            (
                json!({"type": "image", "mimeType": "image/png", "data": "AAAA"}),
                "[image: image/png]",
            ),
            (
                json!({"type": "image", "mimeType": "image/png", "data": "", "uri": "https://x/y.png"}),
                "[image: image/png](https://x/y.png)",
            ),
            (
                json!({"type": "audio", "mimeType": "audio/wav", "data": "AAAA"}),
                "[audio: audio/wav]",
            ),
            (
                json!({"type": "resource_link", "uri": "file:///a/b.rs", "name": "b.rs"}),
                "[b.rs](file:///a/b.rs)",
            ),
            (
                json!({"type": "resource", "resource": {"uri": "file:///a/b.md", "text": "x ``` y"}}),
                "file:///a/b.md\n````\nx ``` y\n````",
            ),
            (
                json!({"type": "resource", "resource": {"uri": "file:///a/b.bin", "blob": "AAAA"}}),
                "[b.bin](file:///a/b.bin)",
            ),
        ];
        for (wire, expected) in cases {
            assert_eq!(markdown(&block(wire)), expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn replayed_user_content_becomes_blocks() {
        assert_eq!(
            user_block(block(json!({"type": "text", "text": "hi"}))),
            Block::Text("hi".into())
        );
        assert_eq!(
            user_block(block(
                json!({"type": "image", "mimeType": "image/png", "data": "AQID"})
            )),
            Block::Image {
                mime: "image/png".into(),
                data: vec![1, 2, 3].into()
            }
        );
        assert_eq!(
            user_block(block(
                json!({"type": "image", "mimeType": "image/png", "data": "not base64!"})
            )),
            Block::Text("[image: image/png]".into())
        );
        assert_eq!(
            user_block(block(
                json!({"type": "resource_link", "uri": "file:///a/b%20c.rs", "name": "b c.rs"})
            )),
            Block::File {
                path: PathBuf::from("/a/b c.rs"),
                text: None
            }
        );
        assert_eq!(
            user_block(block(
                json!({"type": "resource", "resource": {"uri": "file:///a/b.rs", "text": "fn main() {}"}})
            )),
            Block::File {
                path: PathBuf::from("/a/b.rs"),
                text: Some("fn main() {}".into())
            }
        );
        assert_eq!(
            user_block(block(
                json!({"type": "resource_link", "uri": "https://example.com/doc", "name": "doc"})
            )),
            Block::Text("[doc](https://example.com/doc)".into())
        );
    }

    fn accepts(image: bool, embedded: bool) -> acp::PromptCapabilities {
        acp::PromptCapabilities::new()
            .image(image)
            .embedded_context(embedded)
    }

    fn wire(prompt: &Prompt) -> serde_json::Value {
        serde_json::to_value(&prompt.content).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_prompt_carries_what_the_agent_accepts() {
        let folder = tempfile::tempdir().unwrap();
        let existing = folder.path().join("on disk.rs");
        std::fs::write(&existing, "old").unwrap();
        let existing_uri = uri::from_path(&existing);
        let blocks = [
            Block::Text("look at ".into()),
            Block::Image {
                mime: "image/png".into(),
                data: vec![1, 2, 3].into(),
            },
            Block::File {
                path: existing.clone(),
                text: Some("new".into()),
            },
            Block::File {
                path: PathBuf::from("/nowhere/unsaved.rs"),
                text: Some("a ``` b".into()),
            },
            Block::File {
                path: PathBuf::from("/nowhere/linked.rs"),
                text: None,
            },
            Block::Text("this".into()),
        ];

        let rich = prompt(&blocks, &accepts(true, true));
        assert_eq!(
            wire(&rich),
            json!([
                {"type": "text", "text": "look at "},
                {"type": "image", "data": "AQID", "mimeType": "image/png"},
                {"type": "resource", "resource": {"text": "new", "uri": existing_uri}},
                {"type": "resource", "resource": {"text": "a ``` b", "uri": "file:///nowhere/unsaved.rs"}},
                {"type": "resource_link", "name": "linked.rs", "uri": "file:///nowhere/linked.rs"},
                {"type": "text", "text": "this"},
            ])
        );
        assert_eq!(
            (rich.text.as_str(), rich.attachments, rich.left_out.len()),
            ("look at this", 4, 0)
        );

        let plain = prompt(&blocks, &accepts(false, false));
        assert_eq!(
            wire(&plain),
            json!([
                {"type": "text", "text": "look at "},
                {"type": "resource_link", "name": "on disk.rs", "uri": existing_uri},
                {"type": "text", "text": "/nowhere/unsaved.rs\n````\na ``` b\n````"},
                {"type": "resource_link", "name": "linked.rs", "uri": "file:///nowhere/linked.rs"},
                {"type": "text", "text": "this"},
            ])
        );
        assert_eq!(plain.left_out, ["an image"]);
        assert_eq!(plain.attachments, 2);
    }

    #[test]
    fn tool_content_keeps_text_diffs_and_terminals() {
        let content: Vec<acp::ToolCallContent> = serde_json::from_value(json!([
            {"type": "content", "content": {"type": "text", "text": "done"}},
            {"type": "diff", "path": "/a/b.rs", "oldText": null, "newText": "new"},
            {"type": "diff", "path": "/a/c.rs", "oldText": "old", "newText": "new"},
            {"type": "terminal", "terminalId": "term-1"},
            {"type": "terminal", "terminalId": "term-2"},
        ]))
        .unwrap();
        let converted = tool_content(content, |id| {
            (id == "term-1").then(|| Snapshot {
                output: "hi\n".into(),
                truncated: false,
                exit: Some(Exit::Signal(9)),
            })
        });
        assert_eq!(
            converted,
            [
                ToolContent::Text("done".into()),
                ToolContent::Diff {
                    path: "/a/b.rs".into(),
                    old: None,
                    new: "new".into()
                },
                ToolContent::Diff {
                    path: "/a/c.rs".into(),
                    old: Some("old".into()),
                    new: "new".into()
                },
                ToolContent::Terminal {
                    id: "term-1".into(),
                    output: "hi\n".into(),
                    exit: Some(137)
                },
                ToolContent::Terminal {
                    id: "term-2".into(),
                    output: String::new(),
                    exit: None
                },
            ]
        );
    }

    #[test]
    fn a_call_that_starts_a_subagent_and_what_the_subagent_does_are_told_apart() {
        let none: Option<serde_json::Value> = None;
        let told = |agent: Option<&str>, parent: Option<&str>| Delegation {
            agent: agent.map(str::to_owned),
            parent: parent.map(str::to_owned),
            ..Delegation::default()
        };
        // Claude's adapter: the tool called, and whose subagent called it.
        let launch = json!({"claudeCode": {"toolName": "Agent"}});
        let input = json!({"description": "Check the tests", "subagent_type": "Explore"});
        assert_eq!(
            delegation(&Some(launch.clone()), Some(&input)),
            told(Some("Explore"), None)
        );
        // An older name of the same tool, and no kind named.
        let task = json!({"claudeCode": {"toolName": "Task"}});
        assert_eq!(delegation(&Some(task), None), told(Some(""), None));
        let child = json!({"claudeCode": {"toolName": "Read", "parentToolUseId": "toolu_1"}});
        assert_eq!(
            delegation(&Some(child.clone()), None),
            told(None, Some("toolu_1"))
        );
        assert_eq!(said_by_subagent(&Some(child)).as_deref(), Some("toolu_1"));
        // Any other call, and an agent that says nothing of the kind.
        let read = json!({"claudeCode": {"toolName": "Read"}});
        assert_eq!(
            delegation(&Some(read.clone()), Some(&input)),
            told(None, None)
        );
        assert_eq!(said_by_subagent(&Some(read)), None);
        assert_eq!(delegation(&none, Some(&input)), told(None, None));
        assert_eq!(said_by_subagent(&none), None);
        assert_eq!(
            delegation(&Some(json!({"other": 1})), None),
            told(None, None)
        );
    }

    #[test]
    fn what_a_report_says_of_its_command_s_terminal_is_read() {
        let none: Option<serde_json::Value> = None;
        assert_eq!(display(&none), Display::default());
        let announced = json!({
            "claudeCode": {"toolName": "Bash"},
            "terminal_info": {"terminal_id": "toolu_1"},
        });
        assert_eq!(
            display(&Some(announced)),
            Display {
                info: Some("toolu_1".into()),
                ..Display::default()
            }
        );
        let ended = json!({
            "terminal_output": {"terminal_id": "toolu_1", "data": "ok\n"},
            "terminal_exit": {"terminal_id": "toolu_1", "exit_code": 2, "signal": null},
        });
        assert_eq!(
            display(&Some(ended)),
            Display {
                info: None,
                output: Some(("toolu_1".into(), "ok\n".into())),
                exit: Some(("toolu_1".into(), Some(2))),
            }
        );
        // An exit whose status is not known is an exit all the same.
        let unknown = json!({"terminal_exit": {"terminal_id": "t", "exit_code": null}});
        assert_eq!(display(&Some(unknown)).exit, Some(("t".into(), None)));
        // Without an id nothing can be told apart.
        let nameless = json!({"terminal_output": {"data": "x"}, "terminal_exit": {}});
        assert_eq!(display(&Some(nameless)), Display::default());
    }

    #[test]
    fn a_subagent_started_in_the_background_and_what_one_spent_are_read() {
        let launch = json!({"claudeCode": {"toolName": "Agent"}});
        // Asked to run in the background, by the call's arguments.
        let asked = json!({"description": "Watch the build", "run_in_background": true});
        assert_eq!(
            delegation(&Some(launch.clone()), Some(&asked)).background,
            Some(true)
        );
        // Not asked to: nothing is said either way until the tool answers.
        let plain = json!({"description": "Check the tests", "run_in_background": false});
        assert_eq!(
            delegation(&Some(launch.clone()), Some(&plain)).background,
            None
        );
        // What the tool answered says it: started and left to run...
        let started = json!({"claudeCode": {
            "toolName": "Agent",
            "toolResponse": {"status": "async_launched", "agentId": "a1"},
        }});
        assert_eq!(delegation(&Some(started), None).background, Some(true));
        let flagged = json!({"claudeCode": {
            "toolName": "Agent", "toolResponse": {"isAsync": true},
        }});
        assert_eq!(delegation(&Some(flagged), None).background, Some(true));
        // ...or done, with what it used.
        let done = json!({"claudeCode": {
            "toolName": "Agent",
            "toolResponse": {"status": "completed", "isAsync": false, "totalTokens": 48211},
        }});
        let told = delegation(&Some(done), None);
        assert_eq!((told.background, told.tokens), (Some(false), Some(48211)));
        // The same words on another tool's result mean nothing of the kind.
        let other = json!({"claudeCode": {
            "toolName": "Bash", "toolResponse": {"isAsync": true, "totalTokens": 3},
        }});
        let told = delegation(&Some(other), Some(&asked));
        assert_eq!((told.background, told.tokens), (None, None));
    }

    #[test]
    fn a_picture_a_tool_gave_back_is_kept_as_a_picture() {
        let content: Vec<acp::ToolCallContent> = serde_json::from_value(json!([
            {"type": "content", "content": {"type": "image", "mimeType": "image/png", "data": "iVBORw=="}},
            // Without bytes that read, it is named in text as before.
            {"type": "content", "content": {"type": "image", "mimeType": "image/png", "data": "not base64!"}},
            {"type": "content", "content": {"type": "image", "mimeType": "image/png", "data": ""}},
        ]))
        .unwrap();
        let converted = tool_content(content, |_| None);
        assert_eq!(
            converted,
            [
                ToolContent::Image {
                    mime: "image/png".into(),
                    data: vec![0x89, b'P', b'N', b'G'].into(),
                },
                ToolContent::Text("[image: image/png]".into()),
                ToolContent::Text("[image: image/png]".into()),
            ]
        );
    }

    #[test]
    fn kinds_statuses_and_reasons_map_onto_the_thread() {
        let kind = |name: &str| tool_kind(serde_json::from_value(json!(name)).unwrap());
        assert_eq!(kind("execute"), ToolKind::Execute);
        assert_eq!(kind("switch_mode"), ToolKind::Other);
        assert_eq!(kind("something_new"), ToolKind::Other);
        let status = |name: &str| tool_status(serde_json::from_value(json!(name)).unwrap());
        assert_eq!(status("pending"), ToolStatus::Pending);
        assert_eq!(status("in_progress"), ToolStatus::Running);
        assert_eq!(status("completed"), ToolStatus::Done);
        assert_eq!(status("failed"), ToolStatus::Failed);
        let reason = |name: &str| stop(serde_json::from_value(json!(name)).unwrap());
        assert_eq!(reason("end_turn"), Stop::Finished);
        assert_eq!(reason("max_tokens"), Stop::Limit);
        assert_eq!(reason("max_turn_requests"), Stop::Limit);
        assert_eq!(reason("refusal"), Stop::Refused);
        assert_eq!(reason("cancelled"), Stop::Cancelled);
    }

    #[test]
    fn a_notice_keeps_its_severity_and_both_texts() {
        let wire = |value| notice(serde_json::from_value(value).unwrap());
        assert_eq!(
            wire(
                json!({"severity": "warning", "title": "MCP server unavailable", "description": "Continuing without it."})
            ),
            (
                Level::Warning,
                "MCP server unavailable: Continuing without it.".into()
            )
        );
        assert_eq!(
            wire(json!({"severity": "error", "title": "Quota"})),
            (Level::Error, "Quota".into())
        );
        assert_eq!(
            wire(json!({"severity": "_custom", "title": "Hello"})),
            (Level::Info, "Hello".into())
        );
    }
}
