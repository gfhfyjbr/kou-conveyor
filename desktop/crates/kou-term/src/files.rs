//! A guard in front of the emulator for Kitty graphics commands that name a file.
//!
//! libghostty-vt 1.0.0 opens the file of a `t=f` or `t=t` command in blocking mode, so a
//! program that names a pipe nobody writes to (or a terminal device) would stop the
//! reader thread for good. This filter sits in the byte stream before the emulator: it
//! holds back the start of each graphics command until its keys say which medium it
//! uses, and for a file it holds the path until the command ends, checks that the path
//! is a regular file, and otherwise turns the command into one the emulator refuses with
//! an error reply. Every other byte passes through untouched and in order.

use std::path::Path;

/// The longest a command's keys are held back before it is passed on unexamined: keys
/// are a few dozen bytes; anything longer is not a command that names a file.
const MAX_KEYS: usize = 1024;

/// The longest a file command's payload (a base64 path) is held back.
const MAX_PATH_PAYLOAD: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Plain output.
    Ground,
    /// An escape was held back; the next byte decides whether a command starts.
    Escape,
    /// `ESC _` was held back.
    Apc,
    /// Inside a graphics command's keys, held back.
    Keys,
    /// An escape inside the keys: the command may be ending.
    KeysEscape,
    /// Inside the payload of a command that names a file, held back.
    Path,
    /// An escape inside that payload.
    PathEscape,
    /// Inside a command that is passed on as it comes.
    Pass,
    /// An escape inside such a command.
    PassEscape,
    /// The rest of a refused command, dropped up to its end.
    Drop,
    /// An escape inside it.
    DropEscape,
}

/// The filter. One per terminal, fed every byte the program writes.
pub(crate) struct FileGuard {
    state: State,
    held: Vec<u8>,
}

impl Default for FileGuard {
    fn default() -> Self {
        Self {
            state: State::Ground,
            held: Vec::new(),
        }
    }
}

impl FileGuard {
    /// What of `input` goes to the emulator now: `input` itself when nothing in it needs
    /// a look, which is nearly always, otherwise what was filtered into `out`. Bytes held
    /// back come out with a later call.
    pub(crate) fn filter<'a>(&mut self, input: &'a [u8], out: &'a mut Vec<u8>) -> &'a [u8] {
        if self.state == State::Ground && !input.contains(&0x1b) {
            return input;
        }
        out.clear();
        for &byte in input {
            self.step(byte, out);
        }
        out
    }

    fn release(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.held);
    }

    fn step(&mut self, byte: u8, out: &mut Vec<u8>) {
        match self.state {
            State::Ground => {
                if byte == 0x1b {
                    self.held.push(byte);
                    self.state = State::Escape;
                } else {
                    out.push(byte);
                }
            }
            State::Escape => match byte {
                b'_' => {
                    self.held.push(byte);
                    self.state = State::Apc;
                }
                0x1b => {
                    // The first escape introduced nothing; the second may.
                    self.release(out);
                    self.held.push(byte);
                }
                _ => {
                    self.release(out);
                    out.push(byte);
                    self.state = State::Ground;
                }
            },
            State::Apc => {
                self.held.push(byte);
                if byte == b'G' {
                    self.state = State::Keys;
                } else {
                    // Some other application command: none of this filter's business.
                    self.release(out);
                    self.state = if byte == 0x1b {
                        State::PassEscape
                    } else {
                        State::Pass
                    };
                }
            }
            State::Keys => {
                self.held.push(byte);
                match byte {
                    b';' => {
                        if names_a_file(&self.held) {
                            self.state = State::Path;
                        } else {
                            self.release(out);
                            self.state = State::Pass;
                        }
                    }
                    0x1b => self.state = State::KeysEscape,
                    0x18 | 0x1a => {
                        // Cancelled.
                        self.release(out);
                        self.state = State::Ground;
                    }
                    _ if self.held.len() > MAX_KEYS => {
                        self.release(out);
                        self.state = State::Pass;
                    }
                    _ => {}
                }
            }
            State::KeysEscape => {
                // A command without a payload names no file.
                self.held.push(byte);
                self.release(out);
                self.state = if byte == b'\\' {
                    State::Ground
                } else {
                    State::Pass
                };
            }
            State::Path => match byte {
                0x1b => {
                    self.held.push(byte);
                    self.state = State::PathEscape;
                }
                0x18 | 0x1a => {
                    self.held.push(byte);
                    self.release(out);
                    self.state = State::Ground;
                }
                _ => {
                    self.held.push(byte);
                    if self.held.len() > MAX_PATH_PAYLOAD {
                        // No path is this long: refuse it and drop the rest.
                        out.extend_from_slice(&refused(&self.held));
                        self.held.clear();
                        // The emulator has been told the command is over; the bytes that
                        // follow are part of it and go nowhere.
                        self.state = State::Drop;
                    }
                }
            },
            State::PathEscape => {
                self.held.push(byte);
                if byte == b'\\' {
                    let command = std::mem::take(&mut self.held);
                    if path_is_safe(&command) {
                        out.extend_from_slice(&command);
                    } else {
                        out.extend_from_slice(&refused(&command));
                    }
                    self.state = State::Ground;
                } else {
                    // An escape that is not the terminator ends the command anyway.
                    self.release(out);
                    self.state = State::Ground;
                }
            }
            State::Pass => {
                out.push(byte);
                match byte {
                    0x1b => self.state = State::PassEscape,
                    0x18 | 0x1a => self.state = State::Ground,
                    _ => {}
                }
            }
            State::PassEscape => {
                out.push(byte);
                self.state = match byte {
                    0x1b => State::PassEscape,
                    _ => State::Ground,
                };
            }
            State::Drop => match byte {
                0x1b => self.state = State::DropEscape,
                0x18 | 0x1a => self.state = State::Ground,
                _ => {}
            },
            State::DropEscape => {
                if byte == b'\\' {
                    self.state = State::Ground;
                } else {
                    // An escape that is not the terminator starts whatever follows it.
                    self.held.push(0x1b);
                    self.state = State::Escape;
                    self.step(byte, out);
                }
            }
        }
    }
}

/// The keys of a held command (`ESC _ G keys ;`).
fn keys(command: &[u8]) -> &[u8] {
    let body = command.get(3..).unwrap_or_default();
    let end = body
        .iter()
        .position(|&byte| byte == b';' || byte == 0x1b)
        .unwrap_or(body.len());
    &body[..end]
}

/// Whether a command's keys ask for the file or the temporary file medium.
fn names_a_file(command: &[u8]) -> bool {
    keys(command)
        .split(|&byte| byte == b',')
        .any(|pair| pair == b"t=f" || pair == b"t=t")
}

/// Whether the path in a complete file command is a regular file, which can be opened
/// without blocking.
fn path_is_safe(command: &[u8]) -> bool {
    let Some(start) = command.iter().position(|&byte| byte == b';') else {
        return false;
    };
    // The payload ends at the terminator, `ESC \`.
    let payload = &command[start + 1..command.len().saturating_sub(2)];
    let Some(path) = decode_base64(payload) else {
        return false;
    };
    let Ok(path) = String::from_utf8(path) else {
        return false;
    };
    // Follows symbolic links, as opening it would.
    std::fs::metadata(Path::new(&path)).is_ok_and(|metadata| metadata.is_file())
}

/// The command with its file medium replaced by an empty direct transmission, which the
/// emulator answers with an error (when the program asked for answers) instead of
/// opening anything.
fn refused(command: &[u8]) -> Vec<u8> {
    let keys: Vec<&[u8]> = keys(command)
        .split(|&byte| byte == b',')
        .filter(|pair| !pair.starts_with(b"t="))
        .collect();
    let mut out = b"\x1b_G".to_vec();
    out.extend_from_slice(&keys.join(&b","[..]));
    out.extend_from_slice(b",t=d;\x1b\\");
    out
}

fn decode_base64(text: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(text.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut bits = 0;
    for &byte in text {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            decoded.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::tests::base64;

    fn run(chunks: &[&[u8]]) -> Vec<u8> {
        let mut guard = FileGuard::default();
        let mut all = Vec::new();
        let mut out = Vec::new();
        for chunk in chunks {
            all.extend_from_slice(guard.filter(chunk, &mut out));
        }
        all
    }

    fn file_command(path: &str) -> Vec<u8> {
        format!("\x1b_Ga=q,i=1,t=f,f=100;{}\x1b\\", base64(path.as_bytes())).into_bytes()
    }

    #[test]
    fn plain_output_and_other_commands_pass_untouched() {
        let input: &[u8] =
            b"text \x1b[31mred\x1b]0;title\x07 \x1b_Ga=T,f=100;AAAA\x1b\\ \x1b_Xother\x1b\\";
        assert_eq!(run(&[input]), input);
        // Split anywhere, the same bytes come out in the same order.
        for split in 1..input.len() {
            assert_eq!(
                run(&[&input[..split], &input[split..]]),
                input,
                "split at {split}"
            );
        }
    }

    #[test]
    fn a_regular_file_passes_and_anything_else_is_refused() {
        let directory = tempfile::tempdir().expect("a directory");
        let file = directory.path().join("picture.png");
        std::fs::write(&file, b"png").expect("write");
        let good = file_command(&file.to_string_lossy());
        assert_eq!(run(&[&good]), good);

        for bad in ["/dev/zero", "/nonexistent/picture.png", "/tmp"] {
            let command = file_command(bad);
            let out = run(&[&command]);
            assert_eq!(out, b"\x1b_Ga=q,i=1,f=100,t=d;\x1b\\", "for {bad}");
        }
    }

    #[test]
    fn a_held_command_split_across_reads_is_judged_whole() {
        let command = file_command("/dev/null");
        let (first, second) = command.split_at(12);
        let out = run(&[b"before ", first, second, b" after"]);
        assert_eq!(out, b"before \x1b_Ga=q,i=1,f=100,t=d;\x1b\\ after");
    }

    #[test]
    fn a_path_that_never_ends_is_refused_and_does_not_grow() {
        let mut guard = FileGuard::default();
        let mut out = Vec::new();
        let mut passed = Vec::new();
        passed.extend_from_slice(guard.filter(b"\x1b_Ga=t,i=2,t=t;", &mut out));
        for _ in 0..100 {
            passed.extend_from_slice(guard.filter(&[b'A'; 1024], &mut out));
        }
        assert!(guard.held.len() <= MAX_PATH_PAYLOAD + 64);
        assert!(passed.starts_with(b"\x1b_Ga=t,i=2,t=d;\x1b\\"));
    }
}
