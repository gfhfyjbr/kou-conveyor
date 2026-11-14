//! A command's captured output as lines of styled text. This is a transcript,
//! not an emulator: colours and line rewrites are honoured, cursor addressing
//! is dropped.

use std::ops::Range;

/// How a stretch of output is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Style {
    /// Index into the sixteen ANSI colours, or the default foreground.
    pub color: Option<u8>,
    pub bold: bool,
    pub dim: bool,
}

/// One line of output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Line {
    pub text: String,
    /// Styled ranges, sorted and disjoint; text outside them is default.
    pub spans: Vec<(Range<usize>, Style)>,
}

impl Line {
    fn push(&mut self, ch: char, style: Style) {
        let start = self.text.len();
        self.text.push(ch);
        if style == Style::default() {
            return;
        }
        match self.spans.last_mut() {
            Some((range, last)) if range.end == start && *last == style => {
                range.end = self.text.len();
            }
            _ => self.spans.push((start..self.text.len(), style)),
        }
    }

    fn pop(&mut self) {
        if self.text.pop().is_some() {
            let len = self.text.len();
            self.spans.retain_mut(|(range, _)| {
                range.end = range.end.min(len);
                range.start < range.end
            });
        }
    }

    fn clear(&mut self) {
        self.text.clear();
        self.spans.clear();
    }
}

/// Turn raw output, escape sequences included, into lines.
pub(crate) fn lines(output: &str) -> Vec<Line> {
    let mut lines = vec![Line::default()];
    let mut style = Style::default();
    // A carriage return was seen: the next character starts the line over.
    let mut returned = false;
    let mut chars = output.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\n' => {
                returned = false;
                lines.push(Line::default());
            }
            '\r' => returned = true,
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut last = None;
                    for ch in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&ch) {
                            last = Some(ch);
                            break;
                        }
                        params.push(ch);
                    }
                    match last {
                        Some('m') => sgr(&params, &mut style),
                        // Erase in line: a progress line clearing itself.
                        Some('K') if returned || params == "2" => {
                            returned = false;
                            current(&mut lines).clear();
                        }
                        _ => {}
                    }
                }
                // Operating system command: a title or a hyperlink, up to BEL or ST.
                Some(']') => {
                    while let Some(ch) = chars.next() {
                        if ch == '\u{7}' {
                            break;
                        }
                        if ch == '\u{1b}' {
                            chars.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                // Character set selection takes one more byte.
                Some('(' | ')') => {
                    chars.next();
                }
                _ => {}
            },
            '\u{8}' => current(&mut lines).pop(),
            '\t' => {
                let line = start_over(&mut lines, &mut returned);
                let width = line.text.chars().count();
                for _ in 0..8 - width % 8 {
                    line.push(' ', style);
                }
            }
            ch if ch.is_control() => {}
            ch => start_over(&mut lines, &mut returned).push(ch, style),
        }
    }

    // Output that ends with a line break has no empty last line to show.
    if lines.len() > 1 && lines.last().is_some_and(|line| line.text.is_empty()) {
        lines.pop();
    }
    lines
}

fn current(lines: &mut [Line]) -> &mut Line {
    lines.last_mut().expect("there is always a current line")
}

/// The line to write to: emptied first if a carriage return came before this character.
fn start_over<'a>(lines: &'a mut [Line], returned: &mut bool) -> &'a mut Line {
    let line = current(lines);
    if std::mem::take(returned) {
        line.clear();
    }
    line
}

fn sgr(params: &str, style: &mut Style) {
    let mut codes = params
        .split(';')
        .map(|code| code.parse::<u16>().unwrap_or(0));
    while let Some(code) = codes.next() {
        match code {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            30..=37 => style.color = Some((code - 30) as u8),
            39 => style.color = None,
            90..=97 => style.color = Some((code - 90 + 8) as u8),
            38 => match codes.next() {
                // Only the sixteen named colours have a place in the theme.
                Some(5) => {
                    style.color = codes
                        .next()
                        .filter(|index| *index < 16)
                        .map(|index| index as u8)
                }
                Some(2) => {
                    codes.nth(2);
                    style.color = None;
                }
                _ => {}
            },
            // Background colours are skipped along with their arguments.
            48 => match codes.next() {
                Some(5) => {
                    codes.next();
                }
                Some(2) => {
                    codes.nth(2);
                }
                _ => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(output: &str) -> Vec<String> {
        lines(output).into_iter().map(|line| line.text).collect()
    }

    #[test]
    fn plain_output_splits_into_lines() {
        assert_eq!(texts("one\ntwo\r\nthree\n"), ["one", "two", "three"]);
        assert_eq!(texts(""), [""]);
        assert_eq!(texts("a\n\nb"), ["a", "", "b"]);
    }

    #[test]
    fn colours_become_spans_and_reset_ends_them() {
        let lines = lines("\u{1b}[1;32mok\u{1b}[0m rest \u{1b}[91mfail\u{1b}[39m!");
        assert_eq!(lines[0].text, "ok rest fail!");
        assert_eq!(
            lines[0].spans,
            [
                (
                    0..2,
                    Style {
                        color: Some(2),
                        bold: true,
                        dim: false
                    }
                ),
                (
                    8..12,
                    Style {
                        color: Some(9),
                        bold: false,
                        dim: false
                    }
                ),
            ]
        );
    }

    #[test]
    fn extended_colours_keep_only_the_named_sixteen() {
        let lines = lines(
            "\u{1b}[38;5;4mblue\u{1b}[38;5;208morange\u{1b}[38;2;1;2;3mrgb\u{1b}[48;5;1;2mdim",
        );
        assert_eq!(lines[0].text, "blueorangergbdim");
        assert_eq!(lines[0].spans[0].1.color, Some(4));
        assert_eq!(lines[0].spans.len(), 2);
        assert_eq!(
            lines[0].spans[1],
            (
                13..16,
                Style {
                    color: None,
                    bold: false,
                    dim: true
                }
            )
        );
    }

    #[test]
    fn a_carriage_return_rewrites_the_line() {
        assert_eq!(
            texts("building 10%\rbuilding 80%\rdone\u{1b}[K\nnext"),
            ["done", "next"]
        );
        // A return right before the line break is just a Windows line ending.
        assert_eq!(texts("kept\r\n"), ["kept"]);
    }

    #[test]
    fn other_sequences_and_controls_are_dropped() {
        assert_eq!(
            texts(
                "\u{1b}]0;title\u{7}a\u{1b}[2Jb\u{1b}[10;4Hc\u{1b}]8;;http://x\u{1b}\\d\u{7}\u{1b}(Be"
            ),
            ["abcde"]
        );
        assert_eq!(texts("ab\u{8}c"), ["ac"]);
        assert_eq!(texts("a\tb"), ["a       b"]);
    }

    #[test]
    fn a_sequence_cut_off_mid_stream_does_not_panic() {
        assert_eq!(texts("ok\u{1b}"), ["ok"]);
        assert_eq!(texts("ok\u{1b}[3"), ["ok"]);
        assert_eq!(texts("ok\u{1b}]0;ti"), ["ok"]);
    }
}
