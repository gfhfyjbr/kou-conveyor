use super::*;
use regex::Regex;
use std::{collections::HashMap, sync::LazyLock};
const COMPACTION: &str = include_str!("../../../../harness/contextbuilder/prompts/compaction.md");
const COMPACTED: &str = include_str!("../../../../harness/contextbuilder/prompts/compacted.md");
const REMINDER: &str =
    "Reminder: text only, a <review> block and then the <summary> block, and no tool calls.";
const RESUME: &str = "Pick the work up where the summary ends, without stopping to ask the user anything. Go straight to the task: no word about the summary, no retelling of it, no opening line about resuming; to the user, the work simply goes on. Messages after this one take precedence over it.";

impl Builder {
    pub fn build_compaction(&mut self, focus: &str, budget: i64) -> Built {
        let mut result = self.build();
        let mut prompt = COMPACTION.trim().to_owned();
        if !focus.trim().is_empty() {
            prompt.push_str("\n\nThe user asked the summary to focus on:\n");
            prompt.push_str(focus.trim());
        }
        prompt.push_str("\n\n");
        prompt.push_str(REMINDER);
        let tokens = 4 + estimate_text(&prompt);
        if budget > 0 && result.estimated_tokens + tokens > budget {
            let (estimated, changes) = self.fit(
                &mut result.request.input,
                result.estimated_tokens,
                budget - tokens,
            );
            result.estimated_tokens = estimated;
            result.report.changes.extend(changes);
        }
        result.request.input.push(user_message(&prompt));
        result.estimated_tokens += tokens;
        result
    }
    fn fit(&self, input: &mut Vec<Item>, mut estimated: i64, budget: i64) -> (i64, Vec<Change>) {
        let sized = self.tool_tokens + input.iter().map(|item| estimate_item(item)).sum::<i64>();
        let scale = estimated as f64 / sized.max(1) as f64;
        let tokens = |item: &LlmItem| (estimate_item(item) as f64 * scale) as i64;
        let mut results: Vec<usize> = input
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item.data, LlmData::ToolResult(_)))
            .map(|(index, _)| index)
            .collect();
        results.sort_by_key(|index| std::cmp::Reverse(estimate_item(&input[*index])));
        let mut cut = 0;
        for index in results {
            if estimated <= budget {
                break;
            }
            let LlmData::ToolResult(result) = &input[index].data else {
                unreachable!()
            };
            let output = result
                .output
                .iter()
                .map(|part| {
                    if part.kind == "image" {
                        ToolResultOutput {
                            kind: "text".into(),
                            value: "[An image was left out to fit the context window.]".into(),
                        }
                    } else {
                        ToolResultOutput {
                            kind: part.kind.clone(),
                            value: clip(&part.value, 2000),
                        }
                    }
                })
                .collect();
            let trimmed = with_data(
                &input[index],
                LlmData::ToolResult(ToolResult {
                    call_id: result.call_id.clone(),
                    output,
                }),
            );
            let saved = tokens(&input[index]) - tokens(&trimmed);
            if saved > 0 {
                input[index] = trimmed;
                estimated -= saved;
                cut += 1;
            }
        }
        let mut changes = Vec::new();
        if cut > 0 {
            changes.push(Change {
                kind: "truncated".into(),
                source: "tool results".into(),
                reason: format!("{cut} tool results were cut short to fit the context window"),
            });
        }
        let first = if self.compacted > 0 { 2 } else { 1 };
        let mut dropped = 0;
        for value in input.iter().take(input.len().saturating_sub(1)).skip(first) {
            if estimated <= budget {
                break;
            }
            estimated -= tokens(value);
            dropped += 1;
        }
        if dropped > 0 {
            let text = format!(
                "[{dropped} earlier items of the conversation are left out here: they did not fit the context window.]"
            );
            input.splice(first..first + dropped, [user_message(&text)]);
            self.detach_results(input, true);
            estimated += 4 + estimate_text(&text);
            changes.push(Change {
                kind: "omitted".into(),
                source: "conversation".into(),
                reason: format!("{dropped} earlier items were left out to fit the context window"),
            });
        }
        (estimated, changes)
    }
    pub fn compact(&mut self, response: &Response, kept: Option<&Compacted>) -> Option<Compacted> {
        let text = summary(response);
        if text.is_empty() {
            return None;
        }
        self.touched = memory::touched_files(&self.committed, &self.touched, &self.failed_calls);
        self.failed_calls.clear();
        let made = if let Some(kept) = kept.filter(|kept| !kept.message.is_empty()) {
            kept.clone()
        } else {
            self.compacted_message(&text)
        };
        self.shown_notes = made.notes.clone();
        self.compacted = self.committed.len() as i64 - 1 - self.unanswered.len() as i64;
        self.checkpoint_compacted = self.compacted + self.recorded_prefix_delta;
        self.recorded_prefix_delta = 0;
        self.committed = std::iter::once(self.committed[0].clone())
            .chain(std::iter::once(user_message(&made.message)))
            .chain(
                self.unanswered
                    .iter()
                    .filter_map(|input| input.item.clone()),
            )
            .collect();
        self.compactable = false;
        self.pruned = 0;
        self.masked = 0;
        self.thinking_left_out = 0;
        self.images_left_out = 0;
        self.usage = 0;
        self.usage_mark = 0;
        Some(made)
    }
    fn compacted_message(&self, summary: &str) -> Compacted {
        let notes = self.read_notes();
        let mut text = COMPACTED.trim().to_owned();
        let (kept, omitted) = kept_inputs(&self.answered);
        let omitted = omitted as i64 + self.answered_omitted;
        if !kept.is_empty() {
            text.push_str("\n\nThe user's messages before the compaction, oldest first");
            if omitted > 0 {
                text.push_str(&format!(" ({omitted} earlier ones are left out)"));
            }
            text.push_str(":\n<user_messages>\n");
            for message in kept {
                text.push_str("<message>\n");
                text.push_str(&message);
                text.push_str("\n</message>\n");
            }
            text.push_str("</user_messages>");
        }
        let running = self.running_calls();
        if !running.is_empty() {
            text.push_str("\n\nTool calls made before the compaction that are still running; their results arrive later:\n<running_tool_calls>\n");
            for call in running {
                text.push_str(&format!("- {}\n", describe_call(call)));
            }
            text.push_str("</running_tool_calls>");
        }
        text.push_str(&format!(
            "\n\nThe summary:\n<summary>\n{summary}\n</summary>"
        ));
        text.push_str(&self.memory_text(notes.as_deref()));
        text.push_str("\n\n");
        text.push_str(RESUME);
        Compacted {
            message: text,
            notes,
        }
    }
    fn running_calls(&self) -> Vec<&ToolCall> {
        let mut calls = Vec::new();
        let mut running = HashMap::new();
        for (index, item) in self.committed.iter().chain(&self.staged).enumerate() {
            match &item.data {
                LlmData::ToolCall(call) => calls.push(call),
                LlmData::ToolResult(result) => {
                    running.insert(
                        result.call_id.as_str(),
                        index < self.committed.len() && is_running(&result.output),
                    );
                }
                _ => {}
            }
        }
        calls.retain(|call| running.get(call.call_id.as_str()) == Some(&true));
        calls
    }
    pub(super) fn detach_results(&self, input: &mut [Item], always: bool) {
        if self.compacted == 0 && !always {
            return;
        }
        let present: HashSet<String> = input
            .iter()
            .filter_map(|item| {
                if let LlmData::ToolCall(call) = &item.data {
                    Some(call.call_id.clone())
                } else {
                    None
                }
            })
            .collect();
        for value in input {
            if let LlmData::ToolResult(result) = &value.data
                && !present.contains(&result.call_id)
                && let Some(call) = self.call(&result.call_id)
            {
                let description = describe_call(call);
                let text = if is_running(&result.output) {
                    format!(
                        "The tool call {description}, made before the compaction, is still running; its result comes in a later turn."
                    )
                } else {
                    let mut text = format!(
                        "Result of the tool call {description}, made before the compaction:"
                    );
                    for part in &result.output {
                        text.push('\n');
                        text.push_str(if part.kind == "image" { "[An image was attached here; without its call it cannot be shown. If you still need it, view it again.]" } else { &part.value });
                    }
                    text
                };
                *value = user_message(&text);
            }
        }
    }
    pub(super) fn estimate(&self) -> i64 {
        let counted = self.usage > 0;
        let from = if counted {
            self.usage_mark.min(self.committed.len())
        } else {
            0
        };
        let mut tokens = if counted {
            self.usage
        } else {
            self.tool_tokens
        };
        tokens += self.committed[from..]
            .iter()
            .chain(&self.staged)
            .map(|item| estimate_item(item))
            .sum::<i64>();
        if counted && self.resumed && self.pruning.thinking_turns > 0 {
            let size = self.tool_tokens
                + self.committed[..from]
                    .iter()
                    .map(|item| estimate_item(item))
                    .sum::<i64>();
            tokens = tokens.max(size + tokens - self.usage);
        }
        tokens
    }
}
pub fn summary(response: &Response) -> String {
    static REVIEW: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<(?:review|analysis)>.*?</(?:review|analysis)>").unwrap()
    });
    static OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(?:review|analysis)>").unwrap());
    static SUMMARY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<summary>(.*?)</summary>").unwrap());
    let parts: Vec<&str> = response
        .output
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            if let LlmData::Message(message) = &item.data
                && (message.role == "assistant" || message.role.is_empty())
                && !message.text.trim().is_empty()
            {
                Some(message.text.trim())
            } else {
                None
            }
        })
        .collect();
    let joined = parts.join("\n\n");
    let text = REVIEW.replace_all(&joined, "");
    if let Some(found) = SUMMARY.captures(&text) {
        return found[1].trim().into();
    }
    if let Some((_, rest)) = text.split_once("<summary>") {
        return rest.trim().into();
    }
    if OPEN.is_match(&text) {
        return String::new();
    }
    text.trim().into()
}
pub fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut head = limit * 2 / 3;
    let mut tail = text.len() - limit / 3;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n[… {} characters left out …]\n{}",
        &text[..head],
        text[head..tail].chars().count(),
        &text[tail..]
    )
}
pub(super) fn kept_inputs(messages: &[String]) -> (Vec<String>, usize) {
    let mut budget = 40_000;
    let mut omitted = 0;
    let mut kept = Vec::new();
    for message in messages.iter().rev() {
        if message.is_empty() {
            continue;
        }
        if omitted > 0 || message.len() > budget {
            omitted += 1;
            continue;
        }
        budget -= message.len();
        kept.push(message.clone());
    }
    kept.reverse();
    (kept, omitted)
}
pub(super) fn describe_call(call: &ToolCall) -> String {
    let arguments = call
        .arguments
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let arguments = if arguments.chars().count() > 300 {
        format!("{}…", arguments.chars().take(299).collect::<String>())
    } else {
        arguments
    };
    format!("{} ({} {arguments})", call.call_id, call.name)
}
pub(super) fn estimate_item(item: &LlmItem) -> i64 {
    4 + match &item.data {
        LlmData::Message(message) => {
            estimate_text(&message.text)
                + message
                    .images
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|image| images::image_tokens(&image.url))
                    .sum::<i64>()
        }
        LlmData::ToolCall(call) => estimate_text(&call.name) + estimate_text(&call.arguments),
        LlmData::ToolResult(result) => result
            .output
            .iter()
            .map(|part| {
                if part.kind == "image" {
                    images::image_tokens(&part.value)
                } else {
                    estimate_text(&part.value)
                }
            })
            .sum(),
        LlmData::Reasoning(reasoning) => {
            if let Some(raw) = reasoning.raw.as_str() {
                raw.len() as i64 / 4
            } else {
                reasoning
                    .summary
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|text| estimate_text(text))
                    .sum()
            }
        }
    }
}
pub(super) fn estimate_text(text: &str) -> i64 {
    (text.len() as i64 + 3) / 4
}
pub(super) fn estimate_tool(tool: &Tool) -> i64 {
    4 + estimate_text(&tool.name)
        + estimate_text(&tool.description)
        + serde_json::to_vec(&tool.parameters).map_or(0, |value| value.len() as i64 / 4)
}
