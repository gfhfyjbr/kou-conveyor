use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{self, Write};

// json/v2 canonicalizes string escapes supplied by an SDK MarshalJSON method,
// but preserves number spelling and object order. Do that without a message buffer.
struct Canonical<W> {
    out: W,
    in_string: bool,
    escape: Escape,
}
#[derive(Default)]
enum Escape {
    #[default]
    None,
    Slash,
    Hex {
        value: u16,
        digits: u8,
        high: Option<u16>,
    },
    HighSlash(u16),
    HighU(u16),
}
impl<W: Write> Canonical<W> {
    fn new(out: W) -> Self {
        Self {
            out,
            in_string: false,
            escape: Escape::None,
        }
    }
    fn scalar(&mut self, value: u32) -> io::Result<()> {
        let escape = match value {
            8 => Some(&b"\\b"[..]),
            9 => Some(&b"\\t"[..]),
            10 => Some(&b"\\n"[..]),
            12 => Some(&b"\\f"[..]),
            13 => Some(&b"\\r"[..]),
            34 => Some(&b"\\\""[..]),
            92 => Some(&b"\\\\"[..]),
            _ => None,
        };
        if let Some(escape) = escape {
            return self.out.write_all(escape);
        }
        if value < 32 {
            let digits = b"0123456789abcdef";
            return self.out.write_all(&[
                b'\\',
                b'u',
                b'0',
                b'0',
                digits[(value >> 4) as usize],
                digits[(value & 15) as usize],
            ]);
        }
        let scalar =
            char::from_u32(value).ok_or_else(|| io::Error::other("invalid JSON surrogate pair"))?;
        let mut bytes = [0; 4];
        self.out
            .write_all(scalar.encode_utf8(&mut bytes).as_bytes())
    }
    fn byte(&mut self, byte: u8) -> io::Result<()> {
        let bad = || io::Error::other("invalid JSON string escape");
        match std::mem::take(&mut self.escape) {
            Escape::None => {
                if byte == b'"' {
                    self.in_string = !self.in_string;
                }
                if byte == b'\\' && self.in_string {
                    self.escape = Escape::Slash;
                    Ok(())
                } else {
                    self.out.write_all(&[byte])
                }
            }
            Escape::Slash => match byte {
                b'u' => {
                    self.escape = Escape::Hex {
                        value: 0,
                        digits: 0,
                        high: None,
                    };
                    Ok(())
                }
                b'"' | b'\\' | b'/' => self.scalar(byte as u32),
                b'b' => self.scalar(8),
                b'f' => self.scalar(12),
                b'n' => self.scalar(10),
                b'r' => self.scalar(13),
                b't' => self.scalar(9),
                _ => Err(bad()),
            },
            Escape::HighSlash(high) => {
                if byte != b'\\' {
                    return Err(bad());
                }
                self.escape = Escape::HighU(high);
                Ok(())
            }
            Escape::HighU(high) => {
                if byte != b'u' {
                    return Err(bad());
                }
                self.escape = Escape::Hex {
                    value: 0,
                    digits: 0,
                    high: Some(high),
                };
                Ok(())
            }
            Escape::Hex {
                mut value,
                mut digits,
                high,
            } => {
                let digit = match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => return Err(bad()),
                };
                value = (value << 4) | digit as u16;
                digits += 1;
                if digits < 4 {
                    self.escape = Escape::Hex {
                        value,
                        digits,
                        high,
                    };
                    return Ok(());
                }
                if let Some(high) = high {
                    if !(0xdc00..=0xdfff).contains(&value) {
                        return Err(bad());
                    }
                    return self.scalar(
                        0x10000 + ((high as u32 - 0xd800) << 10) + (value as u32 - 0xdc00),
                    );
                }
                if (0xd800..=0xdbff).contains(&value) {
                    self.escape = Escape::HighSlash(value);
                    Ok(())
                } else {
                    self.scalar(value as u32)
                }
            }
        }
    }
    fn finish(self) -> io::Result<W> {
        if self.in_string || !matches!(self.escape, Escape::None) {
            return Err(io::Error::other("unfinished JSON string"));
        }
        Ok(self.out)
    }
}
impl<W: Write> Write for Canonical<W> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let length = bytes.len();
        while !bytes.is_empty() {
            if matches!(self.escape, Escape::None) {
                let count = bytes
                    .iter()
                    .position(|byte| matches!(byte, b'"' | b'\\'))
                    .unwrap_or(bytes.len());
                self.out.write_all(&bytes[..count])?;
                bytes = &bytes[count..];
                if bytes.is_empty() {
                    break;
                }
            }
            self.byte(bytes[0])?;
            bytes = &bytes[1..];
        }
        Ok(length)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}
struct HashWriter<'a>(&'a mut Sha256);
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Default)]
struct Count(u64);
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("prefix length overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode<W: Write>(value: &impl Serialize, writer: W) -> Result<W> {
    let mut canonical = Canonical::new(writer);
    kou_domain::write_compact_json(&mut canonical, value)
        .map_err(|error| Error(error.to_string()))?;
    canonical.finish().map_err(|error| Error(error.to_string()))
}
pub(super) fn fingerprint(value: &impl Serialize) -> Result<String> {
    let mut hasher = Sha256::new();
    encode(value, HashWriter(&mut hasher))?;
    Ok(URL_SAFE_NO_PAD.encode(&hasher.finalize()[..6]))
}
#[derive(Default)]
pub(super) struct Rolling(Sha256);
impl Rolling {
    pub fn observe(&mut self, value: &impl Serialize) -> Result<String> {
        let length = encode(value, Count::default())?.0;
        self.0.update(length.to_be_bytes());
        encode(value, HashWriter(&mut self.0))?;
        Ok(URL_SAFE_NO_PAD.encode(&self.0.clone().finalize()[..6]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_escape_and_chunk_boundaries() {
        let raw=br#"{"b":1.00e+02,"s":"\u003c\/\u2028\uD83D\uDE00\u0000\u000a\u0022\u005c","plain":"line\n"}"#;
        let expected =
            "{\"b\":1.00e+02,\"s\":\"</\u{2028}😀\\u0000\\n\\\"\\\\\",\"plain\":\"line\\n\"}";
        for cut in 0..=raw.len() {
            let mut writer = Canonical::new(Vec::new());
            writer.write_all(&raw[..cut]).unwrap();
            writer.write_all(&raw[cut..]).unwrap();
            assert_eq!(writer.finish().unwrap(), expected.as_bytes(), "cut {cut}");
        }
        let raw: &serde_json::value::RawValue = serde_json::from_slice(raw).unwrap();
        let expected_hash = URL_SAFE_NO_PAD.encode(&Sha256::digest(expected.as_bytes())[..6]);
        assert_eq!(fingerprint(&raw).unwrap(), expected_hash);
    }
    #[test]
    fn rejects_unpaired_surrogates_without_panicking() {
        for raw in [br#""\uD800""#.as_slice(), br#""\uDC00""#, br#""\uD800x""#] {
            let mut writer = Canonical::new(Vec::new());
            assert!(
                writer
                    .write_all(raw)
                    .and_then(|_| writer.finish().map(|_| ()))
                    .is_err()
            );
        }
    }
}
