use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub offset: i64,
    pub data: Vec<u8>,
}

pub struct Framer {
    maximum: i64,
    pass: bool,
    wire_size: i64,
    emitted_offset: i64,
    frame: Vec<u8>,
    delimiter_start: usize,
    line_has_data: bool,
    after_cr: bool,
    ended_at_cr: bool,
    ended_wire_size: i64,
}

impl Framer {
    pub fn new(maximum: i64, pass: bool) -> Self {
        assert!(maximum >= 0);
        Self {
            maximum,
            pass,
            wire_size: 0,
            emitted_offset: 0,
            frame: Vec::new(),
            delimiter_start: 0,
            line_has_data: false,
            after_cr: false,
            ended_at_cr: false,
            ended_wire_size: 0,
        }
    }

    fn append_bytes(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if bytes.len() as i64 > self.maximum - self.wire_size {
            return Err(());
        }
        self.frame.extend_from_slice(bytes);
        self.wire_size += bytes.len() as i64;
        Ok(())
    }

    fn take_frame(&mut self, delimiter_start: usize) -> Frame {
        let mut data = std::mem::take(&mut self.frame);
        if !self.pass {
            data.truncate(delimiter_start);
        }
        let frame = Frame {
            offset: self.emitted_offset,
            data,
        };
        self.emitted_offset += frame.data.len() as i64;
        self.wire_size = 0;
        self.delimiter_start = 0;
        frame
    }

    // A size failure still returns frames completed earlier in this chunk.
    pub fn append(&mut self, mut encoded: &[u8]) -> (Vec<Frame>, bool) {
        let mut frames: Vec<Frame> = Vec::new();
        while !encoded.is_empty() {
            if self.after_cr {
                if encoded[0] == b'\n' {
                    if self.ended_at_cr {
                        if self.ended_wire_size >= self.maximum {
                            return (frames, true);
                        }
                        self.ended_wire_size += 1;
                        if self.pass {
                            if let Some(last) = frames.last_mut() {
                                last.data.push(encoded[0]);
                            } else {
                                frames.push(Frame {
                                    offset: self.emitted_offset,
                                    data: vec![encoded[0]],
                                });
                            }
                            self.emitted_offset += 1;
                        }
                        self.after_cr = false;
                        self.ended_at_cr = false;
                        self.ended_wire_size = 0;
                        encoded = &encoded[1..];
                        continue;
                    }
                    if self.append_bytes(&encoded[..1]).is_err() {
                        return (frames, true);
                    }
                    self.after_cr = false;
                    encoded = &encoded[1..];
                    continue;
                }
                self.after_cr = false;
                self.ended_at_cr = false;
                self.ended_wire_size = 0;
            }
            let Some(line_ending) = memchr::memchr2(b'\r', b'\n', encoded) else {
                if self.append_bytes(encoded).is_err() {
                    return (frames, true);
                }
                self.line_has_data = true;
                break;
            };
            let segment = &encoded[..=line_ending];
            if self.append_bytes(segment).is_err() {
                return (frames, true);
            }
            if line_ending != 0 {
                self.line_has_data = true;
            }
            let character = segment[line_ending];
            encoded = &encoded[line_ending + 1..];
            match character {
                b'\r' => {
                    self.ended_at_cr = !self.line_has_data;
                    if self.line_has_data {
                        self.delimiter_start = self.frame.len() - 1;
                    }
                    self.line_has_data = false;
                    self.after_cr = true;
                    if self.ended_at_cr {
                        self.ended_wire_size = self.wire_size;
                        frames.push(self.take_frame(self.delimiter_start));
                    }
                }
                b'\n' => {
                    if self.line_has_data {
                        self.delimiter_start = self.frame.len() - 1;
                    } else {
                        frames.push(self.take_frame(self.delimiter_start));
                    }
                    self.line_has_data = false;
                }
                _ => unreachable!(),
            }
        }
        (frames, false)
    }

    pub fn finish(&mut self) -> Vec<Frame> {
        self.after_cr = false;
        self.ended_at_cr = false;
        self.ended_wire_size = 0;
        if self.frame.is_empty() || !self.pass {
            self.wire_size = 0;
            self.frame.clear();
            self.delimiter_start = 0;
            return Vec::new();
        }
        vec![self.take_frame(self.frame.len())]
    }
}

#[derive(Deserialize)]
pub struct Case {
    name: String,
    chunks: Vec<String>,
    limit: i64,
    pass: bool,
    frames: Vec<WireFrame>,
    error: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct WireFrame {
    offset: i64,
    data: String,
}

pub fn validate(cases: &[Case]) {
    for case in cases {
        let mut framer = Framer::new(case.limit, case.pass);
        let mut actual = Vec::new();
        let mut error = false;
        for chunk in &case.chunks {
            let (frames, failed) = framer.append(chunk.as_bytes());
            actual.extend(frames);
            if failed {
                error = true;
                break;
            }
        }
        if !error {
            actual.extend(framer.finish());
        }
        let actual: Vec<_> = actual
            .into_iter()
            .map(|f| WireFrame {
                offset: f.offset,
                data: String::from_utf8(f.data).expect("ASCII test fixture"),
            })
            .collect();
        assert_eq!(actual, case.frames, "frames: {}", case.name);
        assert_eq!(error, case.error, "error: {}", case.name);
    }
}

pub fn run(encoded: &[u8], chunked: bool) -> (usize, usize, u64) {
    let mut framer = Framer::new(128, false);
    let mut summary: (usize, usize, u64) = (0, 0, 0);
    let chunk = if chunked { 97 } else { encoded.len() };
    for bytes in encoded.chunks(chunk) {
        let (frames, error) = framer.append(bytes);
        assert!(!error);
        for frame in frames {
            summary.0 += 1;
            summary.1 += frame.data.len();
            summary.2 = summary
                .2
                .wrapping_add(frame.offset as u64)
                .wrapping_add(frame.data.first().copied().unwrap_or(0) as u64)
                .wrapping_add(frame.data.last().copied().unwrap_or(0) as u64);
        }
    }
    assert!(framer.finish().is_empty());
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_frames_survive_failure() {
        let mut f = Framer::new(5, false);
        let (frames, error) = f.append(b"one\n\n123456");
        assert!(error);
        assert_eq!(
            frames,
            vec![Frame {
                offset: 0,
                data: b"one".to_vec()
            }]
        );
    }

    #[test]
    fn split_crlf_counts_toward_limit() {
        let mut f = Framer::new(5, false);
        assert!(!f.append(b"one\n\r").1);
        assert!(f.append(b"\n").1);
    }

    #[test]
    fn passthrough_keeps_incomplete_tail() {
        let mut f = Framer::new(1024, true);
        assert!(f.append(b"data: tail").0.is_empty());
        assert_eq!(f.finish()[0].data, b"data: tail");
    }
}
