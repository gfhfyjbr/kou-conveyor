//! Operating system commands an agent workspace needs: desktop notifications (OSC 9,
//! OSC 777, OSC 99), progress (OSC 9;4) and the working directory (OSC 7, OSC 1337
//! CurrentDir, OSC 9;9).
//!
//! libghostty-vt reports all of them except kitty's OSC 99, which it parses and drops;
//! the scanner here sees the same byte stream as the emulator, on the reader thread, and
//! picks those up. The rest of the module decodes what the emulator hands over.

use std::path::PathBuf;

/// What one recognised sequence asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OscEvent {
    /// A desktop notification.
    Notification {
        /// The headline, when the sequence carries one.
        title: Option<String>,
        /// The message.
        body: String,
    },
    /// Task progress in percent; `None` clears it.
    Progress(Option<u8>),
    /// The program's working directory.
    Cwd(PathBuf),
    /// A shell says the command it ran is over (OSC 133;D, which a shell
    /// that marks its prompts prints before each one).
    CommandFinished,
}

/// A longer string than any notification needs; beyond it the sequence is dropped so a
/// program cannot grow the buffer without bound.
const MAX_OSC_LEN: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Osc,
    OscEscape,
}

/// A kitty notification (OSC 99) that arrives in several chunks under one identifier.
#[derive(Default)]
struct PendingNotification {
    id: String,
    title: String,
    body: String,
}

/// Incremental scanner over the bytes a program writes.
pub(crate) struct OscScanner {
    state: State,
    buffer: Vec<u8>,
    overflowed: bool,
    pending: Option<PendingNotification>,
    /// Report only what the emulator does not: kitty notifications (OSC 99), and a
    /// shell's word that a command is over (OSC 133;D), which the emulator takes in
    /// without saying so.
    only_kitty: bool,
}

impl Default for OscScanner {
    fn default() -> Self {
        Self {
            state: State::Ground,
            buffer: Vec::new(),
            overflowed: false,
            pending: None,
            only_kitty: false,
        }
    }
}

impl OscScanner {
    /// A scanner that reports only kitty desktop notifications (OSC 99). libghostty-vt
    /// parses those but drops them, and reports every other sequence here itself.
    pub(crate) fn kitty_notifications() -> Self {
        Self {
            only_kitty: true,
            ..Self::default()
        }
    }

    /// Feeds the next chunk of output and calls `emit` for every sequence that completed in it.
    pub(crate) fn feed(&mut self, bytes: &[u8], mut emit: impl FnMut(OscEvent)) {
        let mut at = 0;
        while at < bytes.len() {
            if self.state == State::Ground {
                // Plain output is the common case: skip to the next escape in one pass.
                match bytes[at..].iter().position(|byte| *byte == 0x1b) {
                    Some(offset) => {
                        at += offset + 1;
                        self.state = State::Escape;
                    }
                    None => return,
                }
                continue;
            }
            let byte = bytes[at];
            at += 1;
            match self.state {
                State::Ground => {}
                State::Escape => {
                    self.state = match byte {
                        b']' => {
                            self.buffer.clear();
                            self.overflowed = false;
                            State::Osc
                        }
                        0x1b => State::Escape,
                        _ => State::Ground,
                    };
                }
                State::Osc => match byte {
                    0x07 => {
                        self.finish(&mut emit);
                        self.state = State::Ground;
                    }
                    0x1b => self.state = State::OscEscape,
                    // CAN and SUB abort a control string.
                    0x18 | 0x1a => self.state = State::Ground,
                    _ => {
                        if self.buffer.len() < MAX_OSC_LEN {
                            self.buffer.push(byte);
                        } else {
                            self.overflowed = true;
                        }
                    }
                },
                State::OscEscape => {
                    self.state = match byte {
                        b'\\' => {
                            self.finish(&mut emit);
                            State::Ground
                        }
                        // An escape that is not the terminator abandons the string and
                        // starts whatever sequence it introduces.
                        b']' => {
                            self.buffer.clear();
                            self.overflowed = false;
                            State::Osc
                        }
                        0x1b => State::Escape,
                        _ => State::Ground,
                    };
                }
            }
        }
    }

    fn finish(&mut self, emit: &mut impl FnMut(OscEvent)) {
        if self.overflowed {
            return;
        }
        let buffer = std::mem::take(&mut self.buffer);
        let text = String::from_utf8_lossy(&buffer);
        let (code, rest) = match text.split_once(';') {
            Some((code, rest)) => (code, rest),
            None => (text.as_ref(), ""),
        };
        if self.only_kitty && code != "99" && code != "133" {
            self.buffer = buffer;
            self.buffer.clear();
            return;
        }
        match code {
            "7" => {
                if let Some(path) = path_from_uri(rest) {
                    emit(OscEvent::Cwd(path));
                }
            }
            "9" => self.conemu_or_notification(rest, emit),
            "99" => self.kitty_notification(rest, emit),
            "777" => {
                if let Some(rest) = rest.strip_prefix("notify;") {
                    let (title, body) = rest.split_once(';').unwrap_or((rest, ""));
                    emit_notification(title, body, emit);
                }
            }
            "133" => {
                if rest == "D" || rest.starts_with("D;") {
                    emit(OscEvent::CommandFinished);
                }
            }
            "1337" => {
                if let Some(path) = rest.strip_prefix("CurrentDir=")
                    && !path.is_empty()
                {
                    emit(OscEvent::Cwd(PathBuf::from(path)));
                }
            }
            _ => {}
        }
        // Keep the allocation for the next sequence.
        self.buffer = buffer;
        self.buffer.clear();
    }

    /// OSC 9 is iTerm2's "post a notification", except for the ConEmu subcommands that
    /// reuse the number: `4` reports progress and `9` the working directory.
    fn conemu_or_notification(&mut self, rest: &str, emit: &mut impl FnMut(OscEvent)) {
        if let Some(progress) = rest.strip_prefix("4;").or((rest == "4").then_some("")) {
            let mut fields = progress.split(';');
            let state = fields.next().and_then(|state| state.parse::<u8>().ok());
            let value = fields
                .next()
                .and_then(|value| value.parse::<u32>().ok())
                .map_or(0, |value| value.min(100) as u8);
            match state {
                // 0 removes the indicator; anything unparseable is treated the same way
                // rather than leaving a stale bar.
                Some(0) | None => emit(OscEvent::Progress(None)),
                // 3 is "busy, extent unknown": there is no percentage to report.
                Some(3) => emit(OscEvent::Progress(Some(0))),
                Some(_) => emit(OscEvent::Progress(Some(value))),
            }
            return;
        }
        if let Some(path) = rest.strip_prefix("9;") {
            let path = path.trim_matches('"');
            if !path.is_empty() {
                emit(OscEvent::Cwd(PathBuf::from(path)));
            }
            return;
        }
        emit_notification("", rest, emit);
    }

    /// The kitty desktop notification protocol: `99 ; key=value:... ; payload`, where a
    /// notification may be split into a title chunk and a body chunk that share `i=`.
    fn kitty_notification(&mut self, rest: &str, emit: &mut impl FnMut(OscEvent)) {
        let (metadata, payload) = rest.split_once(';').unwrap_or((rest, ""));
        let mut id = "";
        let mut done = true;
        let mut part = "title";
        let mut encoded = false;
        for pair in metadata.split(':') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            match key {
                "i" => id = value,
                "d" => done = value != "0",
                "p" => part = value,
                "e" => encoded = value == "1",
                _ => {}
            }
        }
        let payload = if encoded {
            match decode_base64(payload) {
                Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                None => return,
            }
        } else {
            payload.to_string()
        };
        if !matches!(part, "title" | "body") {
            // Queries, close requests, icons and buttons have nothing to show.
            return;
        }
        let mut pending = match self.pending.take() {
            Some(pending) if pending.id == id => pending,
            _ => PendingNotification {
                id: id.to_string(),
                ..Default::default()
            },
        };
        if part == "title" {
            pending.title.push_str(&payload);
        } else {
            pending.body.push_str(&payload);
        }
        if done {
            emit_notification(&pending.title, &pending.body, emit);
        } else {
            self.pending = Some(pending);
        }
    }
}

/// Reports a notification, moving a lone title into the body so `body` is never empty.
pub(crate) fn emit_notification(title: &str, body: &str, emit: &mut impl FnMut(OscEvent)) {
    let title = title.trim();
    let body = body.trim();
    let event = match (title.is_empty(), body.is_empty()) {
        (true, true) => return,
        (false, true) => OscEvent::Notification {
            title: None,
            body: title.to_string(),
        },
        (true, false) => OscEvent::Notification {
            title: None,
            body: body.to_string(),
        },
        (false, false) => OscEvent::Notification {
            title: Some(title.to_string()),
            body: body.to_string(),
        },
    };
    emit(event);
}

/// Extracts the path from `file://host/path` (OSC 7); a bare absolute path is accepted too.
pub(crate) fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .or_else(|| uri.strip_prefix("kitty-shell-cwd://"));
    let path = match rest {
        Some(rest) => &rest[rest.find('/')?..],
        None if uri.starts_with('/') => uri,
        None => return None,
    };
    let decoded = percent_decode(path);
    (!decoded.is_empty()).then(|| PathBuf::from(decoded))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(hex) = bytes.get(at + 1..at + 3)
            && let Some(value) = std::str::from_utf8(hex)
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(value);
            at += 3;
            continue;
        }
        decoded.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Standard-alphabet base64, with or without padding; `None` on any other character.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(text.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut bits = 0;
    for byte in text.bytes() {
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

    #[test]
    fn a_shells_finished_commands_are_told() {
        // The marks of one command: its start, its end with its status, the
        // next prompt. Only the end counts, cut across reads or not.
        assert_eq!(
            scan(&[b"\x1b]133;C\x07out\r\n\x1b]133;D;0\x07\x1b]133;A;redraw=1\x07% "]),
            [OscEvent::CommandFinished]
        );
        assert_eq!(
            scan(&[b"\x1b]133;", b"D\x1b\\", b"\x1b]133;D;1", b"27\x07"]),
            [OscEvent::CommandFinished, OscEvent::CommandFinished]
        );
        assert_eq!(scan(&[b"\x1b]133;B\x07\x1b]1330;D\x07"]), []);
    }

    fn scan(chunks: &[&[u8]]) -> Vec<OscEvent> {
        let mut scanner = OscScanner::default();
        let mut events = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, |event| events.push(event));
        }
        events
    }

    fn notification(title: Option<&str>, body: &str) -> OscEvent {
        OscEvent::Notification {
            title: title.map(str::to_string),
            body: body.to_string(),
        }
    }

    #[test]
    fn osc_9_is_a_notification() {
        assert_eq!(
            scan(&[b"before\x1b]9;build finished\x07after"]),
            vec![notification(None, "build finished")]
        );
    }

    #[test]
    fn string_terminator_and_split_chunks() {
        assert_eq!(
            scan(&[b"\x1b]9;wai", b"ting for you\x1b", b"\\"]),
            vec![notification(None, "waiting for you")]
        );
    }

    #[test]
    fn osc_777_carries_title_and_body() {
        assert_eq!(
            scan(&[b"\x1b]777;notify;Claude Code;needs; input\x07"]),
            vec![notification(Some("Claude Code"), "needs; input")]
        );
    }

    #[test]
    fn osc_99_simple_and_chunked() {
        assert_eq!(
            scan(&[b"\x1b]99;;hello\x1b\\"]),
            vec![notification(None, "hello")]
        );
        assert_eq!(
            scan(&[
                b"\x1b]99;i=7:d=0;Codex\x1b\\",
                b"\x1b]99;i=7:d=1:p=body;turn complete\x1b\\"
            ]),
            vec![notification(Some("Codex"), "turn complete")]
        );
        // "aGVsbG8=" is "hello".
        assert_eq!(
            scan(&[b"\x1b]99;e=1;aGVsbG8=\x07"]),
            vec![notification(None, "hello")]
        );
    }

    #[test]
    fn progress_states() {
        assert_eq!(
            scan(&[b"\x1b]9;4;1;42\x07\x1b]9;4;0\x07\x1b]9;4;3\x07\x1b]9;4;2;250\x07"]),
            vec![
                OscEvent::Progress(Some(42)),
                OscEvent::Progress(None),
                OscEvent::Progress(Some(0)),
                OscEvent::Progress(Some(100)),
            ]
        );
    }

    #[test]
    fn working_directory() {
        assert_eq!(
            scan(&[b"\x1b]7;file://host.local/Users/me/my%20project\x07"]),
            vec![OscEvent::Cwd(PathBuf::from("/Users/me/my project"))]
        );
        assert_eq!(
            scan(&[b"\x1b]1337;CurrentDir=/tmp/x\x07"]),
            vec![OscEvent::Cwd(PathBuf::from("/tmp/x"))]
        );
    }

    #[test]
    fn other_sequences_are_ignored() {
        assert_eq!(
            scan(&[b"\x1b]0;a title\x07\x1b[31mred\x1b[0m\x1b]8;;http://x\x1b\\"]),
            vec![]
        );
    }

    #[test]
    fn an_interrupted_string_does_not_swallow_the_next() {
        assert_eq!(
            scan(&[b"\x1b]9;lost\x1b[0m\x1b]9;kept\x07"]),
            vec![notification(None, "kept")]
        );
        assert_eq!(scan(&[b"\x1b]9;aborted\x18 text"]), vec![]);
    }

    #[test]
    fn a_kitty_only_scanner_reports_nothing_else() {
        let mut scanner = OscScanner::kitty_notifications();
        let mut events = Vec::new();
        scanner.feed(
            b"\x1b]9;skipped\x07\x1b]7;file:///tmp\x07\x1b]99;;kept\x1b\\",
            |event| events.push(event),
        );
        assert_eq!(events, vec![notification(None, "kept")]);
        // Except a shell's word that a command is over, which nothing else reports.
        events.clear();
        scanner.feed(b"\x1b]133;C\x07done\r\n\x1b]133;D;0\x07", |event| {
            events.push(event)
        });
        assert_eq!(events, vec![OscEvent::CommandFinished]);
    }

    #[test]
    fn an_oversized_string_is_dropped() {
        let mut bytes = b"\x1b]9;".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', MAX_OSC_LEN + 10));
        bytes.push(0x07);
        bytes.extend_from_slice(b"\x1b]9;next\x07");
        assert_eq!(scan(&[&bytes]), vec![notification(None, "next")]);
    }
}
