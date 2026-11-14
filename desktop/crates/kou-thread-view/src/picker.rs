//! The model picker, as the web cockpit's (`plugins/models/web/picker.js`,
//! `models.css`): a search field over the providers, each closed with how
//! many models it has and its newest; a provider opens on its models,
//! sliding in, and the way back slides the providers in again; typing
//! searches every provider at once. What it shows and what a key does is
//! [`crate::models`]; this draws it and moves it.
//!
//! The keyboard stays in the search field the whole time, as on the web:
//! ↑↓ move, → or ↵ open a provider, ← or ⌫ in an empty field go back, ↵
//! takes a model, Esc clears the search, then goes back, then closes.

use crate::{
    dock::{Dock, DockEvent},
    lights::mix,
    models::{Browse, Catalog, Line, Outcome},
};
use gpui::{
    Anchor, AnyElement, App, Bounds, Context, Div, ElementInputHandler, EntityInputHandler,
    FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Pixels, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Transformation, UTF16Selection, Window, actions, anchored, canvas, deferred, div,
    point, prelude::FluentBuilder as _, radians,
};
use kou_thread::Choice;
use kou_ui::{
    Icon, Theme, icon, key_hint, metric,
    motion::cockpit,
    scale::{self, px},
};
use std::{ops::Range, time::Instant};

/// The key context of the open model picker.
pub(crate) const PICKER_CONTEXT: &str = "ThreadModelPicker";

actions!(
    kou_thread_model_picker,
    [
        PickUp,
        PickDown,
        PickPageUp,
        PickPageDown,
        PickLeft,
        PickRight,
        PickEnter,
        PickBackspace,
        PickEscape,
        PickTab,
    ]
);

/// The picker's keys, which win over the pane's while it is open.
pub(crate) fn bindings() -> Vec<KeyBinding> {
    let context = Some(PICKER_CONTEXT);
    vec![
        KeyBinding::new("up", PickUp, context),
        KeyBinding::new("down", PickDown, context),
        KeyBinding::new("pageup", PickPageUp, context),
        KeyBinding::new("pagedown", PickPageDown, context),
        KeyBinding::new("left", PickLeft, context),
        KeyBinding::new("right", PickRight, context),
        KeyBinding::new("enter", PickEnter, context),
        KeyBinding::new("backspace", PickBackspace, context),
        KeyBinding::new("escape", PickEscape, context),
        KeyBinding::new("tab", PickTab, context),
        KeyBinding::new("shift-tab", PickTab, context),
    ]
}

/// The widest the picker gets (`place()`: 520px).
const WIDTH: f32 = 520.;
/// The most its list takes before it scrolls.
const LIST_MAX: f32 = 340.;

/// The picker while it is open.
pub(crate) struct ModelPicker {
    pub browse: Browse,
    pub catalog: Catalog,
    /// The search field's focus: the keyboard stays there.
    pub focus: FocusHandle,
    pub opened: Instant,
    /// The pane on its way out and when the next one began to come in.
    swap: Option<Swap>,
    scroll: ScrollHandle,
}

struct Swap {
    /// 1 for a provider opening, -1 for the way back.
    direction: f32,
    at: Instant,
    old: Browse,
    old_height: f32,
}

/// How tall a line of the picker is (`.mp-row`, `.mp-prov`, `.mp-back`, `.mp-group`).
fn height(line: &Line) -> f32 {
    match line {
        Line::Provider(_) => 48.,
        Line::Back(_) => 41.,
        Line::Heading(_) => 26.,
        Line::Model(_) | Line::Free(_) => 34.,
        Line::Nothing => 36.,
    }
}

fn pane_height(lines: &[Line]) -> f32 {
    (lines.iter().map(height).sum::<f32>() + 8.).min(LIST_MAX)
}

impl ModelPicker {
    pub(crate) fn new(
        models: &[Choice],
        providers: &[kou_thread::Provider],
        current: Option<&str>,
        now: Instant,
        cx: &mut App,
    ) -> Self {
        let catalog = Catalog::of(models, providers);
        let browse = Browse::opening(&catalog, models, current);
        Self {
            browse,
            catalog,
            focus: cx.focus_handle(),
            opened: now,
            swap: None,
            scroll: ScrollHandle::new(),
        }
    }
}

impl Dock {
    /// Opens the model picker on the model in force, or closes it.
    pub(crate) fn toggle_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_picker.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        self.picker = None;
        let now = cx.background_executor().now();
        let picker = ModelPicker::new(
            &self.sync.models,
            &self.sync.providers,
            self.sync.model.as_deref(),
            now,
            cx,
        );
        window.focus(&picker.focus, cx);
        self.model_picker = Some(picker);
        self.reveal_cursor();
        cx.notify();
    }

    pub(crate) fn close_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_picker.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Does what a key or a click came to.
    fn picked(
        &mut self,
        outcome: Outcome,
        old: Browse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Outcome::Ignored => return,
            Outcome::Chosen(id) => {
                cx.emit(DockEvent::SetModel(id));
                self.close_model_picker(window, cx);
                return;
            }
            Outcome::Closed => {
                self.close_model_picker(window, cx);
                return;
            }
            Outcome::Swapped(direction) => {
                let now = cx.background_executor().now();
                if let Some(picker) = &mut self.model_picker
                    && !cx.reduce_motion()
                {
                    let models = &self.sync.models;
                    let old_height = pane_height(&old.lines(&picker.catalog, models));
                    picker.swap = Some(Swap {
                        direction: f32::from(direction),
                        at: now,
                        old,
                        old_height,
                    });
                }
            }
            Outcome::Moved => {}
        }
        self.reveal_cursor();
        cx.notify();
    }

    /// Keeps the line the cursor is on in sight.
    fn reveal_cursor(&self) {
        let Some(picker) = &self.model_picker else {
            return;
        };
        let lines = picker.browse.lines(&picker.catalog, &self.sync.models);
        let nth = picker.browse.cursor;
        if let Some(at) = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.choosable())
            .nth(nth)
            .map(|(at, _)| at)
        {
            picker.scroll.scroll_to_item(at);
        }
    }

    /// A key, as the search field takes it.
    pub(crate) fn picker_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.model_picker else {
            return;
        };
        let old = picker.browse.clone();
        let outcome = picker.browse.key(key, &picker.catalog, &self.sync.models);
        self.picked(outcome, old, window, cx);
    }

    /// The pointer over line `nth` of those the cursor stops on.
    fn picker_hover(&mut self, nth: usize, cx: &mut Context<Self>) {
        if let Some(picker) = &mut self.model_picker {
            let lines = picker.browse.lines(&picker.catalog, &self.sync.models);
            if picker.browse.hover(nth, &lines) == Outcome::Moved {
                cx.notify();
            }
        }
    }

    /// A click on line `nth`: it is taken.
    fn picker_click(&mut self, nth: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.model_picker else {
            return;
        };
        let old = picker.browse.clone();
        picker.browse.cursor = nth;
        let outcome = picker.browse.take(&picker.catalog, &self.sync.models);
        self.picked(outcome, old, window, cx);
        window.focus(
            &self
                .model_picker
                .as_ref()
                .map_or(self.focus.clone(), |picker| picker.focus.clone()),
            cx,
        );
    }

    /// The way back, clicked.
    fn picker_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.model_picker else {
            return;
        };
        let old = picker.browse.clone();
        let outcome = picker.browse.back(&picker.catalog, &self.sync.models);
        self.picked(outcome, old, window, cx);
        if let Some(picker) = &self.model_picker {
            window.focus(&picker.focus, cx);
        }
    }

    /// The picker over the model's control, laid out after everything else:
    /// its `corner` stands where the element is placed, so that it rises
    /// from the control whichever end of the foot the control is at.
    pub(crate) fn model_picker_element(
        &mut self,
        corner: Anchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let now = cx.background_executor().now();
        let still = cx.reduce_motion();
        let models = self.sync.models.clone();
        let current = self.sync.model.clone();
        let picker = self.model_picker.as_mut()?;
        // What moves: the box rising, a pane sliding in, its rows one by one.
        let progress = |since: Instant, delay: f32, length: f32| -> f32 {
            if still {
                return 1.;
            }
            let t = (now.saturating_duration_since(since).as_secs_f32() - delay) / length;
            t.clamp(0., 1.)
        };
        let rise = cockpit::ease_out(progress(picker.opened, 0., cockpit::MENU.as_secs_f32()));
        if picker
            .swap
            .as_ref()
            .is_some_and(|swap| progress(swap.at, 0., 0.7) >= 1.)
        {
            picker.swap = None;
        }
        let moving = rise < 1. || picker.swap.is_some();
        if moving && !still {
            window.request_animation_frame();
        }
        let lines = picker.browse.lines(&picker.catalog, &models);
        let groups = picker.catalog.groups.clone();
        let flat = picker.catalog.flat;
        let cursor = picker.browse.cursor;
        let query = picker.browse.query.clone();
        let open = picker.browse.open;
        let focus = picker.focus.clone();
        let scroll = picker.scroll.clone();
        let swap = picker.swap.as_ref().map(|swap| {
            (
                swap.direction,
                swap.at,
                swap.old.lines(&picker.catalog, &models),
                swap.old_height,
            )
        });
        let unit = px(1.);
        let cx = &*cx;
        let theme = crate::studio::chrome_look_theme(self.around.look.get(), cx);
        let holds_current = |group: usize| {
            current.as_deref().is_some_and(|current| {
                groups[group]
                    .models
                    .iter()
                    .any(|at| models[*at].id == current)
            })
        };

        // The lines of a pane: those of the pane shown, or of one on its way out.
        let render_lines = |lines: &[Line], live: bool| {
            let mut nth = 0;
            let mut rows: Vec<AnyElement> = Vec::new();
            for (at, line) in lines.iter().enumerate() {
                let selected = live && line.choosable() && nth == cursor;
                let element = match line {
                    Line::Provider(group) => provider_row(
                        &groups[*group],
                        &models,
                        holds_current(*group),
                        selected,
                        theme,
                    )
                    .into_any_element(),
                    Line::Back(group) => {
                        back_row(&groups[*group], theme, live, cx).into_any_element()
                    }
                    Line::Heading(group) => {
                        let hits = lines[at + 1..]
                            .iter()
                            .take_while(|line| matches!(line, Line::Model(_)))
                            .count();
                        heading_row(&groups[*group], hits, theme).into_any_element()
                    }
                    Line::Model(index) => model_row(
                        &models[*index],
                        current.as_deref() == Some(models[*index].id.as_str()),
                        selected,
                        theme,
                    )
                    .into_any_element(),
                    Line::Free(id) => free_row(id, selected, theme).into_any_element(),
                    Line::Nothing => div()
                        .h(px(36.))
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .text_size(px(12.5))
                        .text_color(theme.fg_muted)
                        .child(scale::text("No model matches."))
                        .into_any_element(),
                };
                let element = if live && line.choosable() {
                    let index = nth;
                    div()
                        .id(("model-line", at))
                        .debug_selector(move || format!("model-line-{index}"))
                        .cursor_pointer()
                        .on_mouse_move(
                            cx.listener(move |this, _, _, cx| this.picker_hover(index, cx)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.picker_click(index, window, cx)
                        }))
                        .child(element)
                        .into_any_element()
                } else {
                    element
                };
                // A new pane's rows come in one after another (`swap`).
                let element = match (&swap, live) {
                    (Some((_, since, _, _)), true) if at < 16 => {
                        let delay = cockpit::ROW_DELAY.as_secs_f32()
                            + cockpit::ROW_STAGGER.as_secs_f32() * at as f32;
                        let t =
                            cockpit::glide(progress(*since, delay, cockpit::ROW_IN.as_secs_f32()));
                        div()
                            .relative()
                            .top(unit * (6. * (1. - t)))
                            .opacity(t)
                            .child(element)
                            .into_any_element()
                    }
                    _ => element,
                };
                if line.choosable() {
                    nth += 1;
                }
                rows.push(element);
            }
            rows
        };
        let rows = render_lines(&lines, true);
        let new_height = pane_height(&lines);
        let (list_height, ghost, offset_in, fade_in) = match &swap {
            Some((direction, since, old, old_height)) => {
                let t_in = cockpit::glide(progress(*since, 0., cockpit::PANE_IN.as_secs_f32()));
                let t_out = cockpit::glide(progress(*since, 0., cockpit::PANE_OUT.as_secs_f32()));
                let t_height =
                    cockpit::glide(progress(*since, 0., cockpit::PANE_HEIGHT.as_secs_f32()));
                let shift = cockpit::PANE_SHIFT * direction;
                let ghost = (t_out < 1.).then(|| {
                    div()
                        .absolute()
                        .top_0()
                        .left(unit * (-shift * t_out))
                        .w_full()
                        .py(px(4.))
                        .opacity(1. - t_out)
                        .children(render_lines(old, false))
                });
                (
                    old_height + (new_height - old_height) * t_height,
                    ghost,
                    shift * (1. - t_in),
                    t_in,
                )
            }
            None => (new_height, None, 0., 1.),
        };

        let models_count = models.len();
        let providers_count = groups.len();
        let count: SharedString = if flat {
            format!("{models_count} models").into()
        } else {
            format!("{models_count} models · {providers_count} providers").into()
        };
        let keys: Vec<(&str, &str)> = if open.is_some() {
            vec![("←", "back"), ("↑↓", ""), ("↵", "")]
        } else {
            vec![("↑↓", ""), ("↵", "open"), ("esc", "")]
        };
        let view = cx.entity();
        let search = div()
            .h(px(40.))
            .px(px(14.))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme.line)
            .font_family(theme.mono_font.clone())
            .text_size(px(13.))
            .child(if query.is_empty() {
                div().flex().items_center().child(caret(theme)).child(
                    div()
                        .ml(px(3.))
                        .text_color(theme.fg_muted)
                        .child(scale::text(
                            "Search models and providers, or type a model ID",
                        )),
                )
            } else {
                div()
                    .flex()
                    .items_center()
                    .text_color(theme.fg)
                    .child(scale::text(query.clone()))
                    .child(caret(theme))
            })
            .child(
                // The field takes what is typed, in any script, through the
                // platform's input as the prompt field does.
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, view.clone()),
                            cx,
                        );
                    },
                )
                .absolute()
                .size_0(),
            );
        let list = div()
            .relative()
            .h(unit * list_height)
            .overflow_hidden()
            .children(ghost)
            .child(
                div()
                    .id("model-lines")
                    .relative()
                    .left(unit * offset_in)
                    .opacity(fade_in)
                    .size_full()
                    .py(px(4.))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .children(rows),
            );
        let picker = self.model_picker.as_ref()?;
        let width = window.viewport_size().width.as_f32() / scale::factor() - 16.;
        let card = div()
            .id("model-picker-box")
            .debug_selector(|| "model-choices".into())
            .key_context(PICKER_CONTEXT)
            .track_focus(&picker.focus)
            .occlude()
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.close_model_picker(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PickUp, window, cx| this.picker_key("up", window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PickDown, window, cx| this.picker_key("down", window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PickPageUp, window, cx| {
                    this.picker_key("pageup", window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &PickPageDown, window, cx| {
                this.picker_key("pagedown", window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &PickLeft, window, cx| this.picker_key("left", window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PickRight, window, cx| this.picker_key("right", window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PickEnter, window, cx| this.picker_key("enter", window, cx)),
            )
            .on_action(cx.listener(|this, _: &PickBackspace, window, cx| {
                this.picker_key("backspace", window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &PickEscape, window, cx| {
                    this.picker_key("escape", window, cx)
                }),
            )
            .on_action(cx.listener(|_, _: &PickTab, _, _| {}))
            .relative()
            .top(unit * (6. * (1. - rise)))
            .opacity(rise)
            .w(px(WIDTH.min(width.max(240.))))
            .rounded(px(metric::RADIUS_LARGE))
            .border_1()
            .border_color(theme.line_strong)
            .bg(theme.raised)
            .shadow(shadow(theme))
            .overflow_hidden()
            .flex()
            .flex_col()
            .text_color(theme.fg)
            .child(
                // `.mp-head`: what it is for, and its keys.
                div()
                    .px(px(14.))
                    .pt(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(label("Model", theme.fg_muted, theme))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .font_family(theme.mono_font.clone())
                            .text_size(px(11.))
                            .text_color(theme.fg_muted)
                            .child(scale::text("for this session's next prompts")),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .font_family(theme.mono_font.clone())
                            .text_size(px(10.5))
                            .text_color(theme.fg_muted)
                            .children(keys.into_iter().map(|(key, word)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(3.))
                                    .child(key_hint(key, theme))
                                    .children((!word.is_empty()).then(|| scale::text(word)))
                            })),
                    ),
            )
            .child(search)
            .child(list)
            .child(
                // `.mp-foot`: how many there are.
                div()
                    .h(px(36.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(theme.line)
                    .font_family(theme.mono_font.clone())
                    .text_size(px(10.5))
                    .text_color(theme.fg_muted)
                    .child(scale::text(count)),
            );
        Some(
            deferred(
                anchored()
                    .anchor(corner)
                    .offset(point(px(0.), px(-6.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

/// The picker's search caret.
fn caret(theme: &Theme) -> Div {
    div().w(px(1.5)).h(px(16.)).bg(theme.fg)
}

/// What floats casts a shadow (`--shadow`: `0 24px 64px`, half black).
pub(crate) fn shadow(theme: &Theme) -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow {
        color: gpui::black().opacity(if theme.dark { 0.5 } else { 0.18 }),
        offset: point(px(0.), px(24.)),
        blur_radius: px(64.),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// A label in capitals of the monospace (`.label`).
pub(crate) fn label(text: &str, color: Hsla, theme: &Theme) -> Div {
    div()
        .flex_none()
        .font_family(theme.mono_font.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(10.5))
        .text_color(color)
        .child(scale::text(text.to_uppercase()))
}

/// A chip that flags a line: current, media (`.mp-flag`).
fn flag(text: &str, accent: bool, theme: &Theme) -> Div {
    div()
        .flex_none()
        .px(px(5.))
        .h(px(16.))
        .flex()
        .items_center()
        .rounded(px(metric::RADIUS_SMALL))
        .border_1()
        .border_color(if accent {
            mix(theme.line_strong, theme.accent, 0.5)
        } else {
            theme.line_strong
        })
        .font_family(theme.mono_font.clone())
        .text_size(px(10.))
        .text_color(if accent { theme.accent } else { theme.fg_muted })
        .child(scale::text(text.to_uppercase()))
}

/// The dot of a provider's colour.
fn dot(provider: &str, side: f32, theme: &Theme) -> Div {
    div()
        .flex_none()
        .size(px(side))
        .rounded(px(side / 2.))
        .bg(theme.provider(provider))
}

/// A line's shape: a rounded block when the cursor is on it (`.mp-row`).
fn line_box(height: f32, selected: bool, theme: &Theme) -> Div {
    div()
        .mx(px(4.))
        .min_h(px(height))
        .pl(px(10.))
        .pr(px(8.))
        .flex()
        .items_center()
        .gap(px(10.))
        .rounded(px(metric::RADIUS))
        .when(selected, |line| line.bg(theme.hover))
}

fn provider_row(
    group: &crate::models::Group,
    models: &[Choice],
    current: bool,
    selected: bool,
    theme: &Theme,
) -> Div {
    let newest: Vec<&str> = group
        .models
        .iter()
        .take(3)
        .map(|at| models[*at].name.as_str())
        .collect();
    let more = if group.models.len() > newest.len() {
        " …"
    } else {
        ""
    };
    let arrow = icon(
        Icon::ChevronRight,
        if selected {
            theme.accent
        } else {
            theme.fg_faint
        },
    )
    .size(px(12.));
    line_box(48., selected, theme)
        .debug_selector({
            let id = group.id.clone();
            move || format!("provider-{id}")
        })
        .child(dot(&group.id, 9., theme))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(13.5))
                        .line_height(px(16.))
                        .text_color(if selected {
                            mix(theme.fg, theme.provider(&group.id), 0.72)
                        } else {
                            theme.fg
                        })
                        .child(scale::text(group.name.clone())),
                )
                .child(
                    div()
                        .truncate()
                        .font_family(theme.mono_font.clone())
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .text_color(if selected {
                            theme.fg_dim
                        } else {
                            theme.fg_muted
                        })
                        .child(scale::text(format!("{}{more}", newest.join(" · ")))),
                ),
        )
        .children(current.then(|| flag("current", true, theme)))
        .child(
            div()
                .flex_none()
                .min_w(px(18.))
                .flex()
                .justify_end()
                .font_family(theme.mono_font.clone())
                .font_weight(FontWeight::BOLD)
                .text_size(px(12.))
                .text_color(theme.fg_muted)
                .child(scale::text(group.models.len().to_string())),
        )
        .child(
            div()
                .flex_none()
                .w(px(14.))
                .flex()
                .justify_center()
                .when(selected, |arrow| arrow.ml(px(3.)))
                .child(arrow),
        )
}

fn back_row(group: &crate::models::Group, theme: &Theme, live: bool, cx: &Context<Dock>) -> Div {
    let (frame, lit) = (theme.fg_faint, theme.fg);
    let back = div()
        .id(if live { "model-back" } else { "model-back-old" })
        .debug_selector(|| "model-back".into())
        .flex_none()
        .h(px(24.))
        .pl(px(6.))
        .pr(px(9.))
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(theme.line_strong)
        .font_family(theme.mono_font.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(10.))
        .text_color(theme.fg_muted)
        .cursor_pointer()
        .hover(move |style| style.border_color(frame).text_color(lit))
        .child(
            icon(Icon::ChevronRight, theme.fg_muted)
                .size(px(11.))
                .with_transformation(Transformation::rotate(radians(std::f32::consts::PI))),
        )
        .child(scale::text("PROVIDERS"))
        .when(live, |back| {
            back.on_click(cx.listener(|this, _, window, cx| this.picker_back(window, cx)))
        });
    let count = group.models.len();
    div()
        .h(px(41.))
        .pl(px(8.))
        .pr(px(14.))
        .flex()
        .items_center()
        .gap(px(10.))
        .border_b_1()
        .border_color(theme.line)
        .mb(px(4.))
        .child(back)
        .child(dot(&group.id, 9., theme))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(13.5))
                .text_color(mix(theme.fg, theme.provider(&group.id), 0.72))
                .child(scale::text(group.name.clone())),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme.mono_font.clone())
                .text_size(px(11.))
                .text_color(theme.fg_muted)
                .child(scale::text(format!(
                    "{count} model{}",
                    if count == 1 { "" } else { "s" }
                ))),
        )
}

fn heading_row(group: &crate::models::Group, hits: usize, theme: &Theme) -> Div {
    div()
        .h(px(26.))
        .px(px(14.))
        .pt(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .font_family(theme.mono_font.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(10.))
        .text_color(mix(theme.fg_muted, theme.provider(&group.id), 0.7))
        .child(scale::text(group.name.to_uppercase()))
        .child(
            div()
                .font_weight(FontWeight::NORMAL)
                .text_color(theme.fg_muted)
                .child(scale::text(hits.to_string())),
        )
}

fn model_row(model: &Choice, current: bool, selected: bool, theme: &Theme) -> Div {
    let provider = crate::models::provider_of(&model.id, &[]).unwrap_or_default();
    let media = model
        .description
        .as_deref()
        .is_some_and(|description| description.contains("makes media"));
    line_box(34., selected, theme)
        .child(dot(&provider, 7., theme))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .max_w(px(260.))
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(13.))
                        .text_color(if current { theme.accent } else { theme.fg })
                        .child(scale::text(model.name.clone())),
                )
                .children((model.name != model.id).then(|| {
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme.mono_font.clone())
                        .text_size(px(11.5))
                        .text_color(theme.fg_muted)
                        .child(scale::text(model.id.clone()))
                })),
        )
        .children(current.then(|| flag("current", true, theme)))
        .children(media.then(|| flag("media", false, theme)))
}

fn free_row(id: &str, selected: bool, theme: &Theme) -> Div {
    line_box(34., selected, theme)
        .child(dot("", 7., theme))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .font_family(theme.mono_font.clone())
                        .text_size(px(12.5))
                        .text_color(theme.fg)
                        .child(scale::text(format!("Use “{id}”"))),
                )
                .child(
                    div()
                        .font_family(theme.mono_font.clone())
                        .text_size(px(11.5))
                        .text_color(theme.fg_muted)
                        .child(scale::text("not listed")),
                ),
        )
}

/// The search field takes text from the platform's input, so any script and
/// any input method work in it. The text only grows at its end and shrinks
/// by Backspace: there is no cursor to move.
impl EntityInputHandler for Dock {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let query = &self.model_picker.as_ref()?.browse.query;
        let units: Vec<u16> = query.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        adjusted.replace(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self
            .model_picker
            .as_ref()?
            .browse
            .query
            .encode_utf16()
            .count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = &mut self.model_picker else {
            return;
        };
        // Line breaks and tabs are keys here, not text.
        let typed: String = text.chars().filter(|ch| !ch.is_control()).collect();
        if typed.is_empty() {
            return;
        }
        let old = picker.browse.clone();
        let query = format!("{}{typed}", picker.browse.query);
        let outcome = picker.browse.typed(query);
        self.picked(outcome, old, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range, text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
