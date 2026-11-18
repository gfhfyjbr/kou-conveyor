//! What of a `user` line the user said. Claude Code keeps much there that
//! nobody typed: the tools' results, what it adds for the model
//! (`<system-reminder>`), the wrappers of a slash command and of its output
//! (`<command-name>`, `<local-command-stdout>`), the note that a task left in
//! the background ended (`<task-notification>`), the mark of an interrupted
//! turn. These are taken out as Claude's ACP adapter takes them out when it
//! replays a session; a slash command the user ran is shown as they typed it.

use crate::records::tools::strip_tag;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use kou_thread::Block;
use serde_json::Value;
use std::path::PathBuf;

/// What a user line holds for the conversation.
#[derive(Debug, PartialEq)]
pub(crate) enum Spoken {
    /// Something the user said: a prompt, or a message taken into a turn.
    Said(Vec<Block>),
    /// That the user stopped the turn (`[Request interrupted by user]`).
    Interrupted,
    /// Nothing of the user's.
    Nothing,
}

/// Wrappers round what is not speech, each taken out with what it holds.
const MARKERS: [&str; 10] = [
    "command-name",
    "command-message",
    "command-args",
    "local-command-stdout",
    "local-command-stderr",
    "local-command-caveat",
    "system-reminder",
    "task-notification",
    "bash-stdout",
    "bash-stderr",
];

/// Commands of Claude Code's own interface whose lines are no prompt: what
/// they did is shown otherwise or not at all, as the adapter has it.
const HIDDEN_COMMANDS: [&str; 8] = [
    "/context",
    "/heapdump",
    "/extra-usage",
    "/compact",
    "/model",
    "/mcp",
    "/status",
    "/usage",
];

/// What marks a note of the app's own before a first prompt, as `kou-acp`
/// writes it: it is the agent's to read, not the user's.
const PREFACE_OPEN: &str = "<kou-desktop>";
const PREFACE_CLOSE: &str = "</kou-desktop>";

/// What the user said in a line whose message content is `content`.
pub(crate) fn spoken(content: &Value) -> Spoken {
    let blocks: Vec<&Value> = match content {
        Value::Array(blocks) => blocks.iter().collect(),
        Value::String(_) => vec![content],
        _ => return Spoken::Nothing,
    };
    let mut said: Vec<Block> = Vec::new();
    let mut interrupted = false;
    for block in blocks {
        let kind = match block {
            Value::String(_) => "text",
            block => block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        };
        match kind {
            "text" => {
                let text = match block {
                    Value::String(text) => text.as_str(),
                    block => block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                };
                if is_interruption(text) {
                    interrupted = true;
                    continue;
                }
                let Some(text) = speech(text) else {
                    continue;
                };
                place_text(&mut said, text);
            }
            "image" => said.push(image(block)),
            "document" => said.push(Block::Text(
                match block.get("title").and_then(Value::as_str) {
                    Some(title) if !title.is_empty() => format!("[document \"{title}\"]"),
                    _ => "[document]".to_owned(),
                },
            )),
            // A tool's result is the call's, and anything else is no speech.
            _ => {}
        }
    }
    if !said.is_empty() {
        Spoken::Said(said)
    } else if interrupted {
        Spoken::Interrupted
    } else {
        Spoken::Nothing
    }
}

/// The text of a message's content, its text blocks one after another.
pub(crate) fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Whether `text` is the mark Claude Code writes when the user stops a
/// turn: `[Request interrupted by user]` and its kin.
fn is_interruption(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("[Request interrupted by user") && text.ends_with(']')
}

/// What of a text block is speech: `None` when nothing is.
fn speech(text: &str) -> Option<String> {
    let text = without_preface(text)?;
    // A shell command the user ran from Claude Code's prompt (`! ls`).
    if let Some(command) = tag_text(text, "bash-input") {
        return Some(format!("! {}", command.trim()));
    }
    let mut stripped = text.to_owned();
    for tag in MARKERS {
        stripped = strip_tag(&stripped, tag);
    }
    if stripped.len() == text.len() {
        return Some(text.to_owned()).filter(|text| !text.trim().is_empty());
    }
    if !stripped.trim().is_empty() {
        return Some(stripped.trim().to_owned());
    }
    // Wrappers only: a slash command the user ran, as they typed it; or
    // what a command printed, which nobody typed.
    if tag_text(text, "local-command-stdout").is_some()
        || tag_text(text, "local-command-stderr").is_some()
    {
        return None;
    }
    let name = tag_text(text, "command-name")?.trim();
    let first = name.split_whitespace().next().unwrap_or_default();
    if !name.starts_with('/') || HIDDEN_COMMANDS.contains(&first) {
        return None;
    }
    match tag_text(text, "command-args").map(str::trim) {
        Some(args) if !args.is_empty() => Some(format!("{name} {args}")),
        _ => Some(name.to_owned()),
    }
}

/// `text` without the app's note it may begin with; `None` when the note
/// was all of it.
pub(crate) fn without_preface(text: &str) -> Option<&str> {
    let Some(noted) = text.trim_start().strip_prefix(PREFACE_OPEN) else {
        return Some(text);
    };
    let Some(end) = noted.find(PREFACE_CLOSE) else {
        return Some(text);
    };
    let rest = noted[end + PREFACE_CLOSE.len()..].trim_start();
    (!rest.is_empty()).then_some(rest)
}

/// The text between the first `<tag>` and the `</tag>` after it.
pub(crate) fn tag_text<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)?;
    Some(&text[start..start + end])
}

/// Puts a text block among what was said: a file the prompt named is a
/// file again, as the adapter wrote it (`[@name](file:///path)`, and its
/// text after the prompt as `<context ref="file:///path">`).
fn place_text(said: &mut Vec<Block>, text: String) {
    if let Some(path) = linked_file(&text) {
        said.push(Block::File { path, text: None });
        return;
    }
    if let Some((path, contents)) = embedded_file(&text) {
        let named = said.iter_mut().rev().find_map(|block| match block {
            Block::File { path: named, text } if *named == path && text.is_none() => Some(text),
            _ => None,
        });
        match named {
            Some(slot) => *slot = Some(contents),
            None => said.push(Block::File {
                path,
                text: Some(contents),
            }),
        }
        return;
    }
    said.push(Block::Text(text));
}

fn linked_file(text: &str) -> Option<PathBuf> {
    let rest = text.trim().strip_prefix("[@")?;
    // The name may hold anything; the address has its brackets encoded.
    let (_, uri) = rest.rsplit_once("](")?;
    file_path(uri.strip_suffix(')')?)
}

fn embedded_file(text: &str) -> Option<(PathBuf, String)> {
    let rest = text.trim_start().strip_prefix("<context ref=\"")?;
    let (uri, rest) = rest.split_once("\">")?;
    let body = rest.trim_end().strip_suffix("</context>")?;
    let body = body.strip_prefix('\n').unwrap_or(body);
    let body = body.strip_suffix('\n').unwrap_or(body);
    Some((file_path(uri)?, body.to_owned()))
}

/// The path a `file://` URI names.
fn file_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    let path = path.strip_prefix("localhost").unwrap_or(path);
    if !path.starts_with('/') || path.contains(char::is_whitespace) {
        return None;
    }
    Some(PathBuf::from(percent_decode(path)))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(hex) = text.get(index + 1..index + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A picture of a prompt: its bytes, or where it was when only that is kept.
fn image(block: &Value) -> Block {
    let source = block.get("source").unwrap_or(&Value::Null);
    let mime = source
        .get("media_type")
        .and_then(Value::as_str)
        .unwrap_or("image/png");
    match source.get("type").and_then(Value::as_str) {
        Some("base64") => {
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match BASE64.decode(data.trim()) {
                Ok(bytes) if !bytes.is_empty() => Block::Image {
                    mime: mime.to_owned(),
                    data: bytes.into(),
                },
                _ => Block::Text(format!("[image: {mime}]")),
            }
        }
        _ => Block::Text(match source.get("url").and_then(Value::as_str) {
            Some(url) => format!("[image: {url}]"),
            None => "[image]".to_owned(),
        }),
    }
}

/// What a `<task-notification>` in a user line says: the call whose work
/// it is about (`<tool-use-id>`) and how that work ended (`<status>`). A
/// notification without a status comes while the work still runs.
pub(crate) fn notifications(content: &Value) -> Vec<(String, String)> {
    const OPEN: &str = "<task-notification>";
    const CLOSE: &str = "</task-notification>";
    let text = text_of(content);
    let mut found = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else {
            break;
        };
        // What the work printed comes last and may quote anything.
        let body = after[..end].split("<result>").next().unwrap_or_default();
        let field = |tag: &str| {
            tag_text(body, tag)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };
        if let (Some(call), Some(status)) = (field("tool-use-id"), field("status")) {
            found.push((call.to_owned(), status.to_owned()));
        }
        rest = &after[end + CLOSE.len()..];
    }
    found
}

/// The first line's worth of `text`, as a list shows a conversation by it:
/// on one line, at most 200 characters.
pub(crate) fn one_line(text: &str) -> String {
    let flat = text.replace(['\n', '\r'], " ");
    let flat = flat.trim();
    if flat.chars().count() <= 200 {
        return flat.to_owned();
    }
    let cut: String = flat.chars().take(200).collect();
    format!("{}…", cut.trim_end())
}

/// What a list calls a conversation by when it has no name: its first
/// prompt, as the SDK finds it in a line of the record (`None` when the
/// line holds none). A slash command only stands in while no prompt
/// follows: it is put in `command`.
pub(crate) fn prompt_of(line: &Value, command: &mut Option<String>) -> Option<String> {
    if line.get("type").and_then(Value::as_str) != Some("user")
        || line.get("isMeta") == Some(&Value::Bool(true))
        || line.get("isCompactSummary") == Some(&Value::Bool(true))
    {
        return None;
    }
    let content = line.get("message")?.get("content")?;
    let texts: Vec<&str> = match content {
        Value::String(text) => vec![text.as_str()],
        Value::Array(blocks) => {
            let mut texts = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_result") => return None,
                    Some("text") => texts.extend(block.get("text").and_then(Value::as_str)),
                    _ => {}
                }
            }
            texts
        }
        _ => return None,
    };
    for text in texts {
        let text = text.replace('\n', " ");
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(name) = tag_text(text, "command-name") {
            command.get_or_insert_with(|| name.to_owned());
            continue;
        }
        if let Some(input) = tag_text(text, "bash-input") {
            return Some(format!("! {}", input.trim()));
        }
        if starts_with_tag(text) || is_interruption(text) {
            continue;
        }
        return Some(one_line(text));
    }
    None
}

/// Whether `text` starts with a tag (`<system-reminder>`, `<kou-desktop>`):
/// no prompt the user typed, as the SDK takes it.
fn starts_with_tag(text: &str) -> bool {
    let Some(rest) = text.trim_start().strip_prefix('<') else {
        return false;
    };
    let mut characters = rest.chars();
    if !characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
    {
        return false;
    }
    for character in characters {
        if character.is_whitespace() || character == '>' {
            return true;
        }
        if !(character.is_alphanumeric() || character == '_' || character == '-') {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn said(content: Value) -> Vec<Block> {
        match spoken(&content) {
            Spoken::Said(blocks) => blocks,
            other => panic!("nothing said: {other:?}"),
        }
    }

    fn text(text: &str) -> Block {
        Block::Text(text.into())
    }

    #[test]
    fn what_claude_code_adds_round_a_prompt_is_taken_out() {
        assert_eq!(said(json!("fix the build")), [text("fix the build")]);
        assert_eq!(
            said(
                json!([{"type": "text", "text": "fix it<system-reminder>be careful</system-reminder>"}])
            ),
            [text("fix it")]
        );
        assert_eq!(
            spoken(&json!("<system-reminder>only this</system-reminder>")),
            Spoken::Nothing
        );
        // A command's output and its caveat are nobody's words.
        assert_eq!(
            spoken(&json!(
                "<local-command-stdout>Set model to opus</local-command-stdout>"
            )),
            Spoken::Nothing
        );
        assert_eq!(
            spoken(&json!(
                "<local-command-caveat>Caveat: the messages below</local-command-caveat>"
            )),
            Spoken::Nothing
        );
        // A tool's result is the call's.
        assert_eq!(
            spoken(&json!([{"type": "tool_result", "tool_use_id": "t", "content": "ok"}])),
            Spoken::Nothing
        );
    }

    #[test]
    fn a_slash_command_is_shown_as_it_was_typed_and_the_interface_s_own_are_not() {
        let command = "<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args>the diff</command-args>";
        assert_eq!(said(json!(command)), [text("/review the diff")]);
        let bare = "<command-name>/init</command-name><command-message>init</command-message><command-args></command-args>";
        assert_eq!(said(json!(bare)), [text("/init")]);
        let model = "<command-name>/model</command-name><command-args>opus</command-args>";
        assert_eq!(spoken(&json!(model)), Spoken::Nothing);
        assert_eq!(
            said(json!("<bash-input>ls -la</bash-input>")),
            [text("! ls -la")]
        );
        assert_eq!(
            spoken(&json!(
                "<bash-stdout>a\nb</bash-stdout><bash-stderr></bash-stderr>"
            )),
            Spoken::Nothing
        );
    }

    #[test]
    fn an_interruption_is_no_prompt() {
        assert_eq!(
            spoken(&json!([{"type": "text", "text": "[Request interrupted by user]"}])),
            Spoken::Interrupted
        );
        assert_eq!(
            spoken(&json!("[Request interrupted by user for tool use]")),
            Spoken::Interrupted
        );
        assert_eq!(
            said(json!("[not an interruption")),
            [text("[not an interruption")]
        );
    }

    #[test]
    fn pictures_and_files_of_a_prompt_come_back_as_they_were_sent() {
        let blocks = said(json!([
            {"type": "text", "text": "look at "},
            {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "AQID"}},
            {"type": "text", "text": "[@b c.rs](file:///a/b%20c.rs)"},
            {"type": "text", "text": "and"},
            {"type": "image", "source": {"type": "url", "url": "https://x/y.png"}},
            {"type": "text", "text": "\n<context ref=\"file:///a/b%20c.rs\">\nfn main() {}\n</context>"},
        ]));
        assert_eq!(
            blocks,
            [
                text("look at "),
                Block::Image {
                    mime: "image/jpeg".into(),
                    data: vec![1, 2, 3].into()
                },
                Block::File {
                    path: "/a/b c.rs".into(),
                    text: Some("fn main() {}".into())
                },
                text("and"),
                text("[image: https://x/y.png]"),
            ]
        );
        assert_eq!(
            said(
                json!([{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "!!"}}])
            ),
            [text("[image: image/png]")]
        );
    }

    #[test]
    fn the_app_s_note_before_a_first_prompt_is_not_the_user_s() {
        let note = "<kou-desktop>\nA note from the app this session runs in, not from the user:\nYou run in a pane.\n</kou-desktop>";
        assert_eq!(
            said(json!([{"type": "text", "text": note}, {"type": "text", "text": "hello"}])),
            [text("hello")]
        );
        assert_eq!(said(json!(format!("{note}\n\nhello"))), [text("hello")]);
        assert_eq!(spoken(&json!(note)), Spoken::Nothing);
    }

    #[test]
    fn a_task_notification_names_its_call_and_how_its_work_ended() {
        let content = json!(
            "<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>toolu_9</tool-use-id>\n<status>completed</status>\n<summary>done</summary>\n<result><status>quoted</status></result>\n</task-notification>\n<task-notification><task-id>b2</task-id><summary>still running</summary></task-notification>"
        );
        assert_eq!(
            notifications(&content),
            [("toolu_9".to_owned(), "completed".to_owned())]
        );
        assert_eq!(spoken(&content), Spoken::Nothing);
    }

    #[test]
    fn a_conversation_is_named_by_its_first_prompt_on_one_line() {
        let mut command = None;
        let line = json!({"type": "user", "message": {"content": "fix\nthe build"}});
        assert_eq!(
            prompt_of(&line, &mut command).as_deref(),
            Some("fix the build")
        );
        let meta = json!({"type": "user", "isMeta": true, "message": {"content": "x"}});
        assert_eq!(prompt_of(&meta, &mut command), None);
        let tagged = json!({"type": "user", "message": {"content": [
            {"type": "text", "text": "<kou-desktop>note</kou-desktop>"},
            {"type": "text", "text": "the real one"}]}});
        assert_eq!(
            prompt_of(&tagged, &mut command).as_deref(),
            Some("the real one")
        );
        let slash =
            json!({"type": "user", "message": {"content": "<command-name>/init</command-name>"}});
        assert_eq!(prompt_of(&slash, &mut command), None);
        assert_eq!(command.as_deref(), Some("/init"));
        let long = "x".repeat(300);
        assert_eq!(one_line(&long).chars().count(), 201);
    }
}
