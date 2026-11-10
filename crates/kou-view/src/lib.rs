use kou_domain::{Item, ItemData, LlmData, Session, Timestamp};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionFile {
    pub id: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionDescription {
    pub session: Session,
    pub generation: String,
    pub items: usize,
    pub committed_bytes: u64,
    pub read_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemHeader {
    pub sequence: u64,
    pub recorded_at: Timestamp,
    pub kind: String,
    pub preview: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeaderPage {
    pub generation: String,
    pub items: Vec<ItemHeader>,
    pub next_after: u64,
    pub more: bool,
}

impl ItemHeader {
    pub fn from_item(item: &Item) -> Self {
        let text = match &item.data {
            ItemData::Input(input) => {
                #[derive(Default, Deserialize)]
                #[serde(default)]
                struct Payload {
                    message: String,
                }
                match input.payload.decode::<Payload>() {
                    Ok(payload) if !payload.message.is_empty() => preview(&payload.message),
                    _ => preview(&input.kind),
                }
            }
            ItemData::Turn(turn) => preview(&turn.model),
            ItemData::Fork(fork) => preview(&fork.parent_id),
            ItemData::ToolCallStatus(status) => preview(&status.call_id),
            ItemData::ModelResponse(response) => {
                let mut text = String::new();
                for output in response.response.output.as_deref().unwrap_or_default() {
                    match &output.data {
                        LlmData::Message(message) => {
                            text = preview(&message.text);
                        }
                        LlmData::Reasoning(reasoning) => {
                            if let Some(first) = reasoning.summary.as_ref().and_then(|s| s.first())
                            {
                                text = preview(first);
                            }
                        }
                        LlmData::ToolCall(call) => {
                            text = preview(&call.name);
                        }
                        LlmData::ToolResult(_) => {}
                    }
                    if !text.is_empty() {
                        break;
                    }
                }
                if text.is_empty() {
                    preview(&response.response.stop)
                } else {
                    text
                }
            }
        };
        Self {
            sequence: item.sequence,
            recorded_at: item.recorded_at,
            kind: item.data.kind().into(),
            preview: text,
        }
    }
}

fn preview(text: &str) -> String {
    let mut result = String::new();
    let mut whitespace = false;
    let mut count = 0;
    for ch in text.chars() {
        if count >= 160 {
            result.push('…');
            break;
        }
        if ch.is_whitespace() {
            whitespace = !result.is_empty();
            continue;
        }
        if ch.is_control() {
            continue;
        }
        if whitespace {
            result.push(' ');
            count += 1;
            whitespace = false;
        }
        result.push(ch);
        count += 1;
    }
    result
}

#[cfg(feature = "native")]
pub mod native {
    use super::*;
    use kou_store::{Result, SessionLog};

    pub struct SessionView {
        pub log: SessionLog,
        headers: Vec<ItemHeader>,
    }

    impl SessionView {
        pub fn new(log: SessionLog) -> Result<Self> {
            let mut headers = Vec::with_capacity(log.item_count());
            for sequence in 1..=log.item_count() as u64 {
                headers.push(ItemHeader::from_item(&log.item(sequence)?));
            }
            log.ensure_current()?;
            Ok(Self { log, headers })
        }

        pub fn description(&self) -> SessionDescription {
            SessionDescription {
                session: self.log.session().clone(),
                generation: self.log.generation().into(),
                items: self.headers.len(),
                committed_bytes: self.log.committed_bytes(),
                read_only: true,
            }
        }

        pub fn headers(&self, after: u64, limit: usize) -> Result<HeaderPage> {
            self.log.ensure_current()?;
            let start = usize::try_from(after)
                .unwrap_or(usize::MAX)
                .min(self.headers.len());
            let end = start.saturating_add(limit).min(self.headers.len());
            Ok(HeaderPage {
                generation: self.log.generation().into(),
                items: self.headers[start..end].to_vec(),
                next_after: if start == end { after } else { end as u64 },
                more: end < self.headers.len(),
            })
        }

        pub fn estimated_resident_bytes(&self) -> usize {
            self.log.estimated_resident_bytes()
                + self.headers.capacity() * size_of::<ItemHeader>()
                + self
                    .headers
                    .iter()
                    .map(|h| h.kind.capacity() + h.preview.capacity())
                    .sum::<usize>()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_are_bounded_without_cutting_unicode() {
        assert_eq!(preview("\u{1b} a\n b\t c"), "a b c");
        let value = preview(&"я".repeat(1000));
        assert_eq!(value.chars().count(), 161);
        assert!(value.ends_with('…'));
    }
}
