use super::compaction::estimate_item;
use super::*;
use base64::{
    alphabet,
    engine::{GeneralPurpose, GeneralPurposeConfig},
};
use std::io::{self, BufReader, Read};
const LEFT_OUT: &str = "[An image was left out of the context to keep the requests within the provider's limits; view it again if needed.]";
const DECODER: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
);

pub(super) fn image_tokens(reference: &str) -> i64 {
    let Some(rest) = reference.strip_prefix("data:") else {
        return 1600;
    };
    let Some((_, data)) = rest.split_once(',') else {
        return 1600;
    };
    let decoder = base64::read::DecoderReader::new(Encoded(data.bytes()), &DECODER);
    let Some((width, height)) = super::image_config::dimensions(&mut BufReader::new(decoder))
    else {
        return 1600;
    };
    if width == 0 || height == 0 {
        return 1600;
    }
    let mut width = width as f64;
    let mut height = height as f64;
    let longest = width.max(height);
    if longest > 2576.0 {
        width *= 2576.0 / longest;
        height *= 2576.0 / longest;
    }
    ((width * height / 750.0) as i64).max(1)
}
impl Builder {
    pub(super) fn bound_images(&mut self) {
        let (mut count, mut bytes) = (0, 0);
        for value in &self.committed {
            let (n, size) = image_size(value);
            count += n;
            bytes += size;
        }
        if count <= 40 && bytes <= 20 << 20 {
            return;
        }
        let mut left = 0;
        let mut counted = 0;
        for index in 0..self.committed.len() {
            if count <= 30 && bytes <= (20 << 20) * 3 / 4 {
                break;
            }
            let value = &self.committed[index];
            let (n, size) = image_size(value);
            if n == 0 {
                continue;
            }
            let replacement = without_images(value);
            if index < self.usage_mark {
                counted += estimate_item(value) - estimate_item(&replacement);
            }
            self.committed[index] = replacement;
            count -= n;
            bytes -= size;
            left += n;
        }
        self.uncount(counted);
        self.images_left_out += left;
    }
}
fn image_size(value: &LlmItem) -> (usize, usize) {
    match &value.data {
        LlmData::Message(message) => message
            .images
            .as_deref()
            .unwrap_or_default()
            .iter()
            .fold((0, 0), |(n, bytes), image| (n + 1, bytes + image.url.len())),
        LlmData::ToolResult(result) => result
            .output
            .iter()
            .filter(|part| part.kind == "image")
            .fold((0, 0), |(n, bytes), part| (n + 1, bytes + part.value.len())),
        _ => (0, 0),
    }
}
fn without_images(value: &LlmItem) -> Item {
    let data = match &value.data {
        LlmData::Message(message) => {
            let labels: Vec<&str> = message
                .images
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|image| image.label.trim())
                .filter(|label| !label.is_empty())
                .collect();
            let mut note = "[The images of this message were left out of the context to keep the requests within the provider's limits".to_owned();
            if !labels.is_empty() {
                note.push_str(": ");
                note.push_str(&labels.join(", "));
            }
            note.push_str(".]");
            LlmData::Message(Message {
                role: message.role.clone(),
                text: format!("{}\n\n{note}", message.text).trim().into(),
                phase: message.phase.clone(),
                images: None,
            })
        }
        LlmData::ToolResult(result) => LlmData::ToolResult(ToolResult {
            call_id: result.call_id.clone(),
            output: result
                .output
                .iter()
                .map(|part| {
                    if part.kind == "image" {
                        ToolResultOutput {
                            kind: "text".into(),
                            value: LEFT_OUT.into(),
                        }
                    } else {
                        part.clone()
                    }
                })
                .collect(),
        }),
        _ => unreachable!(),
    };
    with_data(value, data)
}

struct Encoded<'a>(std::str::Bytes<'a>);
impl Read for Encoded<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let mut written = 0;
        for target in output {
            let Some(byte) = self.0.find(|byte| !matches!(byte, b'\r' | b'\n')) else {
                break;
            };
            *target = byte;
            written += 1;
        }
        Ok(written)
    }
}
