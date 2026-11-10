use crate::{Error, Result};
use std::borrow::Cow;

/// Strip-mode SSE framing used by the Go provider transport: LF, CRLF and CR
/// end lines; an incomplete frame at EOF is discarded, never dispatched.
pub struct Framer {
    maximum: usize,
    frame: Vec<u8>,
    wire: usize,
    delimiter_start: usize,
    line_data: bool,
    after_cr: bool,
    ended_at_cr: bool,
    ended_wire: usize,
    failed: bool,
}
impl Framer {
    pub fn new(maximum: usize) -> Result<Self> {
        if maximum == 0 {
            return Err(Error("SSE frame limit must be positive".into()));
        }
        Ok(Self {
            maximum,
            frame: Vec::new(),
            wire: 0,
            delimiter_start: 0,
            line_data: false,
            after_cr: false,
            ended_at_cr: false,
            ended_wire: 0,
            failed: false,
        })
    }
    /// A callback returns true to stop reading (for example at a terminal event).
    pub fn feed(
        &mut self,
        bytes: &[u8],
        mut emit: impl FnMut(&[u8]) -> Result<bool>,
    ) -> Result<bool> {
        if self.failed {
            return Err(Error("SSE framer has failed".into()));
        }
        let result = self.append(bytes, &mut emit);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn append(
        &mut self,
        mut bytes: &[u8],
        emit: &mut impl FnMut(&[u8]) -> Result<bool>,
    ) -> Result<bool> {
        while !bytes.is_empty() {
            if self.after_cr {
                if bytes[0] == b'\n' {
                    if self.ended_at_cr {
                        if self.ended_wire >= self.maximum {
                            return Err(self.size_error());
                        }
                    } else {
                        self.save(&bytes[..1])?;
                    }
                    self.after_cr = false;
                    self.ended_at_cr = false;
                    self.ended_wire = 0;
                    bytes = &bytes[1..];
                    continue;
                }
                self.after_cr = false;
                self.ended_at_cr = false;
                self.ended_wire = 0;
            }
            let end = bytes.iter().position(|b| matches!(b, b'\r' | b'\n'));
            let Some(end) = end else {
                self.save(bytes)?;
                self.line_data = true;
                break;
            };
            self.save(&bytes[..end + 1])?;
            if end != 0 {
                self.line_data = true;
            }
            let ending = bytes[end];
            bytes = &bytes[end + 1..];
            if ending == b'\r' {
                self.ended_at_cr = !self.line_data;
                if self.line_data {
                    self.delimiter_start = self.frame.len() - 1;
                }
                self.line_data = false;
                self.after_cr = true;
                if self.ended_at_cr {
                    self.ended_wire = self.wire;
                    if self.take(emit)? {
                        return Ok(true);
                    }
                }
            } else {
                if self.line_data {
                    self.delimiter_start = self.frame.len() - 1;
                } else if self.take(emit)? {
                    return Ok(true);
                }
                self.line_data = false;
            }
        }
        Ok(false)
    }
    fn take(&mut self, emit: &mut impl FnMut(&[u8]) -> Result<bool>) -> Result<bool> {
        let result = emit(&self.frame[..self.delimiter_start]);
        self.frame.clear();
        self.wire = 0;
        self.delimiter_start = 0;
        result
    }
    fn save(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > self.maximum - self.wire {
            return Err(self.size_error());
        }
        let needed = self.frame.len() + bytes.len();
        if needed > self.frame.capacity() {
            let capacity = needed
                .checked_next_power_of_two()
                .unwrap_or(self.maximum)
                .min(self.maximum);
            self.frame.reserve_exact(capacity - self.frame.len());
        }
        self.frame.extend_from_slice(bytes);
        self.wire += bytes.len();
        Ok(())
    }
    fn size_error(&self) -> Error {
        Error(format!("remote SSE frame exceeds {} bytes", self.maximum))
    }
    pub fn discard_partial(&mut self) {
        self.frame = Vec::new();
        self.wire = 0;
        self.delimiter_start = 0;
        self.line_data = false;
        self.after_cr = false;
        self.ended_at_cr = false;
        self.ended_wire = 0;
    }
    pub fn buffered_bytes(&self) -> usize {
        self.frame.len()
    }
}

/// No data field differs from an explicitly empty one. The usual one-line
/// event borrows its payload; multiline data is joined only when needed.
pub fn data(mut frame: &[u8]) -> Option<Cow<'_, [u8]>> {
    if let Some(rest) = frame.strip_prefix(b"\xef\xbb\xbf") {
        frame = rest;
    }
    let mut result = None;
    for line in frame
        .split(|b| matches!(b, b'\r' | b'\n'))
        .filter(|line| !line.is_empty())
    {
        let (field, mut value) = line
            .iter()
            .position(|b| *b == b':')
            .map_or((line, &[][..]), |at| (&line[..at], &line[at + 1..]));
        if field != b"data" {
            continue;
        }
        if let Some(rest) = value.strip_prefix(b" ") {
            value = rest;
        }
        result = Some(match result {
            None => Cow::Borrowed(value),
            Some(Cow::Borrowed(previous)) => {
                let mut combined = Vec::with_capacity(frame.len());
                combined.extend_from_slice(previous);
                combined.push(b'\n');
                combined.extend_from_slice(value);
                Cow::Owned(combined)
            }
            Some(Cow::Owned(mut previous)) => {
                previous.push(b'\n');
                previous.extend_from_slice(value);
                Cow::Owned(previous)
            }
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_chunk_boundaries_preserve_cr_lf_bom_and_multiline_fields() {
        let input = b"\xef\xbb\xbf:comment\r\ndata: one\r\ndata:two\r\n\r\ndata:\r\rdata: three\n\nunfinished";
        for size in 1..=input.len() {
            let mut framer = Framer::new(1024).unwrap();
            let mut frames = Vec::new();
            for bytes in input.chunks(size) {
                framer
                    .feed(bytes, |frame| {
                        frames.push(data(frame).map(Cow::into_owned));
                        Ok(false)
                    })
                    .unwrap();
            }
            framer.discard_partial();
            assert_eq!(
                frames,
                [
                    Some(b"one\ntwo".to_vec()),
                    Some(vec![]),
                    Some(b"three".to_vec())
                ],
                "chunk={size}"
            );
            assert_eq!(framer.buffered_bytes(), 0);
        }
        assert!(data(b":comment\nevent: foo").is_none());
        assert_eq!(data(b"data"), Some(Cow::Borrowed(&b""[..])));
    }
    #[test]
    fn wire_limit_includes_delimiters_even_when_crlf_is_split() {
        for size in 1..=7 {
            let mut framer = Framer::new(6).unwrap();
            let mut failed = false;
            for part in b"data:\r\n\r\n".chunks(size) {
                if framer.feed(part, |_| Ok(false)).is_err() {
                    failed = true;
                    break;
                }
            }
            assert!(failed);
            assert!(framer.buffered_bytes() <= 6);
        }
        let mut framer = Framer::new(8).unwrap();
        assert!(!framer.feed(b"data:x\n\n", |_| Ok(false)).unwrap());
        assert!(framer.feed(b"x", |_| Ok(true)).is_ok());
        framer.discard_partial();
        assert!(framer.feed(b"data:x\n\ngarbage", |_| Ok(true)).unwrap());
    }
}
