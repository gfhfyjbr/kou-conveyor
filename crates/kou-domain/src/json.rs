use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::value::RawValue;
use std::{
    collections::HashSet,
    fmt,
    io::{self, Write},
};

/// Remove insignificant whitespace from raw JSON too. A RawValue may contain
/// literal newlines, which must never split a durable JSONL record.
pub fn write_compact_json<W: Write, T: Serialize + ?Sized>(
    writer: W,
    value: &T,
) -> Result<(), serde_json::Error> {
    serde_json::to_writer(
        CompactWriter {
            writer,
            string: false,
            escaped: false,
        },
        value,
    )
}

struct CompactWriter<W> {
    writer: W,
    string: bool,
    escaped: bool,
}

impl<W: Write> Write for CompactWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut start = 0;
        for (index, &byte) in bytes.iter().enumerate() {
            if self.string {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.string = false;
                }
            } else if byte == b'"' {
                self.string = true;
            } else if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
                self.writer.write_all(&bytes[start..index])?;
                start = index + 1;
            }
        }
        self.writer.write_all(&bytes[start..])?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// Missing and JSON null are different in Go's jsontext.Value wire contract.
#[derive(Clone, Debug, Default)]
pub struct Json(Option<Box<RawValue>>);

impl Json {
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        validate_json(text.as_bytes())?;
        serde_json::from_str(text)
    }

    pub fn is_missing(&self) -> bool {
        self.0.is_none()
    }

    pub fn as_str(&self) -> Option<&str> {
        self.0.as_ref().map(|raw| raw.get())
    }

    pub fn decode<T: de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(self.as_str().unwrap_or("null"))
    }
}

impl PartialEq for Json {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Json {}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Some(value) => value.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Box::<RawValue>::deserialize(de).map(|raw| Self(Some(raw)))
    }
}

/// Match jsontext's unique-name and 10,000-level limits without recursively
/// building values or retaining payload strings. RawValue's syntax scanner is
/// iterative, unlike serde's general-purpose deserialize_any visitor.
pub fn validate_json(bytes: &[u8]) -> Result<(), serde_json::Error> {
    std::str::from_utf8(bytes)
        .map_err(|_| <serde_json::Error as de::Error>::custom("invalid UTF-8"))?;
    let raw: &RawValue = serde_json::from_slice(bytes)?;
    let bytes = raw.get().as_bytes();
    enum Frame {
        Array,
        Object { names: HashSet<String>, key: bool },
    }
    let mut frames = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'{' | b'[' => {
                if frames.len() == 10_000 {
                    return Err(de::Error::custom("JSON exceeds 10000 nesting levels"));
                }
                frames.push(if bytes[cursor] == b'{' {
                    Frame::Object {
                        names: HashSet::new(),
                        key: true,
                    }
                } else {
                    Frame::Array
                });
                cursor += 1;
            }
            b'}' | b']' => {
                frames.pop();
                cursor += 1;
            }
            b',' => {
                if let Some(Frame::Object { key, .. }) = frames.last_mut() {
                    *key = true;
                }
                cursor += 1;
            }
            b'"' => {
                let start = cursor;
                cursor += 1;
                let mut unicode_escape = false;
                loop {
                    let offset =
                        memchr::memchr2(b'"', b'\\', &bytes[cursor..]).ok_or_else(|| {
                            <serde_json::Error as de::Error>::custom("unterminated JSON string")
                        })?;
                    cursor += offset;
                    if bytes[cursor] == b'"' {
                        cursor += 1;
                        break;
                    }
                    unicode_escape |= bytes.get(cursor + 1) == Some(&b'u');
                    cursor += 2;
                }
                if let Some(Frame::Object { names, key }) = frames.last_mut()
                    && *key
                {
                    let name: String = serde_json::from_slice(&bytes[start..cursor])?;
                    if !names.insert(name) {
                        return Err(de::Error::custom("duplicate JSON object name"));
                    }
                    *key = false;
                } else if unicode_escape {
                    // Raw syntax scanning accepts lone UTF-16 surrogates. Decode
                    // only strings containing Unicode escapes to reject them.
                    let mut de = serde_json::Deserializer::from_slice(&bytes[start..cursor]);
                    de.deserialize_any(StringCheck)?;
                }
            }
            _ => cursor += 1,
        }
    }
    Ok(())
}

struct StringCheck;
impl<'de> de::Visitor<'de> for StringCheck {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a valid Unicode string")
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_missing_null_and_verbatim_provider_json() {
        assert!(Json::default().is_missing());
        assert!(!Json::parse("null").unwrap().is_missing());
        let value = Json::parse(r#"{ "encrypted": "opaque", "n": 3 }"#).unwrap();
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            value.as_str().unwrap()
        );
    }

    #[test]
    fn rejects_duplicate_and_escaped_duplicate_unknown_keys() {
        for text in [r#"{"x":1,"x":2}"#, r#"{"data":{"a":1,"\u0061":2}}"#] {
            assert!(validate_json(text.as_bytes()).is_err());
        }
        assert!(validate_json(br#"{"x":[1,"a",true,null,{}]}"#).is_ok());
    }
}
