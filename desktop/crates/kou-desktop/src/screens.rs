//! The screens that stand in for a session's panes: the harness picker, the
//! list of earlier conversations, the settings, and what an empty window or a
//! stopped pane says.
//!
//! An empty state is one drawing of the thing it is about (a folder, the
//! rail, a terminal with a hollow cursor), what is going on, and the one thing
//! to do next. Lists use the rail's row and its rhythm, with larger pictures.
//! The full-window screens lie on the pane surface with its grain.

use crate::{
    plugins::icon_file,
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
    Icon, MONOGRAM_LARGE, Picture, Theme, button_with_icon, grain, icon, key_hint, metric,
    monogram, monogram_absent, monogram_sized, picture, theme, tip,
};
use serde_json::{Value, json};

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
/// How a pane opens, as a plugin's script names it.
fn kind_name(kind: PaneKind) -> &'static str {
    match kind {
        PaneKind::Thread => "thread",
        PaneKind::Terminal => "terminal",
    }
}

/// How a pane opens, from its name in a script.
pub(crate) fn kind_named(name: Option<&str>) -> Option<PaneKind> {
    match name? {
        "thread" => Some(PaneKind::Thread),
        "terminal" => Some(PaneKind::Terminal),
        _ => None,
    }
}

// The screens as the `screens` plugin draws them (see `plugins/screens`),
// each around the parts that stay the app's own; when the plugin cannot,
// as the app draws them itself, below.
impl Shell {
    /// The prompt field of an empty screen, for a screen's `field` slot.
    fn field_slot(&self) -> Vec<(&'static str, AnyElement)> {
        self.prompt
            .clone()
            .map(|prompt| ("field", prompt.into_any_element()))
            .into_iter()
            .collect()
    }

    /// Nothing has been added yet: the first thing to do.
    pub(crate) fn no_spaces(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let field = || self.field_slot();
        self.chrome("noSpaces", "", json!({}), field, theme, cx)
            .unwrap_or_else(|| self.native_no_spaces(theme, cx))
    }

    /// There are spaces, and none of their sessions is on screen.
    pub(crate) fn no_session(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        self.chrome("noSession", "", json!({}), Vec::new, theme, cx)
            .unwrap_or_else(|| self.native_no_session(theme))
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
        // As a tab of its own Escape closes it; over a canvas it goes back.
        let escape = dismissable.then(|| {
            if self.pending_tab(&session.id).is_some() {
                "to close this tab"
            } else {
                "to go back"
            }
        });
        let data = json!({
            "menu": self.ask_menu.then(|| self.picker_data("", true)),
            "escape": escape,
        });
        let field = || self.field_slot();
        self.chrome("pickerScreen", "", data, field, theme, cx)
            .unwrap_or_else(|| self.native_picker_screen(session, parent, dismissable, theme, cx))
    }

    /// What a pane shows when nothing runs in it, and the way to start it again.
    pub(crate) fn stopped(
        &self,
        pane: &Pane,
        reason: Option<SharedString>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let data = json!({
            "pane": pane.id,
            "title": pane.title,
            "reason": reason.as_ref().map(SharedString::as_ref),
        });
        self.chrome("stopped", &pane.id, data, Vec::new, theme, cx)
            .unwrap_or_else(|| self.native_stopped(pane, reason, theme, cx))
    }

    /// What can be started, as a list (see [`Shell::picker`]): `instance`
    /// tells apart the places it is drawn in.
    pub(crate) fn picker_element(
        &self,
        heading: SharedString,
        choosing: bool,
        instance: &str,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let data = self.picker_data(&heading, choosing);
        self.chrome("picker", instance, data, Vec::new, theme, cx)
            .unwrap_or_else(|| self.picker(heading, choosing, theme, cx).into_any_element())
    }

    /// What the picker lists, as its plugin is given it.
    fn picker_data(&self, heading: &str, choosing: bool) -> Value {
        let chosen = self
            .asked()
            .filter(|_| choosing)
            .map(|(harness, kind)| (harness.id.clone(), kind));
        let asked_account = self
            .asked_profile()
            .filter(|_| choosing)
            .map(|profile| profile.id.clone());
        let ready: Vec<Value> = self
            .harnesses
            .iter()
            .filter(|harness| harness.ready)
            .map(|harness| {
                let default = self.default_kind(&harness.id);
                // The other way to open it, when there is one; opened the
                // other way, its second button is the way it is not.
                let other = match default {
                    PaneKind::Thread => {
                        Some((PaneKind::Terminal, Icon::Terminal, "In its terminal"))
                    }
                    PaneKind::Terminal if harness.native => {
                        Some((PaneKind::Thread, Icon::Thread, "As a thread"))
                    }
                    PaneKind::Terminal => None,
                };
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
                let of_harness = chosen.as_ref().is_some_and(|(id, _)| *id == harness.id);
                json!({
                    "id": harness.id,
                    "code": harness.code,
                    "name": harness.name,
                    "version": harness.version,
                    "kind": kind_name(default),
                    "selected": of_harness && asked_account.is_none(),
                    "other": other.map(|(kind, glyph, label)| json!({
                        "kind": kind_name(kind),
                        "icon": icon_file(glyph),
                        "label": label,
                    })),
                    "adapter": harness
                        .adapter
                        .as_ref()
                        .filter(|_| !harness.native)
                        .map(|(message, install)| json!({
                            "message": message,
                            "installable": install.is_some(),
                            "busy": self.installing.as_deref() == Some(harness.id.as_str()),
                        })),
                })
            })
            .collect();
        let accounts: Vec<Value> = self
            .store
            .profiles
            .iter()
            .filter_map(|profile| {
                let harness = self
                    .harness(&profile.harness)
                    .filter(|harness| harness.ready)?;
                let selected = choosing
                    && chosen.as_ref().is_some_and(|(id, _)| *id == harness.id)
                    && self
                        .asked_profile()
                        .is_some_and(|asked| asked.id == profile.id);
                Some(json!({
                    "harness": harness.id,
                    "code": harness.code,
                    "name": harness.name,
                    "account": profile.id,
                    "accountName": profile.name,
                    "kind": kind_name(self.default_kind(&harness.id)),
                    "selected": selected,
                }))
            })
            .collect();
        let missing: Vec<Value> = self
            .harnesses
            .iter()
            .filter(|harness| !harness.ready)
            .map(|harness| {
                json!({
                    "code": harness.code,
                    "name": harness.name,
                    "message": harness.message.as_deref().unwrap_or("not installed"),
                })
            })
            .collect();
        json!({
            "heading": heading,
            "choosing": choosing,
            "scanning": self.scanning,
            "empty": self.harnesses.is_empty(),
            "ready": ready,
            "accounts": accounts,
            "missing": missing,
        })
    }

    pub(crate) fn settings(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let content = if self.appearance.page == crate::appearance::Page::Appearance {
            self.appearance_settings(theme, cx)
        } else {
            self.settings_body(theme, cx)
        };
        self.core_chrome(
            "settingsFrame",
            json!({"page": self.appearance.page.id()}),
            || vec![("body", content)],
            theme,
            cx,
        )
    }

    fn direct_setting(&self, id: &str) -> Option<&'static str> {
        self.harnesses
            .iter()
            .any(|harness| harness.id == id && harness.ready)
            .then_some(if self.store.direct(id) {
                "direct"
            } else {
                "adapter"
            })
    }

    fn settings_body(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        use crate::state::Orchestration as Way;
        let chosen = self.store.accent_rgb();
        let accents: Vec<Value> = kou_ui::ACCENTS
            .into_iter()
            .enumerate()
            .map(|(at, (name, rgb))| {
                // The first is the palette's own: taking it is choosing none.
                let value = (at > 0).then_some(rgb);
                json!({
                    "name": name,
                    "value": value,
                    "color": crate::rail::hex(kou_ui::accent_in(theme.dark, value)),
                    "chosen": chosen == value,
                })
            })
            .collect();
        let harnesses: Vec<Value> = self
            .harnesses
            .iter()
            .map(|harness| {
                let state = match (&harness.message, harness.ready) {
                    (Some(message), _) => message.clone(),
                    (None, true) => "ready".to_owned(),
                    (None, false) => "not installed".to_owned(),
                };
                let variable = kou_usage::home_variable(&harness.id).filter(|_| harness.ready);
                let accounts: Vec<Value> = self
                    .store
                    .profiles_of(&harness.id)
                    .map(|profile| {
                        let state = self.account_state(&harness.id, Some(&profile.id));
                        json!({
                            "id": profile.id,
                            "name": profile.name,
                            "home": abbreviate(&profile.home),
                            "proxy": profile.proxy,
                            "signedOut": state == "not signed in",
                            "state": state,
                        })
                    })
                    .collect();
                json!({
                    "id": harness.id,
                    "code": harness.code,
                    "name": harness.name,
                    "ready": harness.ready,
                    "version": harness.version,
                    "state": state,
                    "variable": variable,
                    "own": self.account_state(&harness.id, None),
                    "accounts": accounts,
                })
            })
            .collect();
        let shortcuts: Vec<Value> = shortcuts()
            .into_iter()
            .map(|(keys, what)| json!([keys, what]))
            .collect();
        let data = json!({
            "page": self.appearance.page.id(),
            "smoothCursor": self.store.smooth_cursor,
            "orchestration": match self.store.orchestration {
                Way::Skills => "skills",
                Way::Mcp => "mcp",
                Way::Off => "off",
            },
            "notifyDone": self.store.notify_done,
            "notifyAsks": self.store.notify_asks,
            "refused": self.notice_permission == crate::notify::Permission::Refused
                && (self.store.notify_done || self.store.notify_asks),
            "dark": theme.dark,
            "accents": accents,
            "scanning": self.scanning,
            "harnesses": harnesses,
            "balance": self.store.balance,
            // How Claude Code's chat is held, when Claude Code is here.
            "claudeKeep": self.store.claude_keep,
            "claude": self.direct_setting("claude"),
            "codex": self.direct_setting("codex"),
            "codexKeep": cfg!(unix).then_some(self.store.codex_keep),
            "form": self.account_form.as_ref().map(|form| json!({
                "harness": form.harness,
                "editing": form.editing,
                "onProxy": form.on_proxy,
                "problem": form.problem,
            })),
            "shortcuts": shortcuts,
        });
        // The form's two fields are the app's own: they take the keys.
        let fields = || match self.account_form {
            Some(_) => vec![
                ("accountName", self.account_field(false, theme, cx)),
                ("accountProxy", self.account_field(true, theme, cx)),
            ],
            None => Vec::new(),
        };
        self.core_chrome("settings", data, fields, theme, cx)
    }

    /// A pane's earlier conversations: pick one to go back to it.
    pub(crate) fn sessions(
        &self,
        open: &Resuming,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let folder = self
            .store
            .pane(&open.pane)
            .map_or_else(String::new, |pane| abbreviate(&pane.cwd));
        let now = std::time::SystemTime::now();
        let sessions: Option<Vec<Value>> = open.sessions.as_ref().map(|sessions| {
            sessions
                .iter()
                .map(|session| {
                    let untitled = session.title.is_empty();
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
                    json!({
                        "id": session.id,
                        "name": if untitled { "No prompt yet" } else { session.title.as_str() },
                        "untitled": untitled,
                        "running": session.running,
                        "about": about,
                    })
                })
                .collect()
        });
        let data = json!({
            "pane": open.pane,
            "heading": format!("Sessions in {folder}"),
            "sessions": sessions,
        });
        self.chrome("sessions", "", data, Vec::new, theme, cx)
            .unwrap_or_else(|| self.native_sessions(open, theme, cx).into_any_element())
    }
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
        let theme = theme(cx).clone();
        let this = shell.read(cx);
        let asked = this.asked().map(|(harness, kind)| {
            // Said only when it is not how this harness opens by itself.
            let way = (kind != this.default_kind(&harness.id)).then_some(match kind {
                PaneKind::Terminal => "in its terminal",
                PaneKind::Thread => "as a chat",
            });
            json!({ "code": harness.code, "name": harness.name, "way": way })
        });
        let data = json!({
            "here": this.store.current().is_some(),
            "harness": asked,
            "account": this.asked_account_shown(),
            "open": this.ask_menu,
        });
        Shell::chrome_of(shell, "askChooser", data, &theme, cx)
            .unwrap_or_else(|| Self::native_ask_chooser(shell, cx))
    }

    fn native_ask_chooser(shell: &Entity<Shell>, cx: &mut App) -> AnyElement {
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
            .when_some(this.asked_account_shown(), |chip, account| {
                chip.child(
                    div()
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_dim)
                        .child(SharedString::from(account)),
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
            .child(icon(Icon::ChevronDown, theme.fg_muted).size(px(metric::ICON_SMALL)))
            .into_any_element()
    }

    fn native_no_spaces(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
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

    fn native_no_session(&self, theme: &Theme) -> AnyElement {
        screen(theme)
            .child(empty(
                Picture::Rail,
                "No session is open",
                "Pick one in the rail, or press the plus beside a space to start one there.",
                theme,
            ))
            .into_any_element()
    }

    fn native_picker_screen(
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

    fn native_stopped(
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
    pub(crate) fn account_field(
        &self,
        proxy: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let form = self.account_form.as_ref();
        let on = form.is_some_and(|form| form.on_proxy == proxy);
        let field = form.map(|form| if proxy { &form.proxy } else { &form.name });
        let shown = div()
            .id(if proxy {
                "account-proxy"
            } else {
                "account-name"
            })
            .debug_selector(move || {
                if proxy {
                    "account-proxy"
                } else {
                    "account-name"
                }
                .to_owned()
            })
            .size_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.account_form_at(proxy, window, cx)),
            );
        let said = |text: &str, ink: gpui::Hsla| {
            div()
                .truncate()
                .text_color(ink)
                .child(SharedString::from(text.to_owned()))
        };
        match field {
            Some(field) if on => shown
                .key_context(crate::accounts::FORM)
                .track_focus(&self.account_focus)
                .on_action(cx.listener(Self::account_form_next))
                .on_key_down(cx.listener(Self::account_form_key))
                .child(kou_canvas::field_lines(
                    if proxy {
                        "account-proxy-field"
                    } else {
                        "account-name-field"
                    },
                    field,
                    theme,
                    px(16.),
                ))
                .into_any_element(),
            Some(field) if !field.text().is_empty() => {
                shown.child(said(field.text(), theme.fg)).into_any_element()
            }
            _ => shown
                .child(said(
                    if proxy { "proxy (optional)" } else { "name" },
                    theme.fg_muted,
                ))
                .into_any_element(),
        }
    }

    pub(crate) fn resuming_here(&self, session: &str) -> Option<&Resuming> {
        self.resuming.as_ref().filter(|open| {
            self.store
                .pane(&open.pane)
                .is_some_and(|pane| pane.session == session)
        })
    }

    fn native_sessions(&self, open: &Resuming, theme: &Theme, cx: &Context<Self>) -> Div {
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
