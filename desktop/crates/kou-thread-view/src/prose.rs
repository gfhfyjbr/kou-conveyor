//! Markdown blocks as elements: text first, in the interface font, with code
//! in the monospace and nothing that needs a web view.

use crate::markdown::{Align, Block, Inline, InlineStyle, Item, Table};
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ClickEvent, Div, Element, ElementId, FocusHandle,
    FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, InteractiveElement as _,
    IntoElement, LayoutId, ParentElement as _, Pixels, SharedString, Stateful,
    StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledText, TextLayout,
    TextRun, UnderlineStyle, Window, div, font, point, prelude::FluentBuilder as _, rgb, size,
};
use kou_ui::{
    Icon, Mark, Theme, icon, metric,
    scale::{self, ScaledText, ScaledTextLayout, exact, px},
    status_mark,
};
use std::{cell::Cell, ops::Range, rc::Rc};

/// Size and line pitch of running text: the web cockpit's prose
/// (`markdown.css`, `.prose`: 14.5px, 1.68).
pub(crate) const PROSE: f32 = 14.5;
pub(crate) const LINE: f32 = 24.;
/// Size and line pitch of a tool call's output and of reasoning opened: a
/// step under the answer.
pub(crate) const SMALL: f32 = 13.5;
pub(crate) const SMALL_LINE: f32 = 22.;
/// Size and line pitch of monospace blocks (`.code pre`: 12.5px, 1.6).
pub(crate) const MONO: f32 = 12.5;
pub(crate) const MONO_LINE: f32 = 20.;
/// Space between the blocks of a document (`.prose > * + *`: 0.8em).
const GAP: f32 = 12.;
/// A table cell wraps at this width; wider tables scroll sideways.
const CELL_MAX: f32 = 340.;

/// The three text colours a document is drawn in.
#[derive(Clone, Copy)]
pub(crate) struct Tone {
    /// Running text.
    pub text: Hsla,
    /// Headings and bold text.
    pub strong: Hsla,
}

impl Tone {
    /// The assistant's answer.
    pub(crate) fn answer(theme: &Theme) -> Self {
        Self {
            text: theme.fg,
            strong: theme.fg,
        }
    }

    /// Output of a tool: a step back from the answer.
    pub(crate) fn output(theme: &Theme) -> Self {
        Self {
            text: theme.fg_dim,
            strong: theme.fg,
        }
    }

    /// Reasoning opened (`.prose.thought`): a step back from the answer.
    pub(crate) fn reasoning(theme: &Theme) -> Self {
        Self {
            text: theme.fg_dim,
            strong: theme.fg,
        }
    }
}

/// Copies text: a key for the check mark the copy leaves, and the text.
pub(crate) type CopyFn = Rc<dyn Fn(SharedString, String, &mut Window, &mut App)>;

/// Opens what a mention names, by its id.
pub(crate) type OpenFn = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Something the prose names that the view can open: an agent this one
/// started, by its title. Where the text says its title, the title is a link
/// in the accent with the agent's small mark before it.
#[derive(Clone)]
pub(crate) struct Mention {
    pub id: String,
    pub title: String,
    /// Its harness's mark, or else its letters.
    pub brand: Option<kou_ui::Brand>,
    pub letters: String,
    /// The mark's colour.
    pub color: Hsla,
}

/// The room a mention's mark takes in the text: an em space and a thin one.
const MARK_ROOM: &str = "\u{2003}\u{2009}";

/// What drawing a document needs besides the document.
pub(crate) struct Look<'a> {
    pub theme: &'a Theme,
    pub tone: Tone,
    /// The prompt field's focus: where the keyboard rests after a pointer click on a link.
    pub field: &'a FocusHandle,
    /// Numbers the stateful elements of one document, so their ids are unique within it.
    next: Cell<usize>,
    /// Copies a code block, when the document offers it; with the entry the
    /// document belongs to, and what was copied last.
    copy: Option<(usize, Option<SharedString>, CopyFn)>,
    codes: Cell<usize>,
    /// The text size and line pitch of running text.
    size: (f32, f32),
    /// How opaque the pieces of the text that came last are, as ranges of
    /// the text as drawn (see [`drawn_len`]): they fade in as they come.
    fades: Vec<(Range<usize>, f32)>,
    /// How much of the text as drawn the blocks drawn so far hold.
    drawn: Cell<usize>,
    /// The caret that ends text still being written, and how lit it is.
    caret: Option<f32>,
    /// How long the whole document's text is as drawn: where it ends.
    total: Cell<usize>,
    /// The caret goes at the end of the last running text, not on a line of its own.
    inline_caret: Cell<bool>,
    /// Drawn in the quieter look (`Look::Studio`): its sizes and gaps, round
    /// bullets, links without a line under them, headings in the text's face.
    studio: bool,
    /// What the text may name that opens, and how it is opened.
    mentions: Option<(Rc<[Mention]>, OpenFn)>,
    /// Code stands in what frames the document already (a tool call's card):
    /// its lines, without a frame and a caption of their own.
    bare: bool,
}

impl<'a> Look<'a> {
    pub(crate) fn new(theme: &'a Theme, tone: Tone, field: &'a FocusHandle) -> Self {
        Self {
            theme,
            tone,
            field,
            next: Cell::new(0),
            copy: None,
            codes: Cell::new(0),
            size: (PROSE, LINE),
            fades: Vec::new(),
            drawn: Cell::new(0),
            caret: None,
            total: Cell::new(0),
            inline_caret: Cell::new(false),
            studio: false,
            mentions: None,
            bare: false,
        }
    }

    /// Where the text names one of `mentions`, the name is a link that opens
    /// it with `open`, its mark before it.
    pub(crate) fn with_mentions(mut self, mentions: Rc<[Mention]>, open: OpenFn) -> Self {
        if !mentions.is_empty() {
            self.mentions = Some((mentions, open));
        }
        self
    }

    /// Inside a frame already: a code block is its lines alone.
    pub(crate) fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    /// The quieter look's prose: its size and line pitch, gaps and marks.
    pub(crate) fn studio(mut self) -> Self {
        self.studio = true;
        self.size = (crate::studio::PROSE, crate::studio::LINE);
        self
    }

    /// The gap between blocks, and between the items of a list.
    fn gaps(&self) -> (f32, f32) {
        if self.studio {
            (crate::studio::GAP, crate::studio::ITEM_GAP)
        } else {
            (GAP, 5.)
        }
    }

    /// Where the document being drawn ends, as drawn.
    fn end(&self) -> usize {
        self.total.get()
    }

    /// Running text a step smaller: a tool's output, reasoning.
    pub(crate) fn small(mut self) -> Self {
        self.size = if self.studio {
            (crate::studio::SMALL, crate::studio::SMALL_LINE)
        } else {
            (SMALL, SMALL_LINE)
        };
        self
    }

    /// The pieces of the text that came last fade in: how opaque each is.
    pub(crate) fn with_fades(mut self, fades: Vec<(Range<usize>, f32)>) -> Self {
        self.fades = fades;
        self
    }

    /// The text is still being written: a caret ends it, this lit.
    pub(crate) fn with_caret(mut self, lit: Option<f32>) -> Self {
        self.caret = lit;
        self
    }

    /// Runs of `text`, drawn next, with the fading pieces among them made
    /// as opaque as they are, and the count of what is drawn moved on.
    fn faded(&self, runs: Vec<TextRun>, text: &str) -> Vec<TextRun> {
        let len = text.len();
        let start = self.drawn.get();
        self.drawn.set(start + len);
        if self.fades.is_empty() {
            return runs;
        }
        // Cut the runs where a piece starts or ends, then fade what is in one.
        let cuts = fade_cuts(text, start, &self.fades);
        let mut out = Vec::with_capacity(runs.len() + cuts.len());
        let mut at = 0;
        for run in runs {
            let run_end = at + run.len;
            let mut from = at;
            for cut in cuts
                .iter()
                .copied()
                .filter(|cut| *cut > at && *cut < run_end)
            {
                out.push(TextRun {
                    len: cut - from,
                    ..run.clone()
                });
                from = cut;
            }
            out.push(TextRun {
                len: run_end - from,
                ..run
            });
            at = run_end;
        }
        let mut at = start;
        for run in &mut out {
            let middle = at + run.len / 2;
            if let Some((_, alpha)) = self.fades.iter().find(|(range, _)| range.contains(&middle)) {
                run.color = run.color.opacity(run.color.a * alpha);
                if let Some(background) = &mut run.background_color {
                    *background = background.opacity(background.a * alpha);
                }
            }
            at += run.len;
        }
        out
    }

    /// Code blocks get a control that copies them.
    pub(crate) fn with_copy(
        mut self,
        entry: usize,
        copied: Option<SharedString>,
        copy: CopyFn,
    ) -> Self {
        self.copy = Some((entry, copied, copy));
        self
    }

    fn id(&self, name: &'static str) -> ElementId {
        let id = self.next.get();
        self.next.set(id + 1);
        (name, id).into()
    }
}

/// A document: its blocks stacked with an even gap.
pub(crate) fn document(blocks: &[Block], look: &Look) -> Div {
    look.drawn.set(0);
    look.total.set(drawn_len(blocks));
    let (size, line) = look.size;
    let ends_in_text = matches!(
        blocks.last(),
        Some(Block::Paragraph(_) | Block::Heading { .. })
    );
    look.inline_caret.set(look.caret.is_some() && ends_in_text);
    let caret_alone = look.caret.is_some() && !ends_in_text;
    div()
        .w_full()
        .min_w_0()
        .text_size(px(size))
        .line_height(px(line))
        .text_color(look.tone.text)
        .child(stack(blocks, look, look.gaps().0))
        // After a block that is not running text, the caret has a line of its own.
        .children(caret_alone.then(|| {
            let lit = look.caret.unwrap_or(1.);
            div().h(px(line)).flex().items_center().child(
                div()
                    .w(px(size * 0.5))
                    .h(px(size * 1.05))
                    .bg(look.theme.accent.opacity(lit)),
            )
        }))
}

/// How long a document's text is as drawn: what the fading of streamed
/// text counts in. Its blocks' texts in the order [`document`] draws them.
pub(crate) fn drawn_len(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(inline) | Block::Heading { text: inline, .. } => inline.text.len(),
            Block::Code { text, .. } => text.len(),
            Block::Quote(blocks) => drawn_len(blocks),
            Block::List { items, .. } => items.iter().map(|item| drawn_len(&item.blocks)).sum(),
            Block::Table(table) => table
                .head
                .iter()
                .chain(table.rows.iter().flatten())
                .map(|cell| cell.text.len())
                .sum(),
            Block::Rule => 0,
        })
        .sum()
}

/// Blocks one under the other, `gap` apart: at a zoom, each where it stands
/// at 1 times the zoom (see `kou_ui::scale::exact`).
fn stack(blocks: &[Block], look: &Look, gap: f32) -> AnyElement {
    let count = blocks.len();
    exact::column(
        div().w_full().min_w_0().flex().flex_col().gap(px(gap)),
        blocks
            .iter()
            .enumerate()
            .map(|(index, block)| self::block(block, look, index == 0, index + 1 == count)),
    )
}

/// One block. `last` is the document's last block at its own level, which
/// running text ends with the caret while it is written.
fn block(block: &Block, look: &Look, first: bool, last: bool) -> AnyElement {
    let theme = look.theme;
    let caret = |inline: &Inline| {
        // Only the document's own last paragraph: a nested one is not its end.
        look.inline_caret.get() && last && look.drawn.get() + inline.text.len() == look.end()
    };
    match block {
        Block::Paragraph(text) => {
            let caret = caret(text);
            inline(
                text,
                look.tone.text,
                FontWeight::NORMAL,
                Align::Left,
                caret,
                look,
            )
        }
        Block::Heading { level, text } => {
            // `.prose h3`, `h4`: 17px and 15px; the rest as `h5`, a label.
            // The quieter look has no labels in capitals: its smallest
            // heading is the text's size, in its stronger weight.
            let (size, line, label) = match (look.studio, level) {
                (true, 1) => (look.size.0 + 2.5, look.size.1 + 2., false),
                (true, _) => (look.size.0, look.size.1, false),
                (false, 1) => (17., 22., false),
                (false, 2) => (15., 20., false),
                (false, _) => (LABEL, 18., true),
            };
            let caret = caret(text);
            let shown = if label { capitals(text) } else { text.clone() };
            div()
                .text_size(px(size))
                .line_height(px(line))
                .when(label, |heading| {
                    heading
                        .font_family(theme.mono_font.clone())
                        .text_color(theme.fg_dim)
                })
                // A heading belongs to what follows it: more air above than below.
                .when(!first, |heading| heading.mt(px(8.)))
                .child(inline(
                    &shown,
                    if label {
                        theme.fg_dim
                    } else {
                        look.tone.strong
                    },
                    if label {
                        FontWeight::BOLD
                    } else {
                        FontWeight::SEMIBOLD
                    },
                    Align::Left,
                    caret,
                    look,
                ))
                .into_any_element()
        }
        Block::Code { lang, text } => code_block(lang.as_deref(), text, look).into_any_element(),
        Block::Quote(blocks) => div()
            .pl(px(14.))
            .border_l_2()
            .border_color(theme.line_strong)
            .text_color(theme.fg_dim)
            .child(stack(blocks, look, look.gaps().0))
            .into_any_element(),
        Block::List { start, items } => list(*start, items, look),
        Block::Table(table) => self::table(table, look).into_any_element(),
        Block::Rule => div().my(px(4.)).h(px(1.)).bg(theme.line).into_any_element(),
    }
}

/// The size of the monospace labels: `.label`, `.meta`, `.prose h5`.
const LABEL: f32 = 10.5;

/// A fenced block of a document, as the web cockpit's `.code` figure: a
/// caption with the language and a control that copies it, over the code
/// coloured by its language.
fn code_block(lang: Option<&str>, text: &SharedString, look: &Look) -> AnyElement {
    let theme = look.theme;
    let id = look.id("code");
    let spans = crate::highlight::highlight(lang, text);
    let mono = font(theme.mono_font.clone());
    let color = |token: crate::highlight::Token| -> Hsla {
        use crate::highlight::Token;
        let ansi = |at: usize| -> Hsla { rgb(theme.ansi.0[at]).into() };
        match token {
            Token::Keyword => ansi(5),
            Token::String => ansi(2),
            Token::Comment => theme.fg_muted,
            Token::Number => ansi(3),
            Token::Type => ansi(6),
            Token::Call => ansi(4),
            Token::Added => theme.ok,
            Token::Removed => theme.error,
            Token::Meta => ansi(6),
        }
    };
    let run = |len: usize, color: Hsla, italic: bool| {
        let mut font = mono.clone();
        if italic {
            font.style = FontStyle::Italic;
        }
        TextRun {
            len,
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    };
    let mut runs = Vec::with_capacity(spans.len() * 2 + 1);
    let mut at = 0;
    for (range, token) in spans {
        if range.start > at {
            runs.push(run(range.start - at, theme.fg_dim, false));
        }
        runs.push(run(
            range.len(),
            color(token),
            token == crate::highlight::Token::Comment,
        ));
        at = range.end;
    }
    if at < text.len() || runs.is_empty() {
        runs.push(run(text.len() - at, theme.fg_dim, false));
    }
    let runs = look.faded(runs, text);
    let body = scale::styled_text(text.clone(), runs);
    let code = div()
        .id(id)
        .w_full()
        .flex()
        .overflow_x_scroll()
        // A vertical wheel over the block still scrolls the transcript.
        .restrict_scroll_to_axis()
        .child(
            div()
                .flex_none()
                .when(!look.bare, |code| code.px(px(14.)).py(px(12.)))
                .text_size(px(MONO))
                .line_height(px(MONO_LINE))
                .whitespace_nowrap()
                .child(body),
        );
    if look.bare {
        return code.into_any_element();
    }
    let label: SharedString = lang
        .filter(|lang| !lang.is_empty())
        .unwrap_or("code")
        .into();
    let label: SharedString = if look.studio {
        label
    } else {
        label.to_uppercase().into()
    };
    let copy = look.copy.as_ref().map(|(entry, copied, copy)| {
        let number = look.codes.get();
        look.codes.set(number + 1);
        let key = SharedString::from(format!("code-{entry}-{number}"));
        let done = copied.as_ref() == Some(&key);
        let (copy, source, field) = (copy.clone(), text.to_string(), look.field.clone());
        let word = if done { "Copied" } else { "Copy" };
        let control = if look.studio {
            quiet_word(("copy-code", number), word, theme)
        } else {
            act(("copy-code", number), word, theme)
        };
        control
            .debug_selector(move || format!("copy-code-{number}"))
            .when(done, |control| control.text_color(theme.ok))
            .on_click(move |click, window, cx| {
                copy(key.clone(), source.clone(), window, cx);
                if !matches!(click, ClickEvent::Keyboard(_)) {
                    window.focus(&field, cx);
                }
            })
    });
    let caption = div()
        .h(px(28.))
        .pl(px(12.))
        .pr(px(4.))
        .flex()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(theme.line)
        .text_color(theme.fg_muted);
    let caption = if look.studio {
        caption.text_size(px(11.))
    } else {
        caption
            .font_family(theme.mono_font.clone())
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(10.))
    };
    div()
        .w_full()
        .rounded(px(metric::RADIUS_MEDIUM))
        .border_1()
        .border_color(theme.line)
        .bg(theme.code)
        .overflow_hidden()
        .child(caption.child(scale::text(label)).children(copy))
        .child(code)
        .into_any_element()
}

/// A small control in the quieter look: a muted word of the text's face,
/// brighter under the pointer, framed under the keyboard.
fn quiet_word(id: impl Into<ElementId>, label: &'static str, theme: &Theme) -> Stateful<Div> {
    let (ring, lit) = (theme.accent, theme.fg);
    div()
        .id(id)
        .flex_none()
        .h(px(22.))
        .px(px(6.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(gpui::transparent_black())
        .flex()
        .items_center()
        .cursor_pointer()
        .tab_index(0)
        .text_size(px(11.))
        .text_color(theme.fg_muted)
        .hover(move |style| style.text_color(lit))
        .focus_visible(move |style| style.border_color(ring).text_color(lit))
        .child(scale::text(label))
}

/// A small control in the web cockpit's `.act` style: a word in capitals of
/// the monospace, framed only under the pointer or the keyboard.
pub(crate) fn act(id: impl Into<ElementId>, label: &'static str, theme: &Theme) -> Stateful<Div> {
    act_shown(id, label, None, theme)
}

/// [`act`], hidden until the pointer is over `group` or over the control, or
/// the keyboard is on it (`.actions`).
pub(crate) fn act_revealed(
    id: impl Into<ElementId>,
    label: &'static str,
    group: SharedString,
    theme: &Theme,
) -> Stateful<Div> {
    act_shown(id, label, Some(group), theme)
}

fn act_shown(
    id: impl Into<ElementId>,
    label: &'static str,
    group: Option<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    let (ring, frame, fill, lit) = (theme.accent, theme.line_strong, theme.pane, theme.fg);
    let hidden = group.is_some();
    let control = div()
        .id(id)
        .flex_none()
        .h(px(22.))
        .px(px(7.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(gpui::transparent_black())
        .flex()
        .items_center()
        .cursor_pointer()
        .tab_index(0)
        .font_family(theme.mono_font.clone())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(10.))
        .text_color(theme.fg_muted)
        .hover(move |style| {
            let style = style.border_color(frame).bg(fill).text_color(lit);
            if hidden { style.opacity(1.) } else { style }
        })
        .focus_visible(move |style| {
            let style = style.border_color(ring).text_color(lit);
            if hidden { style.opacity(1.) } else { style }
        })
        .child(scale::text(label.to_uppercase()));
    match group {
        Some(group) => control
            .opacity(0.)
            .group_hover(group, |style| style.opacity(1.)),
        None => control,
    }
}

fn list(start: Option<u64>, items: &[Item], look: &Look) -> AnyElement {
    if look.studio {
        return studio_list(start, items, look);
    }
    let theme = look.theme;
    let last = start.unwrap_or(0) + items.len().saturating_sub(1) as u64;
    let number_width = 7.5 * last.to_string().len() as f32 + 6.;
    let items = items.iter().enumerate().map(|(index, item)| {
        let marker = match (item.task, start) {
            (Some(done), _) => div()
                .w(px(14.))
                .h(px(look.size.1))
                .flex()
                .items_center()
                .child(check(done, theme)),
            // A number never wraps: at a zoom its box can come out a
            // pixel narrower than the number, which would then take two
            // lines and move everything after it by a line.
            // `ol > li::marker`: the number in the monospace, a step smaller.
            (None, Some(start)) => div()
                .w(px(number_width))
                .h(px(look.size.1))
                .flex()
                .items_center()
                .justify_end()
                .whitespace_nowrap()
                .font_family(theme.mono_font.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(look.size.0 * 0.85))
                .text_color(theme.fg_muted)
                .child(scale::text(format!("{}.", start + index as u64))),
            // `ul > li::marker`: a small square.
            (None, None) => div()
                .w(px(10.))
                .h(px(look.size.1))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(px(5.))
                        .rounded(px(metric::RADIUS_MARK))
                        .bg(theme.fg_faint),
                ),
        };
        div()
            .flex()
            .items_start()
            .gap(px(8.))
            .child(marker.flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(stack(&item.blocks, look, 4.)),
            )
            .into_any_element()
    });
    exact::column(div().w_full().flex().flex_col().gap(px(5.)), items)
}

/// A list in the quieter look: a round bullet, or the number, in a column
/// as wide as [`crate::studio::INDENT`], and the items close together.
fn studio_list(start: Option<u64>, items: &[Item], look: &Look) -> AnyElement {
    use crate::studio::{BULLET, BULLET_AT, INDENT};
    let theme = look.theme;
    let line = look.size.1;
    let items = items.iter().enumerate().map(|(index, item)| {
        let marker = match (item.task, start) {
            (Some(done), _) => div()
                .w(px(INDENT))
                .h(px(line))
                .flex()
                .items_center()
                .child(check(done, theme)),
            (None, Some(start)) => div()
                .w(px(INDENT))
                .h(px(line))
                .pr(px(5.))
                .flex()
                .items_center()
                .justify_end()
                .whitespace_nowrap()
                .text_color(theme.fg_dim)
                .child(scale::text(format!("{}.", start + index as u64))),
            (None, None) => div().w(px(INDENT)).h(px(line)).relative().child(
                div()
                    .absolute()
                    .left(px(BULLET_AT - BULLET / 2.))
                    .top(px((line - BULLET) / 2.))
                    .size(px(BULLET))
                    .rounded_full()
                    .bg(look.tone.text),
            ),
        };
        div()
            .flex()
            .items_start()
            .child(marker.flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(stack(&item.blocks, look, look.gaps().1)),
            )
            .into_any_element()
    });
    exact::column(
        div().w_full().flex().flex_col().gap(px(look.gaps().1)),
        items,
    )
}

/// The mark of a step that is done or not: a ticked circle or an open ring.
pub(crate) fn check(done: bool, theme: &Theme) -> AnyElement {
    if done {
        icon(Icon::CheckCircle, theme.ok)
            .size(px(12.))
            .into_any_element()
    } else {
        div()
            .size(px(12.))
            .flex()
            .items_center()
            .justify_center()
            .child(status_mark(Mark::Idle, theme))
            .into_any_element()
    }
}

fn table(table: &Table, look: &Look) -> Stateful<Div> {
    let theme = look.theme;
    let columns = table.align.len().max(1);
    // `.table th`: a label in capitals of the monospace, on the card's colour.
    let cell = |text: &Inline, column: usize, head: bool, last_row: bool| {
        let (color, weight) = if head {
            (theme.fg_muted, FontWeight::BOLD)
        } else {
            (look.tone.text, FontWeight::NORMAL)
        };
        div()
            .max_w(px(CELL_MAX))
            .px(px(12.))
            .py(px(7.))
            .when(!last_row, |cell| cell.border_b_1().border_color(theme.line))
            .when(head && !look.studio, |cell| {
                cell.bg(theme.raised)
                    .font_family(theme.mono_font.clone())
                    .text_size(px(10.))
            })
            .when(head && look.studio, |cell| cell.bg(theme.raised))
            .map(|cell| match table.align.get(column) {
                Some(Align::Right) => cell.text_right(),
                Some(Align::Center) => cell.text_center(),
                _ => cell,
            })
            .child(inline(
                text,
                color,
                weight,
                table.align.get(column).copied().unwrap_or(Align::Left),
                false,
                look,
            ))
    };
    let rows = table.rows.len();
    let head: Vec<Inline> = if look.studio {
        table.head.clone()
    } else {
        table.head.iter().map(capitals).collect()
    };
    let head = head
        .iter()
        .enumerate()
        .map(|(column, text)| cell(text, column, true, rows == 0));
    let body = table.rows.iter().enumerate().flat_map(|(row, cells)| {
        cells
            .iter()
            .enumerate()
            .map(move |(column, text)| (text, column, row + 1 == rows))
            .map(|(text, column, last)| cell(text, column, false, last))
            .collect::<Vec<_>>()
    });
    div()
        .id(look.id("table"))
        .w_full()
        .flex()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .child(exact::cells(
            div()
                .flex_none()
                .grid()
                .grid_cols_max_content(columns as u16)
                .rounded(px(metric::RADIUS_MEDIUM))
                .border_1()
                .border_color(theme.line)
                .overflow_hidden(),
            head.chain(body).map(IntoElement::into_any_element),
        ))
}

/// Inline text in capitals, where that keeps its length (and so its spans).
fn capitals(inline: &Inline) -> Inline {
    let upper = inline.text.to_uppercase();
    if upper.len() == inline.text.len() {
        Inline {
            text: upper.into(),
            ..inline.clone()
        }
    } else {
        inline.clone()
    }
}

/// Inline text with its styles, and its links as controls. With `caret`, the
/// caret of text still being written ends it.
fn inline(
    inline: &Inline,
    color: Hsla,
    weight: FontWeight,
    align: Align,
    caret: bool,
    look: &Look,
) -> AnyElement {
    let theme = look.theme;
    let run = |len: usize, style: InlineStyle| {
        let mut font = font(if style.code {
            theme.mono_font.clone()
        } else {
            theme.ui_font.clone()
        });
        font.weight = match (style.bold, look.studio) {
            (true, true) => FontWeight::SEMIBOLD,
            (true, false) => FontWeight::BOLD,
            (false, _) => weight,
        };
        if style.italic {
            font.style = FontStyle::Italic;
        }
        // `.prose a`: in the accent, underlined; in the quieter look, the
        // accent alone. Its code is lighter than its text, on its chip.
        let color = match (style.link, style.bold) {
            (true, _) => theme.accent,
            (false, _) if style.code && look.studio => theme.chip_ink,
            (false, true) => look.tone.strong,
            (false, false) => color,
        };
        TextRun {
            len,
            font,
            color,
            // A piece of code stands on a chip the text paints itself,
            // rounded and rimmed: a run's own background is a bare rectangle
            // as wide as the letters and as tall as the line.
            background_color: None,
            underline: (style.link && !look.studio).then_some(UnderlineStyle {
                thickness: px(1.),
                color: Some(color.opacity(0.6)),
                wavy: false,
            }),
            strikethrough: style.strike.then_some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(color),
            }),
        }
    };
    let mut runs = Vec::with_capacity(inline.spans.len() * 2 + 1);
    let mut at = 0;
    for span in &inline.spans {
        if span.range.start > at {
            runs.push(run(span.range.start - at, InlineStyle::default()));
        }
        runs.push(run(span.range.len(), span.style));
        at = span.range.end;
    }
    if at < inline.text.len() {
        runs.push(run(inline.text.len() - at, InlineStyle::default()));
    }
    // `.prose code`: each piece of code, for its chip. How lit a piece is
    // before the text's fading pieces are taken into it, and after: its chip
    // comes in with it.
    let codes: Vec<Range<usize>> = inline
        .spans
        .iter()
        .filter(|span| span.style.code && !span.range.is_empty())
        .map(|span| span.range.clone())
        .collect();
    let lit_at = |runs: &[TextRun], at: usize| {
        let mut start = 0;
        for run in runs {
            if at < start + run.len {
                return run.color.a;
            }
            start += run.len;
        }
        1.
    };
    let full: Vec<f32> = codes
        .iter()
        .map(|range| lit_at(&runs, range.start))
        .collect();
    let mut runs = look.faded(runs, &inline.text);
    let lit: Vec<f32> = codes
        .iter()
        .zip(&full)
        .map(|(range, full)| {
            if *full > 0. {
                (lit_at(&runs, range.start) / full).clamp(0., 1.)
            } else {
                1.
            }
        })
        .collect();
    let mut text = inline.text.clone();
    if caret {
        // `.stream-caret`: a block half a letter wide, in the accent, after
        // the last word. An en space with the accent behind it.
        let lit = look.caret.unwrap_or(1.);
        text = format!("{text}\u{2002}").into();
        runs.push(TextRun {
            len: '\u{2002}'.len_utf8(),
            font: font(theme.ui_font.clone()),
            color: gpui::transparent_black(),
            background_color: Some(theme.accent.opacity(lit)),
            underline: None,
            strikethrough: None,
        });
    }

    let mut links: Vec<(Range<usize>, Target)> = inline
        .links
        .iter()
        .filter(|link| openable(&link.url))
        .map(|link| {
            (
                link.range.clone(),
                Target::Url(SharedString::from(link.url.clone())),
            )
        })
        .collect();
    // Where it names something the view opens: a link, its mark before it;
    // and room inside each piece of code for its chip.
    let mut marks: Vec<(Range<usize>, Mention)> = Vec::new();
    let mut chips: Vec<(Range<usize>, f32)> = Vec::new();
    let pad = if look.studio {
        CHIP_PAD
    } else {
        CHIP_PAD_SLIGHT
    };
    let found = match &look.mentions {
        Some((mentions, _)) => find_mentions(inline, mentions),
        None => Vec::new(),
    };
    if !found.is_empty() || !codes.is_empty() {
        let mut puts: Vec<Put> = Vec::new();
        for range in &codes {
            puts.push(Put::open(range.start, pad));
            puts.push(Put::close(range.end, pad));
        }
        for (range, _) in &found {
            puts.push(Put::open(range.start, MARK_ROOM));
        }
        for (range, _) in &found {
            recolor(&mut runs, range.clone(), theme.accent);
        }
        let (shown, put_runs, puts) = put_in(&text, runs, puts, theme.ui_font.clone());
        runs = put_runs;
        for (link, _) in &mut links {
            *link = shifted(&puts, link.start, true)..shifted(&puts, link.end, false);
        }
        if let Some((mentions, open)) = &look.mentions {
            for (range, at) in &found {
                let Some(mention) = mentions.get(*at) else {
                    continue;
                };
                let start = shifted(&puts, range.start, true);
                let Some(room_start) = start.checked_sub(MARK_ROOM.len()) else {
                    continue;
                };
                let (id, open) = (mention.id.clone(), open.clone());
                links.push((
                    start..start + range.len(),
                    Target::Do(Rc::new(move |window, cx| open(id.clone(), window, cx))),
                ));
                marks.push((room_start..start, mention.clone()));
            }
        }
        for (range, lit) in codes.iter().zip(&lit) {
            let Some(start) = shifted(&puts, range.start, true).checked_sub(pad.len()) else {
                continue;
            };
            let end = shifted(&puts, range.end, false) + pad.len();
            if start < end && end <= shown.len() {
                chips.push((start..end, *lit));
            }
        }
        text = shown.into();
    }
    if links.is_empty() && chips.is_empty() {
        return scale::styled_text(text, runs);
    }
    links.sort_by_key(|(range, _)| range.start);
    let source = text.clone();
    // Zoomed, the text is laid out at its unscaled size and drawn scaled, so
    // that it wraps where it does at 1.
    let (layout, text) = if scale::factor() == 1. && !exact::recording() {
        let text = StyledText::new(text).with_runs(runs);
        (Laid::Text(text.layout().clone()), text.into_any_element())
    } else {
        let text = ScaledText::new(text).with_runs(runs);
        (Laid::Scaled(text.layout().clone()), text.into_any_element())
    };
    LinkedText {
        id: look.id("links"),
        source,
        align,
        field: look.field.clone(),
        layout,
        text,
        links,
        marks,
        chips,
        chip: theme.chip,
        // The quieter look's chip is its fill alone.
        chip_rim: (!look.studio).then_some(theme.line_strong),
        factor: scale::factor(),
        chip_rects: Vec::new(),
        ring: theme.accent,
        controls: Vec::new(),
    }
    .into_any_element()
}

/// Where `inline` names one of `mentions`, outside code and links: the
/// ranges, and which mention each is. The longest title is looked for first.
fn find_mentions(inline: &Inline, mentions: &[Mention]) -> Vec<(Range<usize>, usize)> {
    let mut order: Vec<usize> = (0..mentions.len()).collect();
    order.sort_by_key(|at| std::cmp::Reverse(mentions[*at].title.len()));
    let taken = |range: &Range<usize>, found: &[(Range<usize>, usize)]| {
        inline.spans.iter().any(|span| {
            span.style.code && span.range.start < range.end && range.start < span.range.end
        }) || inline
            .links
            .iter()
            .any(|link| link.range.start < range.end && range.start < link.range.end)
            || found
                .iter()
                .any(|(other, _)| other.start < range.end && range.start < other.end)
    };
    let mut found: Vec<(Range<usize>, usize)> = Vec::new();
    for at in order {
        let title = mentions[at].title.trim();
        if title.chars().count() < 4 {
            continue;
        }
        for (start, _) in inline.text.match_indices(title) {
            let range = start..start + title.len();
            if !taken(&range, &found) {
                found.push((range, at));
            }
        }
    }
    found.sort_by_key(|(range, _)| range.start);
    found
}

/// The room inside a chip of the quieter look, at each end: a thin space
/// and a hair space of the text's face (the picture's chip stands 8 pixels
/// round its code).
const CHIP_PAD: &str = "\u{2009}\u{200a}";
/// The room inside a chip otherwise: a hair space, which with the pixel the
/// chip is drawn wider by keeps the code just clear of the chip's rim.
const CHIP_PAD_SLIGHT: &str = "\u{200a}";

/// What is put into a text at a place: room for a mark, a chip's padding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Put {
    at: usize,
    text: &'static str,
    /// It closes what stands before `at` (a chip's end), and goes before
    /// what opens there.
    closing: bool,
}

impl Put {
    fn open(at: usize, text: &'static str) -> Self {
        Self {
            at,
            text,
            closing: false,
        }
    }

    fn close(at: usize, text: &'static str) -> Self {
        Self {
            at,
            text,
            closing: true,
        }
    }
}

/// `text` with `puts` put in, unseen runs in `runs` for them, and the puts
/// that were (in order). One whose place is past the text or inside a
/// letter is left out: the runs always cover the text, each ending between
/// two letters.
fn put_in(
    text: &str,
    mut runs: Vec<TextRun>,
    mut puts: Vec<Put>,
    family: SharedString,
) -> (String, Vec<TextRun>, Vec<Put>) {
    puts.retain(|put| put.at <= text.len() && text.is_char_boundary(put.at));
    puts.sort_by_key(|put| (put.at, !put.closing));
    let mut shown = text.to_owned();
    // From the last: each goes in where the text before it still stands.
    for put in puts.iter().rev() {
        shown.insert_str(put.at, put.text);
        insert_run(&mut runs, put.at, put.text.len(), family.clone());
    }
    (shown, runs, puts)
}

/// Where `at` of the text before `puts` stands after them: past what was
/// put in at `at` too with `inclusive`.
fn shifted(puts: &[Put], at: usize, inclusive: bool) -> usize {
    at + puts
        .iter()
        .filter(|put| put.at < at || (inclusive && put.at == at))
        .map(|put| put.text.len())
        .sum::<usize>()
}

/// `runs` with the text in `range` in `color`.
fn recolor(runs: &mut Vec<TextRun>, range: Range<usize>, color: Hsla) {
    split_runs(runs, range.start);
    split_runs(runs, range.end);
    let mut at = 0;
    for run in runs.iter_mut() {
        if at >= range.start && at + run.len <= range.end {
            run.color = color.opacity(run.color.a);
        }
        at += run.len;
    }
}

/// `runs` cut at `at`, so that a run starts there.
fn split_runs(runs: &mut Vec<TextRun>, at: usize) {
    let mut start = 0;
    for index in 0..runs.len() {
        let end = start + runs[index].len;
        if at > start && at < end {
            let mut tail = runs[index].clone();
            tail.len = end - at;
            runs[index].len = at - start;
            runs.insert(index + 1, tail);
            return;
        }
        start = end;
    }
}

/// `runs` with an unseen run `len` long put in at `at`.
fn insert_run(runs: &mut Vec<TextRun>, at: usize, len: usize, family: SharedString) {
    split_runs(runs, at);
    let mut start = 0;
    let mut index = runs.len();
    for (position, run) in runs.iter().enumerate() {
        if start >= at {
            index = position;
            break;
        }
        start += run.len;
    }
    runs.insert(
        index,
        TextRun {
            len,
            font: font(family),
            color: gpui::transparent_black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        },
    );
}

/// Something of the view's own a link does.
type Action = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a link does when it is pressed.
#[derive(Clone)]
enum Target {
    /// Hands the address to the system.
    Url(SharedString),
    /// Something of the view's own.
    Do(Action),
}

/// Whether a link from a model may be handed to the system. Only the web and
/// mail: other schemes can start programs.
pub(crate) fn openable(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    ["https://", "http://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

/// A mention's mark at `side`: its harness's mark in its colour, or its
/// letters on a small tile.
fn mention_mark(mention: &Mention, side: Pixels) -> Div {
    match mention.brand {
        Some(found) => div().size(side).child(
            gpui::svg()
                .path(found.path())
                .size(side)
                .text_color(mention.color),
        ),
        None => div()
            .size(side)
            .rounded(px(metric::RADIUS_MARK))
            .bg(mention.color.opacity(0.25))
            .flex()
            .items_center()
            .justify_center()
            .text_size(side * 0.5)
            .line_height(side)
            .font_weight(FontWeight::BOLD)
            .text_color(mention.color)
            .child(mention.letters.clone()),
    }
}

/// Where a text was laid out: by GPUI's own text, or by a scaled one.
enum Laid {
    Text(TextLayout),
    Scaled(ScaledTextLayout),
}

/// Text whose links are controls: clickable, reachable by Tab, opened by Enter
/// or Space, with a focus ring. The text is laid out as one piece, so the
/// controls can only be placed once it has wrapped: they are built in prepaint,
/// over the rows each link landed on.
struct LinkedText {
    id: ElementId,
    text: AnyElement,
    /// The text itself, to find the line a link is on.
    source: SharedString,
    align: Align,
    /// Where the keyboard goes after a pointer click on a link.
    field: FocusHandle,
    layout: Laid,
    links: Vec<(Range<usize>, Target)>,
    /// The marks of what the text names, in the room left for them.
    marks: Vec<(Range<usize>, Mention)>,
    /// The pieces of code, padding included, with a chip behind them, and
    /// how lit each is (a piece still fading in is less than 1); the chips'
    /// fill and their rim, if they have one; the factor in effect; and
    /// where the chips were laid out.
    chips: Vec<(Range<usize>, f32)>,
    chip: Hsla,
    chip_rim: Option<Hsla>,
    factor: f32,
    chip_rects: Vec<(Bounds<Pixels>, f32)>,
    ring: Hsla,
    controls: Vec<AnyElement>,
}

impl LinkedText {
    /// The rectangles a range of the wrapped text covers, one per row, each
    /// as wide as the range's text on that row.
    fn rows(&self, range: &Range<usize>) -> Vec<Bounds<Pixels>> {
        let layout = match &self.layout {
            Laid::Text(layout) => layout,
            Laid::Scaled(layout) => return layout.rects(range.clone()),
        };
        let Some(line) = layout.line_layout_for_index(range.start) else {
            return Vec::new();
        };
        let bounds = layout.bounds();
        let height = layout.line_height();
        // The typed line the link is on, and where its rows begin and end.
        let line_start = self.source[..range.start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let line_end = line_start + line.len();
        let mut edges = vec![line_start];
        edges.extend(line.wrap_boundaries().iter().map(|boundary| {
            line_start + line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
        }));
        edges.push(line_end);

        let mut rows = Vec::new();
        for edge in edges.windows(2) {
            let (row_start, row_end) = (edge[0], edge[1]);
            let from = range.start.max(row_start);
            let to = range.end.min(row_end);
            if from >= to {
                continue;
            }
            // An offset where the text wraps is reported as the end of the row
            // before it: right for an end, wrong for a start.
            let (Some(right), Some(row_right)) = (
                layout.position_for_index(to),
                layout.position_for_index(row_end),
            ) else {
                continue;
            };
            let left = if from == row_start && from != line_start {
                bounds.left()
            } else {
                match layout.position_for_index(from) {
                    Some(left) => left.x,
                    None => continue,
                }
            };
            // The positions are those of left-aligned text; a row is painted
            // shifted by its alignment.
            let slack = (bounds.right() - row_right.x).max(Pixels::ZERO);
            let shift = match self.align {
                Align::Left => Pixels::ZERO,
                Align::Center => slack / 2.,
                Align::Right => slack,
            };
            rows.push(Bounds::from_corners(
                point(left + shift, right.y),
                point(right.x + shift, right.y + height),
            ));
        }
        rows
    }
}

impl IntoElement for LinkedText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for LinkedText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.text.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text.prepaint(window, cx);
        // One focus handle per link, kept from frame to frame with the element.
        let count = self.links.len();
        let fresh = |kept: Option<Vec<FocusHandle>>| {
            let mut handles = kept.unwrap_or_default();
            handles.truncate(count);
            while handles.len() < count {
                handles.push(cx.focus_handle().tab_stop(true));
            }
            handles
        };
        let handles = match id {
            Some(id) => window.with_element_state(id, |kept, _| {
                let handles = fresh(kept);
                (handles.clone(), handles)
            }),
            None => fresh(None),
        };

        // The chips: as tall as the picture's 26 of 34 pixels of a line, a
        // pixel wider than their padding at each end.
        self.chip_rects.clear();
        for (range, lit) in &self.chips {
            for row in self.rows(range) {
                let height = row.size.height * 0.77;
                let wider = gpui::px(1.) * self.factor;
                self.chip_rects.push((
                    Bounds::new(
                        point(
                            row.origin.x - wider,
                            row.origin.y + (row.size.height - height) / 2.,
                        ),
                        size(row.size.width + wider * 2., height),
                    ),
                    *lit,
                ));
            }
        }
        for (range, mention) in &self.marks {
            let Some(room) = self.rows(range).into_iter().next() else {
                continue;
            };
            // The mark at the start of its room, under half the line's height,
            // the rest of the room the gap to the name.
            let side = (room.size.height * 0.45).min(room.size.width);
            let place = room.origin + point(room.size.width * 0.08, (room.size.height - side) / 2.);
            let mut mark = mention_mark(mention, side).into_any_element();
            mark.layout_as_root(
                gpui::size(side, side).map(AvailableSpace::Definite),
                window,
                cx,
            );
            mark.prepaint_at(place, window, cx);
            self.controls.push(mark);
        }
        for (index, (range, url)) in self.links.iter().enumerate() {
            let handle = &handles[index];
            // A link that wraps is one control in several pieces: all of them show its focus.
            let ring = if handle.is_focused(window) && window.last_input_was_keyboard() {
                self.ring
            } else {
                gpui::transparent_black()
            };
            for (part, row) in self.rows(range).into_iter().enumerate() {
                // The ring sits just outside the glyphs.
                let row = Bounds::new(
                    row.origin - point(px(2.), px(0.)),
                    size(row.size.width + px(4.), row.size.height),
                );
                let (url, field) = (url.clone(), self.field.clone());
                let mut control = div()
                    .id(("link", index * 16 + part.min(15)))
                    .debug_selector(|| format!("link-{index}"))
                    .size_full()
                    .rounded(px(metric::RADIUS_SMALL))
                    .border_1()
                    .border_color(ring)
                    .cursor_pointer()
                    // The first piece carries the focus: one link, one tab stop.
                    .when(part == 0, |control| control.track_focus(handle))
                    .on_click(move |click, window, cx| {
                        // After a pointer click the keyboard belongs in the field again.
                        if !matches!(click, ClickEvent::Keyboard(_)) {
                            window.focus(&field, cx);
                        }
                        match &url {
                            Target::Url(url) => cx.open_url(url),
                            Target::Do(action) => action(window, cx),
                        }
                    })
                    .into_any_element();
                control.layout_as_root(row.size.map(AvailableSpace::Definite), window, cx);
                control.prepaint_at(row.origin, window, cx);
                self.controls.push(control);
            }
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let radius = gpui::px(metric::RADIUS_SMALL) * self.factor;
        // (A hairline at any zoom: thinner when drawn small, never thicker.)
        let rim_width = gpui::px(self.factor.min(1.));
        for (rect, lit) in &self.chip_rects {
            let mut chip =
                gpui::fill(*rect, self.chip.opacity(self.chip.a * lit)).corner_radii(radius);
            if let Some(rim) = self.chip_rim {
                chip = chip
                    .border_widths(rim_width)
                    .border_color(rim.opacity(rim.a * lit));
            }
            window.paint_quad(chip);
        }
        self.text.paint(window, cx);
        for control in &mut self.controls {
            control.paint(window, cx);
        }
    }
}

/// Where the runs of `text`, which begins `start` into what is drawn, are
/// cut for the pieces that fade: at each piece's ends that fall inside it.
/// A piece is counted in what was written, which is not what is drawn (the
/// marks of bold and the like are gone from it), so an end may fall inside
/// a letter of several bytes: the cut is made before that letter. A run
/// that ended inside one would be no run at all, and the text system
/// refuses it outright.
fn fade_cuts(text: &str, start: usize, fades: &[(Range<usize>, f32)]) -> Vec<usize> {
    let end = start + text.len();
    let mut cuts: Vec<usize> = fades
        .iter()
        .flat_map(|(range, _)| [range.start, range.end])
        .filter(|at| *at > start && *at < end)
        .map(|at| {
            let mut cut = at - start;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            cut
        })
        .filter(|cut| *cut > 0)
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fading_piece_never_cuts_a_letter_in_two() {
        // As it crashed: bold Russian text, and a piece that ends at byte
        // 26, inside the letter that takes bytes 25 and 26.
        let text = "LI \u{432} \u{442}\u{435}\u{440}\u{43c}\u{438}\u{43d}\u{430}\u{43b}\u{435} \u{43d}\u{430} macOS/Linux. ";
        assert!(!text.is_char_boundary(26));
        let cuts = fade_cuts(text, 0, &[(0..26, 0.8)]);
        assert_eq!(cuts, [25]);
        // Wherever the pieces end, every cut stands between two letters,
        // inside the text, in order and once.
        for from in 0..text.len() + 4 {
            for to in from..text.len() + 4 {
                for start in [0, 3, 17] {
                    let cuts = fade_cuts(text, start, &[(from..to, 0.5), (to..to + 5, 0.2)]);
                    assert!(cuts.windows(2).all(|pair| pair[0] < pair[1]), "{cuts:?}");
                    for cut in cuts {
                        assert!(cut > 0 && cut < text.len(), "{cut}");
                        assert!(
                            text.is_char_boundary(cut),
                            "{from}..{to} from {start}: {cut}"
                        );
                    }
                }
            }
        }
        // A piece that covers the text, or lies outside it, cuts nothing.
        assert!(fade_cuts(text, 10, &[(0..500, 0.5), (600..700, 0.5)]).is_empty());
    }

    #[test]
    fn what_is_put_into_a_text_never_cuts_a_letter_in_two() {
        let text = "Запустил 🦀 `lite`: замер — ёж";
        let plain = |len: usize| TextRun {
            len,
            font: font("sans"),
            color: gpui::white(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        // Runs as a paragraph's styles cut them: at letters' boundaries.
        let cut = text.find('—').unwrap();
        let runs = vec![plain(cut), plain(text.len() - cut)];
        let code = text.find("`lite`").unwrap();
        let name = text.find("замер").unwrap();
        let crab = text.find('🦀').unwrap();
        let puts = vec![
            Put::open(code, CHIP_PAD),
            Put::close(code + "`lite`".len(), CHIP_PAD),
            Put::open(name, MARK_ROOM),
            // Stale places: inside the crab, past the end.
            Put::open(crab + 1, MARK_ROOM),
            Put::open(text.len() + 3, MARK_ROOM),
        ];
        let (shown, runs, puts) = put_in(text, runs, puts, "sans".into());
        let mut at = 0;
        for run in &runs {
            at += run.len;
            assert!(run.len > 0 && shown.is_char_boundary(at), "{runs:?}");
        }
        assert_eq!(at, shown.len());
        assert_eq!(puts.len(), 3, "the stale places are left out");
        // What was before stands where `shifted` says.
        let start = shifted(&puts, name, true);
        assert!(shown[start..].starts_with("замер"));
        assert_eq!(&shown[start - MARK_ROOM.len()..start], MARK_ROOM);
        let chip = shifted(&puts, code, true) - CHIP_PAD.len()
            ..shifted(&puts, code + "`lite`".len(), false) + CHIP_PAD.len();
        assert_eq!(&shown[chip], "\u{2009}\u{200a}`lite`\u{2009}\u{200a}");
        assert_eq!(&shown[..shifted(&puts, crab, true)], "Запустил ");
    }

    #[test]
    fn a_mention_is_recoloured_without_cutting_a_letter() {
        let text = "Запустил lite-measure: замер";
        let mut runs = vec![TextRun {
            len: text.len(),
            font: font("sans"),
            color: gpui::white(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        let start = text.find("lite").unwrap();
        recolor(&mut runs, start..text.len(), gpui::red());
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, start);
        assert_eq!(runs[1].color, gpui::red());
    }

    #[test]
    fn only_web_and_mail_links_are_opened() {
        assert!(openable("https://example.com"));
        assert!(openable("HTTP://example.com"));
        assert!(openable("mailto:someone@example.com"));
        assert!(!openable("file:///etc/passwd"));
        assert!(!openable("vscode://open?file=x"));
        assert!(!openable("javascript:alert(1)"));
        assert!(!openable("example.com"));
    }
}
