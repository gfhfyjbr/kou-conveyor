//! How each entry of the transcript looks, after the web cockpit's timeline
//! (`plugins/timeline/web/timeline.css`, as it is with round corners): a rail
//! down the left with a node for each entry, a line of capitals that says
//! whose an entry is, and the entry under it. A prompt is a card of its own,
//! an answer is prose, a tool call is a card whose head says what it does and
//! how it went, reasoning is a line that opens.

use crate::{
    diff::{self, Diff},
    draft,
    lights::{Light, Lights, mark_spot},
    markdown::{self, Block as Markdown},
    motion,
    nested::NestedState,
    prose::{self, CopyFn, LINE, Look, MONO_LINE, PROSE, Tone, act, act_revealed},
    terminal,
    view::{Shot, ThreadView, allow_key, default_option, display_path},
};
use gpui::{
    AnyElement, AppContext as _, Context, Div, FontWeight, Hsla, ImageSource,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, TextRun, Window, div, font,
    img, prelude::FluentBuilder as _, rgb,
};
use kou_thread::{
    Block, Entry, Level, PendingPermission, PlanStatus, PlanStep, Stop, ThreadState, ToolCall,
    ToolContent, ToolKind, ToolStatus,
};
use kou_ui::{
    Icon, Theme, icon, key_hint, metric,
    scale::{self, exact, px},
};
use std::{path::Path, rc::Rc, time::Instant};

/// Space between the pane's edge and the rail's column.
pub(crate) const EDGE: f32 = 12.;
/// Width of the rail's column, the nodes centred in it.
pub(crate) const GUTTER: f32 = 14.;
/// Space between the rail's column and what an entry shows.
pub(crate) const MARK_GAP: f32 = 10.;
/// Where the transcript's text begins, from the pane's edge: past the rail.
/// The prompt field stands under the text, from this edge to the text's
/// other one, not under the rail as well.
pub(crate) const TEXT_EDGE: f32 = EDGE + GUTTER + MARK_GAP;
/// The widest the transcript's column gets, centred in a wider pane
/// (`--measure`).
pub(crate) const MEASURE: f32 = 860.;
/// The size of the labels in capitals of the monospace (`.meta`, `.label`).
pub(crate) const LABEL: f32 = 10.5;
/// Height of the line of capitals over an entry (`.meta`), and of a fold.
pub(crate) const META: f32 = 22.;
/// Height of a tool call's head (`.tool-head`).
pub(crate) const HEAD: f32 = 34.;
/// Side of a node on the rail (`.entry::before`).
const NODE: f32 = 7.;
/// Lines of a terminal output shown until it is unfolded.
const TERMINAL_TAIL: usize = 16;
/// Lines of a terminal output shown at most: the end of it.
const TERMINAL_MAX: usize = 2000;
/// Lines a call that runs shows while it is closed (`timeline.js`, `TAIL`).
const LIVE_TAIL: usize = 6;
/// Rows of a diff shown at most.
const DIFF_MAX: usize = 400;
/// Lines of a tool's raw arguments or result shown.
const RAW_MAX: usize = 40;
/// Of what a subagent was asked, the lines its card shows; and of what it
/// did, the steps, counted from the last.
const AGENT_ASKED: usize = 10;
const AGENT_STEPS: usize = 12;

/// What an entry parsed to. Kept per entry and thrown away when the entry changes.
#[derive(Clone)]
pub(crate) enum Parsed {
    /// An answer or reasoning as markdown.
    Document(Rc<[Markdown]>),
    /// A user message.
    User(Rc<UserMessage>),
    /// A tool call's content, piece by piece.
    Tool(Rc<[Piece]>),
}

/// A user message as the transcript shows it.
pub(crate) struct UserMessage {
    /// Its text, each picture named by its label where it was.
    pub text: SharedString,
    /// Where the labels are in the text.
    pub labels: Vec<std::ops::Range<usize>>,
    /// Its pictures: label and the block that holds the bytes.
    pub pictures: Vec<(SharedString, usize)>,
    /// The files it brought, as paths to show.
    pub files: Vec<SharedString>,
}

/// One piece of a tool call's content.
pub(crate) enum Piece {
    Document(Rc<[Markdown]>),
    Diff {
        path: SharedString,
        diff: Diff,
    },
    Terminal {
        lines: Vec<terminal::Line>,
        exit: Option<i32>,
    },
    /// A picture the tool gave back: the content at this piece's place.
    Picture,
    /// Not parsed because the call is folded and nothing of this piece shows.
    Folded,
}

pub(crate) fn parse(entry: &Entry, open: bool, base: &Path) -> Option<Parsed> {
    Some(match entry {
        Entry::Assistant(text) | Entry::Thought(text) => {
            Parsed::Document(markdown::parse(text).into())
        }
        Entry::User(blocks) => {
            let (text, pictures) = draft::shown(blocks);
            let mut labels = Vec::new();
            let mut from = 0;
            for (label, _) in &pictures {
                if let Some(at) = text[from..].find(label.as_str()) {
                    labels.push(from + at..from + at + label.len());
                    from += at + label.len();
                }
            }
            Parsed::User(Rc::new(UserMessage {
                text: text.trim_end().to_owned().into(),
                labels,
                pictures: pictures
                    .into_iter()
                    .map(|(label, block)| (label.into(), block))
                    .collect(),
                files: blocks
                    .iter()
                    .filter_map(|block| match block {
                        Block::File { path, .. } => Some(display_path(path, Some(base))),
                        _ => None,
                    })
                    .collect(),
            }))
        }
        Entry::Tool(call) => Parsed::Tool(
            call.content
                .iter()
                .map(|content| match content {
                    // A folded row still shows how many lines a change added and removed.
                    ToolContent::Diff { path, old, new } => Piece::Diff {
                        path: display_path(path, Some(base)),
                        diff: diff::diff(old.as_deref(), new, diff::CONTEXT),
                    },
                    // A call that runs shows the end of its output closed too.
                    ToolContent::Terminal { output, exit, .. } => Piece::Terminal {
                        lines: terminal::lines(output),
                        exit: *exit,
                    },
                    _ if !open => Piece::Folded,
                    ToolContent::Image { .. } => Piece::Picture,
                    ToolContent::Text(text) => {
                        let blocks = markdown::parse(text);
                        // What a command printed, handed over as one fenced
                        // block: its output, drawn as a terminal's is.
                        match (call.kind, blocks.as_slice()) {
                            (ToolKind::Execute, [Markdown::Code { text, .. }]) => Piece::Terminal {
                                lines: terminal::lines(text),
                                exit: None,
                            },
                            _ => Piece::Document(blocks.into()),
                        }
                    }
                })
                .collect(),
        ),
        Entry::Plan(_) | Entry::Notice(..) => return None,
    })
}

/// A node on the rail: filled, hollow with a border, or the short rule of a notice.
pub(crate) enum Node {
    Filled(Hsla),
    Hollow(Hsla),
    Rule,
}

/// The node as drawn, as lit as what blinks is.
pub(crate) fn node(node: Node, lit: f32, theme: &Theme) -> Div {
    let base = div()
        .flex_none()
        .w(px(NODE))
        .h(px(NODE))
        .rounded(px(NODE / 2.));
    match node {
        Node::Filled(color) => base.bg(color.opacity(color.a * lit)),
        Node::Hollow(color) => base.bg(theme.pane).border_1().border_color(color),
        Node::Rule => div().flex_none().w(px(NODE)).h(px(1.)).bg(theme.fg_faint),
    }
}

/// A row of the transcript: its node on the rail, centred on its first line
/// (`first` tall), and its content from the text edge.
pub(crate) fn row(node: impl IntoElement, first: f32, content: impl IntoElement) -> Div {
    div()
        .w_full()
        .px(px(EDGE))
        .flex()
        .items_start()
        .gap(px(MARK_GAP))
        .child(
            div()
                .flex_none()
                .w(px(GUTTER))
                .h(px(first))
                .flex()
                .items_center()
                .justify_center()
                .child(node),
        )
        .child(div().flex_1().min_w_0().child(content))
}

/// A label in capitals of the monospace (`.meta`, `.label`).
pub(crate) fn label(text: impl Into<SharedString>, color: Hsla, theme: &Theme) -> Div {
    let text: SharedString = text.into();
    div()
        .flex_none()
        .font_family(theme.mono_font.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(LABEL))
        .text_color(color)
        .child(scale::text(text.to_uppercase()))
}

/// The line of capitals over an entry: who, then what is known of it, its
/// actions at the end.
fn meta_line() -> Div {
    div()
        .h(px(META))
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.))
}

/// Small monospace text beside a title: a path, a count.
pub(crate) fn meta(theme: &Theme) -> Div {
    div()
        .font_family(theme.mono_font.clone())
        .text_size(px(11.))
        .text_color(theme.fg_muted)
}

/// A section's header in a tool call's body (`.stream header`): what it is,
/// in capitals, and what it says at its end.
fn stream_header(name: impl Into<SharedString>, theme: &Theme) -> Div {
    let name: SharedString = name.into();
    div()
        .h(px(28.))
        .pl(px(12.))
        .pr(px(10.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .font_family(theme.mono_font.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(10.))
        .text_color(theme.fg_muted)
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(scale::text(name.to_uppercase())),
        )
}

/// What a tool call's head says it is: a verb in capitals (`.tname`) and what
/// it was given (`.tinput`). A title that starts with a word of its own
/// (`Run cargo test`, `Read src/main.rs`) gives both; otherwise the kind names it.
pub(crate) fn split_title(call: &ToolCall) -> (String, String) {
    let title = call.title.trim();
    let kind = match call.kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "run",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::Other => "tool",
    };
    if let Some((first, rest)) = title.split_once(char::is_whitespace) {
        let word = first.trim_end_matches(':');
        if !word.is_empty()
            && word.len() <= 12
            && word.chars().all(char::is_alphabetic)
            && word.chars().next().is_some_and(char::is_uppercase)
            && !rest.trim().is_empty()
        {
            return (word.to_owned(), rest.trim().to_owned());
        }
    }
    (kind.to_owned(), title.to_owned())
}

impl ThreadView {
    /// One row of the transcript list: an entry, or after the last entry the
    /// line that says what the thread is doing.
    pub(crate) fn item(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.around.studio() {
            return self.studio_item(index, window, cx);
        }
        if index >= self.thread.entries().len() {
            return self.tail(kou_ui::theme(cx), cx).into_any_element();
        }
        if index < self.cleared {
            // Out of sight after /clear; the last of them says so.
            if index + 1 < self.cleared {
                return div().into_any_element();
            }
            return self.cleared_row(kou_ui::theme(cx), cx).into_any_element();
        }
        // Work that is over folds into one line, at its first row, which a
        // click opens; and what a subagent did is in the card of the call
        // that started it. Either way the row shows nothing of its own:
        // nothing of it is read or laid out.
        let fold = self.fold_of(index);
        // (A run that is all a subagent's doing has no line to lead with:
        // its first row is as empty as the rest.)
        let leads = fold.is_some_and(|(run, _, first)| first && self.steps_of(&run) > 0);
        let hidden = (fold.is_some_and(|(_, open, _)| !open)
            && crate::nested::folds(&self.thread.entries()[index]))
            || self.kept_in_its_agent(index);
        if hidden && !leads {
            let (top, bottom) = self.cards_of(index);
            if top.is_empty() && bottom.is_empty() {
                return gpui::Empty.into_any_element();
            }
        }
        let now = cx.background_executor().now();
        let still = cx.reduce_motion();
        if self.cache.len() <= index {
            self.cache.resize_with(index + 1, || None);
        }
        if !hidden && self.cache[index].is_none() {
            let entry = &self.thread.entries()[index];
            let open = match entry {
                // Closing, a call's content is drawn until it has closed.
                Entry::Tool(call) => {
                    self.tool_open(index, call) || self.motion.unfolds.contains_key(&index)
                }
                _ => true,
            };
            let parsed = parse(entry, open, &self.header.cwd);
            // Text the view watches being written fades in as it comes.
            if let Some(Parsed::Document(blocks)) = &parsed
                && self.motion.fresh.contains_key(&index)
            {
                self.motion.grow(index, prose::drawn_len(blocks), now);
            }
            self.cache[index] = parsed;
        }
        if !hidden {
            self.tool_shots(index, cx);
        }
        // The pictures of a message are decoded the first time it shows.
        let shots: Vec<Shot> = match (&self.thread.entries()[index], &self.cache[index]) {
            (Entry::User(blocks), Some(Parsed::User(message))) => {
                let wanted: Vec<(usize, std::sync::Arc<[u8]>)> = message
                    .pictures
                    .iter()
                    .filter_map(|(_, at)| match &blocks[*at] {
                        Block::Image { data, .. } => Some((*at, data.clone())),
                        _ => None,
                    })
                    .collect();
                wanted
                    .into_iter()
                    .map(|(at, data)| self.shot(index, at, &data, cx))
                    .collect()
            }
            _ => Vec::new(),
        };
        // The loops of what runs or is being written: views of their own.
        let lights = self.row_lights(index, cx);
        let cards_lit = self.card_lights(index, cx);
        let view = cx.weak_entity();
        let copy: CopyFn = std::rc::Rc::new(move |key, text, _, cx| {
            view.update(cx, |view, cx| view.copy(key, text, cx)).ok();
        });
        let rise = if still {
            None
        } else {
            self.motion.rise(index, now)
        };
        // A light made while the rows were built is drawn over them from
        // the next frame: one more is asked for.
        if lights.made
            || cards_lit
            || rise.is_some()
            || (!still && self.motion.flash(index, now).is_some())
        {
            window.request_animation_frame();
        }
        let cx = &*cx;
        let theme = kou_ui::theme(cx);
        let entries = self.thread.entries();
        let parsed = self.cache[index].as_ref();
        let lit = if still { 1. } else { self.motion.blink(now).0 };
        // The line a run of work folds into, at the run's first row. (A run
        // that is all a subagent's doing has none: its card says it.)
        let fold_line = fold
            .filter(|(run, _, first)| *first && self.steps_of(run) > 0)
            .map(|(run, open, _)| (self.worked_row(&run, open, theme, cx), open));
        let body = match (&entries[index], parsed) {
            _ if hidden => div().into_any_element(),
            (Entry::User(_), Some(Parsed::User(message))) => self
                .user(index, message, &shots, now, theme, window, cx)
                .into_any_element(),
            (Entry::Assistant(text), Some(Parsed::Document(blocks))) => {
                let writing = self.writing(index);
                let look = Look::new(theme, Tone::answer(theme), self.field_focus())
                    .with_copy(index, self.copied.clone(), copy.clone())
                    .with_fades(if still {
                        Vec::new()
                    } else {
                        self.motion.fades(index, now)
                    })
                    .with_caret(writing.then(|| {
                        if still {
                            1.
                        } else {
                            self.motion.caret(index, now).0
                        }
                    }));
                let node = if writing {
                    Node::Filled(theme.working)
                } else {
                    Node::Filled(theme.fg)
                };
                self.answer(index, text, blocks, &look, node, lit, cx)
                    .into_any_element()
            }
            (Entry::Thought(_), Some(Parsed::Document(blocks))) => {
                let fades = if still {
                    Vec::new()
                } else {
                    self.motion.fades(index, now)
                };
                self.thought(index, blocks, fades, lights.shimmer, lit, theme, cx)
                    .into_any_element()
            }
            (Entry::Tool(call), Some(Parsed::Tool(pieces))) => self
                .tool(index, call, pieces, lights.run, lit, now, theme, cx)
                .into_any_element(),
            (Entry::Plan(steps), _) => plan(steps, lit, theme).into_any_element(),
            (Entry::Notice(level, text), _) => {
                // The thread records a failed turn as an error notice with the reason.
                let turn = index + 1 == entries.len()
                    && matches!(&self.thread.last_stop, Some(Stop::Failed(reason)) if reason == text);
                notice(*level, text, turn, theme).into_any_element()
            }
            _ => div().into_any_element(),
        };
        // `.entry { padding: 9px 0 }`: the same air between every two entries.
        let first = index == self.cleared;
        let above = if first { 14. } else { 18. };
        // The rail, from the first node down through every entry.
        let rail = div()
            .absolute()
            .left(px(EDGE + GUTTER / 2. - 0.5))
            .top(px(if first { above + META / 2. } else { 0. }))
            .bottom_0()
            .w(px(1.))
            .bg(theme.line);
        // The cards of the agents started before this entry and after it.
        let (top, bottom) = self.cards_of(index);
        let (top, bottom): (Vec<_>, Vec<_>) = (
            top.into_iter().cloned().collect(),
            bottom.into_iter().cloned().collect(),
        );
        let top_count = top.len();
        let column = div()
            .relative()
            .w_full()
            .max_w(px(MEASURE))
            .pt(px(above))
            .child(rail)
            .when(!top.is_empty(), |column| {
                column
                    .child(self.card_group(&top, index, 0, 0., theme, cx))
                    .child(div().h(px(18.)))
            })
            .when_some(fold_line, |column, (line, open)| {
                // Opened, the work stands under its line, an entry's air away.
                column
                    .child(line)
                    .when(open && !hidden, |column| column.child(div().h(px(above))))
            })
            .child(body)
            .when(!bottom.is_empty(), |column| {
                column.child(self.card_group(&bottom, index, top_count, 12., theme, cx))
            });
        // A preview of what the agent writes rises into place (`.entry.rising`):
        // moved, not laid out anew, so nothing around it moves.
        let column = match rise {
            Some(t) => column.top(px(cockpit_rise_by() * (1. - t))).opacity(t),
            None => column,
        };
        let inset = self.around.inset.get();
        let row = div()
            .id(("entry", index))
            .w_full()
            .when(inset > 0., |row| row.pr(px(inset)))
            .flex()
            .justify_center()
            .child(column);
        // What an edit open replaces fades, and lifts under the pointer
        // (`.entry[data-after-edit]`).
        let faded = self.motion.faded(index, now, still);
        if self.motion.after_edit.is_none() {
            return row.into_any_element();
        }
        row.when_some(faded, |row, opacity| row.opacity(opacity))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let now = cx.background_executor().now();
                this.motion.edit_hover(index, *hovered, now);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The line a run of work folds into once it is over, on the rail: how
    /// long it took or how many steps it was, and a chevron. A click opens
    /// the run under it, and closes it again.
    fn worked_row(
        &self,
        run: &crate::nested::Run,
        open: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let words = crate::nested::worked(self.worked_for(run), self.steps_of(run));
        let start = run.start;
        let (hover, ring) = (theme.hover, theme.accent);
        let line = div()
            .id(("worked", start))
            .debug_selector(move || format!("worked-{start}"))
            .h(px(META))
            .mx(px(-6.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(metric::RADIUS))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .tab_index(0)
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener(move |this, click, window, cx| {
                this.toggle_worked(start, cx);
                this.rest_focus(click, window, cx);
            }))
            .child(label(words, theme.fg_muted, theme))
            .child(chevron(open, theme));
        row(
            self::node(Node::Hollow(theme.fg_faint), 1., theme),
            META,
            div().flex().child(line),
        )
    }

    /// Whether entry `index` is being written now: the last entry, an answer
    /// or reasoning, of a turn that runs.
    pub(crate) fn writing(&self, index: usize) -> bool {
        index + 1 == self.thread.entries().len()
            && matches!(
                self.thread.entries().get(index),
                Some(Entry::Assistant(_) | Entry::Thought(_))
            )
            && matches!(self.thread.state(), ThreadState::Working)
            // Waiting on the agents it started, it is not writing.
            && self.nest.waiting_on.is_empty()
    }

    /// Whether a tool call shows its content: the user's choice if there is
    /// one, otherwise open while it waits on the user or has failed.
    pub(crate) fn tool_open(&self, index: usize, call: &ToolCall) -> bool {
        if let Some(open) = self.toggled.get(&index) {
            return *open;
        }
        self.asking(index).is_some() || matches!(call.status, ToolStatus::Failed)
    }

    /// Whether reasoning shows in full: only when the user opens it, as in
    /// the web cockpit, where it is a line that says what it is about.
    pub(crate) fn thought_open(&self, index: usize) -> bool {
        self.toggled.get(&index).copied().unwrap_or(false)
    }

    pub(crate) fn asking(&self, index: usize) -> Option<&PendingPermission> {
        self.thread
            .permissions()
            .iter()
            .find(|pending| pending.entry == index)
    }

    /// The lights row `index` needs now, made the first time and kept while
    /// they are needed; they are drawn over the transcript (see
    /// [`crate::lights`]). Whether each is lit.
    pub(crate) fn row_lights(&mut self, index: usize, cx: &mut Context<Self>) -> RowLights {
        let still = cx.reduce_motion();
        let entry = &self.thread.entries()[index];
        let run = !still
            && matches!(entry, Entry::Tool(call) if call.status == ToolStatus::Running)
            && self.asking(index).is_none();
        let shimmer = !still && matches!(entry, Entry::Thought(_)) && self.writing(index);
        let (epoch, scale, spots) = (self.motion.epoch, self.scale.clone(), self.spots.clone());
        let mut made = false;
        let mut light = |key: (usize, u8), wanted: bool, make: fn() -> Light| {
            if wanted {
                self.lights.entry(key).or_insert_with(|| {
                    made = true;
                    cx.new(|_| Lights::over(make(), epoch, scale.clone(), spots.clone(), key))
                });
            }
            wanted
        };
        let run = light((index, 0), run, || Light::Run {
            radius: metric::RADIUS_MEDIUM,
        });
        let shimmer = light((index, 1), shimmer, || Light::Shimmer {
            label: "THINKING".into(),
        });
        RowLights { made, run, shimmer }
    }

    #[allow(clippy::too_many_arguments)]
    fn thought(
        &self,
        index: usize,
        blocks: &Rc<[Markdown]>,
        fades: Vec<(std::ops::Range<usize>, f32)>,
        shimmer: bool,
        lit: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let open = self.thought_open(index);
        let writing = self.writing(index);
        let (hover, ring) = (theme.fg_dim, theme.accent);
        // Closed, reasoning being written says what it is about now: the title
        // of its last part, else its last line (`timeline.js`, `latestGist`).
        let gist = match self.thread.entries().get(index) {
            Some(Entry::Thought(text)) if writing => latest_gist(text),
            _ => markdown::first_line(blocks).unwrap_or("").to_owned(),
        };
        // While it shimmers, the label is painted over the transcript by its
        // light; here it keeps its room and says where it is.
        let who: AnyElement = if shimmer {
            label("Thinking", gpui::transparent_black(), theme)
                .relative()
                .child(mark_spot(&self.spots, (index, 1)))
                .into_any_element()
        } else {
            label("Thinking", theme.fg_muted, theme).into_any_element()
        };
        let fold = div()
            .id("fold")
            .debug_selector(|| format!("thought-{index}"))
            .h(px(META))
            .w_full()
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(metric::RADIUS))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .tab_index(0)
            .hover(move |style| style.text_color(hover))
            .focus_visible(move |style| style.border_color(ring))
            .text_color(theme.fg_muted)
            .on_click(cx.listener(move |this, click, window, cx| {
                this.toggle(index, !open, cx);
                this.rest_focus(click, window, cx);
            }))
            .child(who)
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .italic()
                    .child(scale::text(gist.replace("**", ""))),
            )
            .child(chevron(open, theme));
        let content = div().w_full().child(fold).children(open.then(|| {
            // `.prose.thought`: a dashed rule beside it, a step back from the answer.
            div()
                .mt(px(8.))
                .pl(px(14.))
                .border_l_1()
                .border_dashed()
                .border_color(theme.line_strong)
                .child(prose::document(
                    blocks,
                    &Look::new(theme, Tone::reasoning(theme), self.field_focus())
                        .small()
                        .with_fades(fades),
                ))
        }));
        let node = if writing {
            Node::Filled(theme.working)
        } else {
            Node::Hollow(theme.fg_faint)
        };
        row(self::node(node, lit, theme), META, content)
    }

    /// A tool call on the rail: its node, and its card.
    #[allow(clippy::too_many_arguments)]
    fn tool(
        &self,
        index: usize,
        call: &ToolCall,
        pieces: &Rc<[Piece]>,
        light: bool,
        lit: f32,
        now: Instant,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let asking = self.asking(index).is_some();
        let level = self.unfold_level(index, self.tool_open(index, call), now);
        let card = self.tool_card(index, call, pieces, light, lit, now, level, theme, cx);
        // A call that starts a subagent says how the subagent goes: it may
        // run on after the call that started it is over.
        let (asking, status) = match call.agent {
            Some(_) => match self.subagent_state(index, call) {
                NestedState::Asking => (true, call.status),
                NestedState::Running => (false, ToolStatus::Running),
                NestedState::Failed => (false, ToolStatus::Failed),
                NestedState::Finished | NestedState::Gone => (false, ToolStatus::Done),
            },
            // A command left running in the background runs on too.
            None if runs_behind(call) => (asking, ToolStatus::Running),
            None => (asking, call.status),
        };
        // (What runs in the background does not blink: it may run for hours.)
        let running = status == ToolStatus::Running && !asking && !runs_behind(call);
        let node = match (asking, status) {
            (true, _) => Node::Filled(theme.blocked),
            (false, ToolStatus::Running) => Node::Filled(theme.working),
            (false, ToolStatus::Failed) => Node::Hollow(theme.error),
            _ => Node::Hollow(theme.fg_faint),
        };
        let lit = if running { lit } else { 1. };
        row(self::node(node, lit, theme), HEAD, card)
    }

    /// A tool call's card (`.tool`): its head says what was run, as a
    /// command after a quiet `$`, and how it went; open, what it gave is
    /// under a rule in the same card, shown down to `level` of its height
    /// (0 to 1) while the card opens or closes.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tool_card(
        &self,
        index: usize,
        call: &ToolCall,
        pieces: &Rc<[Piece]>,
        light: bool,
        lit: f32,
        now: Instant,
        level: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        // A call that starts a subagent is that agent's card.
        if call.agent.is_some() {
            return self.agent_card(index, call, pieces, light, lit, now, level, theme, cx);
        }
        let asking = self.asking(index);
        let open = self.tool_open(index, call);
        let expandable =
            !call.content.is_empty() || call.raw_input.is_some() || call.raw_output.is_some();
        let running = call.status == ToolStatus::Running && asking.is_none();
        // Its call is over and its command runs on, in the background.
        let behind = runs_behind(call);
        let (verb, input) = split_title(call);
        let shell = call.kind == ToolKind::Execute;
        let input = if shell {
            input.trim_matches('`').to_owned()
        } else {
            input
        };
        let (added, removed) = pieces
            .iter()
            .fold((0, 0), |(added, removed), piece| match piece {
                Piece::Diff { diff, .. } => (added + diff.added, removed + diff.removed),
                _ => (added, removed),
            });
        // The first place the call touches, unless what it was given names it.
        let location = call.locations.first().and_then(|location| {
            let path = display_path(&location.path, Some(&self.header.cwd));
            let named = input.contains(path.as_str());
            match location.line {
                Some(line) if named => Some(SharedString::from(format!("line {line}"))),
                Some(line) => Some(SharedString::from(format!("{path}:{line}"))),
                None if named => None,
                None => Some(path),
            }
        });
        let exit = pieces.iter().find_map(|piece| match piece {
            Piece::Terminal { exit, .. } => *exit,
            _ => None,
        });
        // How long it ran, when the view watched it: live while it runs.
        let took = match (
            self.motion.started.get(&index),
            self.motion.took.get(&index),
        ) {
            (Some(at), _) if running => Some(now.saturating_duration_since(*at)),
            // In the background: for how long it has been there, when the
            // view saw it start. (How long its call took says nothing.)
            _ if behind => self
                .nest
                .seen
                .get(&index)
                .map(|at| now.saturating_duration_since(*at)),
            _ if call.work.background => None,
            (_, Some(took)) => Some(*took),
            _ => None,
        };
        let word = match (asking.is_some(), call.status) {
            (true, _) => Some(("Needs you", theme.blocked)),
            (false, ToolStatus::Failed) => Some(("Failed", theme.error)),
            (false, ToolStatus::Cancelled) => Some(("Stopped", theme.fg_muted)),
            (false, ToolStatus::Pending) => Some(("Queued", theme.fg_muted)),
            _ if behind => Some(("In background", theme.working)),
            // It ran there and has ended: what its card holds is what its
            // call said as it left, not what it printed.
            _ if call.work.background && call.work.ended => {
                Some(("Ran in background", theme.fg_muted))
            }
            _ => None,
        };

        let mut place = meta(theme)
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis_start();
        // The path gives way before what the call was given does.
        place.style().flex_shrink = Some(1000.);

        let hover = theme.hover;
        let ring = theme.accent;
        let head = div()
            .id("call")
            .debug_selector(|| format!("tool-{index}"))
            .h(px(HEAD))
            .pl(px(10.))
            .pr(px(6.))
            .flex()
            .items_center()
            .gap(px(10.))
            .border_1()
            .border_color(gpui::transparent_black())
            .rounded(px(metric::RADIUS_MEDIUM - 1.))
            .font_family(theme.mono_font.clone())
            .text_size(px(12.))
            .child(if behind {
                state_glyph(ToolStatus::Running, false, 1., theme)
            } else {
                state_glyph(call.status, asking.is_some(), lit, theme)
            })
            .child(label(verb, theme.fg_dim, theme))
            .child(
                // `.tinput.cmd::before`: a command after a quiet `$`.
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .flex()
                    .items_center()
                    .when(shell, |input_box| {
                        input_box.child(
                            div()
                                .flex_none()
                                .text_color(theme.fg_faint)
                                .child(scale::text("$\u{a0}")),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.fg)
                            .child(scale::text(input.lines().next().unwrap_or("").to_owned())),
                    ),
            )
            .children(location.map(|location| place.child(scale::text(location))))
            .child(div().flex_1())
            .child(
                // `.tmeta`: what changed, how long it took, how it ended.
                meta(theme)
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(added + removed > 0, |stats| {
                        stats.child(
                            div()
                                .flex()
                                .gap(px(5.))
                                .when(added > 0, |stats| {
                                    stats.child(
                                        div()
                                            .text_color(theme.ok)
                                            .child(scale::text(format!("+{added}"))),
                                    )
                                })
                                .when(removed > 0, |stats| {
                                    stats.child(
                                        div()
                                            .text_color(theme.error)
                                            .child(scale::text(format!("-{removed}"))),
                                    )
                                }),
                        )
                    })
                    .children(took.map(|took| {
                        div()
                            .text_color(if running || behind {
                                theme.accent
                            } else {
                                theme.fg_muted
                            })
                            .child(scale::text(if behind {
                                // To the second it would be drawn again
                                // each second for as long as it runs.
                                crate::nested::ran_for(took)
                            } else {
                                motion::duration(took)
                            }))
                    }))
                    .children(exit.filter(|code| *code != 0).map(|code| {
                        div()
                            .text_color(theme.blocked)
                            .child(scale::text(format!("Exit {code}")))
                    }))
                    .children(word.map(|(word, color)| {
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(10.))
                            .text_color(color)
                            .child(scale::text(word.to_uppercase()))
                    })),
            )
            .when(expandable, |head| {
                head.cursor_pointer()
                    .tab_index(0)
                    .hover(move |style| style.bg(hover))
                    .focus_visible(move |style| style.border_color(ring))
                    .on_click(cx.listener(move |this, click, window, cx| {
                        this.toggle(index, !open, cx);
                        this.rest_focus(click, window, cx);
                    }))
                    .child(chevron(open, theme))
            });

        let body = ((open || level > 0.) && expandable).then(|| {
            let shown = self.tool_shown(index, call, pieces, running, false, theme, cx);
            // `.tool-body`: under a rule, on the colour behind code.
            let content = div()
                .w_full()
                .border_t_1()
                .border_color(theme.line)
                .bg(theme.code)
                .child(exact::column(
                    div().w_full().flex().flex_col(),
                    shown.into_iter().enumerate().map(|(at, piece)| {
                        div()
                            .w_full()
                            .when(at > 0, |piece| piece.border_t_1().border_color(theme.line))
                            .child(piece)
                            .into_any_element()
                    }),
                ));
            revealed((index, 0), level, 0., &self.unfold_heights, content)
        });
        // Closed, a call that runs shows the last lines its command printed
        // (`.tool-live`).
        let live = (!open && level <= 0. && running)
            .then(|| {
                pieces.iter().find_map(|piece| match piece {
                    Piece::Terminal { lines, .. } if !lines.is_empty() => Some(lines),
                    _ => None,
                })
            })
            .flatten()
            .map(|lines| {
                let tail = &lines[lines.len().saturating_sub(LIVE_TAIL)..];
                tail.iter()
                    .map(|line| line.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .map(|text| {
                div()
                    .w_full()
                    .pl(px(34.))
                    .pr(px(12.))
                    .pt(px(6.))
                    .pb(px(8.))
                    .border_t_1()
                    .border_dashed()
                    .border_color(theme.line)
                    .bg(theme.code)
                    .font_family(theme.mono_font.clone())
                    .text_size(px(11.5))
                    .line_height(px(17.))
                    .text_color(theme.fg_muted)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(scale::text(text))
            });

        let answer = asking.map(|pending| self.answer_bar(pending, theme, cx));

        // `.tool`: a card; its border says how the call goes.
        let border = if asking.is_some() {
            lights_mix(theme.blocked, theme.line, 0.45)
        } else {
            match call.status {
                ToolStatus::Running => lights_mix(theme.working, theme.line, 0.5),
                ToolStatus::Failed => lights_mix(theme.error, theme.line, 0.45),
                _ if behind => lights_mix(theme.working, theme.line, 0.5),
                _ => theme.line,
            }
        };
        div()
            .relative()
            .w_full()
            .rounded(px(metric::RADIUS_MEDIUM))
            .border_1()
            .border_color(border)
            .bg(theme.raised)
            .overflow_hidden()
            .child(head)
            .children(body)
            .children(live)
            .children(answer)
            // Its time counts while it is on screen, turn or no turn.
            .children(behind.then(|| self.counting(cx)))
            // Its light, drawn over the transcript, goes round where it is.
            .children(light.then(|| mark_spot(&self.spots, (index, 0))))
    }

    /// The answers to a call that waits for the user's (`.tool-ask`): a
    /// button for each, and the key that gives the usual one.
    fn answer_bar(&self, pending: &PendingPermission, theme: &Theme, cx: &Context<Self>) -> Div {
        let default = default_option(&pending.options);
        let request = pending.request;
        div()
            .w_full()
            .px(px(12.))
            .py(px(10.))
            .border_t_1()
            .border_color(theme.blocked.opacity(0.35))
            .bg(theme.blocked.opacity(0.06))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .children(pending.options.iter().enumerate().map(|(at, option)| {
                let id = option.id.clone();
                answer_button(at, option.label.clone(), Some(at) == default, theme)
                    .debug_selector(|| format!("answer-{}", option.id))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.permit(request, id.clone(), cx);
                        // The buttons are gone with the answer.
                        this.focus_field(window, cx);
                    }))
            }))
            .children(default.map(|at| {
                div()
                    .ml(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .font_family(theme.mono_font.clone())
                    .text_size(px(10.5))
                    .text_color(theme.fg_muted)
                    .child(key_hint(allow_key(), theme))
                    .child(scale::text(pending.options[at].label.to_lowercase()))
            }))
    }

    /// The card of a call that starts a subagent: an agent's card, as the
    /// agents started in panes have them, not a tool call's. Its head has
    /// the harness's tile, what the agent was started for, its kind, how
    /// many calls it has made, how long it has been at it, and how it goes
    /// in a word; under the head, while the card is closed, its latest on
    /// one line (what it is doing, what it said last, what it reported).
    /// Open, the card holds what it was asked, what it did and said, and
    /// what it reported.
    #[allow(clippy::too_many_arguments)]
    fn agent_card(
        &self,
        index: usize,
        call: &ToolCall,
        pieces: &Rc<[Piece]>,
        light: bool,
        lit: f32,
        now: Instant,
        level: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let asking = self.asking(index);
        let open = self.tool_open(index, call);
        let state = self.subagent_state(index, call);
        // Started and left to run: its call is over, and it is not.
        let behind = call.work.background && call.status == ToolStatus::Done;
        let (word, tone) = match state {
            NestedState::Running if behind => ("In background", theme.working),
            NestedState::Running => ("Running", theme.working),
            NestedState::Asking => ("Needs you", theme.blocked),
            NestedState::Finished => ("Done", theme.fg_muted),
            NestedState::Gone => ("Stopped", theme.fg_muted),
            NestedState::Failed => ("Failed", theme.error),
        };
        // How long it has been at it while it works; how long it took, of
        // one whose call ended with it. (When one left in the background
        // ended is not told.)
        let took = if state.live() {
            self.motion
                .started
                .get(&index)
                .or_else(|| self.nest.seen.get(&index))
                .map(|at| now.saturating_duration_since(*at))
        } else if call.work.background {
            None
        } else {
            self.motion.took.get(&index).copied()
        };
        let kind = call.agent.as_deref().unwrap_or_default();
        let mut about: Vec<String> = Vec::new();
        match call.work.steps {
            0 => {}
            1 => about.push("1 step".to_owned()),
            steps => about.push(format!("{steps} steps")),
        }
        if let Some(tokens) = call.work.tokens {
            about.push(format!("{} tokens", crate::nested::tokens(tokens)));
        }
        let (hover, ring) = (theme.hover, theme.accent);
        let code = self.header.code.to_string();
        let head = div()
            .id("call")
            .debug_selector(|| format!("tool-{index}"))
            .h(px(HEAD))
            .pl(px(10.))
            .pr(px(6.))
            .flex()
            .items_center()
            .gap(px(10.))
            .border_1()
            .border_color(gpui::transparent_black())
            .rounded(px(metric::RADIUS_MEDIUM - 1.))
            .cursor_pointer()
            .tab_index(0)
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener(move |this, click, window, cx| {
                this.toggle(index, !open, cx);
                this.rest_focus(click, window, cx);
            }))
            .child(kou_ui::monogram_sized(
                &code,
                theme.hue(&code),
                metric::SLOT - 2.,
                theme,
            ))
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .truncate()
                    .text_size(px(13.))
                    .text_color(theme.fg)
                    .child(scale::text(agent_title(call))),
            )
            .children((!kind.is_empty()).then(|| {
                let mut kind_box = meta(theme).min_w_0().truncate();
                // Its kind gives way before what it was started for does.
                kind_box.style().flex_shrink = Some(1000.);
                kind_box.child(scale::text(kind.to_owned()))
            }))
            .child(div().flex_1())
            .children((!about.is_empty()).then(|| {
                // Its counts give way before what it was started for does,
                // after its kind.
                let mut counts = meta(theme).min_w_0().truncate();
                counts.style().flex_shrink = Some(500.);
                counts.child(scale::text(about.join(" · ")))
            }))
            .child(
                meta(theme)
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .children(took.map(|took| {
                        div()
                            .text_color(if state.live() {
                                theme.accent
                            } else {
                                theme.fg_muted
                            })
                            .child(scale::text(motion::duration(took)))
                    }))
                    .child(
                        div()
                            .debug_selector(|| format!("agent-state-{index}"))
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(10.))
                            .text_color(tone)
                            .child(scale::text(word.to_uppercase())),
                    ),
            )
            .child(chevron(open, theme));

        let body = (open || level > 0.).then(|| {
            let shown = self.agent_shown(index, call, pieces, lit, theme);
            let content = div()
                .w_full()
                .border_t_1()
                .border_color(theme.line)
                .bg(theme.code)
                .child(exact::column(
                    div().w_full().flex().flex_col(),
                    shown.into_iter().enumerate().map(|(at, piece)| {
                        div()
                            .w_full()
                            .when(at > 0, |piece| piece.border_t_1().border_color(theme.line))
                            .child(piece)
                            .into_any_element()
                    }),
                ));
            revealed((index, 0), level, 0., &self.unfold_heights, content)
        });
        // Closed, its latest on one line, under its title.
        let latest = (!open && level <= 0.)
            .then(|| self.subagent_detail(call, state))
            .filter(|latest| !latest.is_empty())
            .map(|latest| {
                div()
                    .debug_selector(|| format!("agent-latest-{index}"))
                    .pl(px(36.))
                    .pr(px(12.))
                    .pb(px(9.))
                    .mt(px(-4.))
                    .truncate()
                    .font_family(theme.mono_font.clone())
                    .text_size(px(11.5))
                    .text_color(theme.fg_muted)
                    .child(scale::text(latest))
            });
        let answer = asking.map(|pending| self.answer_bar(pending, theme, cx));
        let border = match state {
            NestedState::Asking => lights_mix(theme.blocked, theme.line, 0.45),
            NestedState::Running => lights_mix(theme.working, theme.line, 0.5),
            NestedState::Failed => lights_mix(theme.error, theme.line, 0.45),
            NestedState::Finished | NestedState::Gone => theme.line,
        };
        div()
            .relative()
            .w_full()
            .rounded(px(metric::RADIUS_MEDIUM))
            .border_1()
            .border_color(border)
            .bg(theme.raised)
            .overflow_hidden()
            .child(head)
            .children(latest)
            .children(body)
            .children(answer)
            // Its light, drawn over the transcript, goes round where it is.
            .children(light.then(|| mark_spot(&self.spots, (index, 0))))
    }

    /// What an open tool call shows of its content, piece by piece: its
    /// text, its changes, its output; or what went in and came back, as the
    /// backend wrote them; or that there is nothing yet.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tool_shown(
        &self,
        index: usize,
        call: &ToolCall,
        pieces: &Rc<[Piece]>,
        running: bool,
        studio: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        // In the call's card already: code is its lines, without a frame.
        let look = Look::new(theme, Tone::output(theme), self.field_focus()).bare();
        let look = if studio { look.studio() } else { look }.small();
        let mut shown: Vec<AnyElement> = Vec::new();
        for (at, piece) in pieces.iter().enumerate() {
            match piece {
                Piece::Document(blocks) => shown.push(
                    div()
                        .px(px(12.))
                        .py(px(10.))
                        .child(prose::document(blocks, &look))
                        .into_any_element(),
                ),
                Piece::Diff { path, diff } => {
                    shown.push(diff_block(at, path, diff, theme).into_any_element())
                }
                Piece::Terminal { lines, exit } => shown.push(
                    self.terminal_block(index, at, lines, *exit, call.status, theme, cx)
                        .into_any_element(),
                ),
                Piece::Picture => {
                    shown.push(self.tool_picture(index, at, call, theme, cx));
                }
                Piece::Folded => {}
            }
        }
        if pieces.is_empty() {
            // No content of its own: what went in and what came back, as written.
            for (at, (label, value)) in [("input", &call.raw_input), ("result", &call.raw_output)]
                .into_iter()
                .enumerate()
            {
                if let Some(value) = value {
                    shown.push(raw_block(at, label, value, theme).into_any_element());
                }
            }
        }
        if shown.is_empty() {
            shown.push(
                meta(theme)
                    .px(px(12.))
                    .py(px(10.))
                    .child(scale::text(if running {
                        "Waiting for output"
                    } else {
                        "No output"
                    }))
                    .into_any_element(),
            );
        }
        shown
    }

    /// Whether row `index` is a call a subagent made: it is shown in the card
    /// of the call that started that subagent, not as a row of its own.
    pub(crate) fn made_by_subagent(&self, index: usize) -> bool {
        let entries = self.thread.entries();
        let Some(Entry::Tool(call)) = entries.get(index) else {
            return false;
        };
        call.parent
            .as_deref()
            .and_then(|parent| self.thread.tool_entry(parent))
            .is_some_and(|at| matches!(&entries[at], Entry::Tool(parent) if parent.agent.is_some()))
    }

    /// Whether row `index` shows nothing of its own because its call is a
    /// subagent's, kept in that agent's card. One that waits for the user's
    /// answer is not: it stands in the transcript, with its answers, for as
    /// long as it asks.
    pub(crate) fn kept_in_its_agent(&self, index: usize) -> bool {
        self.made_by_subagent(index) && self.asking(index).is_none()
    }

    /// How many steps a run of work shows: what a subagent did is counted in
    /// its card.
    pub(crate) fn steps_of(&self, run: &crate::nested::Run) -> usize {
        (run.start..run.end)
            .filter(|row| !self.made_by_subagent(*row))
            .count()
    }

    /// Whether the turn entry `index` is of still runs: no prompt came after
    /// it, and the agent is at work.
    fn turn_runs_at(&self, index: usize) -> bool {
        let entries = self.thread.entries();
        matches!(
            self.thread.state(),
            ThreadState::Working | ThreadState::Blocked
        ) && !entries[(index + 1).min(entries.len())..]
            .iter()
            .any(|entry| matches!(entry, Entry::User(_)))
    }

    /// How the subagent started by the call at `index` is doing. One started
    /// in the background runs on after its call is over: the agent's turn is
    /// held open until such agents are done, so it counts as running for as
    /// long as that turn runs.
    pub(crate) fn subagent_state(&self, index: usize, call: &ToolCall) -> NestedState {
        let entries = self.thread.entries();
        let asks = self.asking(index).is_some()
            || self.thread.permissions().iter().any(|pending| {
                matches!(
                    entries.get(pending.entry),
                    Some(Entry::Tool(step)) if step.parent.as_deref() == Some(call.id.as_str())
                )
            });
        match call.status {
            _ if asks => NestedState::Asking,
            ToolStatus::Pending | ToolStatus::Running => NestedState::Running,
            ToolStatus::Failed => NestedState::Failed,
            ToolStatus::Cancelled => NestedState::Gone,
            ToolStatus::Done if call.work.background && self.turn_runs_at(index) => {
                NestedState::Running
            }
            ToolStatus::Done => NestedState::Finished,
        }
    }

    /// The latest of a subagent on one line: what it is doing or has just
    /// said while it works, and the first line of its report once it is
    /// done. Empty when there is nothing to say yet.
    pub(crate) fn subagent_detail(&self, call: &ToolCall, state: NestedState) -> String {
        let said = call
            .work
            .said
            .last()
            .map(|note| (note.after, last_line(&note.text)))
            .filter(|(_, line)| !line.is_empty());
        let step = || {
            self.subagent_calls(&call.id).last().map(|step| {
                let (verb, input) = step_words(step);
                format!("{verb} {input}").trim().to_owned()
            })
        };
        if state.live() {
            // Its words, when they are the latest of it; else its last call.
            return match said {
                Some((after, line)) if after >= call.work.steps => line,
                said => step()
                    .filter(|step| !step.is_empty())
                    .or(said.map(|(_, line)| line))
                    .unwrap_or_default(),
            };
        }
        // Done: what it reported, unless it was left to run in the
        // background, whose call only says that it was started.
        let reported = (!call.work.background)
            .then(|| {
                call.content.iter().find_map(|content| match content {
                    ToolContent::Text(text) => Some(plain_line(text)),
                    _ => None,
                })
            })
            .flatten()
            .filter(|line| !line.is_empty());
        reported
            .or(said.map(|(_, line)| line))
            .or_else(step)
            .unwrap_or_default()
    }

    /// The agents the conversation's agent started by itself (its
    /// subagents), by the entries of the calls that started them, as the
    /// cards and the rows of agents started in panes are told.
    pub(crate) fn native_agents(&self, now: Instant) -> Vec<(usize, crate::nested::Nested)> {
        let wall = self.wall_now();
        self.thread
            .entries()
            .iter()
            .enumerate()
            .skip(self.cleared)
            .filter_map(|(index, entry)| {
                let Entry::Tool(call) = entry else {
                    return None;
                };
                call.agent.as_ref()?;
                let state = self.subagent_state(index, call);
                let since = self
                    .motion
                    .started
                    .get(&index)
                    .or_else(|| self.nest.seen.get(&index))
                    .map(|at| now.saturating_duration_since(*at))
                    .and_then(|ago| wall.checked_sub(ago))
                    .unwrap_or(wall);
                Some((
                    index,
                    crate::nested::Nested {
                        id: format!("call:{}", call.id),
                        title: agent_title(call),
                        harness: String::new(),
                        code: self.header.code.to_string(),
                        state,
                        // (Its card says its latest; a row of it does not.)
                        detail: String::new(),
                        since,
                        after: Some(index + 1),
                    },
                ))
            })
            .collect()
    }

    /// The commands the agent left running in the background that still
    /// run, by the entries of their calls, as rows of what works for it are
    /// told: the command for a title.
    pub(crate) fn background_commands(&self, now: Instant) -> Vec<(usize, crate::nested::Nested)> {
        let wall = self.wall_now();
        self.thread
            .entries()
            .iter()
            .enumerate()
            .skip(self.cleared)
            .filter_map(|(index, entry)| {
                let Entry::Tool(call) = entry else {
                    return None;
                };
                if !runs_behind(call) {
                    return None;
                }
                let (_, input) = step_words(call);
                let since = self
                    .nest
                    .seen
                    .get(&index)
                    .map(|at| now.saturating_duration_since(*at))
                    .and_then(|ago| wall.checked_sub(ago))
                    .unwrap_or(wall);
                Some((
                    index,
                    crate::nested::Nested {
                        id: format!("call:{}", call.id),
                        title: if input.is_empty() {
                            "Command".to_owned()
                        } else {
                            input
                        },
                        harness: "shell".to_owned(),
                        code: String::new(),
                        state: NestedState::Running,
                        detail: String::new(),
                        since,
                        after: Some(index + 1),
                    },
                ))
            })
            .collect()
    }

    /// Bring the card of the agent that the call at `index` started into
    /// view, open.
    pub(crate) fn show_agent(&mut self, index: usize, cx: &mut Context<Self>) {
        if !matches!(self.thread.entries().get(index), Some(Entry::Tool(_))) {
            return;
        }
        self.list.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: gpui::Pixels::ZERO,
        });
        self.toggle(index, true, cx);
    }

    /// The calls made by the subagent that call `id` started, in order.
    pub(crate) fn subagent_calls(&self, id: &str) -> Vec<&ToolCall> {
        let entries = self.thread.entries();
        let from = self.thread.tool_entry(id).map_or(0, |at| at + 1);
        entries[from.min(entries.len())..]
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool(call) if call.parent.as_deref() == Some(id) => Some(call),
                _ => None,
            })
            .collect()
    }

    /// What the open card of a call that starts a subagent shows: what the
    /// subagent was asked, what it has done so far, and, once it is done,
    /// what it reported.
    fn agent_shown(
        &self,
        index: usize,
        call: &ToolCall,
        pieces: &Rc<[Piece]>,
        lit: f32,
        theme: &Theme,
    ) -> Vec<AnyElement> {
        let _ = index;
        let mut shown: Vec<AnyElement> = Vec::new();
        let words = |text: String| {
            div()
                .px(px(12.))
                .pb(px(12.))
                .text_size(px(12.5))
                .line_height(px(19.))
                .text_color(theme.fg_dim)
                .child(scale::text(text))
        };
        let asked = call
            .raw_input
            .as_ref()
            .and_then(|input| input.get("prompt"))
            .and_then(|prompt| prompt.as_str())
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty());
        if let Some(asked) = asked {
            let total = asked.lines().count();
            let mut text: String = asked
                .lines()
                .take(AGENT_ASKED)
                .collect::<Vec<_>>()
                .join("\n");
            if total > AGENT_ASKED {
                text.push_str(&format!("\n({} more lines)", total - AGENT_ASKED));
            }
            shown.push(
                div()
                    .w_full()
                    .child(stream_header("Asked", theme))
                    .child(words(text))
                    .into_any_element(),
            );
        }
        let steps = self.subagent_calls(&call.id);
        let done = matches!(
            call.status,
            ToolStatus::Done | ToolStatus::Failed | ToolStatus::Cancelled
        ) && !call.work.background;
        // What it said is told where it said it, between its calls. Its last
        // words, once it is done, are its report, which stands below.
        let notes: Vec<&kou_thread::AgentNote> = call
            .work
            .said
            .iter()
            .filter(|note| !note.text.trim().is_empty())
            .filter(|note| !(done && note.after >= steps.len()))
            .collect();
        if !steps.is_empty() || !notes.is_empty() {
            let earlier = steps.len().saturating_sub(AGENT_STEPS);
            let name = match steps.len() {
                0 => "Said".to_owned(),
                1 => "Did · 1 step".to_owned(),
                steps => format!("Did · {steps} steps"),
            };
            let mut block = div().w_full().pb(px(8.)).child(stream_header(name, theme));
            if earlier > 0 {
                block = block.child(
                    meta(theme)
                        .px(px(12.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .child(scale::text(if earlier == 1 {
                            "1 earlier step".to_owned()
                        } else {
                            format!("{earlier} earlier steps")
                        })),
                );
            }
            let look = Look::new(theme, Tone::output(theme), self.field_focus())
                .bare()
                .small();
            let said = |after: usize| {
                notes
                    .iter()
                    .filter(move |note| note.after == after)
                    .map(|note| {
                        div()
                            .w_full()
                            .pl(px(34.))
                            .pr(px(12.))
                            .py(px(4.))
                            .child(prose::document(&markdown::parse(note.text.trim()), &look))
                    })
                    .collect::<Vec<_>>()
            };
            for (at, step) in steps.iter().enumerate().skip(earlier) {
                block = block.children(said(at));
                let (verb, input) = step_words(step);
                let lit = if step.status == ToolStatus::Running {
                    lit
                } else {
                    1.
                };
                block = block.child(
                    div()
                        .h(px(24.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .font_family(theme.mono_font.clone())
                        .text_size(px(12.))
                        .child(state_glyph(step.status, false, lit, theme))
                        .child(label(verb, theme.fg_dim, theme))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.fg_dim)
                                .child(scale::text(input)),
                        ),
                );
            }
            // What it has said since its last call.
            block = block.children(notes.iter().filter(|note| note.after >= steps.len()).map(
                |note| {
                    div()
                        .w_full()
                        .pl(px(34.))
                        .pr(px(12.))
                        .py(px(4.))
                        .child(prose::document(&markdown::parse(note.text.trim()), &look))
                },
            ));
            shown.push(block.into_any_element());
        }
        // Until it is done the call's content is what it was asked again;
        // of one left to run in the background, that it was started.
        if (done || asked.is_none()) && !call.work.background {
            let look = Look::new(theme, Tone::output(theme), self.field_focus())
                .bare()
                .small();
            for piece in pieces.iter() {
                if let Piece::Document(blocks) = piece {
                    shown.push(
                        div()
                            .w_full()
                            .child(stream_header(
                                if done { "Reported" } else { "Asked" },
                                theme,
                            ))
                            .child(
                                div()
                                    .px(px(12.))
                                    .pb(px(12.))
                                    .child(prose::document(blocks, &look)),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        if shown.is_empty() {
            shown.push(
                meta(theme)
                    .px(px(12.))
                    .py(px(10.))
                    .child(scale::text("Nothing to show yet"))
                    .into_any_element(),
            );
        }
        shown
    }

    /// Starts decoding the pictures an open tool call gave back, the first
    /// time its card shows them.
    pub(crate) fn tool_shots(&mut self, index: usize, cx: &mut Context<Self>) {
        let wanted: Vec<(usize, std::sync::Arc<[u8]>)> =
            match (self.thread.entries().get(index), self.cache.get(index)) {
                (Some(Entry::Tool(call)), Some(Some(Parsed::Tool(pieces)))) => pieces
                    .iter()
                    .enumerate()
                    .filter(|(_, piece)| matches!(piece, Piece::Picture))
                    .filter_map(|(at, _)| match call.content.get(at) {
                        Some(ToolContent::Image { data, .. }) => Some((at, data.clone())),
                        _ => None,
                    })
                    .collect(),
                _ => return,
            };
        for (at, data) in wanted {
            self.shot(index, at, &data, cx);
        }
    }

    /// A picture a tool gave back, in its card: as large as it is within
    /// the room a card gives one, whole and in its own shape; pressed, it
    /// is shown large over the transcript, as a message's picture is.
    fn tool_picture(
        &self,
        index: usize,
        at: usize,
        call: &ToolCall,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        /// The most room a picture takes in a card, unscaled.
        const ROOM: (f32, f32) = (420., 280.);
        let mime = match call.content.get(at) {
            Some(ToolContent::Image { mime, .. }) => mime.clone(),
            _ => String::new(),
        };
        let said = |text: String| {
            meta(theme)
                .px(px(12.))
                .py(px(10.))
                .child(scale::text(text))
                .into_any_element()
        };
        match self.shots.get(&(index, at)) {
            Some(Shot::Ready {
                large,
                width,
                height,
                ..
            }) => {
                let (width, height) = ((*width).max(1) as f32, (*height).max(1) as f32);
                let fit = (ROOM.0 / width).min(ROOM.1 / height).min(1.);
                let ring = theme.accent;
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .child(
                        div()
                            .id(("tool-picture", index * 64 + at.min(63)))
                            .debug_selector(move || format!("tool-picture-{index}-{at}"))
                            .w(px((width * fit).max(16.)))
                            .h(px((height * fit).max(16.)))
                            .rounded(px(metric::RADIUS))
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.line)
                            .bg(theme.pane)
                            .cursor_pointer()
                            .tab_index(0)
                            .hover(move |style| style.border_color(ring))
                            .focus_visible(move |style| style.border_color(ring))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.view_picture(index, at, cx)),
                            )
                            .child(
                                img(ImageSource::Render(large.clone()))
                                    .size_full()
                                    .object_fit(gpui::ObjectFit::Contain),
                            ),
                    )
                    .into_any_element()
            }
            Some(Shot::Failed) => said(format!("A picture ({mime}) that could not be read")),
            _ => said("Reading the picture…".to_owned()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn terminal_block(
        &self,
        index: usize,
        piece: usize,
        lines: &[terminal::Line],
        exit: Option<i32>,
        status: ToolStatus,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let unfolded = self.unfolded.contains(&(index, piece));
        let limit = if unfolded {
            TERMINAL_MAX
        } else {
            TERMINAL_TAIL
        };
        let hidden = lines.len().saturating_sub(limit);
        let shown = &lines[hidden..];

        // One text element for the whole tail: lines joined, colours as runs.
        let mut text = String::new();
        let mut runs: Vec<TextRun> = Vec::new();
        let mono = font(theme.mono_font.clone());
        let mut push = |len: usize, style: terminal::Style| {
            if len == 0 {
                return;
            }
            let mut font = mono.clone();
            if style.bold {
                font.weight = FontWeight::BOLD;
            }
            let color: Hsla = match style.color {
                Some(color) => rgb(theme.ansi.0[usize::from(color) % 16]).into(),
                None if style.dim => theme.fg_muted,
                None => theme.fg_dim,
            };
            match runs.last_mut() {
                Some(last) if last.color == color && last.font == font => last.len += len,
                _ => runs.push(TextRun {
                    len,
                    font,
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }),
            }
        };
        for (at, line) in shown.iter().enumerate() {
            let mut from = 0;
            for (range, style) in &line.spans {
                push(range.start - from, terminal::Style::default());
                push(range.len(), *style);
                from = range.end;
            }
            push(line.text.len() - from, terminal::Style::default());
            text.push_str(&line.text);
            if at + 1 < shown.len() {
                text.push('\n');
                push(1, terminal::Style::default());
            }
        }

        let (hover, ring) = (theme.hover, theme.accent);
        // How it ended is the head's to say; while it runs, the stream says so.
        let status_word = match (exit, status) {
            (None, ToolStatus::Running | ToolStatus::Pending) => {
                Some(("running".to_owned(), theme.working))
            }
            _ => None,
        };
        div()
            .w_full()
            .child(
                stream_header("Output", theme).children(status_word.map(|(word, color)| {
                    div()
                        .flex_none()
                        .text_color(color)
                        .child(scale::text(word.to_uppercase()))
                })),
            )
            .when(hidden > 0, |block| {
                let label = if unfolded {
                    format!("{hidden} earlier lines are not kept on screen")
                } else {
                    format!("Show {hidden} earlier lines")
                };
                block.child(
                    meta(theme)
                        .id(("unfold", piece))
                        .debug_selector(|| format!("unfold-{index}-{piece}"))
                        .px(px(12.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(gpui::transparent_black())
                        .when(!unfolded, |more| {
                            more.cursor_pointer()
                                .tab_index(0)
                                .hover(move |style| style.bg(hover))
                                .focus_visible(move |style| style.border_color(ring))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.unfold(index, piece, cx);
                                    this.focus_field(window, cx);
                                }))
                        })
                        .child(scale::text(label)),
                )
            })
            .child(
                div()
                    .id(("output", piece))
                    .w_full()
                    .flex()
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .child(
                        div()
                            .flex_none()
                            .px(px(12.))
                            .pb(px(12.))
                            .min_h(px(MONO_LINE + 12.))
                            .text_size(px(12.))
                            .line_height(px(19.))
                            .whitespace_nowrap()
                            .child(scale::styled_text(text, runs)),
                    ),
            )
    }

    /// The last row: what the thread is doing, when that is not the prompt
    /// area's to say, and what the user can do about it.
    fn tail(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let agent = &self.header.agent;
        let line = |text: String| {
            row(
                node(Node::Hollow(theme.fg_faint), 1., theme),
                META,
                div()
                    .min_h(px(META))
                    .flex()
                    .items_center()
                    .text_color(theme.fg_muted)
                    .child(scale::text(text)),
            )
        };
        let content = match self.thread.state() {
            ThreadState::Starting => Some(line(format!(
                "Starting {agent} in {}. Your first message can be sent once it is ready.",
                display_path(&self.header.cwd, None)
            ))),
            ThreadState::Idle if self.thread.entries().is_empty() => Some(self.ready_card(theme)),
            ThreadState::Idle => match &self.thread.last_stop {
                Some(Stop::Cancelled) => Some(line("Stopped. Send a message to go on.".to_owned())),
                Some(Stop::Limit) => Some(line(format!(
                    "{agent} stopped at its output or turn limit. Send a message to go on."
                ))),
                Some(Stop::Refused) => Some(line(
                    "The model refused to continue. Rephrase the request and send it again."
                        .to_owned(),
                )),
                // The reason is the entry above, marked as the failure; this is the way on.
                Some(Stop::Failed(_)) => Some(line("Send the message again to retry.".to_owned())),
                Some(Stop::Finished) | None => None,
            },
            // The prompt area says what the agent is doing while it works.
            ThreadState::Working => None,
            ThreadState::Blocked => {
                let waiting = self.thread.permissions().len();
                Some(line(if waiting == 1 {
                    format!("{agent} is waiting for your answer above.")
                } else {
                    format!("{agent} is waiting for your answer to {waiting} requests above.")
                }))
            }
            ThreadState::Closed(Some(reason)) => Some(row(
                node(Node::Filled(theme.error), 1., theme),
                META,
                div()
                    .flex()
                    .flex_col()
                    .child(meta_line().child(label(format!("{agent} stopped"), theme.error, theme)))
                    .child(error_box(reason, theme.error, theme))
                    .child(
                        div()
                            .mt(px(8.))
                            .text_color(theme.fg_muted)
                            .child(scale::text(
                                "Close this pane and open it again to start a new session. \
                                 The conversation above stays readable until then.",
                            )),
                    ),
            )),
            ThreadState::Closed(None) => Some(line(
                "This session has ended. Open the pane again to start a new one.".to_owned(),
            )),
        };
        let first = self.thread.entries().is_empty();
        // Before anything was said, the cards of the agents started so far.
        let cards: Vec<_> = if first {
            self.cards_of(0).0.into_iter().cloned().collect()
        } else {
            Vec::new()
        };
        let inset = self.around.inset.get();
        div()
            .w_full()
            .when(inset > 0., |row| row.pr(px(inset)))
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(MEASURE))
                    .pt(px(if first { 14. } else { 18. }))
                    // Under the last row lies the prompt area: room for it.
                    .pb(px(14. + self.dock_height))
                    .when(!cards.is_empty(), |column| {
                        column
                            .child(self.card_group(&cards, 0, 0, 0., theme, cx))
                            .child(div().h(px(14.)))
                    })
                    .children(content),
            )
    }

    /// A new session, before anything was said (`.empty-card`): ready, and
    /// what the agent does. Still: nothing here moves while nothing happens.
    fn ready_card(&self, theme: &Theme) -> Div {
        let agent = &self.header.agent;
        div().w_full().px(px(EDGE)).child(
            div()
                .w_full()
                .px(px(24.))
                .pt(px(22.))
                .pb(px(20.))
                .rounded(px(metric::RADIUS_LARGE))
                .border_1()
                .border_color(theme.line)
                .bg(theme.raised)
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.))
                        .child(label("Session · new", theme.fg_muted, theme))
                        .child(
                            meta(theme)
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis_start()
                                .child(scale::text(display_path(&self.header.cwd, None))),
                        ),
                )
                .child(
                    div()
                        .mt(px(26.))
                        .mb(px(10.))
                        .flex()
                        .items_end()
                        .font_family(theme.mono_font.clone())
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(38.))
                        .line_height(px(38.))
                        .text_color(theme.fg)
                        .child(scale::text("READY"))
                        // `.caret`: the block after it. It does not blink: the
                        // view draws nothing while nothing happens.
                        .child(
                            div()
                                .ml(px(3.))
                                .mb(px(4.))
                                .w(px(19.))
                                .h(px(28.))
                                .bg(theme.accent),
                        ),
                )
                .child(
                    div()
                        .max_w(px(520.))
                        .text_size(px(14.))
                        .line_height(px(22.))
                        .text_color(theme.fg_dim)
                        .child(scale::text(format!(
                            "{agent} works in this folder with a shell. Describe the outcome \
                             you want: it plans, runs commands and reports back as it goes."
                        ))),
                ),
        )
    }
}

/// The lights over a row: whether each is lit, and whether one was made
/// just now, to be drawn from the next frame on.
pub(crate) struct RowLights {
    pub made: bool,
    pub run: bool,
    pub shimmer: bool,
}

/// How far a streamed entry rises from, in pixels.
fn cockpit_rise_by() -> f32 {
    kou_ui::motion::cockpit::RISE_BY
}

/// `color-mix(in srgb, a share, b)`.
pub(crate) fn lights_mix(a: Hsla, b: Hsla, share: f32) -> Hsla {
    crate::lights::mix(b, a, share)
}

/// A fold's chevron: right when closed, down when open.
pub(crate) fn chevron(open: bool, theme: &Theme) -> impl IntoElement {
    icon(
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        theme.fg_faint,
    )
    .size(px(12.))
}

/// The state of a tool call, as a mark of its own shape (`.tstate`): a
/// square while it runs, blinking, a tick done, a cross failed.
fn state_glyph(status: ToolStatus, asking: bool, lit: f32, theme: &Theme) -> AnyElement {
    let cell = div()
        .flex_none()
        .w(px(14.))
        .flex()
        .items_center()
        .justify_center();
    let square = |color: Hsla, filled: bool| {
        let mark = div().size(px(8.)).rounded(px(metric::RADIUS_MARK));
        if filled {
            mark.bg(color)
        } else {
            mark.border_1().border_color(color)
        }
    };
    match (asking, status) {
        (true, _) => cell.child(square(theme.blocked, true)),
        (false, ToolStatus::Running) => cell.child(square(theme.working.opacity(lit), true)),
        (false, ToolStatus::Pending) => cell.child(square(theme.fg_muted, false)),
        (false, ToolStatus::Done) => cell.child(icon(Icon::Check, theme.ok).size(px(12.))),
        (false, ToolStatus::Failed) => cell.child(icon(Icon::Close, theme.error).size(px(12.))),
        (false, ToolStatus::Cancelled) => cell.child(
            div()
                .size(px(9.))
                .rounded_full()
                .border_1()
                .border_color(theme.fg_muted),
        ),
    }
    .into_any_element()
}

/// One of the answers to a permission request: the default one filled with
/// the accent (`.primary`), the others framed (`.act`).
fn answer_button(at: usize, text: String, primary: bool, theme: &Theme) -> gpui::Stateful<Div> {
    let ring = theme.accent;
    let base = div()
        .id(("answer", at))
        .flex_none()
        .h(px(28.))
        .px(px(12.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .flex()
        .items_center()
        .cursor_pointer()
        .tab_index(0)
        .font_family(theme.mono_font.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(10.5))
        .child(scale::text(text.to_uppercase()));
    if primary {
        // The one primary action, as the app's primary buttons are.
        let (ink, fill) = (theme.primary_ink, lights_mix(theme.primary, theme.fg, 0.15));
        base.bg(theme.primary)
            .border_color(theme.primary_rim)
            .text_color(ink)
            .hover(move |style| style.bg(fill))
            .active(|style| style.opacity(0.85))
            .focus_visible(move |style| style.border_color(ink))
    } else {
        let (frame, lit, fill) = (theme.fg_faint, theme.fg, theme.pane);
        base.border_color(theme.line_strong)
            .text_color(theme.fg_dim)
            .hover(move |style| style.border_color(frame).text_color(lit).bg(fill))
            .active(|style| style.opacity(0.85))
            .focus_visible(move |style| style.border_color(ring))
    }
}

/// A small control that appears with the pointer over its entry and with the
/// keyboard on it (`.actions`): copy, edit.
fn row_action(
    id: (&'static str, usize),
    tip: &'static str,
    group: SharedString,
    theme: &Theme,
) -> gpui::Stateful<Div> {
    let name = id.0;
    let index = id.1;
    act_revealed(id, tip, group, theme).debug_selector(move || format!("{name}-{index}"))
}

impl ThreadView {
    /// A message of the user's (`.entry[data-kind="user"]`): who, the model it
    /// went to, then the text in a card of its own with the pictures' labels in
    /// place, the pictures small under it, the files it brought.
    #[allow(clippy::too_many_arguments)]
    fn user(
        &self,
        index: usize,
        message: &UserMessage,
        shots: &[Shot],
        now: Instant,
        theme: &Theme,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let group = SharedString::from(format!("user-{index}"));
        // Being edited: the editor stands where the prompt stood, under a
        // header that says so (`.entry[data-editing]`).
        if let Some(editor) = self.editor.as_ref().filter(|editor| editor.index == index) {
            let head = meta_line()
                .child(label("You", theme.accent, theme))
                .child(Self::editing_flag(theme));
            let ring = theme.accent.opacity(0.22);
            return row(
                node(Node::Filled(theme.accent), 1., theme).shadow(vec![gpui::BoxShadow {
                    color: ring,
                    offset: gpui::point(gpui::px(0.), gpui::px(0.)),
                    blur_radius: gpui::px(0.),
                    spread_radius: px(3.),
                    inset: false,
                }]),
                META,
                div()
                    .id(("prompt", index))
                    .w_full()
                    // `.edit { margin-top: 6px }`, as the column's gap: a
                    // child of a planned column keeps no margin of its own
                    // at a zoom (see `kou_ui::scale::exact`).
                    .child(exact::column(
                        div().w_full().flex().flex_col().gap(px(6.)),
                        [
                            head.into_any_element(),
                            self.editor_element(editor, window, theme, cx),
                        ],
                    )),
            );
        }
        let mut runs = Vec::new();
        let mut at = 0;
        let base = TextRun {
            len: 0,
            font: font(theme.ui_font.clone()),
            color: theme.fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        for label in &message.labels {
            if label.start > at {
                runs.push(TextRun {
                    len: label.start - at,
                    ..base.clone()
                });
            }
            let mut bold = font(theme.ui_font.clone());
            bold.weight = FontWeight::SEMIBOLD;
            runs.push(TextRun {
                len: label.len(),
                font: bold,
                color: theme.accent,
                ..base.clone()
            });
            at = label.end;
        }
        if at < message.text.len() {
            runs.push(TextRun {
                len: message.text.len() - at,
                ..base.clone()
            });
        }
        let copied = self
            .copied
            .as_ref()
            .is_some_and(|key| key == &format!("prompt-{index}"));
        let text = message.text.to_string();
        let actions = div()
            .flex_none()
            .flex()
            .gap(px(2.))
            .child(
                row_action(
                    ("copy-prompt", index),
                    if copied { "Copied" } else { "Copy" },
                    group.clone(),
                    theme,
                )
                .when(copied, |control| control.opacity(1.).text_color(theme.ok))
                .on_click(cx.listener(move |this, click, window, cx| {
                    this.copy(format!("prompt-{index}").into(), text.clone(), cx);
                    this.rest_focus(click, window, cx);
                })),
            )
            .children(self.prompt_action(index, group.clone(), theme, cx));
        let pictures = (!message.pictures.is_empty()).then(|| {
            div().flex().flex_wrap().gap(px(8.)).pt(px(6.)).children(
                message
                    .pictures
                    .iter()
                    .zip(shots)
                    .map(|((label, block), shot)| {
                        let block = *block;
                        let (ring, line) = (theme.accent, theme.line_strong);
                        let frame = div()
                            .id(("message-picture", index * 64 + block.min(63)))
                            .debug_selector(move || format!("message-picture-{index}-{block}"))
                            .relative()
                            .w(px(136.))
                            .h(px(90.))
                            .rounded(px(metric::RADIUS_MEDIUM))
                            .overflow_hidden()
                            .border_1()
                            .border_color(line)
                            .bg(theme.code)
                            .cursor_pointer()
                            .tab_index(0)
                            .hover(move |style| style.border_color(ring))
                            .focus_visible(move |style| style.border_color(ring))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.view_picture(index, block, cx)
                            }));
                        let content: AnyElement = match shot {
                            // Zoomed in, the thumbnail would be drawn larger than it is.
                            Shot::Ready { thumb, large, .. } => img(ImageSource::Render(
                                if scale::factor() > 1. { large } else { thumb }.clone(),
                            ))
                            .size_full()
                            .object_fit(gpui::ObjectFit::Cover)
                            .into_any_element(),
                            Shot::Loading => div().into_any_element(),
                            Shot::Failed => div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(metric::TEXT_SMALL))
                                .text_color(theme.fg_muted)
                                .child(scale::text("not readable"))
                                .into_any_element(),
                        };
                        frame.child(content).child(
                            div()
                                .absolute()
                                .left_0()
                                .bottom_0()
                                .px(px(6.))
                                .rounded_tr(px(metric::RADIUS))
                                .bg(theme.raised.opacity(0.88))
                                .font_family(theme.mono_font.clone())
                                .font_weight(FontWeight::BOLD)
                                .text_size(px(10.))
                                .text_color(theme.fg_dim)
                                .child(scale::text(label.clone())),
                        )
                    }),
            )
        });
        // The model the prompt went to, when the view knows it (`.model-tag`).
        let model = self.prompt_models.get(&index).map(|id| {
            let provider = crate::models::provider_of(id, &self.thread.providers);
            div()
                .flex_none()
                .max_w(px(260.))
                .h(px(18.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(metric::RADIUS_SMALL))
                .border_1()
                .border_color(theme.line)
                .font_family(theme.mono_font.clone())
                .text_size(px(11.))
                .text_color(theme.fg_muted)
                .child(
                    div()
                        .flex_none()
                        .size(px(6.))
                        .rounded(px(metric::RADIUS_MARK))
                        .bg(theme.provider(provider.as_deref().unwrap_or_default())),
                )
                .child(div().min_w_0().truncate().child(scale::text(id.clone())))
        });
        // With less motion, a message simply arrives.
        let flash = if cx.reduce_motion() {
            None
        } else {
            self.motion.flash(index, now)
        };
        let head = meta_line()
            .child(label("You", theme.accent, theme))
            .children(model)
            .child(div().flex_1())
            .child(actions);
        let card = div()
            .id(("user", index))
            .w_full()
            .px(px(15.))
            .pt(px(10.))
            .pb(px(11.))
            .rounded(px(metric::RADIUS_LARGE))
            .border_1()
            .border_color(theme.line)
            .bg(match flash {
                Some(t) => lights_mix(theme.raised, theme.accent, 0.12 * t),
                None => theme.raised,
            })
            .text_size(px(PROSE))
            .line_height(px(LINE))
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(
                (!message.text.is_empty()).then(|| scale::styled_text(message.text.clone(), runs)),
            )
            .children(pictures)
            .children(message.files.iter().map(|path| {
                meta(theme)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(icon(Icon::File, theme.fg_muted).size(px(12.)))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.fg_dim)
                            .child(scale::text(path.clone())),
                    )
            }));
        row(
            node(Node::Filled(theme.accent), 1., theme),
            META,
            div()
                .id(("prompt", index))
                .group(group)
                .w_full()
                .child(exact::column(
                    div().w_full().flex().flex_col().gap(px(6.)),
                    [head.into_any_element(), card.into_any_element()],
                )),
        )
    }

    /// What a prompt's control does with it: Edit, for a backend that rewinds,
    /// where it stands, while the agent is free; otherwise Reuse, which puts
    /// it back in the field.
    fn prompt_action(
        &self,
        index: usize,
        group: SharedString,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        if matches!(self.thread.state(), ThreadState::Closed(_)) {
            return None;
        }
        if !self.edits() {
            return Some(
                row_action(("reuse-prompt", index), "Reuse", group, theme).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.reuse(index, window, cx);
                    },
                )),
            );
        }
        // Not while the agent works, or an edit is on its way.
        if self.edit_blocked().is_some() {
            return None;
        }
        Some(
            row_action(("edit-prompt", index), "Edit", group, theme).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.begin_edit(index, window, cx);
                },
            )),
        )
    }

    /// An answer (`.entry[data-kind="assistant"]`): who, a control to copy all
    /// of it, then the prose.
    #[allow(clippy::too_many_arguments)]
    fn answer(
        &self,
        index: usize,
        text: &str,
        blocks: &Rc<[Markdown]>,
        look: &Look,
        node: Node,
        lit: f32,
        cx: &Context<Self>,
    ) -> Div {
        let theme = look.theme;
        let group = SharedString::from(format!("answer-{index}"));
        let key = format!("answer-{index}");
        let copied = self.copied.as_ref().is_some_and(|copied| copied == &key);
        let text = text.to_owned();
        let writing = self.writing(index);
        // An answer still being written has no actions: it is not all said yet.
        let copy = (!writing).then(|| {
            row_action(
                ("copy-answer", index),
                if copied { "Copied" } else { "Copy" },
                group.clone(),
                theme,
            )
            .when(copied, |control| control.opacity(1.).text_color(theme.ok))
            .on_click(cx.listener(move |this, click, window, cx| {
                this.copy(key.clone().into(), text.clone(), cx);
                this.rest_focus(click, window, cx);
            }))
        });
        let lit = if writing { lit } else { 1. };
        row(
            self::node(node, lit, theme),
            META,
            div()
                .id(("answer", index))
                .debug_selector(move || format!("reply-{index}"))
                .group(group)
                .w_full()
                // At a zoom, the line of capitals and the prose each where
                // they stand at 1 (see `kou_ui::scale::exact`).
                .child(exact::column(
                    div().w_full().flex().flex_col().gap(px(4.)),
                    [
                        meta_line()
                            .child(label("Agent", theme.fg, theme))
                            .child(div().flex_1())
                            .children(copy)
                            .into_any_element(),
                        prose::document(blocks, look).into_any_element(),
                    ],
                )),
        )
    }

    /// What `/clear` left: how much is out of sight, and a way back to it.
    fn cleared_row(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let hidden = self.cleared;
        let notice = div()
            .min_h(px(META))
            .flex()
            .items_center()
            .gap(px(12.))
            .font_family(theme.mono_font.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(LABEL))
            .text_color(theme.fg_muted)
            .child(scale::text(format!(
                "{hidden} EARLIER {} OUT OF SIGHT",
                if hidden == 1 {
                    "ENTRY IS"
                } else {
                    "ENTRIES ARE"
                }
            )))
            .child(
                act("unclear", "Show them", theme)
                    .debug_selector(|| "unclear".into())
                    .border_color(theme.line_strong)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.unclear(cx);
                        this.focus_field(window, cx);
                    })),
            )
            .child(div().flex_1().h(px(1.)).bg(theme.line));
        div().w_full().flex().justify_center().child(
            div().w_full().max_w(px(MEASURE)).pt(px(14.)).child(row(
                node(Node::Rule, 1., theme),
                META,
                notice,
            )),
        )
    }
}

/// What reasoning being written is about now: the title of its last part (a
/// line in bold), or else its last line.
pub(crate) fn latest_gist(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    for line in lines.iter().rev() {
        let line = line.trim();
        if let Some(title) = line
            .strip_prefix("**")
            .and_then(|rest| rest.trim_end_matches(':').strip_suffix("**"))
            && !title.is_empty()
        {
            return title.to_owned();
        }
    }
    lines
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// The agent's plan: its steps, each with a mark of its own shape.
fn plan(steps: &[PlanStep], lit: f32, theme: &Theme) -> Div {
    let done = steps
        .iter()
        .filter(|step| step.status == PlanStatus::Done)
        .count();
    let running = steps.iter().any(|step| step.status == PlanStatus::Running);
    let steps_list = div()
        .mt(px(4.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .children(steps.iter().map(|step| {
            let (mark, color): (AnyElement, Hsla) = match step.status {
                PlanStatus::Pending => (
                    div()
                        .size(px(8.))
                        .rounded(px(metric::RADIUS_MARK))
                        .border_1()
                        .border_color(theme.fg_faint)
                        .into_any_element(),
                    theme.fg_dim,
                ),
                PlanStatus::Running => (
                    div()
                        .size(px(8.))
                        .rounded(px(metric::RADIUS_MARK))
                        .bg(theme.working.opacity(lit))
                        .into_any_element(),
                    theme.fg,
                ),
                PlanStatus::Done => (
                    icon(Icon::Check, theme.ok).size(px(12.)).into_any_element(),
                    theme.fg_muted,
                ),
            };
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .child(
                    div()
                        .flex_none()
                        .w(px(14.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(mark),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(13.5))
                        .line_height(px(22.))
                        .text_color(color)
                        .when(step.status == PlanStatus::Done, |text| text.line_through())
                        .child(scale::text(step.text.clone())),
                )
        }));
    row(
        node(
            if running {
                Node::Filled(theme.working)
            } else {
                Node::Hollow(theme.fg_faint)
            },
            if running { lit } else { 1. },
            theme,
        ),
        META,
        div()
            .w_full()
            .child(
                meta_line().child(label("Plan", theme.fg_dim, theme)).child(
                    meta(theme).child(scale::text(format!("{done} of {} done", steps.len()))),
                ),
            )
            .child(steps_list),
    )
}

/// A box of mono text tinted by `tone` (`.errbox`, `.resume`).
fn error_box(text: &str, tone: Hsla, theme: &Theme) -> Div {
    div()
        .mt(px(6.))
        .w_full()
        .px(px(13.))
        .py(px(10.))
        .rounded(px(metric::RADIUS_MEDIUM))
        .border_1()
        .border_color(lights_mix(tone, theme.line, 0.35))
        .bg(tone.opacity(if theme.dark { 0.09 } else { 0.07 }))
        .font_family(theme.mono_font.clone())
        .text_size(px(12.5))
        .line_height(px(19.))
        .text_color(theme.fg)
        .child(scale::text(text.to_owned()))
}

/// A message from the backend. `turn` marks the error a turn ended with.
fn notice(level: Level, text: &str, turn: bool, theme: &Theme) -> Div {
    match level {
        // `.notice`: a line in capitals, a rule after it.
        Level::Info => row(
            node(Node::Rule, 1., theme),
            META,
            div()
                .min_h(px(META))
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .min_w_0()
                        .font_family(theme.mono_font.clone())
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(LABEL))
                        .line_height(px(16.))
                        .text_color(theme.fg_muted)
                        .child(scale::text(text.to_uppercase())),
                )
                .child(div().flex_1().min_w(px(16.)).h(px(1.)).bg(theme.line)),
        ),
        Level::Warning => row(
            node(Node::Filled(theme.blocked), 1., theme),
            META,
            div()
                .w_full()
                .child(meta_line().child(label("Warning", theme.blocked, theme)))
                .child(error_box(text, theme.blocked, theme)),
        ),
        Level::Error => row(
            node(Node::Filled(theme.error), 1., theme),
            META,
            div()
                .w_full()
                .child(meta_line().child(label(
                    if turn { "Turn failed" } else { "Error" },
                    theme.error,
                    theme,
                )))
                .child(error_box(text, theme.error, theme)),
        ),
    }
}

/// A change to a file (`.fc-rows`): numbered lines, additions and removals on
/// the theme's diff colours, under a header with the path and the counts.
fn diff_block(piece: usize, path: &SharedString, diff: &Diff, theme: &Theme) -> Div {
    let digits = diff
        .rows
        .iter()
        .filter_map(|row| match row {
            diff::Row::Line(line) => line.new.or(line.old),
            diff::Row::Gap(_) => None,
        })
        .max()
        .unwrap_or(1)
        .to_string()
        .len();
    let number_width = px(7.3 * digits as f32 + 22.);
    let shown = diff.rows.len().min(DIFF_MAX);
    let rows = diff.rows[..shown].iter().map(|row| {
        let base = div().h(px(19.)).flex().items_center();
        match row {
            // Between two hunks: the lines of the file left out.
            diff::Row::Gap(count) => base
                .my(px(3.))
                .pl(px(12.))
                .bg(theme.hover)
                .text_color(theme.fg_muted)
                .child(scale::text(format!(
                    "{count} unchanged {}",
                    if *count == 1 { "line" } else { "lines" }
                ))),
            diff::Row::Line(line) => {
                let (sign, sign_color, text_color, fill) = match line.kind {
                    diff::Kind::Context => (" ", theme.fg_faint, theme.fg_dim, None),
                    diff::Kind::Added => ("+", theme.ok, theme.fg, Some(theme.diff_add)),
                    diff::Kind::Removed => ("-", theme.error, theme.fg, Some(theme.diff_del)),
                };
                base.when_some(fill, |row, fill| row.bg(fill))
                    .child(
                        div()
                            .flex_none()
                            .w(number_width)
                            .pr(px(10.))
                            .text_right()
                            .text_color(theme.fg_faint)
                            .child(
                                line.new
                                    .or(line.old)
                                    .map(|n| n.to_string())
                                    .unwrap_or_default(),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(16.))
                            .text_color(sign_color)
                            .child(scale::text(sign)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .pr(px(12.))
                            .text_color(text_color)
                            .child(scale::text(markdown::expand_tabs(&line.text))),
                    )
            }
        }
    });
    let left_out = diff.rows.len() - shown;
    div()
        .w_full()
        .child(
            stream_header("Change", theme).child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .font_weight(FontWeight::NORMAL)
                    .text_size(px(11.))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis_start()
                            .text_color(theme.fg_dim)
                            .child(scale::text(path.clone())),
                    )
                    .child(div().flex_none().child(if diff.rows.is_empty() {
                        "no changes".to_owned()
                    } else {
                        format!("+{} -{}", diff.added, diff.removed)
                    })),
            ),
        )
        .when(!diff.rows.is_empty(), |block| {
            block.child(
                div()
                    .id(("diff", piece))
                    .w_full()
                    .flex()
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .child(exact::column(
                        div()
                            .flex_none()
                            .min_w_full()
                            .pb(px(8.))
                            .flex()
                            .flex_col()
                            .font_family(theme.mono_font.clone())
                            .text_size(px(12.))
                            .line_height(px(19.))
                            .whitespace_nowrap(),
                        rows.map(IntoElement::into_any_element),
                    )),
            )
        })
        .when(left_out > 0, |block| {
            block.child(
                meta(theme)
                    .px(px(12.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_dashed()
                    .border_color(theme.line)
                    .child(scale::text(format!("{left_out} more lines are not shown"))),
            )
        })
}

/// A tool call's arguments or result as the backend gave them.
fn raw_block(
    piece: usize,
    label: &'static str,
    value: &impl std::fmt::Display,
    theme: &Theme,
) -> Div {
    let pretty = format!("{value:#}");
    let total = pretty.lines().count();
    let mut text: String = pretty.lines().take(RAW_MAX).collect::<Vec<_>>().join("\n");
    if total > RAW_MAX {
        text.push_str(&format!("\n({} more lines)", total - RAW_MAX));
    }
    div().w_full().child(stream_header(label, theme)).child(
        div()
            .id((label, piece))
            .w_full()
            .flex()
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .pb(px(12.))
                    .font_family(theme.mono_font.clone())
                    .text_size(px(12.))
                    .line_height(px(19.))
                    .text_color(theme.fg_dim)
                    .whitespace_nowrap()
                    .child(scale::text(markdown::expand_tabs(&text))),
            ),
    )
}

/// A subagent's step in two words' worth: what it does, and on what, on one
/// line; a command without the marks its title is written in.
fn step_words(step: &ToolCall) -> (String, String) {
    let (verb, input) = split_title(step);
    let input = if step.kind == ToolKind::Execute {
        input.trim().trim_matches('`')
    } else {
        input.trim()
    };
    (verb, first_line(input))
}

/// Whether `call` is a command whose call is over while its process runs
/// on: left in the background, and not said to have ended.
pub(crate) fn runs_behind(call: &ToolCall) -> bool {
    call.agent.is_none()
        && call.work.background
        && !call.work.ended
        && call.status == ToolStatus::Done
}

/// The first line of `text`, without the space around it.
fn first_line(text: &str) -> String {
    text.trim().lines().next().unwrap_or("").trim().to_owned()
}

/// The last line of `text` that says something, as plain words.
fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .map(plain_line)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// The first line of `text` that says something, without the marks markdown
/// is written with at a line's start and round its words.
fn plain_line(text: &str) -> String {
    text.lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['#', '>', '-', '*', ' '])
                .replace("**", "")
                .replace('`', "")
                .trim()
                .to_owned()
        })
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// What a subagent was started for, as its card's title: the call's own
/// words for it, or that it is an agent.
fn agent_title(call: &ToolCall) -> String {
    let asked = call
        .raw_input
        .as_ref()
        .and_then(|input| input.get("description"))
        .and_then(|description| description.as_str())
        .map(str::trim)
        .filter(|description| !description.is_empty());
    match (asked, first_line(&call.title)) {
        (Some(asked), _) => asked.to_owned(),
        (None, title) if !title.is_empty() => title,
        _ => "Agent".to_owned(),
    }
}

/// `content` shown from its top down to `level` of the way from `least` to
/// its whole height, 0 to 1: what opens or closes over a few frames. As it
/// is drawn it notes its whole height in `heights` under `key`, unscaled,
/// which is what the frames in between are cut from; at rest (`level` 1) it
/// stands as it is, and fades in on the way there.
pub(crate) fn revealed(
    key: (usize, u8),
    level: f32,
    least: f32,
    heights: &Rc<std::cell::RefCell<std::collections::HashMap<(usize, u8), f32>>>,
    content: impl IntoElement,
) -> Div {
    let factor = scale::factor();
    let noted = heights.clone();
    let measure = gpui::canvas(
        move |bounds, _, _| {
            let height = bounds.size.height.as_f32() / factor.max(0.01);
            if height.is_finite() {
                noted.borrow_mut().insert(key, height);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    if level >= 1. {
        return div().relative().w_full().child(content).child(measure);
    }
    let level = level.clamp(0., 1.);
    let whole = heights.borrow().get(&key).copied().unwrap_or(least);
    div()
        .relative()
        .w_full()
        .h(px(least + (whole - least).max(0.) * level))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .opacity(level)
                .child(content)
                .child(measure),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_command_printed_in_one_fenced_block_is_its_output_and_other_text_a_document() {
        let call = |kind: ToolKind, text: &str| ToolCall {
            id: "t".into(),
            title: "Run cargo test".into(),
            kind,
            status: ToolStatus::Done,
            content: vec![ToolContent::Text(text.into())],
            ..ToolCall::default()
        };
        let pieces = |call: ToolCall| match parse(&Entry::Tool(call), true, Path::new("/work")) {
            Some(Parsed::Tool(pieces)) => pieces,
            _ => panic!("a tool call parses to its pieces"),
        };
        // An agent hands a command's output over as one fenced block: the
        // card shows it as a terminal's output, not as a block in a block.
        let printed = pieces(call(
            ToolKind::Execute,
            "```console\ntest result: ok. 3 passed\nall done\n```",
        ));
        match &printed[0] {
            Piece::Terminal { lines, exit } => {
                let text: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
                assert_eq!(text, ["test result: ok. 3 passed", "all done"]);
                assert_eq!(*exit, None);
            }
            _ => panic!("a command's fenced output is terminal output"),
        }
        // Anything else is a document: prose with code in it, a file read.
        let mixed = pieces(call(
            ToolKind::Execute,
            "It failed:\n\n```console\nerror\n```",
        ));
        assert!(matches!(mixed[0], Piece::Document(_)));
        let read = pieces(call(ToolKind::Read, "```rust\nfn main() {}\n```"));
        assert!(matches!(read[0], Piece::Document(_)));
    }

    #[test]
    fn a_folded_tool_call_parses_only_what_its_row_shows() {
        let call = ToolCall {
            id: "t".into(),
            title: "Edit".into(),
            content: vec![
                ToolContent::Text("**done**".into()),
                ToolContent::Diff {
                    path: "/work/a.rs".into(),
                    old: Some("a\n".into()),
                    new: "b\n".into(),
                },
                ToolContent::Terminal {
                    id: "1".into(),
                    output: "x\n".into(),
                    exit: Some(0),
                },
            ],
            ..ToolCall::default()
        };
        let entry = Entry::Tool(call);
        let Some(Parsed::Tool(folded)) = parse(&entry, false, Path::new("/work")) else {
            panic!("a tool call parses");
        };
        assert!(matches!(folded[0], Piece::Folded));
        assert!(
            matches!(&folded[1], Piece::Diff { path, diff } if path == "a.rs" && diff.added == 1)
        );
        // Closed, a call still shows the last lines its command printed.
        assert!(matches!(&folded[2], Piece::Terminal { lines, .. } if lines.len() == 1));
        let Some(Parsed::Tool(open)) = parse(&entry, true, Path::new("/work")) else {
            panic!("a tool call parses");
        };
        assert!(matches!(open[0], Piece::Document(_)));
        assert!(matches!(&open[2], Piece::Terminal { lines, exit: Some(0) } if lines.len() == 1));
    }

    #[test]
    fn a_tool_call_s_title_gives_its_verb_and_what_it_was_given() {
        let call = |title: &str, kind| ToolCall {
            title: title.into(),
            kind,
            ..ToolCall::default()
        };
        assert_eq!(
            split_title(&call("Run cargo test --all", ToolKind::Execute)),
            ("Run".into(), "cargo test --all".into())
        );
        assert_eq!(
            split_title(&call("Verify: make check", ToolKind::Execute)),
            ("Verify".into(), "make check".into())
        );
        // A title without a verb of its own is named by its kind.
        assert_eq!(
            split_title(&call("`git status`", ToolKind::Execute)),
            ("run".into(), "`git status`".into())
        );
        assert_eq!(
            split_title(&call("src/main.rs", ToolKind::Read)),
            ("read".into(), "src/main.rs".into())
        );
    }

    #[test]
    fn the_gist_of_reasoning_is_its_last_title_or_its_last_line() {
        assert_eq!(
            latest_gist("**Reading the queue**\nIt leases.\n\n**Checking the backoff**\nhm"),
            "Checking the backoff"
        );
        assert_eq!(latest_gist("first\nsecond\n\n"), "second");
    }
}
