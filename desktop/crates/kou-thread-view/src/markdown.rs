//! Markdown as a tree of blocks the view lays out natively.
//!
//! The parser is pulldown-cmark; this module only folds its event stream into
//! a shape that maps one to one onto elements, so drawing never re-parses.

use gpui::SharedString;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// One block of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    /// Running text.
    Paragraph(Inline),
    /// A heading of level 1 to 6.
    Heading { level: u8, text: Inline },
    /// A fenced or indented code block, without its trailing newline and with
    /// tabs as spaces. `lang` is the fence's first word.
    Code {
        lang: Option<String>,
        text: SharedString,
    },
    /// A block quote.
    Quote(Vec<Block>),
    /// A list. `start` is the first number of an ordered list.
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    /// A table.
    Table(Table),
    /// A thematic break.
    Rule,
}

/// One list item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    /// `Some` for a task list item: whether it is checked.
    pub task: Option<bool>,
    /// What the item holds, nested lists included.
    pub blocks: Vec<Block>,
}

/// A table: one header row and the body rows, every row as wide as `align`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Table {
    pub align: Vec<Align>,
    pub head: Vec<Inline>,
    pub rows: Vec<Vec<Inline>>,
}

/// How a table column is aligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

/// What a stretch of inline text looks like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
}

/// A styled stretch of an [`Inline`]'s text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub range: Range<usize>,
    pub style: InlineStyle,
}

/// A link inside an [`Inline`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Link {
    pub range: Range<usize>,
    pub url: String,
}

/// Inline content flattened to text plus the ranges that differ from plain.
/// `spans` are sorted and do not overlap; text outside them is unstyled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Inline {
    pub text: SharedString,
    pub spans: Vec<Span>,
    pub links: Vec<Link>,
}

impl Inline {
    /// Unstyled text.
    #[cfg(test)]
    pub(crate) fn plain(text: &str) -> Self {
        Self {
            text: text.to_owned().into(),
            ..Self::default()
        }
    }
}

/// Parse a document. Never fails: text that is not valid markdown is a paragraph,
/// and a construct cut off mid-stream (an open code fence, half a table) is
/// whatever CommonMark makes of the text so far.
pub(crate) fn parse(source: &str) -> Vec<Block> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut builder = Builder::default();
    for event in Parser::new_ext(source, options) {
        builder.event(event);
    }
    builder.finish()
}

/// The first non-empty line of a document's text, for a collapsed summary.
pub(crate) fn first_line(blocks: &[Block]) -> Option<&str> {
    blocks.iter().find_map(|block| match block {
        Block::Paragraph(inline) | Block::Heading { text: inline, .. } => {
            inline.text.lines().find(|line| !line.trim().is_empty())
        }
        Block::Code { text, .. } => text.lines().find(|line| !line.trim().is_empty()),
        Block::Quote(blocks) => first_line(blocks),
        Block::List { items, .. } => items.iter().find_map(|item| first_line(&item.blocks)),
        Block::Table(table) => table.head.first().map(|cell| &*cell.text),
        Block::Rule => None,
    })
}

enum Frame {
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Item {
        task: Option<bool>,
        blocks: Vec<Block>,
    },
    Table {
        table: Table,
        row: Vec<Inline>,
    },
}

#[derive(Default)]
struct Builder {
    root: Vec<Block>,
    stack: Vec<Frame>,
    inline: Option<InlineBuilder>,
    /// The open inline was started by bare text in a tight list item, not by a tag.
    implicit: bool,
    code: Option<(Option<String>, String)>,
    heading: u8,
}

impl Builder {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.code {
                Some((_, code)) => code.push_str(&text),
                None => self.inline().push(&text, None),
            },
            Event::Code(text) => self.inline().push(&text, Some(true)),
            Event::Html(text) | Event::InlineHtml(text) => self.inline().push(&text, None),
            Event::SoftBreak | Event::HardBreak => self.inline().push("\n", None),
            Event::Rule => {
                self.flush_implicit();
                self.block(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item { task, .. }) = self.stack.last_mut() {
                    *task = Some(checked);
                }
            }
            // Footnotes and math are not enabled; anything else new stays text.
            Event::FootnoteReference(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.inline().push(&text, None)
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.open_inline(),
            Tag::Heading { level, .. } => {
                self.heading = level as u8;
                self.open_inline();
            }
            Tag::BlockQuote(_) => {
                self.flush_implicit();
                self.stack.push(Frame::Quote(Vec::new()));
            }
            Tag::CodeBlock(kind) => {
                self.flush_implicit();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split([' ', ',', '\t'])
                        .next()
                        .filter(|lang| !lang.is_empty())
                        .map(str::to_owned),
                    CodeBlockKind::Indented => None,
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => self.open_inline(),
            Tag::List(start) => {
                self.flush_implicit();
                self.stack.push(Frame::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.flush_implicit();
                self.stack.push(Frame::Item {
                    task: None,
                    blocks: Vec::new(),
                });
            }
            Tag::Table(align) => {
                self.flush_implicit();
                let align = align
                    .into_iter()
                    .map(|align| match align {
                        Alignment::None | Alignment::Left => Align::Left,
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                    })
                    .collect();
                self.stack.push(Frame::Table {
                    table: Table {
                        align,
                        head: Vec::new(),
                        rows: Vec::new(),
                    },
                    row: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {}
            Tag::TableCell => self.open_inline(),
            Tag::Emphasis => self.inline().italic += 1,
            Tag::Strong => self.inline().bold += 1,
            Tag::Strikethrough => self.inline().strike += 1,
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                let inline = self.inline();
                inline.link = Some((inline.text.len(), dest_url.into_string()));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => {
                if let Some(inline) = self.inline.take() {
                    self.block(Block::Paragraph(inline.finish()));
                }
            }
            TagEnd::Heading(_) => {
                if let Some(inline) = self.inline.take() {
                    self.block(Block::Heading {
                        level: self.heading.clamp(1, 6),
                        text: inline.finish(),
                    });
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, mut text)) = self.code.take() {
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    self.block(Block::Code {
                        lang,
                        text: expand_tabs(&text).into(),
                    });
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_implicit();
                if let Some(Frame::Quote(blocks)) = self.stack.pop() {
                    self.block(Block::Quote(blocks));
                }
            }
            TagEnd::List(_) => {
                self.flush_implicit();
                if let Some(Frame::List { start, items }) = self.stack.pop() {
                    self.block(Block::List { start, items });
                }
            }
            TagEnd::Item => {
                self.flush_implicit();
                if let Some(Frame::Item { task, blocks }) = self.stack.pop()
                    && let Some(Frame::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(Item { task, blocks });
                }
            }
            TagEnd::TableCell => {
                let cell = self
                    .inline
                    .take()
                    .map(InlineBuilder::finish)
                    .unwrap_or_default();
                if let Some(Frame::Table { row, .. }) = self.stack.last_mut() {
                    row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(Frame::Table { table, row }) = self.stack.last_mut() {
                    table.head = std::mem::take(row);
                }
            }
            TagEnd::TableRow => {
                if let Some(Frame::Table { table, row }) = self.stack.last_mut() {
                    let mut row = std::mem::take(row);
                    row.resize(table.align.len(), Inline::default());
                    table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(Frame::Table { mut table, .. }) = self.stack.pop() {
                    table.head.resize(table.align.len(), Inline::default());
                    self.block(Block::Table(table));
                }
            }
            TagEnd::Emphasis => {
                let inline = self.inline();
                inline.italic = inline.italic.saturating_sub(1);
            }
            TagEnd::Strong => {
                let inline = self.inline();
                inline.bold = inline.bold.saturating_sub(1);
            }
            TagEnd::Strikethrough => {
                let inline = self.inline();
                inline.strike = inline.strike.saturating_sub(1);
            }
            TagEnd::Link | TagEnd::Image => {
                let inline = self.inline();
                if let Some((start, url)) = inline.link.take() {
                    if inline.text.len() == start {
                        // An image without alt text, or an empty link: show where it points.
                        inline.link = Some((start, String::new()));
                        inline.push(&url, None);
                        inline.link = None;
                    }
                    inline.links.push(Link {
                        range: start..inline.text.len(),
                        url,
                    });
                }
            }
            _ => {}
        }
    }

    fn open_inline(&mut self) {
        self.flush_implicit();
        self.inline = Some(InlineBuilder::default());
        self.implicit = false;
    }

    fn inline(&mut self) -> &mut InlineBuilder {
        if self.inline.is_none() {
            self.implicit = true;
        }
        self.inline.get_or_insert_with(InlineBuilder::default)
    }

    /// A tight list item's text arrives without a paragraph around it.
    fn flush_implicit(&mut self) {
        if self.implicit
            && let Some(inline) = self.inline.take()
        {
            self.implicit = false;
            self.block(Block::Paragraph(inline.finish()));
        }
    }

    fn block(&mut self, block: Block) {
        let target = self.stack.iter_mut().rev().find_map(|frame| match frame {
            Frame::Quote(blocks) | Frame::Item { blocks, .. } => Some(blocks),
            Frame::List { .. } | Frame::Table { .. } => None,
        });
        target.unwrap_or(&mut self.root).push(block);
    }

    fn finish(mut self) -> Vec<Block> {
        // The parser closes everything it opens, so this only matters if it stops early.
        if let Some(inline) = self.inline.take() {
            self.block(Block::Paragraph(inline.finish()));
        }
        while let Some(frame) = self.stack.pop() {
            match frame {
                Frame::Quote(blocks) => self.block(Block::Quote(blocks)),
                Frame::List { start, items } => self.block(Block::List { start, items }),
                Frame::Item { task, blocks } => {
                    if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                        items.push(Item { task, blocks });
                    }
                }
                Frame::Table { table, .. } => self.block(Block::Table(table)),
            }
        }
        self.root
    }
}

#[derive(Default)]
struct InlineBuilder {
    text: String,
    spans: Vec<Span>,
    links: Vec<Link>,
    bold: u32,
    italic: u32,
    strike: u32,
    link: Option<(usize, String)>,
}

impl InlineBuilder {
    fn push(&mut self, text: &str, code: Option<bool>) {
        if text.is_empty() {
            return;
        }
        let style = InlineStyle {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: code.unwrap_or(false),
            link: self.link.is_some(),
        };
        let start = self.text.len();
        self.text.push_str(text);
        if style == InlineStyle::default() {
            return;
        }
        match self.spans.last_mut() {
            Some(last) if last.range.end == start && last.style == style => {
                last.range.end = self.text.len();
            }
            _ => self.spans.push(Span {
                range: start..self.text.len(),
                style,
            }),
        }
    }

    fn finish(mut self) -> Inline {
        // Models write bare addresses far more often than autolinks. Found on the
        // finished text, because the parser splits text at every `_` and `*`.
        for range in bare_urls(&self.text) {
            let taken = self
                .links
                .iter()
                .any(|link| link.range.start < range.end && range.start < link.range.end)
                || self.spans.iter().any(|span| {
                    span.style.code && span.range.start < range.end && range.start < span.range.end
                });
            if taken {
                continue;
            }
            self.spans = restyle(std::mem::take(&mut self.spans), range.clone(), |style| {
                style.link = true
            });
            self.links.push(Link {
                url: self.text[range.clone()].to_owned(),
                range,
            });
        }
        self.links.sort_by_key(|link| link.range.start);
        Inline {
            text: self.text.into(),
            spans: self.spans,
            links: self.links,
        }
    }
}

/// Tabs as four spaces: the text system has no tab stops, and code is full of tabs.
pub(crate) fn expand_tabs(text: &str) -> String {
    text.replace('\t', "    ")
}

/// Apply `change` to the style of `range`, splitting and merging spans as needed.
fn restyle(spans: Vec<Span>, range: Range<usize>, change: impl Fn(&mut InlineStyle)) -> Vec<Span> {
    let mut pieces: Vec<Span> = Vec::with_capacity(spans.len() + 2);
    let mut cursor = range.start;
    let mut inserted = false;
    let fill = |pieces: &mut Vec<Span>, from: usize, to: usize| {
        if from < to {
            let mut style = InlineStyle::default();
            change(&mut style);
            pieces.push(Span {
                range: from..to,
                style,
            });
        }
    };
    for span in spans {
        if span.range.end <= range.start {
            pieces.push(span);
            continue;
        }
        if span.range.start >= range.end {
            if !inserted {
                fill(&mut pieces, cursor, range.end);
                inserted = true;
            }
            pieces.push(span);
            continue;
        }
        // The span overlaps the range: up to three pieces.
        fill(&mut pieces, cursor, span.range.start.max(cursor));
        if span.range.start < range.start {
            pieces.push(Span {
                range: span.range.start..range.start,
                style: span.style,
            });
        }
        let inside = span.range.start.max(range.start)..span.range.end.min(range.end);
        let mut style = span.style;
        change(&mut style);
        pieces.push(Span {
            range: inside.clone(),
            style,
        });
        cursor = inside.end;
        if span.range.end > range.end {
            pieces.push(Span {
                range: range.end..span.range.end,
                style: span.style,
            });
        }
    }
    if !inserted {
        fill(&mut pieces, cursor, range.end);
    }
    let mut merged: Vec<Span> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        match merged.last_mut() {
            Some(last) if last.range.end == piece.range.start && last.style == piece.style => {
                last.range.end = piece.range.end;
            }
            _ => merged.push(piece),
        }
    }
    merged
}

/// Ranges of `http://` and `https://` addresses written as plain text.
fn bare_urls(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("http") {
        let start = from + at;
        let rest = &text[start..];
        let scheme = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            from = start + 4;
            continue;
        };
        let glued = text[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        let mut end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
            .unwrap_or(rest.len());
        // Sentence punctuation after an address is not part of it; a closing
        // bracket is, but only if the address opened it.
        loop {
            let candidate = &rest[..end];
            let Some(last) = candidate.chars().next_back() else {
                break;
            };
            let unbalanced = |open: char, close: char| {
                candidate.matches(close).count() > candidate.matches(open).count()
            };
            let trim = matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '_')
                || (last == ')' && unbalanced('(', ')'))
                || (last == ']' && unbalanced('[', ']'));
            if !trim {
                break;
            }
            end -= last.len_utf8();
        }
        if !glued && end > scheme {
            found.push(start..start + end);
        }
        from = start + end.max(scheme);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph(text: &str) -> Block {
        Block::Paragraph(Inline::plain(text))
    }

    fn inline_of(block: &Block) -> &Inline {
        match block {
            Block::Paragraph(inline) | Block::Heading { text: inline, .. } => inline,
            other => panic!("no inline in {other:?}"),
        }
    }

    fn styled(inline: &Inline) -> Vec<(&str, InlineStyle)> {
        inline
            .spans
            .iter()
            .map(|span| (&inline.text[span.range.clone()], span.style))
            .collect()
    }

    #[test]
    fn paragraphs_and_headings() {
        let blocks = parse("# Title\n\nFirst.\n\nSecond line\nsame paragraph.\n\n### Deep");
        assert_eq!(
            blocks,
            [
                Block::Heading {
                    level: 1,
                    text: Inline::plain("Title")
                },
                paragraph("First."),
                // A model's line break is meant; it stays a line break.
                paragraph("Second line\nsame paragraph."),
                Block::Heading {
                    level: 3,
                    text: Inline::plain("Deep")
                },
            ]
        );
    }

    #[test]
    fn emphasis_code_and_strikethrough_become_spans() {
        let blocks = parse("plain **bold** *it* ***both*** `code` ~~gone~~ end");
        let inline = inline_of(&blocks[0]);
        assert_eq!(inline.text, "plain bold it both code gone end");
        let bold = InlineStyle {
            bold: true,
            ..InlineStyle::default()
        };
        let italic = InlineStyle {
            italic: true,
            ..InlineStyle::default()
        };
        assert_eq!(
            styled(inline),
            [
                ("bold", bold),
                ("it", italic),
                (
                    "both",
                    InlineStyle {
                        bold: true,
                        italic: true,
                        ..InlineStyle::default()
                    }
                ),
                (
                    "code",
                    InlineStyle {
                        code: true,
                        ..InlineStyle::default()
                    }
                ),
                (
                    "gone",
                    InlineStyle {
                        strike: true,
                        ..InlineStyle::default()
                    }
                ),
            ]
        );
    }

    #[test]
    fn links_keep_their_target_and_bare_addresses_become_links() {
        let blocks = parse(
            "See [the docs](https://example.com/docs) and https://example.com/a_b_c. \
             Not `https://in.code` though (https://example.com/x).",
        );
        let inline = inline_of(&blocks[0]);
        let links: Vec<(&str, &str)> = inline
            .links
            .iter()
            .map(|link| (&inline.text[link.range.clone()], link.url.as_str()))
            .collect();
        assert_eq!(
            links,
            [
                ("the docs", "https://example.com/docs"),
                ("https://example.com/a_b_c", "https://example.com/a_b_c"),
                ("https://example.com/x", "https://example.com/x"),
            ]
        );
        for link in &inline.links {
            let covered = inline
                .spans
                .iter()
                .filter(|span| span.style.link)
                .any(|span| {
                    span.range.start <= link.range.start && link.range.end <= span.range.end
                });
            assert!(covered, "{link:?} has no link style");
        }
        // Spans stay sorted and disjoint after the bare links are cut in.
        for pair in inline.spans.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "{:?}",
                inline.spans
            );
        }
    }

    #[test]
    fn a_bare_address_inside_bold_text_keeps_both_styles() {
        let blocks = parse("**see https://example.com now**");
        let inline = inline_of(&blocks[0]);
        assert_eq!(
            styled(inline),
            [
                (
                    "see ",
                    InlineStyle {
                        bold: true,
                        ..InlineStyle::default()
                    }
                ),
                (
                    "https://example.com",
                    InlineStyle {
                        bold: true,
                        link: true,
                        ..InlineStyle::default()
                    }
                ),
                (
                    " now",
                    InlineStyle {
                        bold: true,
                        ..InlineStyle::default()
                    }
                ),
            ]
        );
    }

    #[test]
    fn fenced_and_indented_code() {
        let blocks = parse(
            "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n    indented\n    \ttabbed\n",
        );
        assert_eq!(
            blocks,
            [
                Block::Code {
                    lang: Some("rust".into()),
                    text: "fn main() {\n    println!(\"hi\");\n}".into()
                },
                Block::Code {
                    lang: None,
                    text: "indented\n    tabbed".into()
                },
            ]
        );
    }

    #[test]
    fn an_unterminated_fence_mid_stream_is_already_a_code_block() {
        let blocks = parse("Here:\n\n```sh\ncargo test -p kou\ncargo cl");
        assert_eq!(
            blocks,
            [
                paragraph("Here:"),
                Block::Code {
                    lang: Some("sh".into()),
                    text: "cargo test -p kou\ncargo cl".into()
                },
            ]
        );
        // The fence alone, before any code arrived.
        assert_eq!(
            parse("```"),
            [Block::Code {
                lang: None,
                text: SharedString::default()
            }]
        );
    }

    #[test]
    fn every_prefix_of_a_document_parses() {
        let source = "# T\n\n- a\n  - b\n\n> q\n\n| a | b |\n|---|:-:|\n| 1 | 2 |\n\n```\nx\n```\n\n[l](https://e.x) **b";
        for end in 0..=source.len() {
            if source.is_char_boundary(end) {
                parse(&source[..end]);
            }
        }
    }

    #[test]
    fn unordered_ordered_and_nested_lists() {
        let blocks =
            parse("- one\n- two\n  1. inner a\n  2. inner b\n- three\n\n3. third\n4. fourth");
        let Block::List { start: None, items } = &blocks[0] else {
            panic!("an unordered list: {blocks:?}");
        };
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].blocks, [paragraph("one")]);
        assert_eq!(items[1].blocks[0], paragraph("two"));
        let Block::List {
            start: Some(1),
            items: inner,
        } = &items[1].blocks[1]
        else {
            panic!("a nested ordered list: {:?}", items[1]);
        };
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[1].blocks, [paragraph("inner b")]);
        let Block::List {
            start: Some(3),
            items,
        } = &blocks[1]
        else {
            panic!("an ordered list from 3: {blocks:?}");
        };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn loose_items_and_task_items() {
        let blocks = parse("- [x] done\n- [ ] open\n\n1. first\n\n   more\n\n2. second");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("a list");
        };
        assert_eq!(
            items.iter().map(|item| item.task).collect::<Vec<_>>(),
            [Some(true), Some(false)]
        );
        assert_eq!(items[0].blocks, [paragraph("done")]);
        let Block::List { items, .. } = &blocks[1] else {
            panic!("a list");
        };
        assert_eq!(items[0].blocks, [paragraph("first"), paragraph("more")]);
    }

    #[test]
    fn block_quotes_nest_blocks() {
        let blocks = parse("> quoted **text**\n>\n> - item\n\nafter");
        let Block::Quote(inner) = &blocks[0] else {
            panic!("a quote: {blocks:?}");
        };
        assert_eq!(inline_of(&inner[0]).text, "quoted text");
        assert!(matches!(inner[1], Block::List { .. }));
        assert_eq!(blocks[1], paragraph("after"));
    }

    #[test]
    fn tables_keep_alignment_and_pad_short_rows() {
        let blocks = parse("| name | n | note |\n|:-----|--:|:----:|\n| a | 1 | `x` |\n| b | 2 |");
        let Block::Table(table) = &blocks[0] else {
            panic!("a table: {blocks:?}");
        };
        assert_eq!(table.align, [Align::Left, Align::Right, Align::Center]);
        assert_eq!(
            table
                .head
                .iter()
                .map(|cell| &*cell.text)
                .collect::<Vec<_>>(),
            ["name", "n", "note"]
        );
        assert_eq!(table.rows.len(), 2);
        assert!(table.rows[0][2].spans[0].style.code);
        assert_eq!(table.rows[1].len(), 3);
        assert_eq!(table.rows[1][2].text, "");
    }

    #[test]
    fn rules_and_raw_html() {
        let blocks = parse("above\n\n---\n\n<details>raw</details>\n\nbelow <br> inline");
        assert_eq!(blocks[0], paragraph("above"));
        assert_eq!(blocks[1], Block::Rule);
        assert_eq!(
            inline_of(&blocks[2]).text.trim_end(),
            "<details>raw</details>"
        );
        assert_eq!(inline_of(&blocks[3]).text, "below <br> inline");
    }

    #[test]
    fn the_first_line_skips_blank_lines_and_looks_into_containers() {
        assert_eq!(first_line(&parse("\n\n**Plan** it\nmore")), Some("Plan it"));
        assert_eq!(first_line(&parse("> - nested")), Some("nested"));
        assert_eq!(first_line(&parse("---")), None);
    }

    #[test]
    fn bare_urls_stop_at_sentence_punctuation() {
        let text = "a https://x.y/z, b (http://q.w/(r)) c https://end.";
        let found: Vec<&str> = bare_urls(text)
            .into_iter()
            .map(|range| &text[range])
            .collect();
        assert_eq!(found, ["https://x.y/z", "http://q.w/(r)", "https://end"]);
        assert!(bare_urls("xhttps://glued.to/word http:// https").is_empty());
    }
}
