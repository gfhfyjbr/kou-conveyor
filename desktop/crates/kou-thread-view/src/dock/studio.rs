//! The prompt area in the quieter look (`Look::Studio`), after the user's
//! reference picture: over the field, one line while the agent waits on the
//! agents it started (a spinner, how many, their titles, Stop) or works; the
//! field in a large rounded frame with the paperclip at its lower left and
//! the model and the round send button at its lower right. Its sizes are the
//! picture's, at its 1.7 pixels to the point (see [`crate::studio`]).

use super::{Dock, DockEvent, PickerKind, Press};
use crate::{
    lights::{Light, Lights},
    studio::{COLUMN, SIDE},
};
use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, Div, ExternalPaths, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use kou_thread::ThreadState;
use kou_ui::{
    Brand, Icon, Theme, brand, icon,
    scale::{self, px},
    tip,
};

/// The field's frame: its least height (the picture's 165 pixels, 97 points
/// round a 20-point line, grown with the field's line), its corners, and its
/// padding: over the text, at its left, at its right and under the foot.
/// Over the text it is as at its left, less than the picture's 15.6: at one
/// pixel to the point that much air over the first line read as a margin
/// left there by mistake, most of all over a notice.
const FRAME: f32 = 77. + crate::studio::FIELD_LINE;
const FRAME_RADIUS: f32 = kou_ui::metric::RADIUS_LARGE;
const FRAME_TOP: f32 = 9.6;
const FRAME_LEFT: f32 = 9.6;
const FRAME_RIGHT: f32 = 6.4;
const FRAME_BOTTOM: f32 = 7.8;
/// Under the frame, to the pane's bottom edge (the picture's 29 pixels).
const UNDER: f32 = 17.5;
/// The round send button (the picture's 47 pixels), and the paperclip's box.
const SEND: f32 = 27.6;
const ATTACH: f32 = 24.;
/// The line over the field: its height, the spinner's side and where the
/// words start.
const LINE: f32 = crate::studio::pitch(20.);
const SPINNER: f32 = 11.4;
const SPINNER_AT: f32 = 3.;
const WORDS_AT: f32 = 20.6;
/// The size of the line's words (the picture's 194 pixels for "Waiting on
/// 2 subagents"), and of the model's.
const SMALL: f32 = crate::studio::sized(10.6);
const MODEL: f32 = crate::studio::sized(12.5);

impl Dock {
    /// The prompt area in the quieter look.
    pub(super) fn studio_dock(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let queue = self.queue_element(window, cx);
        let measure = self.measure(cx);
        let line = self.studio_line(cx);
        // The model's list rises from its word at the right of the foot; with
        // no word there (nothing to choose from), from the field's right edge.
        let model_picker = self.model_picker_element(gpui::Anchor::BottomRight, window, cx);
        let (at_word, at_edge) = if self.model_word_shown() {
            (model_picker, None)
        } else {
            (None, model_picker)
        };
        let foot = self.foot.clone().map(|foot| foot(window, cx));
        let theme = crate::studio::studio_theme(kou_ui::theme(cx));
        let frame = self.studio_frame(window, foot, at_word, theme, cx);
        // Standing alone (an empty page's field), it is the frame and nothing
        // around it: no ground of its own, no air under it, and the sides the
        // app aligns what it puts under the field to.
        let standalone = self.standalone;
        let side = if standalone {
            crate::entries::EDGE
        } else {
            SIDE
        };
        let usage = (!standalone && !self.around.context_beside.get())
            .then(|| studio_usage(&self.sync.usage, theme))
            .flatten();
        let chrome = crate::studio::chrome_theme(theme);
        let help = self.help.then(|| self.help_element(window, chrome, cx));
        let inset = self.around.inset.get();
        let column = div()
            .relative()
            .w_full()
            .max_w(px(COLUMN + 2. * SIDE))
            .flex_none()
            .px(px(side))
            .pb(px(if standalone { 0. } else { UNDER }))
            .flex()
            .flex_col()
            .child(measure)
            // The line that says the agent works, then what waits for it: the
            // queue is a tray on the field's top edge, under that line.
            .children(line)
            .child(queue)
            .child(crate::trace::traced(
                &self.trace,
                crate::trace::Part::FieldFrame,
                frame,
            ))
            .children(usage)
            .children(help)
            .children(at_edge.map(|picker| div().absolute().top_0().right(px(side)).child(picker)));
        let band = div()
            .w_full()
            .flex_none()
            .pr(px(inset))
            .flex()
            .justify_center()
            .when(!standalone, |band| band.bg(theme.pane))
            .child(column);
        div()
            .id("dock")
            .when(!standalone, |dock| dock.size_full().justify_end())
            .when(standalone, |dock| dock.w_full())
            .flex()
            .flex_col()
            .text_color(theme.fg)
            .font_family(theme.ui_font.clone())
            .text_size(px(MODEL))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(|this, _: &crate::view::MenuUp, window, cx| {
                this.picker_step(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::view::MenuDown, window, cx| {
                this.picker_step(true, window, cx)
            }))
            .child(band)
            .into_any_element()
    }

    /// The line over the field: while the agent waits on agents it started,
    /// a spinner, how many it waits on, their titles cut to fit, and Stop;
    /// while it works otherwise, the spinner and "Working"; while it waits on
    /// the user, a still dot and what it waits for.
    fn studio_line(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let waiting = self.around.waiting.borrow().clone();
        if !self.busy() && waiting.is_empty() {
            self.activity = None;
            return None;
        }
        let theme = crate::studio::studio_theme(kou_ui::theme(cx));
        let blocked = self.sync.state == ThreadState::Blocked;
        let (epoch, scale) = (self.epoch, self.scale.clone());
        let mark: AnyElement = if blocked {
            div()
                .size(px(SPINNER))
                .flex()
                .items_center()
                .justify_center()
                .child(crate::studio::dot(theme.blocked, 6.))
                .into_any_element()
        } else {
            let color = if waiting.is_empty() {
                theme.fg_muted
            } else {
                theme.fg_dim
            };
            self.activity
                .get_or_insert_with(|| {
                    cx.new(|_| {
                        Lights::new(
                            Light::Spin {
                                color,
                                side: SPINNER,
                            },
                            epoch,
                            scale,
                        )
                    })
                })
                .clone()
                .into_any_element()
        };
        let (words, titles): (String, Option<String>) = match (blocked, waiting.is_empty()) {
            (true, _) => ("Waiting for your answer".to_owned(), None),
            (false, false) => (
                crate::nested::waiting_on(waiting.len()),
                Some(waiting.join(", ")),
            ),
            (false, true) => ("Working".to_owned(), None),
        };
        let busy = self.busy();
        let (hover, ring) = (theme.fg_dim, theme.accent);
        let stop = busy.then(|| {
            div()
                .id("waiting-stop")
                .debug_selector(|| "waiting-stop".into())
                .flex_none()
                .ml(px(10.6))
                .rounded(px(kou_ui::metric::RADIUS))
                .border_1()
                .border_color(gpui::transparent_black())
                .cursor_pointer()
                .tab_index(0)
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.fg)
                .hover(move |style| style.text_color(hover))
                .focus_visible(move |style| style.border_color(ring))
                .child(scale::text("Stop"))
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.emit(DockEvent::Cancel);
                    window.focus(&this.focus, cx);
                }))
        });
        Some(
            div()
                .debug_selector(|| "activity".into())
                .h(px(LINE))
                .flex()
                .items_center()
                .text_size(px(SMALL))
                .child(
                    div()
                        .flex_none()
                        .ml(px(SPINNER_AT))
                        .size(px(SPINNER))
                        .child(mark),
                )
                .child(
                    div()
                        .flex_none()
                        .ml(px(WORDS_AT - SPINNER_AT - SPINNER))
                        .text_color(if blocked { theme.blocked } else { theme.fg_dim })
                        .child(scale::text(words)),
                )
                .children(titles.map(|titles| {
                    div()
                        .debug_selector(|| "waiting-titles".into())
                        .min_w_0()
                        .flex_shrink(1.)
                        .ml(px(5.))
                        .truncate()
                        .text_color(theme.fg_muted)
                        .child(scale::text(titles))
                }))
                .children(stop)
                .into_any_element(),
        )
    }

    /// The field in its frame, its foot under it: the paperclip and what
    /// the app puts there at the left, the model and the send button at the
    /// right.
    fn studio_frame(
        &self,
        window: &Window,
        foot: Option<AnyElement>,
        model_picker: Option<AnyElement>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let _ = window;
        let closed = matches!(self.sync.state, ThreadState::Closed(_));
        let (accent, drop_fill) = (theme.accent, theme.selected);
        let (hover, ring) = (theme.hover, theme.accent);
        let attach = (!closed).then(|| {
            div()
                .id("attach")
                .debug_selector(|| "attach".into())
                .flex_none()
                .size(px(ATTACH))
                .ml(px(19. - FRAME_LEFT - ATTACH / 2.))
                .rounded_full()
                .border_1()
                .border_color(gpui::transparent_black())
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .tab_index(0)
                .hover(move |style| style.bg(hover))
                .focus_visible(move |style| style.border_color(ring))
                .tooltip(tip("Attach files"))
                .child(icon(Icon::Paperclip, theme.fg_muted).size(px(17.)))
                .on_click(cx.listener(|this, _, window, cx| this.attach_files(window, cx)))
        });
        let pickers = self.studio_pickers(model_picker, theme, cx);
        div()
            .id("frame")
            .debug_selector(|| "frame".into())
            .relative()
            .min_h(px(FRAME))
            .rounded(px(FRAME_RADIUS))
            .border_1()
            .border_color(theme.line)
            .bg(theme.raised)
            .pl(px(FRAME_LEFT))
            .pr(px(FRAME_RIGHT))
            .pt(px(FRAME_TOP))
            .pb(px(FRAME_BOTTOM))
            .flex()
            .flex_col()
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(accent).bg(drop_fill)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.composer
                    .update(cx, |composer, cx| composer.drop_paths(paths, window, cx));
            }))
            .child(
                div()
                    .pr(px(FRAME_LEFT - FRAME_RIGHT))
                    .child(self.composer.clone()),
            )
            .child(div().flex_1().min_h(px(6.)))
            .child(
                div()
                    .h(px(SEND))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .children(attach)
                            .children(foot.map(|foot| {
                                div()
                                    .debug_selector(|| "field-foot".into())
                                    .flex_none()
                                    .min_w_0()
                                    .child(foot)
                            })),
                    )
                    .children(pickers)
                    .child(self.studio_controls(theme, cx)),
            )
            .into_any_element()
    }

    /// Whether the foot shows the model's word: there is a model to choose.
    fn model_word_shown(&self) -> bool {
        !matches!(self.sync.state, ThreadState::Closed(_))
            && !self.choices(PickerKind::Model).0.is_empty()
    }

    /// The model and the mode as quiet words at the right of the foot, the
    /// model after the mark of who makes it: each opens its list, and the
    /// model's (`model_picker`, when it is open) ends at its word's right.
    fn studio_pickers(
        &self,
        mut model_picker: Option<AnyElement>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        if matches!(self.sync.state, ThreadState::Closed(_)) {
            return Vec::new();
        }
        let mut shown = Vec::new();
        for kind in [PickerKind::Model, PickerKind::Mode] {
            let (choices, current) = self.choices(kind);
            if choices.is_empty() {
                continue;
            }
            let chosen = current.and_then(|id| choices.iter().find(|choice| choice.id == id));
            let name: SharedString = match (chosen, current) {
                (Some(choice), _) => choice.name.trim_start_matches("Effort: ").to_owned().into(),
                (None, Some(id)) => id.to_owned().into(),
                (None, None) => kind.title().to_owned().into(),
            };
            let mark = (kind == PickerKind::Model).then(|| {
                let provider = current
                    .and_then(|id| crate::models::provider_of(id, &self.sync.providers))
                    .unwrap_or_default();
                maker_mark(&provider, theme)
            });
            let menu = self
                .picker
                .as_ref()
                .filter(|picker| picker.kind == kind)
                .map(|picker| {
                    let chrome = crate::studio::chrome_theme(theme);
                    self.picker_menu(kind, picker, choices, current, chrome, cx)
                });
            let (hover, ring) = (theme.hover, theme.accent);
            let control = div()
                .id(kind.id())
                .debug_selector(|| kind.id().to_owned())
                .h(px(SEND))
                .min_w_0()
                .max_w(px(240.))
                .px(px(2.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(kou_ui::metric::RADIUS))
                .border_1()
                .border_color(gpui::transparent_black())
                .cursor_pointer()
                .tab_index(0)
                .hover(move |style| style.bg(hover))
                .focus_visible(move |style| style.border_color(ring))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.toggle_picker(kind, window, cx)),
                )
                .children(mark)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(MODEL))
                        // The model brighter than the mode after it.
                        .text_color(if kind == PickerKind::Model {
                            theme.fg.opacity(0.85)
                        } else {
                            theme.fg_dim
                        })
                        .child(scale::text(name)),
                );
            shown.push(
                div()
                    .relative()
                    .min_w_0()
                    .when(kind == PickerKind::Mode, |mode| mode.ml(px(-11.5)))
                    .children(menu)
                    .children(
                        (kind == PickerKind::Model)
                            .then(|| model_picker.take())
                            .flatten()
                            .map(|list| div().absolute().top_0().right_0().child(list)),
                    )
                    .child(control)
                    .into_any_element(),
            );
        }
        shown
    }

    /// The round button at the end of the foot: send; while a turn runs,
    /// stop, or queue (and force, where the agent takes it) once something
    /// is written.
    fn studio_controls(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let busy = self.busy();
        let closed = matches!(self.sync.state, ThreadState::Closed(_));
        let typed = !self.draft_empty && self.editing.is_none();
        let row = div().flex_none().flex().items_center().gap(px(6.));
        let press = |action: fn(&mut Self, &mut Window, &mut Context<Self>)| -> Press {
            Box::new(
                cx.listener(move |this: &mut Self, _: &ClickEvent, window, cx| {
                    action(this, window, cx);
                    window.focus(&this.focus, cx);
                }),
            )
        };
        // While a turn runs with nothing written, the button rests: Stop is
        // on the line over the field.
        if busy && !typed {
            return row.child(round_button(
                "send",
                Icon::ArrowUp,
                theme.primary,
                theme.primary_ink.opacity(0.85),
                theme,
                None,
            ));
        }
        if busy {
            // Force is there for every agent: one that takes messages while
            // it works is handed this one, any other is stopped to take it.
            let force = round_button(
                "force",
                Icon::Bolt,
                theme.selected,
                theme.fg_dim,
                theme,
                Some(press(|this, window, cx| {
                    let text = this.composer.read(cx).text().to_owned();
                    this.submit(text, true, window, cx);
                })),
            );
            return row.child(force).child(round_button(
                "queue",
                Icon::ArrowUp,
                theme.primary,
                theme.primary_ink,
                theme,
                Some(press(|this, window, cx| {
                    let text = this.composer.read(cx).text().to_owned();
                    this.submit(text, false, window, cx);
                })),
            ));
        }
        let ready = self.sync.state == ThreadState::Idle && typed && !closed;
        let press = ready.then(|| {
            press(|this, window, cx| {
                let text = this.composer.read(cx).text().to_owned();
                this.submit(text, false, window, cx);
            })
        });
        let (fill, arrow) = if ready {
            (lighter(theme.primary, theme), theme.primary_ink)
        } else {
            (theme.primary, theme.primary_ink.opacity(0.85))
        };
        row.child(round_button(
            "send",
            Icon::ArrowUp,
            fill,
            arrow,
            theme,
            press,
        ))
    }
}

/// The figures under the field, quietly, when the card beside does not show
/// the context: what the backend reported, nothing estimated.
fn studio_usage(usage: &kou_thread::Usage, theme: &Theme) -> Option<Div> {
    let mut parts = Vec::new();
    match (usage.context_used, usage.context_limit) {
        (Some(used), Some(limit)) if limit > 0 => parts.push(format!(
            "Context {} / {}",
            crate::nested::tokens(used),
            crate::nested::tokens(limit)
        )),
        (Some(used), _) => parts.push(format!("Context {}", crate::nested::tokens(used))),
        (None, _) => {}
    }
    if let Some(input) = usage.input_tokens {
        parts.push(format!("{} in", crate::view::tokens(input)));
    }
    if let Some(output) = usage.output_tokens {
        parts.push(format!("{} out", crate::view::tokens(output)));
    }
    if let Some(cost) = usage.cost_usd {
        parts.push(if cost > 0. && cost < 0.01 {
            format!("${cost:.4}")
        } else {
            format!("${cost:.2}")
        });
    }
    (!parts.is_empty()).then(|| {
        div()
            .debug_selector(|| "usage".into())
            .h(px(LINE))
            .mt(px(2.))
            .px(px(SPINNER_AT))
            .flex()
            .items_center()
            .text_size(px(SMALL))
            .text_color(theme.fg_muted)
            .child(scale::text(parts.join(" · ")))
    })
}

/// The send button's fill once there is something to send: a step brighter.
fn lighter(send: Hsla, theme: &Theme) -> Hsla {
    crate::lights::mix(send, theme.fg, 0.16)
}

/// A round button of the foot, filled, with its glyph in `ink`; pressed,
/// it does what `press` does, and without it it is there but does nothing.
fn round_button(
    id: &'static str,
    glyph: Icon,
    fill: Hsla,
    ink: Hsla,
    theme: &Theme,
    press: Option<Press>,
) -> AnyElement {
    let ring = theme.accent;
    let mut button = div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex_none()
        .size(px(SEND))
        .rounded_full()
        .border_1()
        .border_color(gpui::transparent_black())
        .bg(fill)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph, ink).size(px(15.)));
    if let Some(press) = press {
        button = button
            .cursor_pointer()
            .tab_index(0)
            .hover(|style| style.opacity(0.9))
            .active(|style| style.opacity(0.75))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(press);
    }
    button.into_any_element()
}

/// The mark of who makes a model, in their colour, or their colour's dot.
fn maker_mark(provider: &str, theme: &Theme) -> AnyElement {
    let lower = provider.to_ascii_lowercase();
    let found = match lower.as_str() {
        "anthropic" | "claude" => Some(Brand::Claude),
        "openai" | "codex" => Some(Brand::OpenAi),
        "google" | "gemini" => Some(Brand::Gemini),
        "qwen" => Some(Brand::Qwen),
        _ => None,
    };
    match found {
        Some(found) => brand(found, 13., theme.provider(&lower)).into_any_element(),
        None => crate::studio::dot(theme.provider(&lower), 7.).into_any_element(),
    }
}

/// What the empty field says in the quieter look, at length and short.
pub(crate) fn placeholder(state: &ThreadState, _agent: &str) -> (SharedString, SharedString) {
    match state {
        ThreadState::Closed(_) => (
            "This session has ended".into(),
            "This session has ended".into(),
        ),
        _ => (
            "Ask anything. $ files, / commands".into(),
            "Ask anything".into(),
        ),
    }
}
