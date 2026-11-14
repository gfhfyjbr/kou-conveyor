//! The screens that stand in for a session's panes: the harness picker, the
//! list of earlier conversations, the settings, and what an empty window or a
//! stopped pane says.
//!
//! An empty state is one drawing of the thing it is about (a folder, the
//! rail, a terminal with a hollow cursor), what is going on, and the one thing
//! to do next. Lists use the rail's row and its rhythm, with larger pictures.
//! The full-window screens lie on the pane surface with its grain.

use crate::{
    rail::row,
    shell::{Resuming, Shell, abbreviate},
    state::{Pane, PaneKind, Session},
};
use gpui::{
    AnyElement, App, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px,
};
use kou_ui::{
    Icon, MONOGRAM_LARGE, Picture, Theme, button, button_with_icon, grain, icon, key_hint, metric,
    monogram, monogram_absent, monogram_sized, picture, theme, tip,
};

#[cfg(target_os = "macos")]
pub(crate) const APP_KEY: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
pub(crate) const APP_KEY: &str = "Ctrl+Shift";

/// The height of a list row that leads with a large picture: the picker's,
/// the earlier conversations'.
const LIST_ROW: f32 = 36.;
/// How wide the prompt field of an empty screen is, and the list under it.
const ASK_WIDTH: f32 = 620.;
/// How far the field's frame stands in from the field's edges (the room a
/// conversation leaves beside its column).
const ASK_INSET: f32 = 12.;
/// Where a list's text starts, so headings line up with the rows' pictures.
const LIST_EDGE: f32 = metric::PAD;

/// A full-window screen: the pane surface and its grain, its content in the middle.
fn screen(theme: &Theme) -> Div {
    centred().relative().bg(theme.pane).child(grain(theme))
}

pub(crate) fn centred() -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .p(px(24.))
}

pub(crate) fn title(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .text_size(px(15.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.fg)
        .child(text.into())
}

pub(crate) fn detail(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .max_w(px(440.))
        .text_center()
        .text_color(theme.fg_muted)
        .child(text.into())
}

/// An empty state: the drawing, the title under it, what it means, and room
/// for the one action.
fn empty(
    drawing: Picture,
    heading: impl Into<SharedString>,
    explanation: impl Into<SharedString>,
    theme: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .child(picture(drawing, theme.fg_faint).mb(px(16.)))
        .child(title(heading, theme))
        .child(detail(explanation, theme).mt(px(6.)))
}

/// "Esc to go back", under a screen that can be left.
fn go_back(theme: &Theme) -> Div {
    escape("to go back", theme)
}

// What Escape does here, under a screen.
fn escape(does: &'static str, theme: &Theme) -> Div {
    div()
        .mt(px(16.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(metric::TEXT_SMALL))
        .text_color(theme.fg_muted)
        .child(key_hint("Esc", theme))
        .child(does)
}

/// A list's heading, on the rows' left edge.
fn list_heading(text: impl Into<SharedString>, theme: &Theme) -> Div {
    title(text, theme).px(px(LIST_EDGE)).mb(px(8.))
}

/// A settings section's heading: its picture and its name.
fn section(glyph: Icon, name: &'static str, theme: &Theme) -> Div {
    div()
        .h(px(metric::ROW))
        .mt(px(metric::UNIT * 6.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(metric::GAP + 2.))
        .text_size(px(metric::TEXT))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.fg)
        .child(icon(glyph, theme.fg_muted))
        .child(name)
}

impl Shell {
    /// Nothing has been added yet: the first thing to do.
    // The prompt field of an empty screen: where the user says what they
    // want done, before anything else is on screen. Under it, who gets it.
    // The prompt field of an empty screen: a conversation's own (made in
    // `Shell::settle`, before the first frame that shows it).
    fn ask_field(&self, _: &Theme, _: &Context<Self>) -> Div {
        div()
            .w(px(ASK_WIDTH))
            .flex()
            .flex_col()
            .children(self.prompt.clone())
    }

    /// Who gets what is typed in the prompt field, as a button for the
    /// field's foot: it opens the list of what can be started. Built by the
    /// field while it draws, hence from outside the shell.
    pub(crate) fn ask_chooser(shell: &Entity<Shell>, cx: &mut App) -> AnyElement {
        let theme = theme(cx);
        let this = shell.read(cx);
        let here = this.store.current().is_some();
        let Some((harness, kind)) = this.asked() else {
            return div()
                .text_size(px(metric::TEXT_SMALL))
                .text_color(theme.fg_muted)
                .child(if here {
                    "No agent to start yet"
                } else {
                    "Enter chooses a folder to work in"
                })
                .into_any_element();
        };
        // Said only when it is not how this harness opens by itself.
        let way = (kind != this.default_kind(&harness.id)).then_some(match kind {
            PaneKind::Terminal => "in its terminal",
            PaneKind::Thread => "as a chat",
        });
        let open = this.ask_menu;
        let hover = theme.hover;
        div()
            .id("ask-harness")
            .h(px(metric::CONTROL))
            .pl(px(6.))
            .pr(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(metric::RADIUS))
            .cursor_pointer()
            .when(open, |chip| chip.bg(theme.selected))
            .when(!open, |chip| chip.hover(move |style| style.bg(hover)))
            .tooltip(tip("Choose what to start"))
            .on_click({
                let shell = shell.downgrade();
                move |_, _, cx| {
                    cx.stop_propagation();
                    shell.update(cx, |this, cx| this.toggle_ask_menu(cx)).ok();
                }
            })
            .child(monogram(&harness.code, theme.hue(&harness.code), theme))
            .child(
                div()
                    .text_size(px(metric::TEXT_SMALL))
                    .text_color(theme.fg)
                    .child(SharedString::from(harness.name.clone())),
            )
            // Which of its accounts, when not its own.
            .when_some(this.asked_profile(), |chip, profile| {
                chip.child(
                    div()
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_dim)
                        .child(SharedString::from(profile.name.clone())),
                )
            })
            .when_some(way, |chip, way| {
                chip.child(
                    div()
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_muted)
                        .child(way),
                )
            })
            .child(icon(Icon::ChevronDown, theme.fg_muted))
            .into_any_element()
    }

    pub(crate) fn no_spaces(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        screen(theme)
            .child(self.ask_field(theme, cx).mb(px(metric::UNIT * 4.)))
            .child(
                empty(
                    Picture::Spaces,
                    "Add a project folder",
                    "A space is a folder. Its sessions run agents there, side by side or on a canvas.",
                    theme,
                )
                .child(
                    div().mt(px(20.)).child(
                        button_with_icon(
                            "first-space",
                            Icon::FolderPlus,
                            "Choose a folder",
                            true,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.add_space(window, cx))),
                    ),
                ),
            )
            .into_any_element()
    }

    /// There are spaces, and none of their sessions is on screen.
    pub(crate) fn no_session(&self, theme: &Theme) -> AnyElement {
        screen(theme)
            .child(empty(
                Picture::Rail,
                "No session is open",
                "Pick one in the rail, or press the plus beside a space to start one there.",
                theme,
            ))
            .into_any_element()
    }

    /// The picker as a screen of its own, for a session.
    pub(crate) fn picker_screen(
        &self,
        session: &Session,
        parent: Option<&Pane>,
        dismissable: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let heading = match parent {
            Some(parent) => format!("Start an agent from {}", parent.title),
            None => format!("Start an agent in {}", session.name),
        };
        // A clean page: the field and nothing else. Who gets what is typed
        // is chosen in the field itself; its list opens under it.
        let _ = heading;
        screen(theme)
            .key_context("Picker")
            .child(self.ask_field(theme, cx))
            .when(self.ask_menu, |screen| {
                screen.child(
                    // Under the field's frame, which stands in from the
                    // field's own edges.
                    // (In a low window it takes what room is left under
                    // the field, and scrolls.)
                    div()
                        .id("ask-menu")
                        .mt(px(metric::GAP))
                        .w(px(ASK_WIDTH - 2. * ASK_INSET))
                        .min_h_0()
                        .overflow_y_scroll()
                        .p(px(metric::UNIT))
                        .rounded(px(metric::RADIUS_LARGE))
                        .border_1()
                        .border_color(theme.line_strong)
                        .bg(theme.raised)
                        .child(self.picker("".into(), true, theme, cx)),
                )
            })
            .when(dismissable, |screen| {
                // As a tab of its own Escape closes it; over a canvas it goes back.
                let tab = self.pending_tab(&session.id).is_some();
                screen.child(escape(
                    if tab {
                        "to close this tab"
                    } else {
                        "to go back"
                    },
                    theme,
                ))
            })
            .into_any_element()
    }

    /// What a pane shows when nothing runs in it, and the way to start it again.
    pub(crate) fn stopped(
        &self,
        pane: &Pane,
        reason: Option<SharedString>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = pane.id.clone();
        centred()
            .flex_1()
            .min_h_0()
            .child(
                empty(
                    Picture::Stopped,
                    format!("{} is not running", pane.title),
                    reason.unwrap_or_else(|| {
                        "It was open when the app last closed. Its process did not survive that."
                            .into()
                    }),
                    theme,
                )
                .child(
                    div().mt(px(20.)).child(
                        button_with_icon(
                            SharedString::from(format!("restart-{id}")),
                            Icon::Play,
                            format!("Start {} again", pane.title),
                            true,
                            theme,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.restart(&id, window, cx);
                            },
                        )),
                    ),
                ),
            )
            .into_any_element()
    }

    /// The harnesses of this machine as a list: those that can be started,
    /// then the rest with why not. A click on one starts it, or, with
    /// `choosing`, makes it the one the prompt field's Enter starts.
    pub(crate) fn picker(
        &self,
        heading: SharedString,
        choosing: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let chosen = self
            .asked()
            .filter(|_| choosing)
            .map(|(harness, kind)| (harness.id.clone(), kind));
        let (ready, missing): (Vec<_>, Vec<_>) = self
            .harnesses
            .iter()
            .enumerate()
            .partition(|(_, harness)| harness.ready);
        // Under the prompt field it is as wide as what it hangs from.
        // As tall as its rows: where it does not fit, what holds it scrolls
        // (a height held to its holder's would leave nothing to scroll).
        let list = div()
            .debug_selector(|| "pick-list".into())
            .when(choosing, |list| list.w_full())
            .when(!choosing, |list| list.w(px(460.)).max_w_full())
            .flex_none()
            .flex()
            .flex_col();
        if self.harnesses.is_empty() {
            // Still looking is not the same as found nothing.
            return list.items_center().child(if self.scanning {
                empty(
                    Picture::Search,
                    heading,
                    "Looking for agent harnesses on this machine...",
                    theme,
                )
            } else {
                empty(
                    Picture::Search,
                    heading,
                    "No agent harness was found on this machine. Install one (Claude Code, Codex, OpenCode, ...) and check again in Settings.",
                    theme,
                )
            });
        }
        let mut list = list.when(!heading.is_empty(), |list| {
            list.child(list_heading(heading, theme))
        });
        for (index, harness) in ready {
            let id = harness.id.clone();
            let default = self.default_kind(&harness.id);
            // The other way to open it, when there is one.
            let other = match default {
                PaneKind::Thread => Some((PaneKind::Terminal, Icon::Terminal, "In its terminal")),
                PaneKind::Terminal if harness.native => {
                    Some((PaneKind::Thread, Icon::Thread, "As a thread"))
                }
                PaneKind::Terminal => None,
            };
            let asked_account = self
                .asked_profile()
                .filter(|_| choosing)
                .map(|profile| profile.id.clone());
            let of_harness = chosen
                .as_ref()
                .is_some_and(|(chosen, _)| *chosen == harness.id);
            let selected = of_harness && asked_account.is_none();
            // Opened the other way: its second button is the way it is not.
            let other = match (&chosen, other) {
                (Some((id, kind)), Some(_)) if *id == harness.id && *kind != default => {
                    match default {
                        PaneKind::Thread => Some((PaneKind::Thread, Icon::Thread, "As a chat")),
                        PaneKind::Terminal => {
                            Some((PaneKind::Terminal, Icon::Terminal, "In its terminal"))
                        }
                    }
                }
                (_, other) => other,
            };
            list = list.child(
                row(("pick", index), selected, theme)
                    .mx(px(0.))
                    .h(px(LIST_ROW))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, window, cx| {
                            // A shell takes no prompt: chosen, it starts.
                            if choosing && id != "shell" {
                                this.choose_asked(&id, default, cx);
                            } else {
                                this.ask_menu = false;
                                this.launch_profile = None;
                                this.start(&id, default, window, cx);
                            }
                        }
                    }))
                    .child(monogram_sized(
                        &harness.code,
                        theme.hue(&harness.code),
                        MONOGRAM_LARGE,
                        theme,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.fg)
                            .child(SharedString::from(harness.name.clone())),
                    )
                    .when_some(other, |row, (kind, glyph, label)| {
                        row.child(
                            button_with_icon(("pick-other", index), glyph, label, false, theme)
                                .h(px(metric::CONTROL_SMALL))
                                .text_size(px(metric::TEXT_SMALL))
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        if choosing {
                                            this.choose_asked(&id, kind, cx);
                                        } else {
                                            this.start(&id, kind, window, cx);
                                        }
                                    }
                                })),
                        )
                    })
                    // Its chat goes through an adapter that is not here yet.
                    .when_some(
                        harness.adapter.as_ref().filter(|_| !harness.native),
                        |row, (message, install)| {
                            let busy = self.installing.as_deref() == Some(harness.id.as_str());
                            let id = harness.id.clone();
                            row.child(
                                button_with_icon(
                                    ("pick-adapter", index),
                                    Icon::Thread,
                                    if busy {
                                        "Installing..."
                                    } else {
                                        "Install its chat"
                                    },
                                    false,
                                    theme,
                                )
                                .h(px(metric::CONTROL_SMALL))
                                .text_size(px(metric::TEXT_SMALL))
                                .tooltip(tip(message.clone()))
                                .when(install.is_some() && !busy, |button| {
                                    button.on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.install_adapter(&id, window, cx);
                                    }))
                                })
                                .when(install.is_none(), |button| button.opacity(0.5)),
                            )
                        },
                    )
                    .child(
                        // One column for versions, so the buttons before them line up.
                        div()
                            .flex_none()
                            .w(px(72.))
                            .truncate()
                            .text_right()
                            .text_size(px(metric::TEXT_SMALL))
                            .text_color(theme.fg_muted)
                            .child(SharedString::from(
                                harness.version.clone().unwrap_or_default(),
                            )),
                    ),
            );
        }
        if !missing.is_empty() {
            list = list.child(
                div()
                    .mt(px(metric::UNIT * 4.))
                    .mb(px(metric::UNIT))
                    .px(px(LIST_EDGE))
                    .text_size(px(metric::TEXT_SMALL))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.fg_muted)
                    .child("Not available here"),
            );
        }
        // Under the harnesses, their other accounts: the same program with
        // its home elsewhere.
        for (index, profile) in self.store.profiles.iter().enumerate() {
            let Some(harness) = self
                .harness(&profile.harness)
                .filter(|harness| harness.ready)
            else {
                continue;
            };
            let default = self.default_kind(&harness.id);
            let selected = choosing
                && chosen
                    .as_ref()
                    .is_some_and(|(chosen, _)| *chosen == harness.id)
                && self
                    .asked_profile()
                    .is_some_and(|asked| asked.id == profile.id);
            let (id, account) = (harness.id.clone(), profile.id.clone());
            list = list.child(
                row(("pick-account", index), selected, theme)
                    .mx(px(0.))
                    .h(px(LIST_ROW))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if choosing {
                            this.choose_account(&id, Some(account.clone()), default, cx);
                        } else {
                            this.ask_menu = false;
                            this.launch_profile = Some(account.clone());
                            this.start(&id, default, window, cx);
                        }
                    }))
                    .child(monogram_sized(
                        &harness.code,
                        theme.hue(&harness.code),
                        MONOGRAM_LARGE,
                        theme,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.fg)
                            .child(SharedString::from(harness.name.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.fg_dim)
                            .child(SharedString::from(profile.name.clone())),
                    ),
            );
        }
        for (_, harness) in missing {
            list = list.child(
                div()
                    .h(px(LIST_ROW))
                    .px(px(LIST_EDGE))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(metric::GAP))
                    .child(monogram_absent(&harness.code, MONOGRAM_LARGE, theme))
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.fg_muted)
                            .child(SharedString::from(harness.name.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_right()
                            .text_size(px(metric::TEXT_SMALL))
                            .text_color(theme.fg_muted)
                            .child(SharedString::from(
                                harness
                                    .message
                                    .clone()
                                    .unwrap_or_else(|| "not installed".to_owned()),
                            )),
                    ),
            );
        }
        list
    }

    // Whether one of the two notifications is posted, as a button that says
    // which it is and changes it.
    fn notice_toggle(&self, asks: bool, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let on = if asks {
            self.store.notify_asks
        } else {
            self.store.notify_done
        };
        button(
            if asks {
                "settings-notify-asks"
            } else {
                "settings-notify-done"
            },
            if on { "Notify me" } else { "Stay quiet" },
            false,
            theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_notice(asks, cx)))
        .into_any_element()
    }

    // The three ways an agent can be given the means to work with the
    // others, as buttons side by side with the chosen one filled.
    fn orchestration_choice(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        use crate::state::Orchestration as Way;
        let chosen = self.store.orchestration;
        let way = |id: &'static str, label: &'static str, way: Way| {
            button(id, label, false, theme)
                .when(chosen == way, |button| {
                    button.bg(theme.selected).border_color(theme.accent)
                })
                .on_click(cx.listener(move |this, _, _, cx| this.set_orchestration(way, cx)))
        };
        div()
            .flex()
            .items_center()
            .gap(px(metric::GAP))
            .child(way(
                "settings-together-skills",
                "A skill and the command line",
                Way::Skills,
            ))
            .child(way("settings-together-mcp", "MCP tools", Way::Mcp))
            .child(way("settings-together-off", "Nothing", Way::Off))
            .into_any_element()
    }

    /// The accents on offer as discs, the one in force ringed: a press takes
    /// it for links, focus, what is selected and an agent at work.
    fn accent_choice(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let chosen = self.store.accent_rgb();
        let (ring, focus) = (theme.fg, theme.fg_muted);
        let mut row = div().flex().items_center().gap(px(metric::GAP));
        for (at, (name, rgb)) in kou_ui::ACCENTS.into_iter().enumerate() {
            // The first is the palette's own: taking it is choosing none.
            let value = (at > 0).then_some(rgb);
            row = row.child(
                div()
                    .id(("settings-accent", at))
                    .size(px(22.))
                    .flex_none()
                    .rounded_full()
                    .border_2()
                    .border_color(if chosen == value {
                        ring
                    } else {
                        gpui::transparent_black()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tab_index(0)
                    .focus_visible(move |style| style.border_color(focus))
                    .tooltip(tip(name))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_accent(value, cx)))
                    .child(
                        div()
                            .size(px(12.))
                            .rounded_full()
                            .bg(gpui::rgb(kou_ui::accent_in(theme.dark, value))),
                    ),
            );
        }
        row.into_any_element()
    }

    pub(crate) fn settings(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        // A setting: its picture and name in one column, its value in the next.
        let setting = |glyph: Icon, name: &'static str, value: AnyElement| {
            div()
                .h(px(40.))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(metric::GAP + 2.))
                .child(icon(glyph, theme.fg_muted))
                .child(
                    div()
                        .w(px(160.))
                        .flex_none()
                        .text_color(theme.fg)
                        .child(name),
                )
                .child(value)
        };
        // The columns of a harness row and of a key row start where a
        // setting's value does.
        let value_edge = metric::ICON + metric::GAP + 2.;
        let mut harnesses = div().flex().flex_col().pl(px(value_edge));
        for harness in &self.harnesses {
            let state = match (&harness.message, harness.ready) {
                (Some(message), _) => message.clone(),
                (None, true) => "ready".to_owned(),
                (None, false) => "not installed".to_owned(),
            };
            harnesses = harnesses.child(
                div()
                    .h(px(32.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(metric::GAP + 2.))
                    .child(if harness.ready {
                        monogram_sized(
                            &harness.code,
                            theme.hue(&harness.code),
                            MONOGRAM_LARGE,
                            theme,
                        )
                    } else {
                        monogram_absent(&harness.code, MONOGRAM_LARGE, theme)
                    })
                    .child(
                        div()
                            .w(px(132.))
                            .flex_none()
                            .truncate()
                            .text_color(if harness.ready {
                                theme.fg
                            } else {
                                theme.fg_muted
                            })
                            .child(SharedString::from(harness.name.clone())),
                    )
                    .child(
                        div()
                            .w(px(96.))
                            .flex_none()
                            .truncate()
                            .font_family(theme.mono_font.clone())
                            .text_size(px(metric::TEXT_SMALL))
                            .text_color(theme.fg_muted)
                            .child(SharedString::from(
                                harness.version.clone().unwrap_or_default(),
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(metric::GAP))
                            .when(harness.ready, |state| {
                                state.child(icon(Icon::Check, theme.ok))
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(metric::TEXT_SMALL))
                                    .text_color(theme.fg_muted)
                                    .child(SharedString::from(state)),
                            ),
                    ),
            );
            // Its other accounts, and the way to add one: the same program
            // with its home in another folder.
            let Some(variable) = kou_usage::home_variable(&harness.id).filter(|_| harness.ready)
            else {
                continue;
            };
            let indent = MONOGRAM_LARGE + metric::GAP + 2.;
            for profile in self.store.profiles_of(&harness.id) {
                let number = self
                    .store
                    .profiles
                    .iter()
                    .position(|known| known.id == profile.id)
                    .unwrap_or_default();
                let account = profile.id.clone();
                harnesses = harnesses.child(
                    div()
                        .h(px(metric::ROW))
                        .pl(px(indent))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(metric::GAP + 2.))
                        .child(
                            div()
                                .w(px(132.))
                                .flex_none()
                                .truncate()
                                .text_color(theme.fg_dim)
                                .child(SharedString::from(profile.name.clone())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(theme.mono_font.clone())
                                .text_size(px(metric::TEXT_SMALL))
                                .text_color(theme.fg_muted)
                                .child(SharedString::from(abbreviate(&profile.home))),
                        )
                        .child(
                            kou_ui::icon_button(("account-remove", number), Icon::Close, theme)
                                .tooltip(tip("Forget this account (its folder is left as it is)"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_account(&account, cx)
                                })),
                        ),
                );
            }
            let id = harness.id.clone();
            let number = self
                .harnesses
                .iter()
                .position(|known| known.id == harness.id)
                .unwrap_or_default();
            harnesses = harnesses.child(
                div()
                    .h(px(metric::ROW))
                    .pl(px(indent))
                    .mb(px(metric::UNIT))
                    .flex()
                    .flex_none()
                    .items_center()
                    .child(
                        button_with_icon(
                            ("account-add", number),
                            Icon::Plus,
                            "Add an account",
                            false,
                            theme,
                        )
                        .h(px(metric::CONTROL_SMALL))
                        .text_size(px(metric::TEXT_SMALL))
                        .tooltip(tip(format!(
                            "Another folder for its home ({variable}): its own login and sessions"
                        )))
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.add_account(&id, window, cx)
                            }),
                        ),
                    ),
            );
        }
        div()
            .id("settings")
            .size_full()
            .relative()
            .overflow_y_scroll()
            .bg(theme.pane)
            .child(grain(theme))
            .child(
                div()
                    .max_w(px(680.))
                    .px(px(32.))
                    .pt(px(24.))
                    .pb(px(40.))
                    .flex()
                    .flex_col()
                    .child(title("Settings", theme).mb(px(metric::UNIT * 2.)))
                    .child(setting(
                        Icon::Cursor,
                        "Terminal cursor",
                        button(
                            "settings-cursor",
                            if self.store.smooth_cursor {
                                "Glides and stretches"
                            } else {
                                "Jumps"
                            },
                            false,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_cursor(cx)))
                        .into_any_element(),
                    ))
                    .child(setting(
                        Icon::Branch,
                        "Working together",
                        self.orchestration_choice(theme, cx),
                    ))
                    .child(
                        div()
                            .pl(px(value_edge + 160. + metric::GAP + 2.))
                            .mt(px(-4.))
                            .mb(px(metric::UNIT))
                            .max_w(px(560.))
                            .text_size(px(metric::TEXT_SMALL))
                            .text_color(theme.fg_muted)
                            .child(match self.store.orchestration {
                                crate::state::Orchestration::Skills => {
                                    "An agent started in a pane is given a skill on working with \
                                     the other agents, and does it through the app's command line. \
                                     Nothing is added to its context until it needs it."
                                }
                                crate::state::Orchestration::Mcp => {
                                    "An agent started in a pane is given the app's tools through \
                                     an MCP server. They are in its context in every session."
                                }
                                crate::state::Orchestration::Off => {
                                    "A harness is started as it is. An agent can still be told \
                                     about the app's command line in a prompt."
                                }
                            })
                            .child(
                                div()
                                    .mt(px(2.))
                                    .child("For agents started from now on. Nothing is written into a harness's own settings."),
                            ),
                    )
                    .child(setting(
                        Icon::Check,
                        "When an agent is done",
                        self.notice_toggle(false, theme, cx),
                    ))
                    .child(setting(
                        Icon::Eye,
                        "When an agent asks",
                        self.notice_toggle(true, theme, cx),
                    ))
                    .child(
                        div()
                            .pl(px(value_edge + 160. + metric::GAP + 2.))
                            .mt(px(-4.))
                            .mb(px(metric::UNIT))
                            .max_w(px(560.))
                            .text_size(px(metric::TEXT_SMALL))
                            .text_color(theme.fg_muted)
                            .child(
                                "A notification when you are not looking at the agent's pane: \
                                 when one you started finishes its turn, and when any agent \
                                 stops for an answer or a permission.",
                            )
                            // Refused by the system, the two above do nothing:
                            // said here, with the way to where it is allowed.
                            .when(
                                self.notice_permission == crate::notify::Permission::Refused
                                    && (self.store.notify_done || self.store.notify_asks),
                                |note| {
                                    note.child(
                                        div()
                                            .mt(px(metric::GAP))
                                            .flex()
                                            .items_center()
                                            .gap(px(metric::GAP))
                                            .child(icon(Icon::Info, theme.blocked))
                                            .child(
                                                div().text_color(theme.fg).child(
                                                    "The system is not letting kou show \
                                                     notifications.",
                                                ),
                                            )
                                            .child(
                                                button(
                                                    "settings-notify-allow",
                                                    "Open System Settings",
                                                    false,
                                                    theme,
                                                )
                                                .on_click(|_, _, _| {
                                                    crate::notify::open_system_settings()
                                                }),
                                            ),
                                    )
                                },
                            ),
                    )
                    .child(setting(
                        Icon::Appearance,
                        "Appearance",
                        div()
                            .text_color(theme.fg_muted)
                            .child(if theme.dark {
                                "Dark, following the system"
                            } else {
                                "Light, following the system"
                            })
                            .into_any_element(),
                    ))
                    .child(setting(
                        Icon::Target,
                        "Accent",
                        self.accent_choice(theme, cx),
                    ))
                    .child(
                        section(Icon::Terminal, "Harnesses", theme).child(
                            button_with_icon(
                                "settings-scan",
                                Icon::Search,
                                if self.scanning {
                                    "Checking..."
                                } else {
                                    "Check again"
                                },
                                false,
                                theme,
                            )
                            .ml(px(metric::UNIT))
                            .on_click(cx.listener(|this, _, _, cx| this.scan(cx))),
                        ),
                    )
                    .child(
                        div()
                            .pl(px(value_edge))
                            .mb(px(metric::UNIT * 2.))
                            .text_color(theme.fg_muted)
                            .child(
                                "Found by looking for each command on your PATH and asking it for its version.",
                            ),
                    )
                    .child(harnesses)
                    .when(self.harnesses.is_empty(), |settings| {
                        settings.child(
                            div()
                                .pl(px(value_edge))
                                .text_color(theme.fg_muted)
                                .child(if self.scanning {
                                    "Looking for harnesses..."
                                } else {
                                    "None found. Install one and press Check again."
                                }),
                        )
                    })
                    .child(section(Icon::Keyboard, "Keys", theme).mb(px(metric::UNIT)))
                    .children(shortcuts().into_iter().map(|(keys, what)| {
                        div()
                            .h(px(28.))
                            .pl(px(value_edge))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(metric::GAP + 2.))
                            // Ends where a harness's name column does, so the
                            // descriptions start under the versions and the values.
                            .child(div().w(px(160.)).flex_none().flex().child(key_hint(keys, theme)))
                            .child(div().text_color(theme.fg).child(what))
                    })),
            )
            .into_any_element()
    }

    // The list of earlier conversations, when it is open for a pane of this session.
    pub(crate) fn resuming_here(&self, session: &str) -> Option<&Resuming> {
        self.resuming.as_ref().filter(|open| {
            self.store
                .pane(&open.pane)
                .is_some_and(|pane| pane.session == session)
        })
    }

    // A pane's earlier conversations: pick one to go back to it.
    pub(crate) fn sessions(&self, open: &Resuming, theme: &Theme, cx: &Context<Self>) -> Div {
        let folder = self
            .store
            .pane(&open.pane)
            .map_or_else(String::new, |pane| abbreviate(&pane.cwd));
        let heading = SharedString::from(format!("Sessions in {folder}"));
        let Some(sessions) = &open.sessions else {
            return screen(theme)
                .key_context("Picker")
                .child(
                    div().w(px(540.)).child(list_heading(heading, theme)).child(
                        detail("Reading them...", theme)
                            .text_left()
                            .px(px(LIST_EDGE)),
                    ),
                )
                .child(go_back(theme));
        };
        if sessions.is_empty() {
            return screen(theme)
                .key_context("Picker")
                .child(empty(
                    Picture::Conversations,
                    heading,
                    "There are none yet. This conversation will be the first.",
                    theme,
                ))
                .child(go_back(theme));
        }
        let mut list = div()
            .id("sessions")
            .w(px(540.))
            .max_h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(list_heading(heading, theme));
        let now = std::time::SystemTime::now();
        for (index, session) in sessions.iter().enumerate() {
            let (pane, id) = (open.pane.clone(), session.id.clone());
            let untitled = session.title.is_empty();
            let name = if untitled {
                "No prompt yet".to_owned()
            } else {
                session.title.clone()
            };
            // What is known of it: how much was said, and how long ago.
            let ago = session.updated.map(|updated| {
                crate::state::age(
                    now.duration_since(updated)
                        .map_or(0, |ago| ago.as_millis() as u64),
                )
            });
            let said = session.prompts.map(|prompts| {
                format!(
                    "{prompts} {}",
                    if prompts == 1 { "prompt" } else { "prompts" }
                )
            });
            let about = [said, ago]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" \u{b7} ");
            list = list.child(
                row(("session", index), false, theme)
                    .mx(px(0.))
                    .h(px(LIST_ROW))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reopen(&pane, Some(id.clone()), window, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if untitled { theme.fg_muted } else { theme.fg })
                            .child(SharedString::from(name)),
                    )
                    .when(session.running, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(metric::GAP))
                                .text_size(px(metric::TEXT_SMALL))
                                .text_color(theme.working)
                                .child(kou_ui::status_mark(kou_ui::Mark::Working, theme))
                                .child("running"),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(metric::TEXT_SMALL))
                            .text_color(theme.fg_muted)
                            .child(SharedString::from(about)),
                    ),
            );
        }
        screen(theme)
            .key_context("Picker")
            .child(list)
            .child(go_back(theme))
    }
}

/// The app's keys, for the place that lists them.
pub fn shortcuts() -> [(String, &'static str); 16] {
    [
        (format!("{APP_KEY}+T"), "start an agent in a new tab"),
        (format!("{APP_KEY}+Option+T"), "open a shell in a new tab"),
        (
            format!("{APP_KEY}+D"),
            "split right: a terminal beside this pane",
        ),
        (
            format!("{APP_KEY}+Shift+D"),
            "split down: a terminal under this pane",
        ),
        (format!("{APP_KEY}+R"), "name the tab and choose its icon"),
        (format!("{APP_KEY}+W"), "close the pane"),
        (
            format!("{APP_KEY}+Q"),
            "quit; with agents at work, pressed twice",
        ),
        (
            format!("{APP_KEY}+`"),
            "the next pane of the tab; with Shift, the previous",
        ),
        (
            format!("{APP_KEY}+[ ]"),
            "previous and next pane, on into the next tab",
        ),
        (
            format!("{APP_KEY}+Left Right"),
            "previous and next tab (also Shift+[ ])",
        ),
        (format!("{APP_KEY}+1 ... 9"), "the tab at that place"),
        (
            format!("{APP_KEY}+Option+1 2"),
            "this tab as panes, as its canvas",
        ),
        (format!("{APP_KEY}+B"), "show or hide the rail"),
        (
            format!("{APP_KEY}+U"),
            "the usage page: what is left of the plans and what the chats spent",
        ),
        (
            format!("{APP_KEY}+E"),
            "leave the pane for the window's controls",
        ),
        ("Tab".to_owned(), "move between controls"),
    ]
}
