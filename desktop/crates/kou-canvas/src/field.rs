//! A small text field for what the canvas lets the user write in place: a
//! node's title in its header or the inspector, an edge's template, a note.
//! It edits on key presses (characters as the keyboard types them, the usual
//! movement and deletion keys, select all, copy, cut and paste) and draws its
//! text with a caret and the selection. It has no input method, so text that
//! needs composing (Chinese, Japanese) cannot be typed into it.

use gpui::Keystroke;

/// What a key did to a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Keep what was written (Enter; Cmd+Enter in a field of several lines).
    Commit,
    /// Leave it as it was (Escape).
    Cancel,
    /// The text changed.
    Edited,
    /// Only the caret or the selection moved.
    Moved,
    /// Put this on the clipboard (copy, or cut, which also edited).
    Copy(String),
    /// Not a key the field uses.
    Ignored,
}

/// A piece of a line as it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// Text, selected or not.
    Text(String, bool),
    /// Where the caret is.
    Caret,
}

/// The text being edited, the caret and the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    multiline: bool,
}

impl Field {
    /// A field holding `text`, all of it selected, so typing replaces it.
    pub fn new(text: &str, multiline: bool) -> Self {
        Self {
            text: text.to_owned(),
            cursor: text.len(),
            anchor: (!text.is_empty()).then_some(0),
            multiline,
        }
    }

    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// A number that changes whenever the caret moves or the text changes
    /// length, for its blink (see [`kou_ui::Blink::at`]).
    pub fn place(&self) -> u64 {
        [
            self.text.len(),
            self.cursor,
            self.anchor.map_or(0, |at| at + 1),
        ]
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, part| {
            (hash ^ *part as u64).wrapping_mul(0x0100_0000_01b3)
        })
    }

    /// The selected range, start before end.
    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| (anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.text.replace_range(start..end, "");
        self.cursor = start;
        self.anchor = None;
        true
    }

    /// Put text where the caret is, over the selection.
    pub fn insert(&mut self, text: &str) {
        let text = if self.multiline {
            text.to_owned()
        } else {
            text.replace(['\n', '\r'], " ")
        };
        self.delete_selection();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.anchor = None;
    }

    fn previous(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn following(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }

    fn word_back(&self, at: usize) -> usize {
        let before = &self.text[..at];
        let trimmed = before.trim_end();
        trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8())
    }

    fn word_forward(&self, at: usize) -> usize {
        let after = &self.text[at..];
        let skipped = after.len() - after.trim_start().len();
        after[skipped..]
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map_or(self.text.len(), |(i, _)| at + skipped + i)
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i)
    }

    fn vertical(&self, at: usize, down: bool) -> usize {
        let start = self.line_start(at);
        let column = self.text[start..at].chars().count();
        let target = if down {
            let end = self.line_end(at);
            if end >= self.text.len() {
                return self.text.len();
            }
            end + 1
        } else {
            if start == 0 {
                return 0;
            }
            self.line_start(start - 1)
        };
        let end = self.line_end(target);
        self.text[target..end]
            .char_indices()
            .nth(column)
            .map_or(end, |(i, _)| target + i)
    }

    fn go(&mut self, to: usize, extend: bool) -> Outcome {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else if let Some((start, end)) = self.selection() {
            // A selection collapses towards the way the caret was sent.
            self.anchor = None;
            self.cursor = if to < self.cursor { start } else { end };
            return Outcome::Moved;
        } else {
            self.anchor = None;
        }
        self.cursor = to;
        Outcome::Moved
    }

    /// Answer a key. `clipboard` is what a paste would put in.
    pub fn key(&mut self, keystroke: &Keystroke, clipboard: Option<String>) -> Outcome {
        let m = &keystroke.modifiers;
        let command = m.platform || m.control;
        let extend = m.shift;
        let at = self.cursor;
        match keystroke.key.as_str() {
            "escape" => Outcome::Cancel,
            "enter" if self.multiline && !command => {
                self.insert("\n");
                Outcome::Edited
            }
            "enter" => Outcome::Commit,
            "backspace" => {
                if !self.delete_selection() {
                    let from = if command {
                        self.line_start(at)
                    } else if m.alt {
                        self.word_back(at)
                    } else {
                        self.previous(at)
                    };
                    self.text.replace_range(from..at, "");
                    self.cursor = from;
                }
                Outcome::Edited
            }
            "delete" => {
                if !self.delete_selection() {
                    let to = if m.alt {
                        self.word_forward(at)
                    } else {
                        self.following(at)
                    };
                    self.text.replace_range(at..to, "");
                }
                Outcome::Edited
            }
            "left" => {
                let to = if command {
                    self.line_start(at)
                } else if m.alt {
                    self.word_back(at)
                } else {
                    self.previous(at)
                };
                self.go(to, extend)
            }
            "right" => {
                let to = if command {
                    self.line_end(at)
                } else if m.alt {
                    self.word_forward(at)
                } else {
                    self.following(at)
                };
                self.go(to, extend)
            }
            "up" if self.multiline => {
                let to = self.vertical(at, false);
                self.go(to, extend)
            }
            "down" if self.multiline => {
                let to = self.vertical(at, true);
                self.go(to, extend)
            }
            "home" => {
                let to = self.line_start(at);
                self.go(to, extend)
            }
            "end" => {
                let to = self.line_end(at);
                self.go(to, extend)
            }
            "a" if command => {
                self.anchor = Some(0);
                self.cursor = self.text.len();
                Outcome::Moved
            }
            "c" if command => match self.selection() {
                Some((start, end)) => Outcome::Copy(self.text[start..end].to_owned()),
                None => Outcome::Ignored,
            },
            "x" if command => match self.selection() {
                Some((start, end)) => {
                    let cut = self.text[start..end].to_owned();
                    self.delete_selection();
                    Outcome::Copy(cut)
                }
                None => Outcome::Ignored,
            },
            "v" if command => match clipboard {
                Some(text) => {
                    self.insert(&text);
                    Outcome::Edited
                }
                None => Outcome::Ignored,
            },
            _ if command => Outcome::Ignored,
            _ => match keystroke.key_char.as_deref() {
                Some(typed) if !typed.chars().any(char::is_control) => {
                    self.insert(typed);
                    Outcome::Edited
                }
                _ if keystroke.key == "space" => {
                    self.insert(" ");
                    Outcome::Edited
                }
                _ => Outcome::Ignored,
            },
        }
    }

    /// The lines as they are drawn: text, the selection marked, the caret.
    pub fn lines(&self) -> Vec<Vec<Part>> {
        let selection = self.selection();
        let mut lines = Vec::new();
        let mut start = 0;
        for line in self.text.split('\n') {
            let end = start + line.len();
            let mut parts = Vec::new();
            // Break the line where the selection and the caret fall.
            let mut cuts = vec![start, end];
            if let Some((a, b)) = selection {
                cuts.extend([a.clamp(start, end), b.clamp(start, end)]);
            }
            if (start..=end).contains(&self.cursor) {
                cuts.push(self.cursor);
            }
            cuts.sort_unstable();
            cuts.dedup();
            for pair in cuts.windows(2) {
                if pair[0] == self.cursor && (start..=end).contains(&self.cursor) {
                    parts.push(Part::Caret);
                }
                let selected = selection.is_some_and(|(a, b)| pair[0] >= a && pair[1] <= b);
                parts.push(Part::Text(self.text[pair[0]..pair[1]].to_owned(), selected));
            }
            if self.cursor == end && !parts.contains(&Part::Caret) {
                parts.push(Part::Caret);
            }
            lines.push(parts);
            start = end + 1;
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(field: &mut Field, keys: &str) -> Outcome {
        let mut last = Outcome::Ignored;
        for key in keys.split(' ') {
            let mut stroke = Keystroke::parse_with_os(key, gpui::OperatingSystem::Mac).unwrap();
            if stroke.key_char.is_none()
                && stroke.key.chars().count() == 1
                && !stroke.modifiers.platform
                && !stroke.modifiers.control
            {
                stroke.key_char = Some(stroke.key.clone());
            }
            last = field.key(&stroke, Some("pasted\ntext".into()));
        }
        last
    }

    #[test]
    fn typing_replaces_the_text_it_starts_with() {
        let mut field = Field::new("lead", false);
        press(&mut field, "n e w");
        assert_eq!(field.text(), "new");
        assert_eq!(press(&mut field, "enter"), Outcome::Commit);
        assert_eq!(press(&mut field, "escape"), Outcome::Cancel);
    }

    #[test]
    fn the_caret_moves_and_deletes_by_character_word_and_line() {
        let mut field = Field::new("review the diff", false);
        press(&mut field, "end");
        press(&mut field, "alt-backspace");
        assert_eq!(field.text(), "review the ");
        press(&mut field, "left left left left left left backspace");
        assert_eq!(field.text(), "reviw the ");
        press(&mut field, "cmd-left delete");
        assert_eq!(field.text(), "eviw the ");
        press(&mut field, "shift-right shift-right");
        assert_eq!(press(&mut field, "cmd-c"), Outcome::Copy("ev".into()));
        press(&mut field, "x");
        assert_eq!(field.text(), "xiw the ");
        press(&mut field, "cmd-a cmd-v");
        assert_eq!(field.text(), "pasted text", "one line keeps one line");
        assert_eq!(press(&mut field, "cmd-q"), Outcome::Ignored);
    }

    #[test]
    fn a_field_of_several_lines_takes_enter_and_moves_between_lines() {
        let mut field = Field::new("1. schema", true);
        press(&mut field, "end enter 2 . space x");
        assert_eq!(field.text(), "1. schema\n2. x");
        press(&mut field, "up");
        press(&mut field, "y");
        assert_eq!(field.text(), "1. sychema\n2. x");
        assert_eq!(press(&mut field, "cmd-enter"), Outcome::Commit);
        let lines = field.lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(&Part::Caret));
        assert!(!lines[1].contains(&Part::Caret));
    }

    #[test]
    fn lines_mark_the_selection_and_the_caret() {
        let field = Field::new("ab", false);
        assert_eq!(
            field.lines(),
            vec![vec![Part::Text("ab".into(), true), Part::Caret]]
        );
        let mut field = Field::new("", false);
        assert_eq!(field.lines(), vec![vec![Part::Caret]]);
        field.insert("añb");
        press(&mut field, "left");
        assert_eq!(
            field.lines(),
            vec![vec![
                Part::Text("añ".into(), false),
                Part::Caret,
                Part::Text("b".into(), false)
            ]]
        );
        press(&mut field, "backspace");
        assert_eq!(field.text(), "ab");
    }
}
