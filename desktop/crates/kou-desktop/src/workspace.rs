//! Everything right of the rail: the tab strip and the session's panes, as
//! tabs that split or as a canvas. The screens that stand in for them (the
//! harness picker, the settings, the empty states) are in `screens.rs`.
//!
//! Nothing here glides. A tab, a split, a layout is simply there when asked
//! for; the one exception is a pane made a moment ago, which comes in once.

use crate::plugins::icon_file;
use crate::{
    rail::{WINDOW_BUTTONS, mark},
    shell::{
        Content, Dismiss, FocusNext, FocusPrevious, LeavePane, Live, NewAgent, NewShell, NextTab,
        PreviousTab, Quit, RenameTab, Shell, ShowCanvas, ShowPanes, ShowTab1, ShowTab2, ShowTab3,
        ShowTab4, ShowTab5, ShowTab6, ShowTab7, ShowTab8, ShowTab9, ShowUsage, SplitDown,
        SplitRight, SwapPane, SwapPaneBack, SwitchInterface, ToggleRail, abbreviate,
    },
    state::{Axis, Id, Layout, Pane, PaneKind, Place, RAIL_WIDTHS, Session, Tab},
};
use gpui::{
    AnimationExt as _, AnyElement, Context, CursorStyle, Div, DragMoveEvent, Empty, FontWeight,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, WindowControlArea, canvas, div,
    prelude::FluentBuilder as _, px,
};
use kou_ui::{
    Icon, Mark, Theme, button, icon, icon_button, icon_button_sized, metric, monogram, motion,
    theme, tip,
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The line between the two parts of a split, in pixels.
const GAP: f32 = 1.;
/// How far to either side of that line the pointer can take hold of it.
const GRIP: f32 = 4.;
/// How long a pane made a moment ago takes to come in.
const COME_IN: Duration = Duration::from_millis(160);

/// What a divider carries while it is dragged: which split it divides, and
/// the room that split has, which the pointer is measured against.
#[derive(Clone)]
struct DividerDrag {
    session: Id,
    tab: usize,
    path: Vec<bool>,
    axis: Axis,
    room: Place,
}

/// The rail's edge, while it is dragged.
#[derive(Clone)]
struct RailDrag;

// A drag of an edge shows nothing under the pointer: the edge itself moves.
struct NothingDragged;

impl Render for NothingDragged {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

// The place in a tab's splits of a half that is still being chosen for:
// no pane has this id.
const CHOOSING: &str = "\u{0}choosing";

/// What the account in a pane's header says under the pointer.
const ACCOUNT_TIP: &str =
    "The account it runs under. Press to go on under the one with the most left of its limits.";

// Tabs share the strip like a browser's: one width for all, within limits.
fn tab_width(room: f32, tabs: usize) -> f32 {
    ((room - 200.) / tabs.max(1) as f32).clamp(104., 196.)
}

/// How wide the panel a tab is named in is.
const TAB_EDITOR_WIDTH: f32 = 268.;

// Where the panel of the tab at `at` stands: under its tab, kept inside the
// window.
fn tab_editor_left(room: f32, tabs: usize, at: usize) -> f32 {
    let step = tab_width(room, tabs) + 2.;
    (step * at as f32).clamp(0., (room - TAB_EDITOR_WIDTH - 16.).max(0.))
}

// The state a tab shows: the most pressing among its panes.
fn pressing(marks: impl Iterator<Item = Mark>) -> Mark {
    let rank = |mark: Mark| match mark {
        Mark::Blocked => 4,
        Mark::Failed => 3,
        Mark::Working => 2,
        Mark::Done => 1,
        Mark::Idle => 0,
    };
    marks.max_by_key(|mark| rank(*mark)).unwrap_or(Mark::Idle)
}

impl Shell {
    /// How wide the rail is now: nothing when hidden.
    pub(crate) fn rail_width(&self) -> f32 {
        if self.store.rail_hidden {
            0.
        } else {
            self.store.rail_width.clamp(RAIL_WIDTHS.0, RAIL_WIDTHS.1)
        }
    }

    /// The tab strip, as the `workspace` plugin draws it (its `tabs` view),
    /// or as the app does when the plugin cannot.
    pub(crate) fn tab_strip(
        &self,
        session: Option<&Session>,
        room: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let data = self.tabs_data(session, room);
        match self.chrome("tabs", "", data, Vec::new, theme, cx) {
            Some(drawn) => drawn,
            None => self
                .native_tab_strip(session, room, theme, cx)
                .into_any_element(),
        }
    }

    /// What the tab strip shows, as its plugin is given it.
    fn tabs_data(&self, session: Option<&Session>, room: f32) -> Value {
        let pending = session.map(|session| {
            (
                self.pending_tab(&session.id).map(|picker| picker.front),
                session.tabs.is_empty(),
            )
        });
        let (waiting, empty) = pending.unwrap_or((None, false));
        let waiting_front = !self.page_open() && (waiting == Some(true) || empty);
        let layout = session
            .filter(|_| !waiting_front)
            .map(|session| session.layout());
        let mut tabs = Vec::new();
        let mut width = 0.;
        let shown = waiting.is_some() || empty;
        if let Some(session) = session {
            width = tab_width(room, session.tabs.len() + usize::from(shown));
            let front = session.tab();
            for (index, tab) in session.tabs.iter().enumerate() {
                let Some(pane) = self.store.pane(&tab.active) else {
                    continue;
                };
                let panes = tab.root.panes();
                let code = self
                    .harness(&pane.harness)
                    .map_or_else(|| pane.harness.clone(), |harness| harness.code.clone());
                let state = pressing(
                    panes
                        .iter()
                        .filter_map(|pane| self.live.get(*pane))
                        .map(Live::shown),
                );
                tabs.push(json!({
                    "pane": pane.id,
                    "session": session.id,
                    "index": index,
                    "name": self.tab_name(tab),
                    "code": code,
                    "icon": tab.icon.as_deref().and_then(kou_ui::tab_icon).map(icon_file),
                    "active": !self.page_open() && !waiting_front && front == Some(index),
                    "more": panes.len() - 1,
                    "state": state.name(),
                }));
            }
        }
        let shows = |to: Layout| !self.page_open() && layout == Some(to);
        json!({
            "railHidden": self.store.rail_hidden,
            "windowButtons": WINDOW_BUTTONS,
            "session": session.is_some(),
            "width": width,
            "tabs": tabs,
            "waiting": (session.is_some() && shown)
                .then(|| json!({ "active": waiting_front, "closable": !empty })),
            "splittable": layout == Some(Layout::Panes)
                && !self.page_open()
                && session.is_some_and(|session| !session.tabs.is_empty()),
            "panes": shows(Layout::Panes),
            "canvas": shows(Layout::Canvas),
            "newMenu": self.new_menu,
            "shell": self.shell_ready(),
            "appKey": crate::screens::APP_KEY,
        })
    }

    fn native_tab_strip(
        &self,
        session: Option<&Session>,
        room: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        // A tab waiting for its agent: the last in the strip. A session with
        // no tab at all shows one too, which is simply what it is.
        let pending = session.map(|session| {
            (
                self.pending_tab(&session.id).map(|picker| picker.front),
                session.tabs.is_empty(),
            )
        });
        let (waiting, empty) = pending.unwrap_or((None, false));
        let waiting_front = !self.page_open() && (waiting == Some(true) || empty);
        // The switch is the tab's own: each tab is panes or its canvas.
        let layout = session
            .filter(|_| !waiting_front)
            .map(|session| session.layout());
        let switch = |id: &'static str, glyph: Icon, to: Layout| {
            let active = !self.page_open() && layout == Some(to);
            icon_button(id, glyph, theme)
                .tooltip(tip(match to {
                    Layout::Panes => "This tab as panes",
                    Layout::Canvas => "This tab as a canvas",
                }))
                .when(active, |button| button.bg(theme.selected))
                .on_click(cx.listener(move |this, _, _, cx| this.set_layout(to, cx)))
        };
        let mut tabs = div()
            .id("tabs")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .gap(px(2.))
            .overflow_x_scroll();
        if let Some(session) = session {
            let shown = waiting.is_some() || empty;
            let width = tab_width(room, session.tabs.len() + usize::from(shown));
            let front = session.tab();
            for (index, tab) in session.tabs.iter().enumerate() {
                let active = !self.page_open() && !waiting_front && front == Some(index);
                tabs = tabs.child(self.tab(session, index, tab, active, width, theme, cx));
            }
            if shown {
                tabs = tabs.child(self.waiting_tab(waiting_front, !empty, width, theme, cx));
            }
        }
        let splittable = layout == Some(Layout::Panes)
            && !self.page_open()
            && session.is_some_and(|session| !session.tabs.is_empty());
        div()
            .debug_selector(|| "tabs-native".into())
            .w_full()
            .h(px(metric::BAR))
            .pl(px(if self.store.rail_hidden {
                WINDOW_BUTTONS
            } else {
                6.
            }))
            .pr(px(8.))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .relative()
            .bg(theme.rail)
            // The strip and the rail are one surface, with one grain.
            .child(kou_ui::grain(theme))
            .window_control_area(WindowControlArea::Drag)
            .when(self.store.rail_hidden, |strip| {
                strip.child(
                    icon_button("rail-show", Icon::Sidebar, theme)
                        .tooltip(tip("Show the rail"))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_rail(cx))),
                )
            })
            .child(tabs)
            .when(session.is_some(), |strip| {
                strip
                    .when(splittable, |strip| {
                        strip
                            .child(
                                icon_button("split-right", Icon::Split, theme)
                                    .tooltip(tip("Split right"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.split(Axis::Row, window, cx);
                                    })),
                            )
                            .child(
                                icon_button("split-down", Icon::SplitDown, theme)
                                    .tooltip(tip("Split down"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.split(Axis::Column, window, cx);
                                    })),
                            )
                            .child(div().w(px(6.)))
                    })
                    .child(switch("layout-panes", Icon::Pane, Layout::Panes))
                    .child(switch("layout-canvas", Icon::Canvas, Layout::Canvas))
                    .child(div().w(px(6.)))
                    .child(
                        // The plus asks what the new tab is to be: an agent,
                        // which is asked something first, or a shell.
                        div()
                            .relative()
                            .child(
                                icon_button("agent-new", Icon::Plus, theme)
                                    .when(!self.new_menu, |plus| plus.tooltip(tip("New tab")))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_new_menu(cx);
                                    })),
                            )
                            .children(self.new_menu.then(|| self.new_tab_menu(theme, cx))),
                    )
            })
    }

    /// What a new tab can be, as a list under the tab strip's plus.
    fn new_tab_menu(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        use crate::screens::APP_KEY;
        let item = |id: &'static str, glyph: Icon, name: &'static str, keys: String| {
            crate::rail::row(id, false, theme)
                .mx(px(0.))
                .child(icon(glyph, theme.fg_dim))
                .child(div().flex_1().min_w_0().text_color(theme.fg).child(name))
                .child(kou_ui::key_hint(keys, theme))
        };
        let shell = self.shell_ready();
        let list = div()
            .id("new-tab-menu")
            .occlude()
            .w(px(232.))
            .p(px(metric::UNIT))
            .flex()
            .flex_col()
            .rounded(px(metric::RADIUS_LARGE))
            .border_1()
            .border_color(theme.line_strong)
            .bg(theme.raised)
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if std::mem::take(&mut this.new_menu) {
                    cx.notify();
                }
            }))
            .child(
                item("new-agent", Icon::Thread, "Agent", format!("{APP_KEY}+T")).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.new_menu = false;
                        this.pick(None, cx);
                    }),
                ),
            )
            .when(shell, |list| {
                list.child(
                    item(
                        "new-shell",
                        Icon::Terminal,
                        "Shell",
                        format!("{APP_KEY}+Option+T"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.new_shell(window, cx))),
                )
            });
        div()
            .absolute()
            .top(px(24. + metric::GAP))
            .right_0()
            .child(
                gpui::deferred(
                    gpui::anchored()
                        .anchor(gpui::Anchor::TopRight)
                        .snap_to_window_with_margin(px(8.))
                        .child(list),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    // The tab an agent is yet to be chosen in. `closable` unless it is all
    // the session has.
    fn waiting_tab(
        &self,
        active: bool,
        closable: bool,
        width: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (hover, ring) = (theme.hover, theme.accent);
        div()
            .id("tab-waiting")
            .flex_none()
            .w(px(width))
            .h(px(metric::ROW))
            .pl(px(5.))
            .pr(px(4.))
            .flex()
            .items_center()
            .gap(px(metric::GAP + 2.))
            .rounded(px(metric::RADIUS))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .tab_index(0)
            .when(active, |tab| tab.bg(theme.selected))
            .when(!active, |tab| tab.hover(move |style| style.bg(hover)))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener(|this, _, _, cx| this.show_pending_tab(cx)))
            .child(
                div()
                    .flex_none()
                    .size(px(kou_ui::MONOGRAM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(
                        Icon::Plus,
                        if active { theme.fg } else { theme.fg_dim },
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if active { theme.fg } else { theme.fg_dim })
                    .child("New tab"),
            )
            .when(closable, |tab| {
                tab.child(
                    icon_button_sized("tab-close-waiting", Icon::Close, 20., theme)
                        .tooltip(tip("Close"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_pending_tab(cx);
                        })),
                )
            })
            .into_any_element()
    }

    // One tab: the pane of it that had the keys last, how many more it holds,
    // and the most pressing state among them.
    #[allow(clippy::too_many_arguments)]
    fn tab(
        &self,
        session: &Session,
        index: usize,
        tab: &Tab,
        active: bool,
        width: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let panes = tab.root.panes();
        let Some(pane) = self.store.pane(&tab.active) else {
            return Empty.into_any_element();
        };
        let code = self
            .harness(&pane.harness)
            .map_or_else(|| pane.harness.clone(), |harness| harness.code.clone());
        let shown = pressing(
            panes
                .iter()
                .filter_map(|pane| self.live.get(*pane))
                .map(Live::shown),
        );
        let (id, session, more) = (pane.id.clone(), session.id.clone(), panes.len() - 1);
        let (hover, ring) = (theme.hover, theme.accent);
        div()
            .id(SharedString::from(format!("tab-{id}")))
            .group("tab")
            .flex_none()
            .w(px(width))
            .h(px(metric::ROW))
            // Its mark sits right over the pane header's below it, 12 in
            // from the pane's edge: the strip's 6, the border's 1, and 5.
            .pl(px(5.))
            .pr(px(4.))
            .flex()
            .items_center()
            .gap(px(metric::GAP + 2.))
            .rounded(px(metric::RADIUS))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .tab_index(0)
            .when(active, |tab| tab.bg(theme.selected))
            .when(!active, |tab| tab.hover(move |style| style.bg(hover)))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener({
                let (id, session) = (id.clone(), session.clone());
                move |this, event: &gpui::ClickEvent, window, cx| {
                    this.focus_pane(&id, cx);
                    // Twice: its name and its icon.
                    if event.click_count() >= 2 {
                        this.edit_tab(&session, index, window, cx);
                    }
                }
            }))
            // The icon it was given, else the mark of what runs in it.
            .child(match tab.icon.as_deref().and_then(kou_ui::tab_icon) {
                Some(glyph) => div()
                    .flex_none()
                    .size(px(kou_ui::MONOGRAM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(glyph, if active { theme.fg } else { theme.fg_dim }))
                    .into_any_element(),
                None => monogram(&code, theme.hue(&code), theme).into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if active { theme.fg } else { theme.fg_dim })
                    .child(SharedString::from(self.tab_name(tab))),
            )
            .when(more > 0, |tab| {
                tab.child(
                    div()
                        .flex_none()
                        .px(px(5.))
                        .rounded(px(metric::RADIUS_SMALL))
                        .bg(theme.line)
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_muted)
                        .child(SharedString::from(format!("{}", more + 1))),
                )
            })
            // Only a state worth a glance gets a mark in the strip.
            .when(shown != Mark::Idle, |tab| tab.child(mark(shown, theme)))
            .child(
                icon_button_sized(
                    SharedString::from(format!("tab-close-{id}")),
                    Icon::Close,
                    20.,
                    theme,
                )
                .tooltip(tip(if more > 0 {
                    "Close the tab and its panes"
                } else {
                    "Close"
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.close_tab(&session, index, window, cx);
                })),
            )
            .into_any_element()
    }

    // The small panel under a tab where it is named and given an icon, as
    // the `workspace` plugin draws it (its `tabEditor` view), or as the app
    // does when the plugin cannot.
    fn tab_editor(&self, room: f32, theme: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let edit = self.tab_edit.as_ref()?;
        let session = self.store.session(&edit.session)?;
        let at = session.tabs.iter().position(|tab| tab.id == edit.id)?;
        let code = self
            .store
            .pane(&session.tabs[at].active)
            .and_then(|pane| self.harness(&pane.harness))
            .map_or_else(String::new, |harness| harness.code.clone());
        let icons: Vec<Value> = kou_ui::TAB_ICONS
            .iter()
            .map(|(key, glyph)| json!({ "key": key, "icon": icon_file(*glyph) }))
            .collect();
        let data = json!({
            "left": tab_editor_left(room, session.tabs.len(), at),
            "code": code,
            "icons": icons,
            "chosen": edit.icon,
        });
        let field = || vec![("field", self.tab_name_field(theme, cx).into_any_element())];
        self.chrome("tabEditor", "", data, field, theme, cx)
            .or_else(|| self.native_tab_editor(room, theme, cx))
    }

    // The field a tab's name is typed in.
    fn tab_name_field(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let field = div()
            .h(px(metric::CONTROL))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(metric::RADIUS))
            .border_1()
            .border_color(theme.accent)
            .bg(theme.pane)
            .text_color(theme.fg)
            .track_focus(&self.tab_focus)
            .on_key_down(cx.listener(Self::tab_edit_key));
        match self.tab_edit.as_ref() {
            Some(edit) => field.child(kou_canvas::field_lines(
                "tab-name-field",
                &edit.field,
                theme,
                px(16.),
            )),
            None => field,
        }
    }

    fn native_tab_editor(
        &self,
        room: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let edit = self.tab_edit.as_ref()?;
        let session = self.store.session(&edit.session)?;
        // Under its tab, wherever that is in the strip by now.
        let at = session.tabs.iter().position(|tab| tab.id == edit.id)?;
        let tab = &session.tabs[at];
        let code = self
            .store
            .pane(&tab.active)
            .and_then(|pane| self.harness(&pane.harness))
            .map_or_else(String::new, |harness| harness.code.clone());
        let left = tab_editor_left(room, session.tabs.len(), at);
        let (accent, hover, selected) = (theme.accent, theme.hover, theme.selected);
        let choice = |key: Option<&'static str>, glyph: AnyElement, name: &'static str| {
            let chosen = edit.icon.as_deref() == key;
            div()
                .id(SharedString::from(format!(
                    "tab-icon-{}",
                    key.unwrap_or("auto")
                )))
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(metric::RADIUS))
                .border_1()
                .border_color(if chosen {
                    accent
                } else {
                    gpui::transparent_black()
                })
                .when(chosen, |tile| tile.bg(selected))
                .when(!chosen, |tile| tile.hover(move |style| style.bg(hover)))
                .cursor_pointer()
                .tooltip(tip(name))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(edit) = this.tab_edit.as_mut() {
                        edit.icon = key.map(str::to_owned);
                        cx.notify();
                    }
                }))
                .child(glyph)
        };
        let mut icons = div().flex().flex_wrap().gap(px(2.)).child(choice(
            None,
            monogram(&code, theme.hue(&code), theme).into_any_element(),
            "What runs in it",
        ));
        for (key, glyph) in kou_ui::TAB_ICONS {
            icons = icons.child(choice(
                Some(key),
                icon(*glyph, theme.fg_dim).into_any_element(),
                key,
            ));
        }
        Some(
            div()
                .id("tab-editor")
                .absolute()
                .top(px(4.))
                .left(px(left + 6.))
                .w(px(TAB_EDITOR_WIDTH))
                .p(px(metric::PAD))
                .flex()
                .flex_col()
                .gap(px(metric::PAD))
                .rounded(px(metric::RADIUS_LARGE))
                .border_1()
                .border_color(theme.line_strong)
                .bg(theme.raised)
                .shadow_lg()
                // A press inside it is not a press outside it.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(self.tab_name_field(theme, cx))
                .child(icons)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_muted)
                        .child(kou_ui::key_hint("Enter", theme))
                        .child("to keep")
                        .child(kou_ui::key_hint("Esc", theme))
                        .child("to leave as it was"),
                )
                .into_any_element(),
        )
    }

    /// A pane's header, as the `workspace` plugin draws it (its
    /// `paneHeader` view, once for each pane), or as the app does when the
    /// plugin cannot.
    pub(crate) fn pane_header(
        &self,
        pane: &Pane,
        focused: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let code = self
            .harness(&pane.harness)
            .map_or_else(|| pane.harness.clone(), |harness| harness.code.clone());
        let shown = self.live.get(&pane.id).map_or(Mark::Idle, Live::shown);
        let other = self
            .other_way(pane)
            .map(|(glyph, label)| json!({ "icon": icon_file(glyph), "label": label }));
        let data = json!({
            "pane": pane.id,
            "code": code,
            "title": pane.title,
            "focused": focused,
            "account": self.account_shown(pane),
            "cwd": abbreviate(&pane.cwd),
            "parent": pane
                .parent
                .as_deref()
                .and_then(|parent| self.store.pane(parent))
                .map(|parent| format!("from {}", parent.title)),
            "other": other,
            "closed": self.threads.get(&pane.id).is_some_and(|thread| thread.closed()),
            "state": shown.name(),
            "stateLabel": shown.label(),
        });
        match self.chrome("paneHeader", &pane.id, data, Vec::new, theme, cx) {
            Some(drawn) => drawn,
            None => self
                .native_pane_header(pane, focused, theme, cx)
                .into_any_element(),
        }
    }

    /// The other way to open what runs in a pane: a chat as its harness's
    /// own terminal interface, a terminal as a chat.
    fn other_way(&self, pane: &Pane) -> Option<(Icon, &'static str)> {
        self.harness(&pane.harness)
            .filter(|_| pane.harness != "shell")
            .and_then(|harness| match pane.kind {
                PaneKind::Thread if harness.ready => Some((Icon::Terminal, "Open in its terminal")),
                PaneKind::Terminal if harness.native => Some((Icon::Thread, "Open as a chat")),
                _ => None,
            })
    }

    fn native_pane_header(
        &self,
        pane: &Pane,
        focused: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let code = self
            .harness(&pane.harness)
            .map_or_else(|| pane.harness.clone(), |harness| harness.code.clone());
        let shown = self.live.get(&pane.id).map_or(Mark::Idle, Live::shown);
        let parent = pane
            .parent
            .as_deref()
            .and_then(|parent| self.store.pane(parent))
            .map(|parent| format!("from {}", parent.title));
        let id = pane.id.clone();
        // The account it runs under, when that is not the harness's own.
        let account = self.account_shown(pane);
        div()
            .h(px(34.))
            .pl(px(12.))
            // Its cross stands under the strip's plus: 20 in from the pane's edge.
            .pr(px(9.))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .child(monogram(&code, theme.hue(&code), theme))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if focused { theme.fg } else { theme.fg_dim })
                    .child(SharedString::from(pane.title.clone())),
            )
            .when_some(account, |header, account| {
                let (id, hover) = (id.clone(), theme.hover);
                header.child(
                    div()
                        .id(SharedString::from(format!("pane-account-{id}")))
                        .flex_none()
                        .h(px(18.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .rounded(px(metric::RADIUS_SMALL))
                        .border_1()
                        .border_color(theme.line_strong)
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_dim)
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .tooltip(tip(ACCOUNT_TIP))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.move_to_best_account(&id, window, cx);
                        }))
                        .child(SharedString::from(account)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.fg_muted)
                    .child(SharedString::from(abbreviate(&pane.cwd))),
            )
            .when_some(parent, |header, parent| {
                header.child(
                    // Gives way before the state does when a pane is narrow.
                    div()
                        .flex_shrink(1.)
                        .min_w_0()
                        .truncate()
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_muted)
                        .child(SharedString::from(parent)),
                )
            })
            // The other way to open what runs here.
            .when_some(self.other_way(pane), |header, (glyph, label)| {
                let id = id.clone();
                header.child(
                    icon_button_sized(
                        SharedString::from(format!("pane-switch-{id}")),
                        glyph,
                        22.,
                        theme,
                    )
                    .tooltip(tip(label))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.switch_interface(&id, window, cx);
                    })),
                )
            })
            // A conversation whose agent is gone keeps its transcript on screen;
            // the way to bring the agent back is here.
            .when(
                self.threads
                    .get(&pane.id)
                    .is_some_and(|thread| thread.closed()),
                |header| {
                    let id = id.clone();
                    header.child(
                        button(
                            SharedString::from(format!("restart-thread-{id}")),
                            "Start again",
                            false,
                            theme,
                        )
                        .h(px(metric::CONTROL_SMALL))
                        .text_size(px(metric::TEXT_SMALL))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.restart(&id, window, cx);
                            },
                        )),
                    )
                },
            )
            .when(shown != Mark::Idle, |header| {
                header
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(metric::TEXT_SMALL))
                            .text_color(theme.fg_muted)
                            .child(shown.label()),
                    )
                    .child(mark(shown, theme))
            })
            .child(
                icon_button_sized(
                    SharedString::from(format!("pane-close-{id}")),
                    Icon::Close,
                    22.,
                    theme,
                )
                .tooltip(tip("Close the pane"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.request_close(&id, window, cx);
                })),
            )
    }

    fn pane_body(&self, pane: &Pane, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        match self.live.get(&pane.id).map(|live| &live.content) {
            Some(Content::View(view)) => div()
                .flex_1()
                .min_h_0()
                .key_context(if pane.kind == PaneKind::Terminal {
                    "Terminal"
                } else {
                    "Thread"
                })
                .child(view.clone())
                .into_any_element(),
            Some(Content::Stopped(reason)) => self.stopped(pane, reason.clone(), theme, cx),
            None => div().flex_1().into_any_element(),
        }
    }

    // The half of a split that waits for its agent: what can be started
    // there, under a header that says so and closes it. As the `workspace`
    // plugin draws it (its `choosing` view) around the app's own list, or
    // as the app does when the plugin cannot.
    fn choosing_pane(&self, session: &Session, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let picker = || {
            let heading = SharedString::from(format!("Start an agent in {}", session.name));
            vec![(
                "picker",
                self.picker_element(heading, false, "choosing", theme, cx),
            )]
        };
        match self.chrome("choosing", "", json!({}), picker, theme, cx) {
            Some(drawn) => drawn,
            None => self
                .native_choosing_pane(session, theme, cx)
                .into_any_element(),
        }
    }

    fn native_choosing_pane(
        &self,
        session: &Session,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id("pane-choosing")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.pane)
            .border_t_1()
            .border_color(theme.accent)
            .child(
                div()
                    .h(px(34.))
                    .pl(px(12.))
                    .pr(px(9.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .child(icon(Icon::Plus, theme.fg_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.fg)
                            .child("New pane"),
                    )
                    .child(
                        icon_button_sized("pane-choosing-close", Icon::Close, 22., theme)
                            .tooltip(tip("Close"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.picker = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("pane-choosing-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .pt(px(24.))
                    .pb(px(16.))
                    .child(self.picker(
                        SharedString::from(format!("Start an agent in {}", session.name)),
                        false,
                        theme,
                        cx,
                    )),
            )
    }

    /// A pane: its header over what runs in it, and, among several, a line
    /// along its top that is the accent's on the one with the keys (`line`).
    /// As the `workspace` plugin draws it (its `paneFrame` view), around the
    /// header and the body, or as the app does when the plugin cannot.
    pub(crate) fn pane(
        &self,
        pane: &Pane,
        focused: bool,
        line: Option<bool>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let parts = std::cell::RefCell::new(Some((
            self.pane_header(pane, focused, theme, cx),
            self.pane_body(pane, theme, cx),
        )));
        let slots = || {
            parts
                .borrow_mut()
                .take()
                .map(|(header, body)| vec![("header", header), ("body", body)])
                .unwrap_or_default()
        };
        let data = json!({ "line": line });
        if let Some(drawn) = self.chrome("paneFrame", &pane.id, data, slots, theme, cx) {
            return div().size_full().child(drawn);
        }
        let Some((header, body)) = parts.into_inner() else {
            return div();
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.pane)
            .child(header)
            .child(body)
            .when_some(line, |pane, lit| {
                pane.border_t_1().border_color(if lit {
                    theme.accent
                } else {
                    gpui::transparent_black()
                })
            })
    }

    // The tab on screen: its panes where its splits put them, and the lines
    // between them, which can be dragged.
    fn panes_area(&self, session: &Session, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let area = self.content_area.get().size;
        let (mut width, mut height): (f32, f32) = (area.width.into(), area.height.into());
        // Not measured yet on the first frame the panes are shown: the window's
        // size less the rail and the strip is where they will be.
        if width <= 0. || height <= 0. {
            (width, height) = self.area_guess.get();
        }
        let board = div()
            .id("panes")
            .size_full()
            .relative()
            .bg(theme.line)
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<DividerDrag>, _, cx| {
                    let drag = event.drag(cx).clone();
                    let at = event.event.position - event.bounds.origin;
                    let [x, y, w, h] = drag.room;
                    let share = match drag.axis {
                        Axis::Row => (f32::from(at.x) - x) / w.max(1.),
                        Axis::Column => (f32::from(at.y) - y) / h.max(1.),
                    };
                    this.set_share(&drag.session, drag.tab, &drag.path, share, cx);
                }),
            )
            .on_drop(cx.listener(|this, _: &DividerDrag, _, cx| this.save(cx)));
        let Some(tab) = session.tab() else {
            return board.into_any_element();
        };
        if width <= 0. || height <= 0. {
            return board.into_any_element();
        }
        // A half being chosen for is laid out as the pane it will be: the
        // tab's splits with one more, which is not in the store yet.
        let choosing = self.choosing_beside();
        let mut root = session.tabs[tab].root.clone();
        if let Some((at, axis)) = &choosing {
            root.divide(at, *axis, CHOOSING.to_owned());
        }
        let (places, dividers) = root.lay_out([0., 0., width, height], GAP);
        let several = places.len() > 1;
        let now = Instant::now();
        let choice = places
            .iter()
            .find(|(id, _)| id == CHOOSING)
            .map(|(_, [x, y, w, h])| {
                div()
                    .absolute()
                    .left(px(*x))
                    .top(px(*y))
                    .w(px(w.max(0.)))
                    .h(px(h.max(0.)))
                    .overflow_hidden()
                    .bg(theme.pane)
                    .child(self.choosing_pane(session, theme, cx))
            });
        let panes = places.into_iter().filter_map(|(id, [x, y, w, h])| {
            let pane = self.store.pane(&id)?;
            // While a half is chosen for, that half is what has the keys.
            let focused = session.focused.as_ref() == Some(&id) && choosing.is_none();
            // Which pane has the keys: a line of the accent along its top.
            let line = several.then_some(focused);
            let mut inside = self.pane(pane, focused || !several, line, theme, cx);
            if several {
                inside = inside.on_mouse_down(
                    MouseButton::Left,
                    cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            if this.store.current().and_then(|s| s.focused.as_ref()) != Some(&id) {
                                this.focus_pane(&id, cx);
                            }
                        }
                    }),
                );
            }
            // A pane made a moment ago comes in once; one that was already
            // there, or is shown again, is simply there.
            let fresh = self
                .live
                .get(&id)
                .and_then(|live| live.born)
                .is_some_and(|born| now.duration_since(born) < COME_IN)
                && !cx.reduce_motion();
            let inside = if fresh {
                inside
                    .with_animation(
                        SharedString::from(format!("come-in-{id}")),
                        motion::enter(COME_IN),
                        |pane, arrived| pane.opacity(arrived).mt(px((1. - arrived) * 6.)),
                    )
                    .into_any_element()
            } else {
                inside.into_any_element()
            };
            Some(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w.max(0.)))
                    .h(px(h.max(0.)))
                    .overflow_hidden()
                    .bg(theme.pane)
                    .child(inside),
            )
        });
        let accent = theme.accent;
        let grips = dividers.into_iter().enumerate().map(|(index, divider)| {
            let [x, y, w, h] = divider.line;
            let (place, cursor) = match divider.axis {
                Axis::Row => (
                    [x - GRIP, y, w + GRIP * 2., h],
                    CursorStyle::ResizeLeftRight,
                ),
                Axis::Column => ([x, y - GRIP, w, h + GRIP * 2.], CursorStyle::ResizeUpDown),
            };
            let drag = DividerDrag {
                session: session.id.clone(),
                tab,
                path: divider.path,
                axis: divider.axis,
                room: divider.room,
            };
            div()
                .id(("divider", index))
                .absolute()
                .left(px(place[0]))
                .top(px(place[1]))
                .w(px(place[2]))
                .h(px(place[3]))
                .flex()
                .items_center()
                .justify_center()
                .cursor(cursor)
                .group("divider")
                .on_drag(drag, |_, _, _, cx| {
                    gpui::AppContext::new(cx, |_| NothingDragged)
                })
                // The line itself, which lights up under the pointer.
                .child(
                    div()
                        .when(divider.axis == Axis::Row, |line| line.w(px(3.)).h_full())
                        .when(divider.axis == Axis::Column, |line| line.h(px(3.)).w_full())
                        .rounded_full()
                        .group_hover("divider", move |style| style.bg(accent.opacity(0.7))),
                )
        });
        board
            .children(panes)
            .children(choice)
            .children(grips)
            .into_any_element()
    }

    fn content(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.settings_open {
            return self.settings(theme, cx);
        }
        if self.usage_open {
            return self.usage_page(theme, window, cx);
        }
        if let Some(page) = self.plugin_page.clone() {
            return self.plugin_page_element(&page, serde_json::Value::Null, theme, cx);
        }
        let Some(session) = self.store.current().cloned() else {
            return if self.store.spaces.is_empty() {
                self.no_spaces(theme, cx)
            } else {
                self.no_session(theme, cx)
            };
        };
        if let Some(open) = self.resuming_here(&session.id) {
            return self.sessions(open, theme, cx).into_any_element();
        }
        // (Chosen for as a split, it is drawn in its half, among the panes.)
        if let Some(picker) = self.picker.as_ref().filter(|picker| {
            picker.session == session.id && picker.front && picker.beside.is_none()
        }) {
            let parent = picker
                .parent
                .as_deref()
                .and_then(|parent| self.store.pane(parent));
            return self.picker_screen(&session, parent, true, theme, cx);
        }
        let _ = window;
        let Some(tab) = session.front() else {
            return self.picker_screen(&session, None, false, theme, cx);
        };
        match tab.layout {
            Layout::Panes => self.panes_area(&session, theme, cx),
            // The tab's own canvas: its panes and nothing else.
            Layout::Canvas => match self.canvas(&tab.id, cx) {
                Some(canvas) => div().size_full().child(canvas).into_any_element(),
                None => div().size_full().bg(theme.pane).into_any_element(),
            },
        }
    }
}

/// Which part of the window a [`Part`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    Rail,
    Tabs,
}

/// A part of the window drawn as a view of its own: the window's frames in
/// which it shows what it showed (a drag between panes, a pane coming in)
/// reuse what it drew, rather than build it again. The window tells it to
/// draw again when what it shows changes (see `Shell::draw`).
pub(crate) struct Part {
    shell: gpui::WeakEntity<Shell>,
    which: Which,
    /// The plugin's drawing of it the window last handed it, which it makes
    /// into elements without reading the window: so the window's frames do
    /// not draw it again. `None` draws it from the window as it stands.
    pub(crate) drawn: Option<Drawn>,
    /// How many times it was drawn: what a test of its keeping counts.
    pub(crate) draws: u64,
}

/// A plugin's drawing of a part, as the window hands it over: the tree,
/// what its elements are known by, and what they do when used.
pub(crate) struct Drawn {
    pub tree: std::rc::Rc<kou_plugins::Node>,
    pub page: SharedString,
    pub on: crate::plugin_view::On,
}

impl Part {
    pub(crate) fn new(which: Which, cx: &mut Context<Shell>) -> gpui::Entity<Self> {
        let shell = cx.weak_entity();
        gpui::AppContext::new(cx, |_| Self {
            shell,
            which,
            drawn: None,
            draws: 0,
        })
    }
}

/// What a part showed when it was last handed a drawing: its data, and the
/// palette and the scripts it was drawn with.
pub(crate) struct Shows {
    data: serde_json::Value,
    palette: kou_ui::Palette,
    epoch: u64,
}

impl Render for Part {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.draws += 1;
        let (which, theme) = (self.which, theme(cx).clone());
        if let Some(drawn) = &self.drawn {
            let on = drawn.on.clone();
            return crate::plugins::drawn(&drawn.tree, &drawn.page, Vec::new(), &theme, on);
        }
        self.shell
            .update(cx, |shell, cx| match which {
                Which::Rail => shell.render_rail(&theme, cx),
                Which::Tabs => {
                    let session = if shell.page_open() {
                        None
                    } else {
                        shell.store.current()
                    };
                    let room = f32::from(window.viewport_size().width) - shell.rail_width();
                    shell.tab_strip(session, room, &theme, cx)
                }
            })
            .unwrap_or_else(|_| Empty.into_any_element())
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Everything drawn goes through here, which is where a hot patch
        // takes over: see `hot.rs`.
        crate::hot::hot!(self.draw(window, cx))
    }
}

impl Shell {
    pub(crate) fn draw(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        self.settle(window, cx);
        // The rail and the rest of the window's own parts are drawn by
        // plugins where they can be.
        self.start_plugins();
        {
            let mut springs = self.springs.borrow_mut();
            springs.set_still(cx.reduce_motion());
            springs.step(Instant::now());
        }
        // The theme is cloned out because drawing the content may create the
        // canvas: the one in force, with the accent the user chose.
        let palette = theme(cx).clone();
        let theme = &palette;
        let viewport = window.viewport_size();
        let rail = self.rail_width();
        self.area_guess.set((
            f32::from(viewport.width) - rail,
            f32::from(viewport.height) - metric::BAR,
        ));
        let content = self.content(theme, window, cx);
        let room = f32::from(viewport.width) - rail;
        // The rail and the tab strip are drawn again when what they show
        // changed since they last drew, and only then: each is handed the
        // plugin's drawing of what it shows now. (While a name is typed in
        // the rail, the rail is drawn from the window, field and all.)
        let epoch = self.plugins.borrow().epoch;
        let same = |known: &Option<Shows>, data: &serde_json::Value| {
            known.as_ref().is_some_and(|known| {
                known.epoch == epoch && known.palette == theme.palette && known.data == *data
            })
        };
        if rail > 0. {
            let data = self.rail_data(crate::rail::Room::at(rail));
            if !same(&self.rail_shows, &data) || self.name_edit.is_some() {
                let drawn = match self.name_edit {
                    Some(_) => None,
                    None => self.chrome_drawing("rail", "", data.clone(), theme, cx),
                };
                self.rail_shows = Some(Shows {
                    data,
                    palette: theme.palette,
                    epoch,
                });
                self.rail_part.update(cx, |part, cx| {
                    part.drawn = drawn;
                    cx.notify();
                });
            }
        }
        let data = {
            let session = if self.page_open() {
                None
            } else {
                self.store.current()
            };
            self.tabs_data(session, room)
        };
        if !same(&self.tabs_shows, &data) {
            let drawn = self.chrome_drawing("tabs", "", data.clone(), theme, cx);
            self.tabs_shows = Some(Shows {
                data,
                palette: theme.palette,
                epoch,
            });
            self.tabs_part.update(cx, |part, cx| {
                part.drawn = drawn;
                cx.notify();
            });
        }
        let cx = &*cx;
        let editor = self.tab_editor(room, theme, cx);
        let area = self.content_area.clone();
        // A hot patch that was declined (see `hot.rs`) outranks other notices:
        // what is on screen is no longer what is on disk.
        let notice = if crate::hot::stale() {
            Some(SharedString::from(
                "The code changed in a way a running app cannot take. Restart it to use the new code.",
            ))
        } else {
            self.notice.clone()
        };
        let data = json!({ "rail": rail, "editor": editor.is_some(), "notice": notice.is_some() });
        let body = self.core_chrome(
            "shell",
            data,
            || {
                let mut slots = vec![
                    (
                        "rail",
                        self.rail_part
                            .clone()
                            .cached(gpui::StyleRefinement::default().size_full())
                            .into_any_element(),
                    ),
                    (
                        "tabs",
                        self.tabs_part
                            .clone()
                            .cached(
                                gpui::StyleRefinement::default()
                                    .w_full()
                                    .h(px(metric::BAR))
                                    .flex_none(),
                            )
                            .into_any_element(),
                    ),
                    ("content", content),
                    (
                        "measure",
                        canvas(move |bounds, _, _| area.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full()
                            .into_any_element(),
                    ),
                    (
                        "railResize",
                        div()
                            .id("rail-edge")
                            .size_full()
                            .cursor(CursorStyle::ResizeLeftRight)
                            .on_drag(RailDrag, |_, _, _, cx| {
                                gpui::AppContext::new(cx, |_| NothingDragged)
                            })
                            .into_any_element(),
                    ),
                ];
                slots.extend(editor.map(|editor| ("editor", editor)));
                slots.extend(notice.map(|notice| {
                    (
                        "notice",
                        self.core_chrome(
                            "notice",
                            json!({"text": notice.as_ref()}),
                            Vec::new,
                            theme,
                            cx,
                        ),
                    )
                }));
                slots
            },
            theme,
            cx,
        );
        let shell = div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .bg(theme.rail)
            .text_color(theme.fg)
            .font_family(theme.ui_font.clone())
            .text_size(px(metric::TEXT))
            .on_action(cx.listener(|this, _: &ToggleRail, _, cx| this.toggle_rail(cx)))
            .on_action(cx.listener(|this, _: &NewAgent, _, cx| this.pick(None, cx)))
            .on_action(cx.listener(|this, _: &NewShell, window, cx| this.new_shell(window, cx)))
            .on_action(cx.listener(|this, _: &Quit, _, cx| this.quit(cx)))
            .on_action(cx.listener(|this, _: &ShowUsage, _, cx| this.toggle_usage(cx)))
            .on_action(cx.listener(Self::on_close_pane))
            .on_action(cx.listener(|this, _: &ShowPanes, _, cx| this.set_layout(Layout::Panes, cx)))
            .on_action(
                cx.listener(|this, _: &ShowCanvas, _, cx| this.set_layout(Layout::Canvas, cx)),
            )
            // On the canvas these keys are the board's own (Cmd+D duplicates a
            // node there): what the window does not take goes on to it.
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                if !this.split(Axis::Row, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                if !this.split(Axis::Column, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &SwapPane, window, cx| {
                if !this.swap_pane(true, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &SwapPaneBack, window, cx| {
                if !this.swap_pane(false, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(Self::on_next_pane))
            .on_action(cx.listener(Self::on_previous_pane))
            .on_action(cx.listener(|this, _: &ShowTab1, _, cx| this.show_tab(1, cx)))
            .on_action(cx.listener(|this, _: &ShowTab2, _, cx| this.show_tab(2, cx)))
            .on_action(cx.listener(|this, _: &ShowTab3, _, cx| this.show_tab(3, cx)))
            .on_action(cx.listener(|this, _: &ShowTab4, _, cx| this.show_tab(4, cx)))
            .on_action(cx.listener(|this, _: &ShowTab5, _, cx| this.show_tab(5, cx)))
            .on_action(cx.listener(|this, _: &ShowTab6, _, cx| this.show_tab(6, cx)))
            .on_action(cx.listener(|this, _: &ShowTab7, _, cx| this.show_tab(7, cx)))
            .on_action(cx.listener(|this, _: &ShowTab8, _, cx| this.show_tab(8, cx)))
            .on_action(cx.listener(|this, _: &ShowTab9, _, cx| this.show_tab(9, cx)))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.step_tab(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, _, cx| this.step_tab(false, cx)))
            .on_action(cx.listener(|this, _: &SwitchInterface, window, cx| {
                let front = this
                    .store
                    .current()
                    .filter(|_| !this.page_open() && !this.pending_in_front())
                    .and_then(|session| session.focused.clone());
                if !front.is_some_and(|pane| this.switch_interface(&pane, window, cx)) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                let front = this
                    .store
                    .current()
                    .filter(|_| !this.page_open() && !this.pending_in_front())
                    .and_then(|session| Some((session.id.clone(), session.tab()?)));
                match front {
                    Some((session, tab)) => this.edit_tab(&session, tab, window, cx),
                    None => cx.propagate(),
                }
            }))
            .on_action(cx.listener(|_, _: &FocusNext, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPrevious, window, cx| window.focus_prev(cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.dismiss(cx)))
            .on_action(cx.listener(|this, _: &LeavePane, window, cx| {
                // A terminal keeps Tab for its program; this is the way out of it.
                window.focus(&this.focus, cx);
                window.focus_next(cx);
            }))
            // The rail's edge follows the pointer while it is dragged.
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<RailDrag>, _, cx| {
                this.set_rail_width(f32::from(event.event.position.x), cx);
            }))
            .on_drop(cx.listener(|this, _: &RailDrag, _, cx| this.save(cx)))
            .child(body);
        // Anything still gliding needs the next frame; so does a pane area that
        // has not been measured yet.
        let unmeasured = self.content_area.get().size.width <= px(0.);
        if self.springs.borrow().moving() || unmeasured {
            window.request_animation_frame();
        }
        self.mounted.set(true);
        shell
    }
}
