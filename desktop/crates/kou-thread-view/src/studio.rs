//! The transcript in the quieter look (`Look::Studio`), after the user's
//! reference picture: one centred column of plain prose on a black ground,
//! no rail and no labels over the entries, a prompt as a bubble at the
//! right, a finished turn's work folded into one line ("Worked for 1m 15s")
//! that opens, and the work of a turn that runs as quiet lines as it goes.
//!
//! Its sizes are the picture's, measured: the picture is drawn at 1.7 pixels
//! to the point (a 12-point traffic light is 20.4 of its pixels, the
//! window's 13-point title has the prose's height), so each size here is
//! what the picture shows divided by 1.7.

use crate::{
    entries::{Parsed, Piece, UserMessage, chevron, latest_gist, lights_mix, parse, split_title},
    markdown::{self, Block as Markdown},
    motion,
    nested::{Look, folds},
    prose::{self, CopyFn, Look as Prose, Tone},
    view::{Shot, ThreadView, allow_key, default_option, display_path},
};
use gpui::{
    AnyElement, App, Context, Div, FontWeight, Hsla, ImageSource, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, TextRun, Window, div, font, img, prelude::FluentBuilder as _, relative,
};
use kou_thread::{
    Block, Entry, Level, PlanStatus, PlanStep, Stop, ThreadState, ToolCall, ToolStatus,
};
use kou_ui::{
    Icon, Theme, icon, key_hint,
    scale::{self, exact, px},
};
use std::{cell::RefCell, rc::Rc, time::Instant};

/// How much larger than the picture the look's type is set, and with it
/// what follows the type: line pitches, the air between paragraphs and
/// entries, row heights, the column's width. The picture's sizes are for its
/// 1.7 pixels to the point; on a display at one pixel to the point the same
/// sizes read small, so the look sets them this much larger (12 points of
/// prose become 13). One number: a bit bigger or smaller is a change here.
pub(crate) const TYPE: f32 = 13. / 12.;

/// A size of the picture, set at the look's type size.
pub(crate) const fn sized(picture: f32) -> f32 {
    picture * TYPE
}

/// A line pitch of the picture, set at the look's type size and kept to a
/// whole point, so that lines stand on whole pixels at one pixel to the point.
pub(crate) const fn pitch(picture: f32) -> f32 {
    whole(sized(picture))
}

/// `value` to the nearest whole number; for the sizes here, all positive.
const fn whole(value: f32) -> f32 {
    (value + 0.5) as u32 as f32
}

/// The widest the column of text gets (the picture's 1088 pixels), and the
/// least room on each side of it in a narrower pane.
pub(crate) const COLUMN: f32 = sized(640.);
pub(crate) const SIDE: f32 = 24.;
/// Size and line pitch of the prose (the picture's 34-pixel lines).
pub(crate) const PROSE: f32 = sized(12.);
/// How far the prose stands in from the column's edges (the picture's 5 pixels).
pub(crate) const PROSE_INSET: f32 = 3.;
pub(crate) const LINE: f32 = pitch(20.);
/// Size and line pitch of what is typed in the prompt field: a step over
/// the prose (14 points to its 13), as the user asked for, at the prose's
/// proportion of pitch to size.
pub(crate) const FIELD: f32 = PROSE * 14. / 13.;
pub(crate) const FIELD_LINE: f32 = whole(FIELD * 20. / 12.);
/// A step smaller: a tool's output, reasoning opened.
pub(crate) const SMALL: f32 = sized(11.5);
pub(crate) const SMALL_LINE: f32 = pitch(18.);
/// Between two paragraphs, two entries of prose, a card and what follows
/// it (the picture's 15 pixels), and between two items of a list (6).
pub(crate) const GAP: f32 = sized(9.);
pub(crate) const ITEM_GAP: f32 = sized(3.5);
/// Where an item's text starts in a list, and its bullet: its centre and side.
pub(crate) const INDENT: f32 = sized(17.6);
pub(crate) const BULLET_AT: f32 = sized(11.2);
pub(crate) const BULLET: f32 = 4.;
/// How far down the transcript fades out under its top edge.
pub(crate) const FADE: f32 = 28.;
/// Corners of what is framed in the column: code, a tool's content.
pub(crate) const RADIUS: f32 = kou_ui::metric::RADIUS_MEDIUM;
/// Air over the first row.
const TOP: f32 = 16.;
/// Air under the last row, over the prompt area (the picture's 64 pixels
/// from the last line of prose to the line over the field).
const TAIL: f32 = sized(29.2);
/// Over a prompt, and over the prose that answers it.
const OVER_PROMPT: f32 = sized(24.);
const UNDER_PROMPT: f32 = sized(16.);
/// A step of work: a tool call, reasoning, the plan.
const STEP: f32 = 24.;
/// Air over a group of nested agents' cards after prose (the picture's 69
/// pixels), and right under the folded line (9).
pub(crate) const CARDS_AFTER_PROSE: f32 = sized(42.1);
/// Air under cards, over the prose that follows them (the picture's 38
/// pixels), and under the cards that follow a folded line (26).
const AFTER_CARDS: f32 = sized(10.2);
const AFTER_FOLD_CARDS: f32 = sized(3.);
pub(crate) const CARDS_AFTER_FOLD: f32 = sized(5.6);
const CARDS_AFTER_STEP: f32 = sized(8.);
/// The folded line: its height, and how far under its middle its rule is.
const WORKED: f32 = pitch(20.);
const WORKED_RULE: f32 = sized(14.7);
/// How far in from the column its words stand (the picture's 8 pixels).
const WORKED_INSET: f32 = 4.7;
/// A prompt's bubble.
const BUBBLE_RADIUS: f32 = kou_ui::metric::RADIUS_LARGE;

thread_local! {
    /// The quieter look's themes: one for each palette the app has drawn with
    /// (dark, light, and each accent the user chose) in each face, made once
    /// and kept.
    static THEMES: RefCell<Vec<(kou_ui::Palette, bool, &'static Theme)>> =
        const { RefCell::new(Vec::new()) };
}

/// `app` where a view can keep it while it borrows its context; with
/// `chrome`, in the text's face where the cockpit has its monospace.
fn kept(app: &Theme, chrome: bool) -> &'static Theme {
    THEMES.with(|themes| {
        let mut themes = themes.borrow_mut();
        if let Some((_, _, theme)) = themes
            .iter()
            .find(|(palette, face, _)| *palette == app.palette && *face == chrome)
        {
            return *theme;
        }
        let mut theme = app.clone();
        if chrome {
            theme.mono_font = theme.ui_font.clone();
        }
        let theme: &'static Theme = Box::leak(Box::new(theme));
        themes.push((app.palette, chrome, theme));
        theme
    })
}

/// The quieter look's theme: the app's own, accent and all, held where a
/// view can keep it while it borrows its context.
pub(crate) fn studio_theme(app: &Theme) -> &'static Theme {
    kept(app, false)
}

/// The quieter look's theme for what frames the conversation (menus, the
/// queue, the editor's controls): the text's face where the cockpit has its
/// monospace, which the quieter look keeps for code alone.
pub(crate) fn chrome_theme(app: &Theme) -> &'static Theme {
    kept(app, true)
}

/// The theme of what frames the conversation in a look: the app's for the
/// cockpit, [`chrome_theme`] for the studio.
pub(crate) fn chrome_look_theme(look: Look, cx: &App) -> &Theme {
    let theme = kou_ui::theme(cx);
    match look {
        Look::Cockpit => theme,
        Look::Studio => chrome_theme(theme),
    }
}

/// The theme a look draws with: the app's for the cockpit, the quieter
/// look's own in the same brightness for the studio.
pub(crate) fn look_theme(look: Look, cx: &App) -> &Theme {
    let theme = kou_ui::theme(cx);
    match look {
        Look::Cockpit => theme,
        Look::Studio => studio_theme(theme),
    }
}

/// What the row before shows last, for the air over what follows it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Prose,
    Prompt,
    Step,
    Fold,
    Nothing,
}

impl ThreadView {
    /// One row of the transcript in the quieter look: an entry, the cards of
    /// the agents started after it, or after the last entry what the thread
    /// is doing.
    pub(crate) fn studio_item(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = studio_theme(kou_ui::theme(cx));
        let count = self.thread.entries().len();
        if index >= count {
            let cards: Vec<_> = if count == 0 {
                self.cards_of(0).0.into_iter().cloned().collect()
            } else {
                Vec::new()
            };
            if self.card_lights(index, cx) {
                window.request_animation_frame();
            }
            let cards =
                (!cards.is_empty()).then(|| self.card_group(&cards, index, 0, TOP, theme, cx));
            return self.studio_tail(cards, theme, cx).into_any_element();
        }
        if index < self.cleared {
            if index + 1 < self.cleared {
                return div().into_any_element();
            }
            return self.studio_cleared(theme, cx).into_any_element();
        }
        let now = cx.background_executor().now();
        let still = cx.reduce_motion();
        // Work folded away under a closed line shows nothing: nothing of it
        // is read, decoded or laid out. (A long turn is hundreds of such
        // rows, each asked for at every frame they scroll past.)
        let fold = self.fold_of(index);
        // What a subagent did is in the card of the call that started it,
        // and shows nothing here either.
        let folded = (fold.is_some_and(|(_, open, _)| !open) && self.folds_at(index))
            || self.kept_in_its_agent(index);
        if folded && !fold.is_some_and(|(_, _, first)| first) {
            let (top, bottom) = self.cards_of(index);
            if top.is_empty() && bottom.is_empty() {
                return gpui::Empty.into_any_element();
            }
        }
        if self.cache.len() <= index {
            self.cache.resize_with(index + 1, || None);
        }
        if !folded && self.cache[index].is_none() {
            let entry = &self.thread.entries()[index];
            let open = match entry {
                // Closing, a call's content is drawn until it has closed.
                Entry::Tool(call) => {
                    self.tool_open(index, call) || self.motion.unfolds.contains_key(&index)
                }
                _ => true,
            };
            let parsed = parse(entry, open, &self.header.cwd);
            if let Some(Parsed::Document(blocks)) = &parsed
                && self.motion.fresh.contains_key(&index)
            {
                self.motion.grow(index, prose::drawn_len(blocks), now);
            }
            self.cache[index] = parsed;
        }
        if !folded {
            self.tool_shots(index, cx);
        }
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
        let rise = if still {
            None
        } else {
            self.motion.rise(index, now)
        };
        let cards_lit = self.card_lights(index, cx);
        // An open tool call is drawn as the cockpit's card: while it runs,
        // with the light that goes round it.
        let call_light = match &self.thread.entries()[index] {
            Entry::Tool(call) if self.tool_open(index, call) || call.agent.is_some() => {
                let lights = self.row_lights(index, cx);
                if lights.made {
                    window.request_animation_frame();
                }
                lights.run
            }
            _ => false,
        };
        if cards_lit || rise.is_some() || (!still && self.motion.flash(index, now).is_some()) {
            window.request_animation_frame();
        }
        let view = cx.weak_entity();
        let copy: CopyFn = Rc::new(move |key, text, _, cx| {
            view.update(cx, |view, cx| view.copy(key, text, cx)).ok();
        });
        let cx = &*cx;
        let lit = if still { 1. } else { self.motion.blink(now).0 };
        let entries = self.thread.entries();
        let entry = &entries[index];
        // (A run that is all a subagent's doing has no line: its card says it.)
        let fold_line = fold
            .filter(|(run, _, first)| *first && self.steps_of(run) > 0)
            .map(|(run, open, _)| self.worked_line(&run, open, theme, cx));
        let parsed = self.cache[index].as_ref();
        let body: Option<AnyElement> = if folded {
            None
        } else {
            match (entry, parsed) {
                (Entry::User(_), Some(Parsed::User(message))) => Some(
                    self.studio_user(index, message, &shots, now, theme, window, cx)
                        .into_any_element(),
                ),
                (Entry::Assistant(text), Some(Parsed::Document(blocks))) => {
                    let writing = self.writing(index);
                    let look = Prose::new(theme, Tone::answer(theme), self.field_focus())
                        .with_links(&self.header.cwd, self.link_opener(cx))
                        .studio()
                        .with_mentions(self.mentions(theme), self.opener(cx))
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
                    Some(
                        self.studio_answer(index, text, blocks, &look, cx)
                            .into_any_element(),
                    )
                }
                (Entry::Thought(_), Some(Parsed::Document(blocks))) => {
                    let fades = if still {
                        Vec::new()
                    } else {
                        self.motion.fades(index, now)
                    };
                    Some(
                        self.studio_thought(index, blocks, fades, lit, theme, cx)
                            .into_any_element(),
                    )
                }
                (Entry::Tool(call), Some(Parsed::Tool(pieces))) => Some(
                    self.studio_tool(index, call, pieces, call_light, lit, now, theme, cx)
                        .into_any_element(),
                ),
                (Entry::Plan(steps), _) => Some(
                    self.plan_face(index, steps, lit, theme, cx)
                        .unwrap_or_else(|| studio_plan(steps, lit, theme).into_any_element()),
                ),
                (Entry::Notice(level, text), _) => {
                    let turn = index + 1 == entries.len()
                        && matches!(&self.thread.last_stop, Some(Stop::Failed(reason)) if reason == text);
                    Some(
                        self.notice_face(index, *level, text, turn, cx)
                            .unwrap_or_else(|| {
                                studio_notice(*level, text, turn, theme).into_any_element()
                            }),
                    )
                }
                _ => None,
            }
        };
        let own = match (&body, &fold_line) {
            (Some(_), _) => shown_by(entry),
            (None, Some(_)) => Shown::Fold,
            (None, None) => Shown::Nothing,
        };
        let before = self.shown_before(index);
        // Cards end the row before: after a folded line's cards the prose
        // follows closer than after cards that follow prose.
        let after_cards = (index > self.cleared && !self.cards_of(index - 1).1.is_empty())
            .then(|| self.fold_of(index - 1).is_some_and(|(_, open, _)| !open));
        let above = if index == self.cleared {
            TOP
        } else {
            match (fold_line.is_some(), own) {
                (true, _) => GAP,
                (false, Shown::Nothing) => 0.,
                (false, Shown::Prompt) => OVER_PROMPT,
                (false, Shown::Prose) if after_cards == Some(true) => AFTER_FOLD_CARDS,
                (false, Shown::Prose) if after_cards == Some(false) => AFTER_CARDS,
                (false, Shown::Prose) if before == Shown::Prompt => UNDER_PROMPT,
                (false, Shown::Prose) => GAP,
                (false, Shown::Step) if before == Shown::Step => 2.,
                (false, Shown::Step) if before == Shown::Prompt => UNDER_PROMPT,
                (false, _) => 8.,
            }
        };
        let (top, bottom) = self.cards_of(index);
        let (top, bottom): (Vec<_>, Vec<_>) = (
            top.into_iter().cloned().collect(),
            bottom.into_iter().cloned().collect(),
        );
        let mut parts: Vec<AnyElement> = Vec::new();
        if !top.is_empty() {
            parts.push(self.card_group(&top, index, 0, 0., theme, cx));
        }
        if let Some((_, open, _)) = fold.filter(|(_, _, first)| *first)
            && let Some(line) = fold_line
        {
            parts.push(line);
            // Opened, the work stands under the line.
            if open && body.is_some() {
                parts.push(div().h(px(6.)).into_any_element());
            }
        }
        if let Some(body) = body {
            parts.push(body);
        }
        if !bottom.is_empty() {
            let last = if own == Shown::Nothing { before } else { own };
            let air = match last {
                Shown::Prose | Shown::Prompt | Shown::Nothing => CARDS_AFTER_PROSE,
                Shown::Fold => CARDS_AFTER_FOLD,
                Shown::Step => CARDS_AFTER_STEP,
            };
            parts.push(self.card_group(&bottom, index, top.len(), air, theme, cx));
        }
        let column = self
            .studio_column()
            .relative()
            .pt(px(if parts.is_empty() { 0. } else { above }))
            .child(exact::column(div().w_full().flex().flex_col(), parts));
        let column = match rise {
            Some(t) => column
                .top(px(kou_ui::motion::cockpit::RISE_BY * (1. - t)))
                .opacity(t),
            None => column,
        };
        let row = self.studio_row(index).child(column);
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

    /// A row of the transcript: as wide as the list, its column centred in
    /// what the card beside leaves.
    fn studio_row(&self, index: usize) -> gpui::Stateful<Div> {
        div()
            .id(("entry", index))
            .w_full()
            .pr(px(self.around.inset.get()))
            .flex()
            .justify_center()
    }

    /// The column of a row: at most [`COLUMN`] wide, [`SIDE`] from the
    /// pane's edges at least.
    pub(crate) fn studio_column(&self) -> Div {
        div().w_full().max_w(px(COLUMN + 2. * SIDE)).px(px(SIDE))
    }

    /// What the rows before `index` show last, past what folds away.
    fn shown_before(&self, index: usize) -> Shown {
        let entries = self.thread.entries();
        let mut at = index;
        while at > self.cleared {
            at -= 1;
            let entry = &entries[at];
            // What a subagent did shows nowhere but in its card.
            if self.made_by_subagent(at) && !self.fold_of(at).is_some_and(|(_, _, first)| first) {
                continue;
            }
            match self.fold_of(at) {
                // Folded work shows as its run's line, at the run's first
                // row: the rows between are stepped over at once.
                Some((run, open, first)) if folds(entry) && !open => {
                    if first {
                        return Shown::Fold;
                    }
                    // A run that began before what `/clear` put out of
                    // sight has no line on screen.
                    if run.start < self.cleared {
                        return Shown::Nothing;
                    }
                    at = run.start + 1;
                }
                _ => return shown_by(entry),
            }
        }
        Shown::Nothing
    }

    /// The line a finished turn's work folds into: how long it worked, a
    /// chevron, and a rule under it. A click opens it, and closes it again.
    fn worked_line(
        &self,
        run: &crate::nested::Run,
        open: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let words = crate::nested::worked(self.worked_for(run), self.steps_of(run));
        let start = run.start;
        if let Some(line) = self.entry_face(
            "studio-worked",
            start,
            None,
            || serde_json::json!({"start": start, "open": open, "text": words}),
            Vec::new(),
            cx,
        ) {
            return line;
        }
        let (hover, ring) = (theme.fg_dim, theme.accent);
        let line = div()
            .id(("worked", start))
            .debug_selector(move || format!("worked-{start}"))
            .h(px(WORKED))
            .ml(px(WORKED_INSET))
            .flex()
            .items_center()
            .gap(px(5.))
            .rounded(px(kou_ui::metric::RADIUS))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .tab_index(0)
            .text_size(px(PROSE))
            .text_color(theme.fg_muted)
            .hover(move |style| style.text_color(hover))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener(move |this, click, window, cx| {
                this.toggle_worked(start, cx);
                this.rest_focus(click, window, cx);
            }))
            .child(scale::text(words))
            .child(chevron(open, theme));
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_start()
            .child(line)
            .child(
                div()
                    .mt(px(WORKED_RULE - WORKED / 2.))
                    .w_full()
                    .h(px(1.))
                    .bg(theme.line),
            )
            .into_any_element()
    }

    /// A prompt of the user's: a bubble at the right, its pictures over it
    /// and the files it brought in it; copy and edit at its left when the
    /// pointer is over it.
    #[allow(clippy::too_many_arguments)]
    fn studio_user(
        &self,
        index: usize,
        message: &UserMessage,
        shots: &[Shot],
        _now: Instant,
        theme: &Theme,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        if let Some(editor) = self.editor.as_ref().filter(|editor| editor.index == index) {
            return div().id(("prompt", index)).w_full().child(exact::column(
                div().w_full().flex().flex_col().gap(px(6.)),
                [
                    div()
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .text_size(px(SMALL))
                        .text_color(theme.accent)
                        .child(scale::text("Editing"))
                        .into_any_element(),
                    self.editor_element(editor, window, chrome_theme(theme), cx),
                ],
            ));
        }
        let group = SharedString::from(format!("user-{index}"));
        let runs = prompt_runs(&message.text, &message.labels, theme);
        let copied = self
            .copied
            .as_ref()
            .is_some_and(|key| key == &format!("prompt-{index}"));
        let text = message.text.to_string();
        let copy = quiet_action(
            ("copy-prompt", index),
            if copied { Icon::Check } else { Icon::Copy },
            group.clone(),
            theme,
        )
        .on_click(cx.listener(move |this, click, window, cx| {
            this.copy(format!("prompt-{index}").into(), text.clone(), cx);
            this.rest_focus(click, window, cx);
        }));
        let again = if matches!(self.thread.state(), ThreadState::Closed(_)) {
            None
        } else if !self.edits() {
            Some(
                quiet_action(("reuse-prompt", index), Icon::Undo, group.clone(), theme).on_click(
                    cx.listener(move |this, _, window, cx| this.reuse(index, window, cx)),
                ),
            )
        } else if self.edit_blocked().is_none() {
            Some(
                quiet_action(("edit-prompt", index), Icon::Pencil, group.clone(), theme).on_click(
                    cx.listener(move |this, _, window, cx| this.begin_edit(index, window, cx)),
                ),
            )
        } else {
            None
        };
        let pictures = || {
            (!message.pictures.is_empty()).then(|| {
                div().flex().flex_wrap().justify_end().gap(px(8.)).children(
                    message
                        .pictures
                        .iter()
                        .zip(shots)
                        .map(|((label, block), shot)| {
                            let block = *block;
                            let ring = theme.accent;
                            let frame = div()
                                .id(("message-picture", index * 64 + block.min(63)))
                                .debug_selector(move || format!("message-picture-{index}-{block}"))
                                .relative()
                                .w(px(136.))
                                .h(px(90.))
                                .rounded(px(RADIUS))
                                .overflow_hidden()
                                .border_1()
                                .border_color(theme.line_strong)
                                .bg(theme.raised)
                                .cursor_pointer()
                                .tab_index(0)
                                .hover(move |style| style.border_color(ring))
                                .focus_visible(move |style| style.border_color(ring))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.view_picture(index, block, cx)
                                }));
                            let content: AnyElement = match shot {
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
                                    .text_size(px(SMALL))
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
                                    .rounded_tr(px(kou_ui::metric::RADIUS))
                                    .bg(theme.raised.opacity(0.88))
                                    .text_size(px(10.5))
                                    .text_color(theme.fg_dim)
                                    .child(scale::text(label.clone())),
                            )
                        }),
                )
            })
        };
        if self.chat_face.is_some() {
            let editing = !matches!(self.thread.state(), ThreadState::Closed(_));
            let again = if !editing {
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
                    "index": index, "copied": copied, "again": again,
                    "files": message.files, "text": !message.text.is_empty(),
                    "pictures": !message.pictures.is_empty(),
                })
            };
            let tops = self.bubble_tops.clone();
            let mut slots = vec![(
                "measure",
                gpui::canvas(
                    move |bounds, _, _| tops.borrow_mut().push(bounds.top()),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
                .into_any_element(),
            )];
            if !message.text.is_empty() {
                slots.push((
                    "text",
                    scale::styled_text(message.text.clone(), runs.clone()).into_any_element(),
                ));
            }
            slots.extend(pictures().map(|pictures| ("pictures", pictures.into_any_element())));
            if let Some(body) = self.entry_face("studio-user", index, None, data, slots, cx) {
                return div().id(("prompt", index)).w_full().child(body);
            }
        }
        let fill = theme.selected;
        let tops = self.bubble_tops.clone();
        let bubble = div()
            .id(("user", index))
            .debug_selector(move || format!("bubble-{index}"))
            .relative()
            // Where it stands, for the fade under the top edge to stop above it.
            .child(
                gpui::canvas(
                    move |bounds, _, _| tops.borrow_mut().push(bounds.top()),
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .min_w_0()
            .max_w(relative(0.85))
            .px(px(12.))
            .py(px(6.))
            .rounded(px(BUBBLE_RADIUS))
            .bg(fill)
            .text_size(px(PROSE))
            .line_height(px(LINE))
            .text_color(theme.fg)
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(
                (!message.text.is_empty()).then(|| scale::styled_text(message.text.clone(), runs)),
            )
            .children(message.files.iter().map(|path| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(SMALL))
                    .text_color(theme.fg_dim)
                    .child(icon(Icon::File, theme.fg_muted).size(px(12.)))
                    .child(div().min_w_0().truncate().child(scale::text(path.clone())))
            }));
        let line = div()
            .w_full()
            .flex()
            .justify_end()
            .items_start()
            .gap(px(4.))
            .child(
                div()
                    .flex_none()
                    .pt(px(4.))
                    .flex()
                    .gap(px(2.))
                    .child(copy)
                    .children(again),
            )
            .child(bubble);
        div()
            .id(("prompt", index))
            .group(group)
            .w_full()
            .child(exact::column(
                div().w_full().flex().flex_col().gap(px(6.)),
                pictures()
                    .map(IntoElement::into_any_element)
                    .into_iter()
                    .chain([line.into_any_element()]),
            ))
    }

    /// An answer: its prose alone, and a control at its right that copies
    /// it, there with the pointer.
    fn studio_answer(
        &self,
        index: usize,
        text: &str,
        blocks: &Rc<[Markdown]>,
        look: &Prose,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let theme = look.theme;
        let group = SharedString::from(format!("answer-{index}"));
        let key = format!("answer-{index}");
        let copied = self.copied.as_ref().is_some_and(|copied| copied == &key);
        if let Some(body) = self.entry_face("studio-answer", index, None,
            || serde_json::json!({"index": index, "copied": copied, "writing": self.writing(index)}),
            vec![("prose", prose::document(blocks, look).into_any_element())], cx) {
            return div().id(("answer", index)).w_full().child(body);
        }
        let text = text.to_owned();
        let copy = (!self.writing(index)).then(|| {
            quiet_action(
                ("copy-answer", index),
                if copied { Icon::Check } else { Icon::Copy },
                group.clone(),
                theme,
            )
            .absolute()
            .top_0()
            .right(px(-24.))
            .on_click(cx.listener(move |this, click, window, cx| {
                this.copy(key.clone().into(), text.clone(), cx);
                this.rest_focus(click, window, cx);
            }))
        });
        div()
            .id(("answer", index))
            .debug_selector(move || format!("reply-{index}"))
            .group(group)
            .relative()
            .w_full()
            .px(px(PROSE_INSET))
            .child(prose::document(blocks, look))
            .children(copy)
    }

    /// The agents this one started, as the prose may name them.
    fn mentions(&self, theme: &Theme) -> Rc<[prose::Mention]> {
        self.nest
            .agents
            .iter()
            .map(|agent| prose::Mention {
                id: agent.id.clone(),
                title: agent.title.clone(),
                brand: kou_ui::Brand::of_harness(&agent.harness)
                    .or_else(|| kou_ui::Brand::of(&agent.code)),
                letters: agent
                    .code
                    .chars()
                    .take(2)
                    .collect::<String>()
                    .to_uppercase(),
                color: theme.provider(&agent.harness),
            })
            .collect()
    }

    /// Opens an agent the prose names: the app is told.
    fn opener(&self, cx: &Context<Self>) -> prose::OpenFn {
        let view = cx.weak_entity();
        Rc::new(move |id, _, cx| {
            view.update(cx, |_, cx| {
                cx.emit(crate::view::ThreadViewEvent::OpenNested(id))
            })
            .ok();
        })
    }

    /// Reasoning: a quiet line, "Thinking" while it is written and
    /// "Thought" after, with what it is about; opened, the reasoning under it.
    fn studio_thought(
        &self,
        index: usize,
        blocks: &Rc<[Markdown]>,
        fades: Vec<(std::ops::Range<usize>, f32)>,
        lit: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let open = self.thought_open(index);
        let writing = self.writing(index);
        let gist = match self.thread.entries().get(index) {
            Some(Entry::Thought(text)) if writing => latest_gist(text),
            _ => markdown::first_line(blocks).unwrap_or("").to_owned(),
        };
        if self.chat_face.is_some() {
            let slots = open
                .then(|| {
                    (
                        "prose",
                        prose::document(
                            blocks,
                            &Prose::new(theme, Tone::reasoning(theme), self.field_focus())
                                .with_links(&self.header.cwd, self.link_opener(cx))
                                .studio()
                                .small()
                                .with_fades(fades.clone()),
                        )
                        .into_any_element(),
                    )
                })
                .into_iter()
                .collect();
            if let Some(body) = self.entry_face("studio-thought", index, None,
                || serde_json::json!({"index": index, "open": open, "writing": writing, "gist": gist.replace("**", "")}), slots, cx) {
                return div().w_full().child(body);
            }
        }
        let (hover, ring) = (theme.fg_dim, theme.accent);
        let line = div()
            .id("fold")
            .debug_selector(|| format!("thought-{index}"))
            .h(px(STEP))
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(kou_ui::metric::RADIUS))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .tab_index(0)
            .text_size(px(PROSE))
            .text_color(theme.fg_muted)
            .hover(move |style| style.text_color(hover))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener(move |this, click, window, cx| {
                this.toggle(index, !open, cx);
                this.rest_focus(click, window, cx);
            }))
            .child(
                div()
                    .flex_none()
                    .when(writing, |word| word.opacity(lit.max(0.45)))
                    .child(scale::text(if writing { "Thinking" } else { "Thought" })),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .italic()
                    .text_color(theme.fg_muted)
                    .child(scale::text(gist.replace("**", ""))),
            )
            .child(chevron(open, theme));
        div().w_full().child(line).children(open.then(|| {
            div()
                .mt(px(4.))
                .mb(px(4.))
                .pl(px(12.))
                .border_l_1()
                .border_color(theme.line_strong)
                .child(prose::document(
                    blocks,
                    &Prose::new(theme, Tone::reasoning(theme), self.field_focus())
                        .with_links(&self.header.cwd, self.link_opener(cx))
                        .studio()
                        .small()
                        .with_fades(fades),
                ))
        }))
    }

    /// A tool call as a quiet line: how it goes, what it does, on what, and
    /// what came of it. Opened, it is the card the cockpit draws a call as
    /// (see [`ThreadView::tool_card`]): the command in its head, what it
    /// gave under it; the line gives way to the card, and the card to the
    /// line, over a few frames.
    #[allow(clippy::too_many_arguments)]
    fn studio_tool(
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
        let asking = self.asking(index);
        let open = self.tool_open(index, call);
        // A subagent or background command can outlive the call that started
        // it. Its card owns the current status and elapsed time.
        if call.agent.is_some() || call.work.background {
            let level = self.unfold_level(index, open, now);
            return div()
                .w_full()
                .py(px(4.))
                .child(self.tool_card(index, call, pieces, light, lit, now, level, theme, cx));
        }
        let expandable =
            !call.content.is_empty() || call.raw_input.is_some() || call.raw_output.is_some();
        let running = call.status == ToolStatus::Running && asking.is_none();
        let (verb, input) = split_title(call);
        let input = input
            .trim_matches('`')
            .lines()
            .next()
            .unwrap_or("")
            .to_owned();
        let (added, removed) = pieces
            .iter()
            .fold((0, 0), |(added, removed), piece| match piece {
                Piece::Diff { diff, .. } => (added + diff.added, removed + diff.removed),
                _ => (added, removed),
            });
        let location = call.locations.first().and_then(|location| {
            let path = display_path(&location.path, Some(&self.header.cwd));
            (!input.contains(path.as_str())).then_some(path)
        });
        let took = match (
            self.motion.started.get(&index),
            self.motion.took.get(&index),
        ) {
            (Some(at), _) if running => Some(now.saturating_duration_since(*at)),
            (_, Some(took)) => Some(*took),
            _ => None,
        };
        let word = match (asking.is_some(), call.status) {
            (true, _) => Some(("Needs you", theme.blocked)),
            (false, ToolStatus::Failed) => Some(("Failed", theme.error)),
            (false, ToolStatus::Cancelled) => Some(("Stopped", theme.fg_muted)),
            (false, ToolStatus::Pending) => Some(("Queued", theme.fg_muted)),
            _ => None,
        };
        let glyph = match (asking.is_some(), call.status) {
            (true, _) => dot(theme.blocked, 7.).into_any_element(),
            (false, ToolStatus::Running) => {
                dot(theme.working.opacity(lit.max(0.3)), 7.).into_any_element()
            }
            (false, ToolStatus::Failed) => icon(Icon::Close, theme.error)
                .size(px(12.))
                .into_any_element(),
            (false, ToolStatus::Done) => icon(Icon::Check, theme.fg_faint)
                .size(px(12.))
                .into_any_element(),
            _ => div()
                .size(px(7.))
                .rounded_full()
                .border_1()
                .border_color(theme.fg_faint)
                .into_any_element(),
        };
        let (hover, ring) = (theme.hover, theme.accent);
        let head = div()
            .id("call")
            .debug_selector(|| format!("tool-{index}"))
            .h(px(STEP))
            .mx(px(-6.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(kou_ui::metric::RADIUS))
            .border_1()
            .border_color(gpui::transparent_black())
            .text_size(px(PROSE))
            .child(
                div()
                    .flex_none()
                    .w(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(glyph),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme.fg_dim)
                    .child(scale::text(verb.clone())),
            )
            .when(!input.is_empty(), |head| {
                head.child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .truncate()
                        .font_family(theme.mono_font.clone())
                        .text_size(px(SMALL))
                        .text_color(theme.fg_muted)
                        .child(scale::text(input.clone())),
                )
            })
            .children(location.clone().map(|path| {
                let mut place = div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .font_family(theme.mono_font.clone())
                    .text_size(px(SMALL))
                    .text_color(theme.fg_muted);
                place.style().flex_shrink = Some(1000.);
                place.child(scale::text(path))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .font_family(theme.mono_font.clone())
                    .text_size(px(SMALL))
                    .text_color(theme.fg_muted)
                    .when(added > 0, |meta| {
                        meta.child(
                            div()
                                .text_color(theme.ok)
                                .child(scale::text(format!("+{added}"))),
                        )
                    })
                    .when(removed > 0, |meta| {
                        meta.child(
                            div()
                                .text_color(theme.error)
                                .child(scale::text(format!("-{removed}"))),
                        )
                    })
                    .children(took.map(|took| scale::text(motion::duration(took))))
                    .children(word.map(|(word, color)| {
                        div()
                            .font_family(theme.ui_font.clone())
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
            })
            .into_any_element();
        let head = self.entry_face("studio-tool-head", index, None, || serde_json::json!({
            "index": index, "verb": verb, "input": input, "location": location,
            "added": added, "removed": removed, "took": took.map(motion::duration),
            "word": word.map(|(text, color)| serde_json::json!({"text": text, "color": crate::face::hex(color)})),
            "status": if asking.is_some() { "blocked" } else { match call.status {
                ToolStatus::Running => "working", ToolStatus::Done => "done", ToolStatus::Failed => "error", _ => "idle",
            }},
            "expandable": expandable, "open": open,
        }), Vec::new(), cx).unwrap_or(head);
        // Open, or on its way open or closed: the card, the line fading
        // over it as it grows from the line's height or shrinks back to it.
        let level = self.unfold_level(index, open, now);
        if expandable && (open || level > 0.) {
            let card = div().w_full().py(px(4.)).child(self.tool_card(
                index,
                call,
                pieces,
                light && level >= 1.,
                lit,
                now,
                1.,
                theme,
                cx,
            ));
            let card =
                crate::entries::revealed((index, 1), level, STEP, &self.unfold_heights, card);
            return div()
                .relative()
                .w_full()
                .child(card)
                .when(level < 1., |row| {
                    row.child(
                        div()
                            .id("line")
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .opacity(1. - level)
                            .child(head),
                    )
                });
        }
        // Closed, a call that runs shows the last lines its command printed.
        let live = (!open && running)
            .then(|| {
                pieces.iter().find_map(|piece| match piece {
                    Piece::Terminal { lines, .. } if !lines.is_empty() => Some(lines),
                    _ => None,
                })
            })
            .flatten()
            .map(|lines| {
                let tail = &lines[lines.len().saturating_sub(crate::entries::LIVE_TAIL)..];
                let text: Vec<&str> = tail.iter().map(|line| line.text.as_str()).collect();
                div()
                    .w_full()
                    .pl(px(22.))
                    .pb(px(2.))
                    .font_family(theme.mono_font.clone())
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(theme.fg_muted)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(scale::text(text.join("\n")))
            });
        let answer = asking.map(|pending| {
            // The questions a call asks, as the cockpit's card has them.
            if let Some(asks) = self.asks_of(pending) {
                return framed(theme)
                    .mt(px(6.))
                    .child(self.question_block(&asks, theme, cx));
            }
            let default = default_option(&pending.options);
            let request = pending.request;
            framed(theme)
                .mt(px(6.))
                .px(px(12.))
                .py(px(10.))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .children(pending.options.iter().enumerate().map(|(at, option)| {
                    let id = option.id.clone();
                    studio_answer_button(at, option.label.clone(), Some(at) == default, theme)
                        .debug_selector(|| format!("answer-{}", option.id))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.permit(request, id.clone(), cx);
                            this.focus_field(window, cx);
                        }))
                }))
                .children(default.map(|at| {
                    div()
                        .ml(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(SMALL))
                        .text_color(theme.fg_muted)
                        .child(key_hint(allow_key(), theme))
                        .child(scale::text(pending.options[at].label.to_lowercase()))
                }))
        });
        div().w_full().child(head).children(live).children(answer)
    }

    /// The last row: what the thread is doing when that is not the prompt
    /// area's to say. Under it, room for the prompt area.
    fn studio_tail(
        &self,
        cards: Option<AnyElement>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let agent = &self.header.agent;
        let line = |text: String| {
            if let Some(body) = self.entry_face(
                "studio-status",
                "tail",
                None,
                || serde_json::json!({"text": text}),
                Vec::new(),
                cx,
            ) {
                return body;
            }
            div()
                .min_h(px(LINE))
                .flex()
                .items_center()
                .text_size(px(PROSE))
                .text_color(theme.fg_muted)
                .child(scale::text(text))
                .into_any_element()
        };
        let content: Option<AnyElement> = match self.thread.state() {
            ThreadState::Starting => Some(line(format!(
                "Starting {agent} in {}…",
                display_path(&self.header.cwd, None)
            ))),
            ThreadState::Idle if self.thread.entries().is_empty() && cards.is_none() => {
                Some(self.studio_ready(theme, cx).into_any_element())
            }
            ThreadState::Idle => match &self.thread.last_stop {
                Some(Stop::Cancelled) => Some(line("Stopped. Send a message to go on.".into())),
                Some(Stop::Limit) => Some(line(format!(
                    "{agent} stopped at its output or turn limit. Send a message to go on."
                ))),
                Some(Stop::Refused) => Some(line(
                    "The model refused to continue. Rephrase the request and send it again.".into(),
                )),
                Some(Stop::Failed(_)) => Some(line("Send the message again to retry.".into())),
                Some(Stop::Finished) | None => None,
            },
            ThreadState::Working => None,
            ThreadState::Blocked => {
                let waiting = self.thread.permissions().len();
                Some(line(if waiting == 1 {
                    format!("{agent} is waiting for your answer above.")
                } else {
                    format!("{agent} is waiting for your answer to {waiting} requests above.")
                }))
            }
            ThreadState::Closed(Some(reason)) => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(PROSE))
                            .text_color(theme.error)
                            .child(scale::text(format!("{agent} stopped"))),
                    )
                    .child(tinted(reason, theme.error, theme))
                    .child(div().text_size(px(SMALL)).text_color(theme.fg_muted).child(
                        scale::text(
                            "Close this pane and open it again to start a new session. \
                                 The conversation above stays readable until then.",
                        ),
                    ))
                    .into_any_element(),
            ),
            ThreadState::Closed(None) => Some(line(
                "This session has ended. Open the pane again to start a new one.".into(),
            )),
        };
        let first = self.thread.entries().is_empty();
        self.studio_row(self.thread.entries().len()).child(
            self.studio_column()
                .pt(px(if first { TOP } else { GAP }))
                .pb(px(TAIL + self.dock_height))
                .children(cards)
                .children(content),
        )
    }

    /// A new session before anything was said: a quiet question in the
    /// middle of the column, and the folder under it.
    fn studio_ready(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        if let Some(body) = self.entry_face("studio-ready", "ready", None,
            || serde_json::json!({"agent": self.header.agent, "path": display_path(&self.header.cwd, None)}), Vec::new(), cx) {
            return div().w_full().child(body);
        }
        div()
            .w_full()
            .pt(px(72.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.fg)
                    .child(scale::text(format!(
                        "What should {} work on?",
                        self.header.agent
                    ))),
            )
            .child(
                div()
                    .text_size(px(PROSE))
                    .text_color(theme.fg_muted)
                    .child(scale::text(display_path(&self.header.cwd, None))),
            )
    }

    /// What `/clear` left: how much is out of sight, and a way back to it.
    fn studio_cleared(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let hidden = self.cleared;
        if let Some(body) = self.entry_face(
            "studio-cleared",
            "cleared",
            None,
            || serde_json::json!({"hidden": hidden}),
            Vec::new(),
            cx,
        ) {
            return div()
                .w_full()
                .flex()
                .justify_center()
                .child(self.studio_column().pt(px(TOP)).child(body));
        }
        let (hover, ring) = (theme.fg, theme.accent);
        div().w_full().flex().justify_center().child(
            self.studio_column().pt(px(TOP)).child(
                div()
                    .min_h(px(LINE))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(PROSE))
                    .text_color(theme.fg_muted)
                    .child(scale::text(if hidden == 1 {
                        "1 earlier entry is out of sight".to_owned()
                    } else {
                        format!("{hidden} earlier entries are out of sight")
                    }))
                    .child(
                        div()
                            .id("unclear")
                            .debug_selector(|| "unclear".into())
                            .rounded(px(kou_ui::metric::RADIUS))
                            .border_1()
                            .border_color(gpui::transparent_black())
                            .cursor_pointer()
                            .tab_index(0)
                            .text_color(theme.fg_dim)
                            .hover(move |style| style.text_color(hover))
                            .focus_visible(move |style| style.border_color(ring))
                            .child(scale::text("Show them"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.unclear(cx);
                                this.focus_field(window, cx);
                            })),
                    )
                    .child(div().flex_1().h(px(1.)).bg(theme.line)),
            ),
        )
    }
}

/// The runs of a prompt's text: the labels of its pictures in the accent,
/// the rest plain. A label that is out of order, past the text's end or not
/// on a letter's boundary is drawn plain: the runs always cover the text
/// exactly, each ending between two letters.
pub(crate) fn prompt_runs(
    text: &str,
    labels: &[std::ops::Range<usize>],
    theme: &Theme,
) -> Vec<TextRun> {
    let base = TextRun {
        len: 0,
        font: font(theme.ui_font.clone()),
        color: theme.fg,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut bold = font(theme.ui_font.clone());
    bold.weight = FontWeight::SEMIBOLD;
    let mut runs = Vec::new();
    let mut at = 0;
    for label in labels {
        let fits = label.start >= at
            && label.end > label.start
            && label.end <= text.len()
            && text.is_char_boundary(label.start)
            && text.is_char_boundary(label.end);
        if !fits {
            continue;
        }
        if label.start > at {
            runs.push(TextRun {
                len: label.start - at,
                ..base.clone()
            });
        }
        runs.push(TextRun {
            len: label.len(),
            font: bold.clone(),
            color: theme.accent,
            ..base.clone()
        });
        at = label.end;
    }
    if at < text.len() {
        runs.push(TextRun {
            len: text.len() - at,
            ..base
        });
    }
    runs
}

/// What an entry shows, for the air around what follows it.
fn shown_by(entry: &Entry) -> Shown {
    match entry {
        Entry::User(_) => Shown::Prompt,
        Entry::Assistant(_) | Entry::Notice(..) => Shown::Prose,
        Entry::Tool(_) | Entry::Thought(_) | Entry::Plan(_) => Shown::Step,
    }
}

/// A round mark of `side` in `color`.
pub(crate) fn dot(color: Hsla, side: f32) -> Div {
    div().flex_none().size(px(side)).rounded_full().bg(color)
}

/// What is framed in the column: a tool's content, a question.
pub(crate) fn framed(theme: &Theme) -> Div {
    div()
        .w_full()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(theme.line_strong)
        .bg(theme.raised)
        .overflow_hidden()
}

/// A box of text tinted by `tone`: a warning, an error.
fn tinted(text: &str, tone: Hsla, theme: &Theme) -> Div {
    div()
        .w_full()
        .px(px(12.))
        .py(px(9.))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(lights_mix(tone, theme.line, 0.35))
        .bg(tone.opacity(if theme.dark { 0.08 } else { 0.06 }))
        .text_size(px(PROSE))
        .line_height(px(LINE))
        .text_color(theme.fg)
        .child(scale::text(text.to_owned()))
}

/// A small control there with the pointer over `group`, or with the
/// keyboard on it: copy, edit.
pub(crate) fn quiet_action(
    id: (&'static str, usize),
    glyph: Icon,
    group: SharedString,
    theme: &Theme,
) -> gpui::Stateful<Div> {
    let (name, index) = id;
    let (hover, ring) = (theme.hover, theme.accent);
    div()
        .id(id)
        .debug_selector(move || format!("{name}-{index}"))
        .flex_none()
        .size(px(22.))
        .rounded(px(kou_ui::metric::RADIUS))
        .border_1()
        .border_color(gpui::transparent_black())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tab_index(0)
        .opacity(0.)
        .group_hover(group, |style| style.opacity(1.))
        .hover(move |style| style.bg(hover).opacity(1.))
        .focus_visible(move |style| style.border_color(ring).opacity(1.))
        .child(icon(glyph, theme.fg_muted).size(px(13.)))
}

/// One of the answers to a permission request: the default filled with the
/// text's colour, the others framed.
fn studio_answer_button(
    at: usize,
    text: String,
    primary: bool,
    theme: &Theme,
) -> gpui::Stateful<Div> {
    let ring = theme.accent;
    let base = div()
        .id(("answer", at))
        .flex_none()
        .h(px(28.))
        .px(px(12.))
        .rounded(px(kou_ui::metric::RADIUS))
        .border_1()
        .flex()
        .items_center()
        .cursor_pointer()
        .tab_index(0)
        .text_size(px(PROSE))
        .child(scale::text(text));
    if primary {
        let (fill, ink) = (theme.fg, theme.pane);
        base.bg(fill)
            .border_color(fill)
            .text_color(ink)
            .font_weight(FontWeight::MEDIUM)
            .hover(|style| style.opacity(0.9))
            .active(|style| style.opacity(0.8))
            .focus_visible(move |style| style.border_color(ring))
    } else {
        let (frame, lit) = (theme.fg_faint, theme.fg);
        base.border_color(theme.line_strong)
            .text_color(theme.fg_dim)
            .hover(move |style| style.border_color(frame).text_color(lit))
            .active(|style| style.opacity(0.8))
            .focus_visible(move |style| style.border_color(ring))
    }
}

/// The plan: what it is and how far along, and its steps.
fn studio_plan(steps: &[PlanStep], lit: f32, theme: &Theme) -> Div {
    let done = steps
        .iter()
        .filter(|step| step.status == PlanStatus::Done)
        .count();
    div()
        .w_full()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(STEP))
                .flex()
                .items_center()
                .text_size(px(PROSE))
                .text_color(theme.fg_muted)
                .child(scale::text(format!(
                    "Plan · {done} of {} done",
                    steps.len()
                ))),
        )
        .children(steps.iter().map(|step| {
            let (mark, color): (AnyElement, Hsla) = match step.status {
                PlanStatus::Pending => (
                    div()
                        .size(px(8.))
                        .rounded_full()
                        .border_1()
                        .border_color(theme.fg_faint)
                        .into_any_element(),
                    theme.fg_dim,
                ),
                PlanStatus::Running => (
                    dot(theme.working.opacity(lit), 8.).into_any_element(),
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
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .w(px(14.))
                        .h(px(LINE))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(mark),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(PROSE))
                        .line_height(px(LINE))
                        .text_color(color)
                        .when(step.status == PlanStatus::Done, |text| text.line_through())
                        .child(scale::text(step.text.clone())),
                )
        }))
}

/// A message from the backend: a quiet line, or a tinted box for a warning
/// and an error.
fn studio_notice(level: Level, text: &str, turn: bool, theme: &Theme) -> Div {
    match level {
        Level::Info => div()
            .w_full()
            .min_h(px(LINE))
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(px(SMALL))
            .text_color(theme.fg_muted)
            .child(div().min_w_0().child(scale::text(text.to_owned())))
            .child(div().flex_1().min_w(px(16.)).h(px(1.)).bg(theme.line)),
        Level::Warning => div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(SMALL))
                    .text_color(theme.blocked)
                    .child(scale::text("Warning")),
            )
            .child(tinted(text, theme.blocked, theme)),
        Level::Error => div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(SMALL))
                    .text_color(theme.error)
                    .child(scale::text(if turn { "Turn failed" } else { "Error" })),
            )
            .child(tinted(text, theme.error, theme)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `runs` cover `text` exactly, each ending between two letters.
    pub(crate) fn covers(text: &str, runs: &[TextRun]) -> bool {
        let mut at = 0;
        for run in runs {
            at += run.len;
            if run.len == 0 || !text.is_char_boundary(at) {
                return false;
            }
        }
        at == text.len()
    }

    #[test]
    fn a_prompt_s_runs_cover_its_text_whatever_its_labels() {
        let theme = studio_theme(&Theme::new(true));
        let text = "Сравни [Image 1] и 🦀 [Image 2] — ёж";
        let first = text.find("[Image 1]").unwrap();
        let second = text.find("[Image 2]").unwrap();
        let labels = [first..first + 9, second..second + 9];
        let runs = prompt_runs(text, &labels, theme);
        assert!(covers(text, &runs));
        assert_eq!(runs.len(), 5);
        assert_eq!(runs[1].color, theme.accent);
        // Labels a stale parse could give: inside a letter, past the end,
        // out of order. They are drawn plain, and nothing is cut in two.
        let crab = text.find('🦀').unwrap();
        let one = |range: std::ops::Range<usize>| vec![range];
        for labels in [
            one(crab + 1..crab + 3),
            one(first..text.len() + 4),
            vec![second..second + 9, first..first + 9],
            one(first..first),
        ] {
            let runs = prompt_runs(text, &labels, theme);
            assert!(covers(text, &runs), "{labels:?}");
        }
        assert!(prompt_runs("", &[], theme).is_empty());
    }
}
