//! The composer's text and selection, and every edit to them, without a window:
//! what a key does is decided here and tested here. Offsets are UTF-8 bytes on
//! grapheme boundaries; the input method speaks UTF-16 and is converted at the edge.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation as _;

/// How many undo steps are kept.
const UNDO_DEPTH: usize = 200;

/// What the last edit was, so a run of typing undoes as one step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Last {
    /// Nothing an edit can join.
    Other,
    /// Typed text ending at this offset.
    Typed(usize),
    /// A deletion that left the cursor at this offset.
    Deleted(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    text: String,
    selection: Range<usize>,
    reversed: bool,
}

/// Text with a selection, a composition range and an undo history.
#[derive(Clone, Debug)]
pub(crate) struct Buffer {
    text: String,
    selection: Range<usize>,
    /// The cursor is at the start of the selection rather than its end.
    reversed: bool,
    /// Text the input method is still composing.
    marked: Option<Range<usize>>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last: Last,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            text: String::new(),
            selection: 0..0,
            reversed: false,
            marked: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last: Last::Other,
        }
    }
}

impl Buffer {
    /// The whole text.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Whether there is no text.
    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The selected range; empty when there is only a cursor.
    pub(crate) fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    /// Where the cursor is: the moving end of the selection.
    pub(crate) fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    /// Whether the cursor is at the start of the selection.
    pub(crate) fn reversed(&self) -> bool {
        self.reversed
    }

    /// The range the input method is composing, if any.
    pub(crate) fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// The selected text.
    pub(crate) fn selected_text(&self) -> &str {
        &self.text[self.selection.clone()]
    }

    /// Replace everything, as one undoable step, with the cursor at the end.
    pub(crate) fn set_text(&mut self, text: &str) {
        if self.text == text {
            self.move_to(text.len());
            return;
        }
        self.checkpoint();
        self.text = text.to_owned();
        self.selection = text.len()..text.len();
        self.reversed = false;
        self.marked = None;
        self.last = Last::Other;
    }

    /// Take the text out and start over with an empty history.
    pub(crate) fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        *self = Self::default();
        text
    }

    /// Put the cursor at `offset`, dropping the selection.
    pub(crate) fn move_to(&mut self, offset: usize) {
        let offset = self.clamp(offset);
        self.selection = offset..offset;
        self.reversed = false;
        self.last = Last::Other;
    }

    /// Move the cursor to `offset`, keeping the other end of the selection.
    pub(crate) fn select_to(&mut self, offset: usize) {
        let offset = self.clamp(offset);
        let anchor = if self.reversed {
            self.selection.end
        } else {
            self.selection.start
        };
        self.reversed = offset < anchor;
        self.selection = offset.min(anchor)..offset.max(anchor);
        self.last = Last::Other;
    }

    /// Select `range`, cursor at its end.
    pub(crate) fn select(&mut self, range: Range<usize>) {
        let start = self.clamp(range.start);
        let end = self.clamp(range.end).max(start);
        self.selection = start..end;
        self.reversed = false;
        self.last = Last::Other;
    }

    /// Select everything.
    pub(crate) fn select_all(&mut self) {
        self.select(0..self.text.len());
    }

    /// Move or extend by a motion that needs no layout.
    pub(crate) fn motion(&mut self, motion: Motion, extend: bool) {
        let cursor = self.cursor();
        let target = match motion {
            // Without shift, an arrow collapses a selection to its edge instead of moving.
            Motion::Left if !extend && !self.selection.is_empty() => self.selection.start,
            Motion::Right if !extend && !self.selection.is_empty() => self.selection.end,
            Motion::Left => previous_grapheme(&self.text, cursor),
            Motion::Right => next_grapheme(&self.text, cursor),
            Motion::WordLeft => previous_word_start(&self.text, cursor),
            Motion::WordRight => next_word_end(&self.text, cursor),
            Motion::LineStart => line_start(&self.text, cursor),
            Motion::LineEnd => line_end(&self.text, cursor),
            Motion::Start => 0,
            Motion::End => self.text.len(),
        };
        if extend {
            self.select_to(target);
        } else {
            self.move_to(target);
        }
    }

    /// Type or paste `text` over the selection (or over the composition, if one is open).
    pub(crate) fn insert(&mut self, text: &str) {
        let range = self
            .marked
            .clone()
            .unwrap_or_else(|| self.selection.clone());
        self.replace(range, text);
    }

    /// Replace `range` with `text` and put the cursor after it.
    pub(crate) fn replace(&mut self, range: Range<usize>, text: &str) {
        let range = self.clamp(range.start)..self.clamp(range.end).max(self.clamp(range.start));
        if range.is_empty() && text.is_empty() {
            self.marked = None;
            return;
        }
        // A run of typing is one undo step, broken at each new word and each new line.
        let joins = range.is_empty()
            && self.last == Last::Typed(range.start)
            && text != "\n"
            && !(text.starts_with(char::is_alphanumeric)
                && self.text[..range.start].ends_with(char::is_whitespace));
        // Committing a composition is part of the step taken when it opened.
        if !joins && self.marked.is_none() {
            self.checkpoint();
        }
        self.text.replace_range(range.clone(), text);
        let cursor = range.start + text.len();
        self.selection = cursor..cursor;
        self.reversed = false;
        self.marked = None;
        self.last = Last::Typed(cursor);
    }

    /// Delete the selection, or else what `motion` would move over.
    pub(crate) fn delete(&mut self, motion: Motion) {
        let range = if self.selection.is_empty() {
            let mut probe = self.clone();
            probe.motion(motion, true);
            probe.selection
        } else {
            self.selection.clone()
        };
        if range.is_empty() {
            return;
        }
        if self.last != Last::Deleted(range.end) && self.last != Last::Deleted(range.start) {
            self.checkpoint();
        }
        self.text.replace_range(range.clone(), "");
        self.selection = range.start..range.start;
        self.reversed = false;
        self.marked = None;
        self.last = Last::Deleted(range.start);
    }

    /// The input method replaced `range` (or the open composition, or the
    /// selection) with text it is still composing. `selected` is relative to `text`.
    pub(crate) fn compose(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let range = range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        let range = self.clamp(range.start)..self.clamp(range.end).max(self.clamp(range.start));
        // One undo step for a whole composition, taken when it opens.
        if self.marked.is_none() {
            self.checkpoint();
        }
        self.text.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.selection = match selected {
            Some(selected) => {
                let clamp = |offset: usize| {
                    let mut offset = offset.min(text.len());
                    while !text.is_char_boundary(offset) {
                        offset -= 1;
                    }
                    range.start + offset
                };
                clamp(selected.start)..clamp(selected.end).max(clamp(selected.start))
            }
            None => range.start + text.len()..range.start + text.len(),
        };
        self.reversed = false;
        self.last = Last::Other;
    }

    /// The input method is done composing; the text stays.
    pub(crate) fn unmark(&mut self) {
        self.marked = None;
    }

    /// Step back. Returns whether anything changed.
    pub(crate) fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    /// Step forward again. Returns whether anything changed.
    pub(crate) fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    /// The word around `offset`, or the run of other characters there.
    pub(crate) fn word_at(&self, offset: usize) -> Range<usize> {
        let offset = self.clamp(offset);
        for (start, word) in self.text.split_word_bound_indices() {
            let end = start + word.len();
            if offset < end || end == self.text.len() {
                return start..end;
            }
        }
        offset..offset
    }

    /// The line around `offset`, without its line break.
    pub(crate) fn line_at(&self, offset: usize) -> Range<usize> {
        let offset = self.clamp(offset);
        line_start(&self.text, offset)..line_end(&self.text, offset)
    }

    /// A UTF-16 offset as a byte offset, clamped to the text.
    pub(crate) fn offset_from_utf16(&self, utf16: usize) -> usize {
        let mut units = 0;
        for (index, ch) in self.text.char_indices() {
            if units >= utf16 {
                return index;
            }
            units += ch.len_utf16();
        }
        self.text.len()
    }

    /// A byte offset as a UTF-16 offset.
    pub(crate) fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text[..self.clamp(offset)].encode_utf16().count()
    }

    /// A UTF-16 range as a byte range.
    pub(crate) fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    /// A byte range as a UTF-16 range.
    pub(crate) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn clamp(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
            reversed: self.reversed,
        }
    }

    fn checkpoint(&mut self) {
        self.redo.clear();
        let snapshot = self.snapshot();
        if self
            .undo
            .last()
            .is_some_and(|last| last.text == snapshot.text)
        {
            return;
        }
        self.undo.push(snapshot);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.selection = snapshot.selection;
        self.reversed = snapshot.reversed;
        self.marked = None;
        self.last = Last::Other;
    }
}

/// A cursor movement that depends only on the text. Movement by visual line
/// needs the wrapped layout and lives with the element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    /// Start of the line as typed (not as wrapped).
    LineStart,
    /// End of the line as typed.
    LineEnd,
    Start,
    End,
}

/// The grapheme boundary before `offset`.
pub(crate) fn previous_grapheme(text: &str, offset: usize) -> usize {
    text[..offset]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(index, _)| index)
}

/// The grapheme boundary after `offset`.
pub(crate) fn next_grapheme(text: &str, offset: usize) -> usize {
    text[offset..]
        .graphemes(true)
        .next()
        .map_or(text.len(), |grapheme| offset + grapheme.len())
}

fn is_word(segment: &str) -> bool {
    segment.chars().any(|c| c.is_alphanumeric() || c == '_')
}

/// Where Option+Left (Ctrl+Left) lands: the start of the word before `offset`.
pub(crate) fn previous_word_start(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .take_while(|(start, _)| *start < offset)
        .filter(|(_, segment)| is_word(segment))
        .last()
        .map_or(0, |(start, _)| start)
}

/// Where Option+Right (Ctrl+Right) lands: the end of the word after `offset`.
pub(crate) fn next_word_end(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .filter(|(_, segment)| is_word(segment))
        .map(|(start, segment)| start + segment.len())
        .find(|end| *end > offset)
        .unwrap_or(text.len())
}

/// The start of the line `offset` is on.
pub(crate) fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

/// The end of the line `offset` is on, before its line break.
pub(crate) fn line_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> Buffer {
        let mut buffer = Buffer::default();
        buffer.insert(text);
        buffer
    }

    /// The text with `|` at the cursor, or `[`..`]` around a selection.
    fn show(buffer: &Buffer) -> String {
        let mut text = buffer.text().to_owned();
        let selection = buffer.selection();
        if selection.is_empty() {
            text.insert(selection.start, '|');
        } else {
            text.insert(selection.end, ']');
            text.insert(selection.start, '[');
        }
        text
    }

    #[test]
    fn typing_replaces_the_selection_and_moves_the_cursor() {
        let mut buffer = buffer("hello world");
        assert_eq!(show(&buffer), "hello world|");
        buffer.select(0..5);
        buffer.insert("привет");
        assert_eq!(show(&buffer), "привет| world");
        buffer.insert(",");
        assert_eq!(show(&buffer), "привет,| world");
    }

    #[test]
    fn deleting_goes_by_grapheme_word_and_line() {
        let mut buffer = buffer("один два\nтри e\u{301}👨‍👩‍👧");
        buffer.delete(Motion::Left);
        assert_eq!(show(&buffer), "один два\nтри e\u{301}|");
        buffer.delete(Motion::Left);
        assert_eq!(show(&buffer), "один два\nтри |");
        buffer.delete(Motion::WordLeft);
        assert_eq!(show(&buffer), "один два\n|");
        buffer.move_to(0);
        buffer.delete(Motion::Right);
        assert_eq!(show(&buffer), "|дин два\n");
        buffer.delete(Motion::WordRight);
        assert_eq!(show(&buffer), "| два\n");
        buffer.delete(Motion::LineEnd);
        assert_eq!(show(&buffer), "|\n");
        // At an edge there is nothing to delete, and nothing breaks.
        buffer.delete(Motion::Left);
        assert_eq!(show(&buffer), "|\n");
        buffer.select_all();
        buffer.delete(Motion::Left);
        assert_eq!(show(&buffer), "|");
        buffer.delete(Motion::Right);
        assert_eq!(show(&buffer), "|");
    }

    #[test]
    fn deleting_to_the_line_start_stops_at_the_line_break() {
        let mut buffer = buffer("first\nsecond line");
        buffer.delete(Motion::LineStart);
        assert_eq!(show(&buffer), "first\n|");
        buffer.delete(Motion::LineStart);
        assert_eq!(show(&buffer), "first\n|");
    }

    #[test]
    fn movement_by_character_word_line_and_document() {
        let mut buffer = buffer("foo bar_baz, qux\nвторая строка");
        buffer.move_to(0);
        buffer.motion(Motion::WordRight, false);
        assert_eq!(show(&buffer), "foo| bar_baz, qux\nвторая строка");
        buffer.motion(Motion::WordRight, false);
        assert_eq!(show(&buffer), "foo bar_baz|, qux\nвторая строка");
        buffer.motion(Motion::LineEnd, false);
        assert_eq!(show(&buffer), "foo bar_baz, qux|\nвторая строка");
        buffer.motion(Motion::Right, false);
        assert_eq!(show(&buffer), "foo bar_baz, qux\n|вторая строка");
        buffer.motion(Motion::WordRight, false);
        assert_eq!(show(&buffer), "foo bar_baz, qux\nвторая| строка");
        buffer.motion(Motion::End, false);
        buffer.motion(Motion::WordLeft, false);
        assert_eq!(show(&buffer), "foo bar_baz, qux\nвторая |строка");
        buffer.motion(Motion::LineStart, false);
        assert_eq!(show(&buffer), "foo bar_baz, qux\n|вторая строка");
        buffer.motion(Motion::Left, false);
        buffer.motion(Motion::WordLeft, false);
        assert_eq!(show(&buffer), "foo bar_baz, |qux\nвторая строка");
        buffer.motion(Motion::Start, false);
        assert_eq!(show(&buffer), "|foo bar_baz, qux\nвторая строка");
        buffer.motion(Motion::Left, false);
        assert_eq!(buffer.cursor(), 0);
    }

    #[test]
    fn shift_extends_and_a_plain_arrow_collapses_to_the_edge() {
        let mut buffer = buffer("abc def");
        buffer.move_to(4);
        buffer.motion(Motion::Right, true);
        buffer.motion(Motion::Right, true);
        assert_eq!(show(&buffer), "abc [de]f");
        assert!(!buffer.reversed());
        // Shift+Left past the anchor flips the selection.
        buffer.motion(Motion::WordLeft, true);
        buffer.motion(Motion::WordLeft, true);
        assert_eq!(show(&buffer), "[abc ]def");
        assert!(buffer.reversed());
        assert_eq!(buffer.cursor(), 0);
        assert_eq!(buffer.selected_text(), "abc ");
        buffer.motion(Motion::Right, false);
        assert_eq!(show(&buffer), "abc |def");
        buffer.select_all();
        buffer.motion(Motion::Left, false);
        assert_eq!(show(&buffer), "|abc def");
        buffer.motion(Motion::LineEnd, true);
        assert_eq!(show(&buffer), "[abc def]");
    }

    #[test]
    fn words_and_lines_under_a_double_and_triple_click() {
        let buffer = buffer("сделай git push\nвторая");
        assert_eq!(&buffer.text()[buffer.word_at(2)], "сделай");
        assert_eq!(&buffer.text()[buffer.word_at(14)], "git");
        assert_eq!(&buffer.text()[buffer.line_at(14)], "сделай git push");
        assert_eq!(
            &buffer.text()[buffer.line_at(buffer.text().len())],
            "вторая"
        );
        assert_eq!(
            &buffer.text()[buffer.word_at(buffer.text().len())],
            "вторая"
        );
        assert_eq!(Buffer::default().word_at(0), 0..0);
    }

    #[test]
    fn undo_steps_back_word_by_word_and_redo_returns() {
        let mut buffer = Buffer::default();
        for ch in "fix the".chars() {
            buffer.insert(&ch.to_string());
        }
        buffer.insert("\n");
        buffer.insert("b");
        buffer.insert("u");
        buffer.insert("g");
        assert_eq!(show(&buffer), "fix the\nbug|");
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "fix the\n|");
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "fix the|");
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "fix |");
        assert!(buffer.redo());
        assert_eq!(show(&buffer), "fix the|");
        // A new edit forgets what could have been redone.
        buffer.insert("!");
        assert!(!buffer.redo());
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "fix the|");
        assert!(buffer.undo());
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "|");
        assert!(!buffer.undo());
    }

    #[test]
    fn a_run_of_backspaces_is_one_undo_step_and_moving_breaks_a_run() {
        let mut buffer = buffer("abcdef");
        buffer.delete(Motion::Left);
        buffer.delete(Motion::Left);
        buffer.delete(Motion::Left);
        assert_eq!(show(&buffer), "abc|");
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "abcdef|");

        let mut buffer = Buffer::default();
        buffer.insert("a");
        buffer.insert("b");
        buffer.move_to(0);
        buffer.move_to(2);
        buffer.insert("c");
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "ab|");
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "|");
    }

    #[test]
    fn undo_restores_a_replaced_selection() {
        let mut buffer = buffer("keep this");
        buffer.select(5..9);
        buffer.insert("that");
        assert_eq!(show(&buffer), "keep that|");
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "keep [this]");
    }

    #[test]
    fn a_composition_marks_its_text_and_commits_as_one_step() {
        let mut buffer = buffer("A🙂Z");
        // Replace the emoji (UTF-16 1..3) with a composition, selecting inside it.
        let range = buffer.range_from_utf16(&(1..3));
        assert_eq!(range, 1..5);
        buffer.compose(Some(range), "é🙂", Some(2..6));
        assert_eq!(buffer.text(), "Aé🙂Z");
        assert_eq!(buffer.marked(), Some(1..7));
        assert_eq!(buffer.selection(), 3..7);
        // The next composition update replaces the marked text.
        buffer.compose(None, "漢", None);
        assert_eq!(buffer.text(), "A漢Z");
        assert_eq!(buffer.marked(), Some(1..4));
        assert_eq!(buffer.selection(), 4..4);
        // Committing replaces the marked text and closes the composition.
        buffer.insert("字");
        assert_eq!(show(&buffer), "A字|Z");
        assert_eq!(buffer.marked(), None);
        // An emptied composition is no composition.
        buffer.compose(None, "x", None);
        buffer.compose(None, "", None);
        assert_eq!(buffer.marked(), None);
        assert_eq!(buffer.text(), "A字Z");
    }

    #[test]
    fn utf16_offsets_round_trip_and_clamp() {
        let buffer = buffer("e🙂漢я");
        assert_eq!(buffer.offset_to_utf16(buffer.text().len()), 5);
        assert_eq!(buffer.offset_from_utf16(1), 1);
        assert_eq!(buffer.offset_from_utf16(3), 5);
        assert_eq!(buffer.offset_from_utf16(4), 8);
        assert_eq!(buffer.offset_from_utf16(99), buffer.text().len());
        assert_eq!(buffer.range_to_utf16(&(1..8)), 1..4);
        assert_eq!(buffer.range_from_utf16(&(3..99)), 5..10);
    }

    #[test]
    fn offsets_inside_a_character_are_pulled_back_to_its_start() {
        let mut buffer = buffer("яя");
        buffer.move_to(1);
        assert_eq!(buffer.cursor(), 0);
        buffer.select(1..3);
        assert_eq!(buffer.selection(), 0..2);
        buffer.replace(3..99, "!");
        assert_eq!(buffer.text(), "я!");
    }

    #[test]
    fn set_text_is_undoable_and_take_starts_over() {
        let mut buffer = buffer("draft");
        buffer.set_text("recalled prompt");
        assert_eq!(show(&buffer), "recalled prompt|");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "draft");
        assert_eq!(buffer.take(), "draft");
        assert!(buffer.is_empty());
        assert!(!buffer.undo());
    }
}
