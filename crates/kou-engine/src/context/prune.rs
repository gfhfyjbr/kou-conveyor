use super::compaction::estimate_item;
use super::*;
const PRUNED: &str = "[The rest of this result was pruned from the context to save space";
const MASKED: &str = "[This result was masked to save space";

impl Builder {
    pub(super) fn mask(&mut self) {
        let keep = self.masking.results;
        if keep == 0 {
            return;
        }
        let boundary = self
            .committed
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, item)| matches!(item.data, LlmData::ToolResult(_)))
            .take(keep)
            .last()
            .map_or(self.committed.len(), |(index, _)| index);
        let mut counted = 0;
        let mut masked = 0;
        for index in 0..boundary {
            let value = &self.committed[index];
            let LlmData::ToolResult(result) = &value.data else {
                continue;
            };
            if marked(&result.output, MASKED) || is_running(&result.output) {
                continue;
            }
            let cut = cut_result(value, None);
            let saving = estimate_item(value) - estimate_item(&cut);
            if saving <= 0 {
                continue;
            }
            self.committed[index] = cut;
            if index < self.usage_mark {
                counted += saving;
            }
            masked += 1;
        }
        self.uncount(counted);
        self.masked += masked;
    }
    pub(super) fn prune(&mut self) {
        let budget = self.pruning.budget;
        if budget <= 0 {
            return;
        }
        let estimated = self.estimate();
        if estimated <= budget {
            return;
        }
        let thinking = self.old_thinking(self.pruning.thinking_turns);
        let mut saved = 0;
        let mut counted = 0;
        for &index in &thinking {
            let saving = estimate_item(&self.committed[index]);
            saved += saving;
            if index < self.usage_mark {
                counted += saving;
            }
        }
        let mut results: Vec<usize> = self
            .committed
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                if let LlmData::ToolResult(result) = &item.data
                    && !marked(&result.output, PRUNED)
                    && !marked(&result.output, MASKED)
                    && !is_running(&result.output)
                {
                    Some(index)
                } else {
                    None
                }
            })
            .collect();
        results.truncate(results.len().saturating_sub(self.pruning.keep));
        if let Some(first) = &self.pruning.first {
            results.sort_by_key(|index| {
                let LlmData::ToolResult(result) = &self.committed[*index].data else {
                    unreachable!()
                };
                !first(
                    self.call(&result.call_id)
                        .map_or("", |call| call.name.as_str()),
                )
            });
        }
        let target = budget - budget / 4;
        let mut cut = Vec::new();
        for index in results {
            if estimated - saved <= target {
                break;
            }
            let value = &self.committed[index];
            let pruned = cut_result(value, Some(self.pruning.bytes));
            let saving = estimate_item(value) - estimate_item(&pruned);
            if saving > 0 {
                cut.push((index, pruned));
                saved += saving;
                if index < self.usage_mark {
                    counted += saving;
                }
            }
        }
        if saved < budget / 8 {
            return;
        }
        self.pruned += cut.len();
        for (index, value) in cut {
            self.committed[index] = value;
        }
        self.recorded_prefix_delta += thinking.len() as i64;
        let mut thinking = thinking.into_iter().peekable();
        let mut index = 0;
        let old_mark = self.usage_mark;
        self.committed.retain(|_| {
            let keep = thinking.peek() != Some(&index);
            if !keep {
                thinking.next();
                if index < old_mark {
                    self.usage_mark -= 1;
                }
                self.thinking_left_out += 1;
            }
            index += 1;
            keep
        });
        self.uncount(counted);
    }
    fn old_thinking(&self, turns: usize) -> Vec<usize> {
        if turns == 0 {
            return Vec::new();
        }
        let starts = turn_starts(&self.committed);
        let mut seen = 0;
        let mut thinking = false;
        for index in (1..self.committed.len()).rev() {
            if matches!(self.committed[index].data, LlmData::Reasoning(_)) {
                thinking = true;
            } else if starts[index] && thinking {
                seen += 1;
                thinking = false;
                if seen < turns {
                    continue;
                }
                return self.committed[..index]
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        matches!(item.data, LlmData::Reasoning(_)).then_some(index)
                    })
                    .collect();
            }
        }
        Vec::new()
    }
}
fn marked(output: &[ToolResultOutput], note: &str) -> bool {
    output.len() == 1 && output[0].kind == "text" && output[0].value.contains(note)
}
fn cut_result(value: &LlmItem, bytes: Option<usize>) -> Item {
    let LlmData::ToolResult(result) = &value.data else {
        unreachable!()
    };
    let mut text = String::new();
    let mut images = 0;
    for part in &result.output {
        if part.kind == "image" {
            images += 1;
        } else {
            text.push_str(&part.value);
        }
    }
    let paths = output_paths(&text);
    let mut note = if let Some(bytes) = bytes {
        pruned_result(&text, bytes, paths)
    } else {
        let mut note = MASKED.to_owned();
        if !paths.is_empty() {
            note.push_str("; the whole output is in ");
            note.push_str(paths);
        }
        note.push_str("; TranscriptSearch finds it.]");
        note
    };
    if images > 0 {
        note = format!(
            "[{images} image(s) were {} from the context; view them again if needed.]\n{note}",
            if bytes.is_some() { "pruned" } else { "masked" }
        );
    }
    with_data(
        value,
        LlmData::ToolResult(ToolResult {
            call_id: result.call_id.clone(),
            output: vec![ToolResultOutput {
                kind: "text".into(),
                value: note,
            }],
        }),
    )
}
fn pruned_result(text: &str, bytes: usize, paths: &str) -> String {
    let mut head = String::new();
    if bytes > 0 && !text.is_empty() {
        head = clip(text, bytes);
        if head == text {
            return head;
        }
        if let Some(at) = head.find("\n[… ") {
            head.truncate(at);
        }
        head.truncate(head.trim_end_matches('\n').len());
        head.push('\n');
    }
    head.push_str(PRUNED);
    if !paths.is_empty() {
        head.push_str("; the whole output is in ");
        head.push_str(paths);
    }
    head.push_str("; TranscriptSearch finds it too.]");
    head
}
fn output_paths(text: &str) -> &str {
    let Some((_, rest)) = text.split_once("complete output in ") else {
        return "";
    };
    rest.split("...")
        .next()
        .unwrap_or_default()
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim()
}
fn turn_starts(items: &[Item]) -> Vec<bool> {
    let mut starts = vec![false; items.len()];
    let mut calls = HashSet::new();
    let mut responding = false;
    for (index, item) in items.iter().enumerate() {
        match &item.data {
            LlmData::ToolCall(call) => {
                if !responding {
                    calls.clear();
                }
                responding = true;
                calls.insert(call.call_id.as_str());
            }
            LlmData::Reasoning(_) => {
                if !responding {
                    calls.clear();
                }
                responding = true;
            }
            LlmData::Message(message) if message.role == "assistant" => {
                if !responding {
                    calls.clear();
                }
                responding = true;
            }
            LlmData::Message(message) => {
                starts[index] = index > 0
                    && (!message.text.trim().is_empty()
                        || message
                            .images
                            .as_ref()
                            .is_some_and(|images| !images.is_empty()));
                responding = false;
            }
            LlmData::ToolResult(result) => {
                starts[index] = !calls.contains(result.call_id.as_str());
                responding = false;
            }
        }
    }
    starts
}
