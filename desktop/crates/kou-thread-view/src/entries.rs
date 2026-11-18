//! The cockpit transcript. Messages, work summaries and tool calls share
//! the prompt field's left and right edges.

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

mod faces;

/// Horizontal inset shared by the transcript and prompt field.
pub(crate) const EDGE: f32 = 12.;
/// The widest the transcript's column gets, centred in a wider pane
/// (`--measure`).
pub(crate) const MEASURE: f32 = 860.;
/// The size of the labels in capitals of the monospace (`.meta`, `.label`).
pub(crate) const LABEL: f32 = 10.5;
/// Height of the line of capitals over an entry (`.meta`), and of a fold.
pub(crate) const META: f32 = 22.;
/// Height of a tool call's head (`.tool-head`).
pub(crate) const HEAD: f32 = metric::CONTROL;
/// Lines of a terminal output shown until it is unfolded.
const TERMINAL_TAIL: usize = 16;
/// Lines of a terminal output shown at most: the end of it.
const TERMINAL_MAX: usize = 2000;
/// Lines a call that runs shows while it is closed (`timeline.js`, `TAIL`).
pub(crate) const LIVE_TAIL: usize = 3;
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

/// Align an entry with the prompt field, including in a split pane.
pub(crate) fn row(content: impl IntoElement) -> Div {
    div().w_full().px(px(EDGE)).child(content)
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
        .h(px(24.))
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

/// What a tool call's head says it is: the action (`.tname`) and what
/// it was given (`.tinput`). A title that starts with a word of its own
/// (`Run cargo test`, `Read src/main.rs`) gives both; otherwise the kind names it.
pub(crate) fn split_title(call: &ToolCall) -> (String, String) {
    let title = call.title.trim();
    let kind = match call.kind {
        ToolKind::Read => "Read",
        ToolKind::Edit => "Edit",
        ToolKind::Delete => "Delete",
        ToolKind::Move => "Move",
        ToolKind::Search => "Search",
        ToolKind::Execute => "Run",
        ToolKind::Think => "Think",
        ToolKind::Fetch => "Fetch",
        ToolKind::Other => "Tool",
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
    (
        kind.to_owned(),
        if title.eq_ignore_ascii_case(kind) {
            String::new()
        } else {
            title.to_owned()
        },
    )
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
        let hidden = (fold.is_some_and(|(_, open, _)| !open) && self.folds_at(index))
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
                    .with_links(&self.header.cwd, self.link_opener(cx))
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
                self.answer(index, text, blocks, &look, cx)
                    .into_any_element()
            }
            (Entry::Thought(_), Some(Parsed::Document(blocks))) => {
                let fades = if still {
                    Vec::new()
                } else {
                    self.motion.fades(index, now)
                };
                self.thought(index, blocks, fades, lights.shimmer, theme, cx)
                    .into_any_element()
            }
            (Entry::Tool(call), Some(Parsed::Tool(pieces))) => {
                let level = self.unfold_level(index, self.tool_open(index, call), now);
                row(self.tool_card(index, call, pieces, lights.run, lit, now, level, theme, cx))
                    .into_any_element()
            }
            (Entry::Plan(steps), _) => {
                let drawn = self.plan_face(index, steps, lit, theme, cx);
                plan(steps, lit, theme, drawn).into_any_element()
            }
            (Entry::Notice(level, text), _) => {
                // The thread records a failed turn as an error notice with the reason.
                let turn = index + 1 == entries.len()
                    && matches!(&self.thread.last_stop, Some(Stop::Failed(reason)) if reason == text);
                let drawn = self.notice_face(index, *level, text, turn, cx);
                notice(*level, text, turn, theme, drawn).into_any_element()
            }
            _ => div().into_any_element(),
        };
        let is_step =
            |entry: &Entry| matches!(entry, Entry::Tool(_) | Entry::Thought(_) | Entry::Plan(_));
        let above = if index == self.cleared {
            14.
        } else if !leads && is_step(&entries[index]) && is_step(&entries[index - 1]) {
            4.
        } else {
            14.
        };
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
            .when(!top.is_empty(), |column| {
                column
                    .child(self.card_group(&top, index, 0, 0., theme, cx))
                    .child(div().h(px(18.)))
            })
            .when_some(fold_line, |column, (line, open)| {
                // Opened, the work stands under its line, an entry's air away.
                column
                    .child(line)
                    .when(open && !hidden, |column| column.child(div().h(px(4.))))
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

    /// The line a run of work folds into once it is over: how
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
        // The app draws the line when it can (`chat-worked`).
        // (The digest is of what the data is made from, and only that.)
        let memo = crate::face::digest((start, &words, open));
        let data = || serde_json::json!({ "start": start, "words": words, "open": open });
        if let Some(line) = self.entry_face("chat-worked", start, Some(memo), data, Vec::new(), cx)
        {
            return row(line);
        }
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
        row(div().flex().child(line))
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
        // (A call that asks the user questions shows them, and keeps what
        // it was given, which says the same, folded.)
        self.asking(index)
            .is_some_and(|pending| pending.questions.is_empty())
            || matches!(call.status, ToolStatus::Failed)
            // A surface the agent made shows: its newest call is the surface.
            || (self.tool_view.is_some() && self.newest_surfaces().contains(&index))
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
        // The app draws it when it can (`chat-thought`). The light's label,
        // the gist while it is written and the reasoning stay the view's.
        if self.chat_face.is_some() {
            let mut slots = Vec::new();
            if shimmer {
                let who = label("Thinking", gpui::transparent_black(), theme)
                    .relative()
                    .child(mark_spot(&self.spots, (index, 1)));
                slots.push(("who", who.into_any_element()));
            }
            if writing {
                let said = div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .italic()
                    .child(scale::text(gist.replace("**", "")));
                slots.push(("gist", said.into_any_element()));
            }
            if open {
                let look = Look::new(theme, Tone::reasoning(theme), self.field_focus())
                    .with_links(&self.header.cwd, self.link_opener(cx))
                    .small()
                    .with_fades(fades.clone());
                slots.push(("prose", prose::document(blocks, &look).into_any_element()));
            }
            let memo = crate::face::digest((index, open, (!writing).then_some(&gist)));
            let data = || {
                serde_json::json!({
                    "index": index,
                    "open": open,
                    "gist": (!writing).then(|| gist.replace("**", "")),
                })
            };
            if let Some(drawn) = self.entry_face("chat-thought", index, Some(memo), data, slots, cx)
            {
                return row(drawn);
            }
        }
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
                        .with_links(&self.header.cwd, self.link_opener(cx))
                        .small()
                        .with_fades(fades),
                ))
        }));
        row(content)
    }

    /// A tool call's card (`.tool`): its head names the action, input and
    /// outcome. When opened, the output is
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
    ) -> AnyElement {
        // A call that starts a subagent is that agent's card.
        if call.agent.is_some() {
            return self
                .agent_card(index, call, pieces, light, lit, now, level, theme, cx)
                .into_any_element();
        }
        // And one the app has a face of its own for (a surface the agent
        // made) is drawn by the app, in the view's frame.
        if let Some(card) = self.app_card(index, call, level, theme, cx) {
            return card.into_any_element();
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
        // (Its colour by the name a drawing of the app's knows it by too.)
        let word = match (asking.is_some(), call.status) {
            (true, _) => Some(("Needs you", "blocked", theme.blocked)),
            (false, ToolStatus::Failed) => Some(("Failed", "error", theme.error)),
            (false, ToolStatus::Cancelled) => Some(("Stopped", "fgMuted", theme.fg_muted)),
            (false, ToolStatus::Pending) => Some(("Queued", "fgMuted", theme.fg_muted)),
            _ if behind => Some(("In background", "working", theme.working)),
            // It ran there and has ended: what its card holds is what its
            // call said as it left, not what it printed.
            _ if call.work.background && call.work.ended => {
                Some(("Ran in background", "fgMuted", theme.fg_muted))
            }
            _ => None,
        };

        // How long it ran, in the head: it counts while it runs.
        let took_shown = |took: std::time::Duration| {
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
        };
        // What the call asks the user, when it asks questions.
        let asks = asking.and_then(|pending| self.asks_of(pending));
        // The app draws the card when it can (`chat-tool`): its frame, its
        // head and the answers it waits for, or the questions. What moves
        // (the mark of a call that runs, its time, the card opening, its
        // light) and what it gave stay the view's.
        if self.chat_face.is_some() {
            let glyph = match (asking.is_some(), call.status) {
                _ if behind => Some("working"),
                (true, _) => Some("asking"),
                (false, ToolStatus::Running) => None,
                (false, ToolStatus::Pending) => Some("pending"),
                (false, ToolStatus::Done) => Some("done"),
                (false, ToolStatus::Failed) => Some("failed"),
                (false, ToolStatus::Cancelled) => Some("cancelled"),
            };
            let border = match (asking.is_some(), call.status) {
                (true, _) => "blocked",
                (false, ToolStatus::Running) => "working",
                (false, ToolStatus::Failed) => "error",
                _ if behind => "working",
                _ => "line",
            };
            let answers = || {
                asking.map(|pending| {
                    let default = default_option(&pending.options);
                    serde_json::json!({
                        "request": pending.request,
                        "options": pending.options.iter().enumerate().map(|(at, option)| {
                            serde_json::json!({
                                "id": option.id,
                                "label": option.label,
                                "primary": Some(at) == default,
                            })
                        }).collect::<Vec<_>>(),
                        "hint": default.map(|at| pending.options[at].label.to_lowercase()),
                        "key": allow_key(),
                    })
                })
            };
            let data = || {
                serde_json::json!({
                    "index": index,
                    "glyph": glyph,
                    "verb": verb,
                    "input": input.lines().next().unwrap_or(""),
                    "shell": shell,
                    "location": location.as_ref().map(|location| location.to_string()),
                    "added": added,
                    "removed": removed,
                    "exit": exit.filter(|code| *code != 0),
                    "word": word.map(|(word, tone, _)| serde_json::json!({ "text": word, "tone": tone })),
                    "expandable": expandable,
                    "open": open,
                    "border": border,
                    "answers": answers(),
                    "asks": asks,
                })
            };
            let mut slots: Vec<(&'static str, AnyElement)> = Vec::new();
            if glyph.is_none() {
                let mark = state_glyph(call.status, asking.is_some(), lit, theme);
                slots.push(("glyph", mark));
            }
            if let Some(took) = took {
                slots.push(("took", took_shown(took).into_any_element()));
            }
            let (body, live) = self.tool_parts(
                index, call, pieces, open, level, expandable, running, theme, cx,
            );
            slots.extend(body.map(|body| ("body", body)));
            slots.extend(live.map(|live| ("live", live)));
            if behind {
                slots.push(("counting", self.counting(cx).into_any_element()));
            }
            if light {
                slots.push((
                    "spot",
                    mark_spot(&self.spots, (index, 0)).into_any_element(),
                ));
            }
            let memo = crate::face::digest((
                (index, glyph, &verb, input.lines().next(), shell),
                (
                    location.as_deref(),
                    added,
                    removed,
                    exit.filter(|code| *code != 0),
                ),
                (
                    word.map(|(word, tone, _)| (word, tone)),
                    expandable,
                    open,
                    border,
                ),
                asking.map(|pending| {
                    let options: Vec<(&str, &str)> = pending
                        .options
                        .iter()
                        .map(|option| (option.id.as_str(), option.label.as_str()))
                        .collect();
                    let default = default_option(&pending.options);
                    (pending.request, options, default, allow_key())
                }),
                &asks,
            ));
            if let Some(card) = self.entry_face("chat-tool", &call.id, Some(memo), data, slots, cx)
            {
                return card;
            }
        }
        let (body, live) = self.tool_parts(
            index, call, pieces, open, level, expandable, running, theme, cx,
        );

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
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
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
            .child(
                div()
                    .flex_none()
                    .font_family(theme.ui_font.clone())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.fg_dim)
                    .child(scale::text(verb)),
            )
            .when(!input.is_empty(), |head| {
                head.child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .truncate()
                        .text_color(theme.fg)
                        .child(scale::text(input.lines().next().unwrap_or("").to_owned())),
                )
            })
            .children(location.map(|location| place.child(scale::text(location))))
            .child(
                // `.tmeta`: what changed, how long it took, how it ended.
                meta(theme)
                    .flex_grow(1.)
                    .flex_shrink(0.)
                    .justify_end()
                    .flex()
                    .items_center()
                    .gap(px(6.))
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
                    .children(took.map(took_shown))
                    .children(exit.filter(|code| *code != 0).map(|code| {
                        div()
                            .text_color(theme.blocked)
                            .child(scale::text(format!("Exit {code}")))
                    }))
                    .children(word.map(|(word, _, color)| {
                        div()
                            .font_family(theme.ui_font.clone())
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(12.))
                            .text_color(color)
                            .child(scale::text(word))
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

        let answer = match &asks {
            Some(asks) => Some(self.question_block(asks, theme, cx)),
            None => asking.map(|pending| self.answer_bar(pending, theme, cx)),
        };

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
        let quiet = call.status == ToolStatus::Done
            && !behind
            && asking.is_none()
            && !open
            && body.is_none();
        div()
            .relative()
            .w_full()
            .rounded(px(metric::RADIUS_MEDIUM))
            .border_1()
            .border_color(if quiet {
                gpui::transparent_black()
            } else {
                border
            })
            .when(!quiet, |card| card.bg(theme.raised))
            .overflow_hidden()
            .child(head)
            .children(body)
            .children(live)
            .children(answer)
            // Its time counts while it is on screen, turn or no turn.
            .children(behind.then(|| self.counting(cx)))
            // Its light, drawn over the transcript, goes round where it is.
            .children(light.then(|| mark_spot(&self.spots, (index, 0))))
            .into_any_element()
    }

    /// What a tool call's card holds under its head: what it gave, shown
    /// down to `level` of its height while the card opens or closes; and,
    /// closed, the last lines a call that runs printed.
    #[allow(clippy::too_many_arguments)]
    fn tool_parts(
        &self,
        index: usize,
        call: &ToolCall,
        pieces: &Rc<[Piece]>,
        open: bool,
        level: f32,
        expandable: bool,
        running: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
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
            revealed((index, 0), level, 0., &self.unfold_heights, content).into_any_element()
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
                    .pl(px(12.))
                    .pr(px(12.))
                    .py(px(4.))
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

        (body, live.map(IntoElement::into_any_element))
    }

    /// The card of a call the app draws itself ([`crate::ToolView`]): the
    /// view's frame and head (its title, what stands beside it, how it
    /// stands, and a chevron that folds it), with the app's own body.
    /// `None` when the app has no face for this call.
    fn app_card(
        &self,
        index: usize,
        call: &ToolCall,
        level: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let view = self.tool_view.as_ref()?;
        // (Asked of the calls that name a surface only: every other call
        // is the view's own to draw, and is not looked at twice.)
        call.surface()?;
        let newest = self.newest_surfaces().contains(&index);
        let look = Look::new(theme, Tone::output(theme), self.field_focus())
            .with_links(&self.header.cwd, self.link_opener(cx))
            .small();
        let markdown =
            |text: &str| prose::document(&markdown::parse(text), &look).into_any_element();
        let bare = Look::new(theme, Tone::output(theme), self.field_focus())
            .with_links(&self.header.cwd, self.link_opener(cx))
            .bare()
            .small();
        let code = |text: &str, lang: Option<&str>| {
            prose::document(
                &[Markdown::Code {
                    lang: lang.map(str::to_owned),
                    text: SharedString::from(markdown::expand_tabs(text)),
                }],
                &bare,
            )
            .into_any_element()
        };
        let face = view(
            call,
            &crate::ToolAids {
                markdown: &markdown,
                code: &code,
                newest,
            },
            cx,
        )?;
        // The newest version of a surface shows; an earlier one is a row
        // that opens to what it was (see `tool_open`).
        let open = self.tool_open(index, call);
        let body = |body: AnyElement| {
            (open || level > 0.).then(|| {
                let content = div()
                    .w_full()
                    .border_t_1()
                    .border_color(theme.line)
                    .child(body);
                revealed((index, 0), level, 0., &self.unfold_heights, content)
            })
        };
        // The app draws the card's frame and head when it can
        // (`chat-app-card`); what the surface shows, and its opening, stay
        // the view's (`body`).
        if self.chat_face.is_some() {
            let what = AppCard {
                index,
                title: &face.title,
                tag: &face.tag,
                status: &face.status,
                live: face.live,
                open,
            };
            let memo = crate::face::digest(&what);
            let data = || serde_json::to_value(&what).unwrap_or_default();
            let slots: Vec<(&'static str, AnyElement)> = body(face.body)
                .map(|body| ("body", body.into_any_element()))
                .into_iter()
                .collect();
            if let Some(card) =
                self.entry_face("chat-app-card", &call.id, Some(memo), data, slots, cx)
            {
                return Some(div().w_full().child(card));
            }
            // The app did not draw it: the surface is asked for its body
            // again, the first having gone to the app.
            let face = view(
                call,
                &crate::ToolAids {
                    markdown: &markdown,
                    code: &code,
                    newest,
                },
                cx,
            )?;
            return Some(self.app_card_native(
                index,
                open,
                face.title,
                face.tag,
                face.status,
                face.live,
                body(face.body).map(IntoElement::into_any_element),
                theme,
                cx,
            ));
        }
        Some(self.app_card_native(
            index,
            open,
            face.title,
            face.tag,
            face.status,
            face.live,
            body(face.body).map(IntoElement::into_any_element),
            theme,
            cx,
        ))
    }

    /// The card of a surface as the view draws it: its frame and head, and
    /// `body` under them.
    #[allow(clippy::too_many_arguments)]
    fn app_card_native(
        &self,
        index: usize,
        open: bool,
        title: SharedString,
        tag_text: SharedString,
        status: SharedString,
        live: bool,
        body: Option<AnyElement>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let (hover, ring) = (theme.hover, theme.accent);
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
            .child(
                div().flex_none().w(px(14.)).flex().justify_center().child(
                    icon(
                        Icon::Canvas,
                        if live { theme.accent } else { theme.fg_muted },
                    )
                    .size(px(13.)),
                ),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .truncate()
                    .text_size(px(13.))
                    .text_color(theme.fg)
                    .child(scale::text(title)),
            )
            .child({
                let mut tag = meta(theme).min_w_0().truncate();
                tag.style().flex_shrink = Some(1000.);
                tag.child(scale::text(tag_text))
            })
            .child(div().flex_1())
            .child(
                div()
                    .debug_selector(|| format!("surface-status-{index}"))
                    .flex_none()
                    .font_family(theme.mono_font.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(10.))
                    .text_color(if live { theme.ok } else { theme.fg_muted })
                    .child(scale::text(status.to_uppercase())),
            )
            .child(chevron(open, theme));
        div()
            .relative()
            .w_full()
            .rounded(px(metric::RADIUS_MEDIUM))
            .border_1()
            .border_color(theme.line)
            .bg(theme.raised)
            .overflow_hidden()
            .child(head)
            .children(body)
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
                answer_button(
                    ("answer", at),
                    option.label.clone(),
                    Some(at) == default,
                    theme,
                )
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
    ) -> AnyElement {
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
        let code = self.header.code.to_string();
        // How long it has been at it, in the head: it counts while it works.
        let took_shown = |took: std::time::Duration| {
            div()
                .text_color(if state.live() {
                    theme.accent
                } else {
                    theme.fg_muted
                })
                .child(scale::text(motion::duration(took)))
        };
        // Closed, its latest on one line, under its title.
        let latest = (!open && level <= 0.)
            .then(|| self.subagent_detail(call, state))
            .filter(|latest| !latest.is_empty());
        let body = || {
            (open || level > 0.).then(|| {
                let shown = self.agent_shown(index, call, pieces, lit, theme, cx);
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
            })
        };
        // The app draws the card when it can (`chat-agent`): its frame, its
        // head, the frame of its latest line and the answers it waits for.
        // What moves or streams (its time, its latest words, the card
        // opening, what it did, its light) stays the view's.
        if self.chat_face.is_some() {
            let tone = match state {
                NestedState::Running => "working",
                NestedState::Asking => "blocked",
                NestedState::Finished | NestedState::Gone => "fgMuted",
                NestedState::Failed => "error",
            };
            let border = match state {
                NestedState::Asking => "blocked",
                NestedState::Running => "working",
                NestedState::Failed => "error",
                NestedState::Finished | NestedState::Gone => "line",
            };
            let what = AgentCard {
                index,
                code: &code,
                title: agent_title(call),
                kind: (!kind.is_empty()).then_some(kind),
                about: (!about.is_empty()).then(|| about.join(" · ")),
                word,
                tone,
                open,
                border,
                answers: asking.map(Answers::of),
                asks: asking.and_then(|pending| self.asks_of(pending)),
            };
            let mut slots: Vec<(&'static str, AnyElement)> = Vec::new();
            if let Some(took) = took {
                slots.push(("took", took_shown(took).into_any_element()));
            }
            if let Some(latest) = &latest {
                slots.push(("latest", scale::text(latest.clone()).into_any_element()));
            }
            if let Some(body) = body() {
                slots.push(("body", body.into_any_element()));
            }
            if light {
                slots.push((
                    "spot",
                    mark_spot(&self.spots, (index, 0)).into_any_element(),
                ));
            }
            let memo = crate::face::digest(&what);
            let data = || serde_json::to_value(&what).unwrap_or_default();
            if let Some(card) = self.entry_face("chat-agent", &call.id, Some(memo), data, slots, cx)
            {
                return card;
            }
        }
        let (hover, ring) = (theme.hover, theme.accent);
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
                    .children(took.map(took_shown))
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

        let body = body();
        let latest = latest.map(|latest| {
            div()
                .debug_selector(|| format!("agent-latest-{index}"))
                .pl(px(36.))
                .pr(px(12.))
                .pb(px(6.))
                .truncate()
                .font_family(theme.mono_font.clone())
                .text_size(px(11.5))
                .text_color(theme.fg_muted)
                .child(scale::text(latest))
        });
        let answer = asking.map(|pending| match self.asks_of(pending) {
            Some(asks) => self.question_block(&asks, theme, cx),
            None => self.answer_bar(pending, theme, cx),
        });
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
            .into_any_element()
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
        let look = Look::new(theme, Tone::output(theme), self.field_focus())
            .with_links(&self.header.cwd, self.link_opener(cx))
            .bare();
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
                    let left_out = diff.rows.len() - diff.rows.len().min(DIFF_MAX);
                    let what = PieceFace::Diff {
                        path,
                        added: diff.added,
                        removed: diff.removed,
                        empty: diff.rows.is_empty(),
                        left_out,
                    };
                    let lines = diff_lines(at, diff, theme).map(IntoElement::into_any_element);
                    shown.push(
                        self.piece_face(call, at, what, lines, cx)
                            .unwrap_or_else(|| {
                                diff_block(at, path, diff, theme).into_any_element()
                            }),
                    )
                }
                Piece::Terminal { lines, exit } => {
                    shown.push(self.terminal_block(index, at, lines, *exit, call.status, theme, cx))
                }
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
                    let what = PieceFace::Raw { label };
                    let lines = raw_lines(at, label, value, theme).into_any_element();
                    shown.push(
                        self.piece_face(call, at, what, Some(lines), cx)
                            .unwrap_or_else(|| {
                                raw_block(at, label, value, theme).into_any_element()
                            }),
                    );
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

    /// Whether the turn entry `index` is of still runs: the agent is at
    /// work, and the entry is of the turn that runs (a message of the user's
    /// that this turn took in after it ends nothing), or no prompt came
    /// after it.
    fn turn_runs_at(&self, index: usize) -> bool {
        let entries = self.thread.entries();
        let Some(from) = self.thread.turn_from() else {
            return false;
        };
        index >= from
            || !entries[(index + 1).min(entries.len())..]
                .iter()
                .any(|entry| matches!(entry, Entry::User(_)))
    }

    /// How the subagent started by the call at `index` is doing. One started
    /// in the background runs on after its call is over: the agent's turn is
    /// held open until such agents are done, so it counts as running for as
    /// long as that turn runs, unless its backend has taken it to have
    /// ended (`work.ended`): with two of them the turn stays open for the
    /// second long after the first is done.
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
            ToolStatus::Done
                if call.work.background && !call.work.ended && self.turn_runs_at(index) =>
            {
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
        cx: &Context<Self>,
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
                .with_links(&self.header.cwd, self.link_opener(cx))
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
                .with_links(&self.header.cwd, self.link_opener(cx))
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
    ) -> AnyElement {
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

        // How it ended is the head's to say; while it runs, the stream says so.
        let running = matches!(
            (exit, status),
            (None, ToolStatus::Running | ToolStatus::Pending)
        );
        let lines = || {
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
                        .child(scale::styled_text(text.clone(), runs.clone())),
                )
        };
        // The app draws its frame when it can: its head, and the line that
        // shows the lines left out.
        if let Some(Entry::Tool(call)) = self.thread.entries().get(index) {
            let what = PieceFace::Terminal {
                index,
                piece,
                running,
                hidden,
                unfolded,
            };
            let shown = Some(lines().into_any_element());
            if let Some(block) = self.piece_face(call, piece, what, shown, cx) {
                return block;
            }
        }
        let (hover, ring) = (theme.hover, theme.accent);
        let status_word = running.then(|| ("running".to_owned(), theme.working));
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
            .child(lines())
            .into_any_element()
    }

    /// A piece of what a call gave, framed by the app (`chat-tool-piece`):
    /// `what` it is, its lines drawn by the view (`lines`). `None` when the
    /// app does not draw it.
    fn piece_face(
        &self,
        call: &ToolCall,
        at: usize,
        what: PieceFace,
        lines: Option<AnyElement>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        self.chat_face.as_ref()?;
        let key = format!("{}:{}{at}", call.id, what.kind());
        let memo = crate::face::digest(&what);
        let data = || serde_json::to_value(&what).unwrap_or_default();
        let slots = lines.map(|lines| ("lines", lines)).into_iter().collect();
        self.entry_face("chat-tool-piece", key, Some(memo), data, slots, cx)
    }

    /// The last row: what the thread is doing, when that is not the prompt
    /// area's to say, and what the user can do about it.
    fn tail(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let agent = &self.header.agent;
        // The app can draw the session status
        // (`chat-tail`): a line, `{ text }`, or what ended the session,
        // `{ agent, reason }`.
        let drawn = |made_from: u64, data: &dyn Fn() -> serde_json::Value| {
            self.entry_face("chat-tail", "tail", Some(made_from), data, Vec::new(), cx)
        };
        let line = |text: String| {
            let data = || serde_json::json!({ "text": text });
            let content = drawn(crate::face::digest(&text), &data).unwrap_or_else(|| {
                div()
                    .min_h(px(META))
                    .flex()
                    .items_center()
                    .text_color(theme.fg_muted)
                    .child(scale::text(text.clone()))
                    .into_any_element()
            });
            row(content)
        };
        let content = match self.thread.state() {
            ThreadState::Starting => Some(line(format!(
                "Starting {agent} in {}. Your first message can be sent once it is ready.",
                display_path(&self.header.cwd, None)
            ))),
            // (A new session says nothing of itself: the field is there to
            // be typed in.)
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
            ThreadState::Closed(Some(reason)) => {
                let data = || serde_json::json!({ "agent": agent, "reason": reason });
                let made_from = crate::face::digest((agent, reason));
                let content = drawn(made_from, &data).unwrap_or_else(|| {
                    div()
                        .flex()
                        .flex_col()
                        .child(meta_line().child(label(
                            format!("{agent} stopped"),
                            theme.error,
                            theme,
                        )))
                        .child(error_box(reason, theme.error, theme))
                        .child(
                            div()
                                .mt(px(8.))
                                .text_color(theme.fg_muted)
                                .child(scale::text(
                                    "Close this pane and open it again to start a new session. \
                                     The conversation above stays readable until then.",
                                )),
                        )
                        .into_any_element()
                });
                Some(row(content))
            }
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
pub(crate) fn answer_button(
    id: impl Into<gpui::ElementId>,
    text: String,
    primary: bool,
    theme: &Theme,
) -> gpui::Stateful<Div> {
    let ring = theme.accent;
    let base = div()
        .id(id)
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
        _now: Instant,
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
            return row(div()
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
                )));
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
        // (The view's own line over the prompt is made only when the app
        // does not draw it.)
        let actions = || {
            let text = message.text.to_string();
            div()
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
                .children(self.prompt_action(index, group.clone(), theme, cx))
        };
        let pictures_shown = || {
            (!message.pictures.is_empty()).then(|| {
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
            })
        };
        // The model the prompt went to, when the view knows it (`.model-tag`).
        let model = || {
            let id = self.prompt_models.get(&index)?;
            let provider = crate::models::provider_of(id, &self.thread.providers);
            let tag = div()
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
                .child(div().min_w_0().truncate().child(scale::text(id.clone())));
            Some(tag)
        };
        // The line over the prompt is the app's to draw when it can
        // (`chat-user-head`).
        let shown_model = self.prompt_models.get(&index).map(|id| {
            let provider = crate::models::provider_of(id, &self.thread.providers);
            (id, theme.provider(provider.as_deref().unwrap_or_default()))
        });
        let action = if matches!(self.thread.state(), ThreadState::Closed(_)) {
            None
        } else if !self.edits() {
            Some("reuse")
        } else if self.edit_blocked().is_none() {
            Some("edit")
        } else {
            None
        };
        let data = || {
            serde_json::json!({
                "index": index,
                "copied": copied,
                "model": shown_model.map(|(id, dot)| {
                    serde_json::json!({ "id": id, "dot": crate::face::hex(dot) })
                }),
                "action": action,
            })
        };
        let went_to = shown_model.map(|(id, dot)| (id, crate::face::bits(dot)));
        let memo = crate::face::digest((index, copied, went_to, action));
        let head = match self.entry_face("chat-user-head", index, Some(memo), data, Vec::new(), cx)
        {
            Some(head) => head,
            None => meta_line()
                .child(label("You", theme.accent, theme))
                .children(model())
                .child(div().flex_1())
                .child(actions())
                .into_any_element(),
        };
        // Text and pictures stay native when the plugin draws the card.
        let drawn_card = if self.chat_face.is_some() {
            let mut slots: Vec<(&'static str, AnyElement)> = Vec::new();
            if !message.text.is_empty() {
                let text = scale::styled_text(message.text.clone(), runs.clone());
                slots.push(("text", text.into_any_element()));
            }
            if let Some(pictures) = pictures_shown() {
                slots.push(("pictures", pictures.into_any_element()));
            }
            let memo = crate::face::digest(&message.files);
            let data = || serde_json::json!({ "files": message.files });
            self.entry_face("chat-user-card", index, Some(memo), data, slots, cx)
        } else {
            None
        };
        let card = || {
            div()
                .id(("user", index))
                .w_full()
                .px(px(15.))
                .pt(px(10.))
                .pb(px(11.))
                .rounded(px(metric::RADIUS_LARGE))
                .border_1()
                .border_color(theme.line)
                .bg(theme.raised)
                .text_size(px(PROSE))
                .line_height(px(LINE))
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(
                    (!message.text.is_empty())
                        .then(|| scale::styled_text(message.text.clone(), runs.clone())),
                )
                .children(pictures_shown())
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
                }))
        };
        row(div()
            .id(("prompt", index))
            .group(group)
            .w_full()
            .child(exact::column(
                div().w_full().flex().flex_col().gap(px(6.)),
                [
                    head,
                    drawn_card.unwrap_or_else(|| card().into_any_element()),
                ],
            )))
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
        cx: &Context<Self>,
    ) -> Div {
        let theme = look.theme;
        let group = SharedString::from(format!("answer-{index}"));
        let key = format!("answer-{index}");
        let copied = self.copied.as_ref().is_some_and(|copied| copied == &key);
        let writing = self.writing(index);
        // An answer still being written has no actions: it is not all said
        // yet. (Made only when the app does not draw the line it is on.)
        let copy = || {
            let text = text.to_owned();
            (!writing).then(|| {
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
            })
        };
        // The line over it is the app's to draw when it can
        // (`chat-answer-head`); what was said is the view's.
        let data = || serde_json::json!({ "index": index, "copied": copied, "writing": writing });
        let memo = crate::face::digest((index, copied, writing));
        let head =
            match self.entry_face("chat-answer-head", index, Some(memo), data, Vec::new(), cx) {
                Some(head) => head,
                None => meta_line()
                    .child(label("Agent", theme.fg, theme))
                    .child(div().flex_1())
                    .children(copy())
                    .into_any_element(),
            };
        row(div()
            .id(("answer", index))
            .debug_selector(move || format!("reply-{index}"))
            .group(group)
            .w_full()
            // At a zoom, the line of capitals and the prose each where
            // they stand at 1 (see `kou_ui::scale::exact`).
            .child(exact::column(
                div().w_full().flex().flex_col().gap(px(4.)),
                [head, prose::document(blocks, look).into_any_element()],
            )))
    }

    /// What `/clear` left: how much is out of sight, and a way back to it.
    fn cleared_row(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let hidden = self.cleared;
        // The app can draw the summary
        // (`chat-cleared`: `{ hidden }`).
        let data = || serde_json::json!({ "hidden": hidden });
        let drawn = self.entry_face(
            "chat-cleared",
            "cleared",
            Some(crate::face::digest(hidden)),
            data,
            Vec::new(),
            cx,
        );
        let notice = || {
            div()
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
                .child(div().flex_1().h(px(1.)).bg(theme.line))
                .into_any_element()
        };
        div().w_full().flex().justify_center().child(
            div()
                .w_full()
                .max_w(px(MEASURE))
                .pt(px(14.))
                .child(row(drawn.unwrap_or_else(notice))),
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
/// A plan's card; `drawn` is the app's drawing of what stands beside its
/// node, when it has one.
/// The card of a surface as the app draws it (`chat-app-card`): its title,
/// what stands beside it, how it stands (`live` while it follows its
/// sources), and whether it is open.
#[derive(Hash, serde::Serialize)]
struct AppCard<'a> {
    index: usize,
    title: &'a str,
    tag: &'a str,
    status: &'a str,
    live: bool,
    open: bool,
}

/// A piece of what a call gave as the app frames it (`chat-tool-piece`), by
/// its `kind`: a change to a file (its path, lines added and removed,
/// whether it changes nothing, how many of its lines are left out); what a
/// command printed (the call's entry, the piece, whether it runs, how many
/// earlier lines are hidden, whether more are kept); or what went in or
/// came back as the backend wrote it (its `label`).
#[derive(Hash, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum PieceFace<'a> {
    #[serde(rename_all = "camelCase")]
    Diff {
        path: &'a str,
        added: usize,
        removed: usize,
        empty: bool,
        left_out: usize,
    },
    Terminal {
        index: usize,
        piece: usize,
        running: bool,
        hidden: usize,
        unfolded: bool,
    },
    Raw {
        label: &'static str,
    },
}

impl PieceFace<'_> {
    /// Its kind, as its key says it.
    fn kind(&self) -> &'static str {
        match self {
            Self::Diff { .. } => "diff",
            Self::Terminal { .. } => "terminal",
            Self::Raw { .. } => "raw",
        }
    }
}

/// The card of a call that starts a subagent as the app draws it
/// (`chat-agent`): the tile's `code`, what the agent was started for, its
/// kind, its counts (`about`), how it goes in a word with that word's colour
/// (`tone`), whether it is open, its border's colour by name, and the answers
/// it waits for, or the questions it asks.
#[derive(Hash, serde::Serialize)]
struct AgentCard<'a> {
    index: usize,
    code: &'a str,
    title: String,
    kind: Option<&'a str>,
    about: Option<String>,
    word: &'static str,
    tone: &'static str,
    open: bool,
    border: &'static str,
    answers: Option<Answers<'a>>,
    asks: Option<crate::questions::Asks<'a>>,
}

/// The answers to a call that waits for the user's, as the app draws them:
/// the request they answer, each answer (`primary`: the usual one), the
/// usual one's words as the hint says them, and the key that gives it.
#[derive(Hash, serde::Serialize)]
struct Answers<'a> {
    request: u64,
    options: Vec<AnswerOption<'a>>,
    hint: Option<String>,
    key: &'static str,
}

#[derive(Hash, serde::Serialize)]
struct AnswerOption<'a> {
    id: &'a str,
    label: &'a str,
    primary: bool,
}

impl<'a> Answers<'a> {
    fn of(pending: &'a PendingPermission) -> Self {
        let default = default_option(&pending.options);
        Self {
            request: pending.request,
            options: pending
                .options
                .iter()
                .enumerate()
                .map(|(at, option)| AnswerOption {
                    id: &option.id,
                    label: &option.label,
                    primary: Some(at) == default,
                })
                .collect(),
            hint: default.map(|at| pending.options[at].label.to_lowercase()),
            key: allow_key(),
        }
    }
}

fn plan(steps: &[PlanStep], lit: f32, theme: &Theme, drawn: Option<AnyElement>) -> Div {
    let done = steps
        .iter()
        .filter(|step| step.status == PlanStatus::Done)
        .count();
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
    row(drawn.unwrap_or_else(|| {
        div()
            .w_full()
            .child(
                meta_line().child(label("Plan", theme.fg_dim, theme)).child(
                    meta(theme).child(scale::text(format!("{done} of {} done", steps.len()))),
                ),
            )
            .child(steps_list)
            .into_any_element()
    }))
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

/// A message from the backend. `turn` marks the error a turn ended with;
/// `drawn` is the app's rendering of the notice.
fn notice(level: Level, text: &str, turn: bool, theme: &Theme, drawn: Option<AnyElement>) -> Div {
    if let Some(drawn) = drawn {
        return row(drawn);
    }
    match level {
        // `.notice`: a line in capitals, a rule after it.
        Level::Info => row(div()
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
            .child(div().flex_1().min_w(px(16.)).h(px(1.)).bg(theme.line))),
        Level::Warning => row(div()
            .w_full()
            .child(meta_line().child(label("Warning", theme.blocked, theme)))
            .child(error_box(text, theme.blocked, theme))),
        Level::Error => row(div()
            .w_full()
            .child(meta_line().child(label(
                if turn { "Turn failed" } else { "Error" },
                theme.error,
                theme,
            )))
            .child(error_box(text, theme.error, theme))),
    }
}

/// The lines of a change to a file (`.fc-rows`): numbered, additions and
/// removals on the theme's diff colours, as many as are shown; none for a
/// change of nothing.
fn diff_lines(piece: usize, diff: &Diff, theme: &Theme) -> Option<gpui::Stateful<Div>> {
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
    (!diff.rows.is_empty()).then(|| {
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
            ))
    })
}

/// A change to a file, as the view draws it: under a header with the path
/// and the counts, its lines (see [`diff_lines`]), and how many were left
/// out.
fn diff_block(piece: usize, path: &SharedString, diff: &Diff, theme: &Theme) -> Div {
    let left_out = diff.rows.len() - diff.rows.len().min(DIFF_MAX);
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
                    .child(
                        div()
                            .flex_none()
                            .child(scale::text(if diff.rows.is_empty() {
                                "no changes".to_owned()
                            } else {
                                format!("+{} -{}", diff.added, diff.removed)
                            })),
                    ),
            ),
        )
        .children(diff_lines(piece, diff, theme))
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
    div()
        .w_full()
        .child(stream_header(label, theme))
        .child(raw_lines(piece, label, value, theme))
}

/// What [`raw_block`] shows under its header: what the backend wrote, as
/// it wrote it, in so many lines at most.
fn raw_lines(
    piece: usize,
    label: &'static str,
    value: &impl std::fmt::Display,
    theme: &Theme,
) -> gpui::Stateful<Div> {
    let pretty = format!("{value:#}");
    let total = pretty.lines().count();
    let mut text: String = pretty.lines().take(RAW_MAX).collect::<Vec<_>>().join("\n");
    if total > RAW_MAX {
        text.push_str(&format!("\n({} more lines)", total - RAW_MAX));
    }
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
        assert_eq!(
            split_title(&call("read", ToolKind::Read)),
            ("Read".into(), String::new())
        );
        // A title without a verb of its own is named by its kind.
        assert_eq!(
            split_title(&call("`git status`", ToolKind::Execute)),
            ("Run".into(), "`git status`".into())
        );
        assert_eq!(
            split_title(&call("src/main.rs", ToolKind::Read)),
            ("Read".into(), "src/main.rs".into())
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
