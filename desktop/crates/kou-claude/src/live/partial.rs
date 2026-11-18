//! A call's input as it streams (`input_json_delta`): JSON cut anywhere,
//! which grows a piece at a time. Each time a field at its top level is
//! whole (the comma after it has come), the fields so far can be read: the
//! text up to that comma, closed. The ACP adapter reads a streaming input
//! the same way (`scanStreamedToolInput`), so that a long `Write` shows its
//! file before its content has all come.

use serde_json::Value;

#[derive(Debug, Default)]
pub(crate) struct Partial {
    text: String,
    in_string: bool,
    escaped: bool,
    objects: u32,
    arrays: u32,
    /// Where the last comma between two fields of the top level is.
    comma: Option<usize>,
    /// The comma up to which the fields were last read.
    read: Option<usize>,
}

impl Partial {
    /// All of the input that has come.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Add `piece`. The fields of the input that are whole, when one more
    /// is since they were last given.
    pub fn push(&mut self, piece: &str) -> Option<Value> {
        let offset = self.text.len();
        self.text.push_str(piece);
        for (at, byte) in piece.bytes().enumerate() {
            if self.in_string {
                match byte {
                    _ if self.escaped => self.escaped = false,
                    b'\\' => self.escaped = true,
                    b'"' => self.in_string = false,
                    _ => {}
                }
                continue;
            }
            match byte {
                b'"' => self.in_string = true,
                b'{' => self.objects += 1,
                b'}' => self.objects = self.objects.saturating_sub(1),
                b'[' => self.arrays += 1,
                b']' => self.arrays = self.arrays.saturating_sub(1),
                b',' if self.objects == 1 && self.arrays == 0 => self.comma = Some(offset + at),
                _ => {}
            }
        }
        let comma = self.comma?;
        if self.read == Some(comma) {
            return None;
        }
        self.read = Some(comma);
        let fields: Value = serde_json::from_str(&format!("{}}}", &self.text[..comma])).ok()?;
        fields.is_object().then_some(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_fields_of_a_streaming_input_are_read_as_each_becomes_whole() {
        let mut input = Partial::default();
        assert_eq!(input.push(r#"{"file_"#), None);
        assert_eq!(
            input.push(r#"path": "/work/a, b.rs", "con"#),
            Some(json!({"file_path": "/work/a, b.rs"}))
        );
        // Nothing new is whole: nothing is said again.
        assert_eq!(input.push(r#"tent": "x\", {\"y\": [1, 2]"#), None);
        assert_eq!(
            input.push(r#"}", "#),
            Some(json!({"file_path": "/work/a, b.rs", "content": "x\", {\"y\": [1, 2]}"}))
        );
        assert_eq!(
            input.push(r#""mode": "w"}"#),
            None,
            "the last field is whole only with the input"
        );
        assert_eq!(
            input.text(),
            r#"{"file_path": "/work/a, b.rs", "content": "x\", {\"y\": [1, 2]}", "mode": "w"}"#
        );
        let mut nested = Partial::default();
        assert_eq!(
            nested.push(r#"{"todos": [{"a": 1}, {"b": 2}], "#),
            Some(json!({"todos": [{"a": 1}, {"b": 2}]}))
        );
    }
}
