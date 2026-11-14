//! The app's plugins, as the window runs them: scripts that draw parts of
//! its interface and are read again as their files change (see
//! `kou-plugins`). The Usage page is the first part drawn by one.
//!
//! Plugins come, in this order, from the program itself (the built-in
//! ones), from the checkout the program was built from while that is still
//! on disk (the same built-in ones, read live, so that a change to one
//! shows as it is saved), and from `plugins/` in the app's folder (the
//! user's). A later one of the same name takes an earlier one's place.

use crate::{
    plugin_view::{Drawing, element, theme_json},
    shell::Shell,
};
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};
use kou_plugins::{Host, Source};
use kou_ui::{Theme, grain, metric};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    time::Duration,
};

/// The plugins that are part of the program.
const BUILT_IN: &[kou_plugins::File] = &[
    (
        "usage",
        "plugin.json",
        include_str!("../../../plugins/usage/plugin.json"),
    ),
    (
        "usage",
        "desktop/usage.js",
        include_str!("../../../plugins/usage/desktop/usage.js"),
    ),
];

/// How often the plugins' folders are looked at for changes.
const LOOK: Duration = Duration::from_millis(200);

/// The folder the built-in plugins are kept in, in the checkout this
/// program was built from, while it is still there.
fn checkout() -> Option<PathBuf> {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
    folder
        .is_dir()
        .then(|| folder.canonicalize().unwrap_or(folder))
}

impl Shell {
    /// The folders plugins are read from as they are on disk: none for a
    /// window that touches nothing outside itself (a test, a capture).
    fn plugin_folders(&self) -> Vec<PathBuf> {
        if !self.plugins_live {
            return Vec::new();
        }
        checkout()
            .into_iter()
            .chain([self.home.join("plugins")])
            .collect()
    }

    /// The plugins, started the first time they are asked for. `None` when
    /// the interpreter could not be started.
    pub(crate) fn plugin_host(&mut self) -> Option<&mut Host> {
        if self.plugins.is_none() && !self.plugins_tried {
            self.plugins_tried = true;
            let sources = [Source::Embedded(BUILT_IN)]
                .into_iter()
                .chain(self.plugin_folders().into_iter().map(Source::Folder))
                .collect();
            match Host::new(sources) {
                Ok(host) => {
                    self.plugin_pages = host.pages();
                    self.plugins = Some(host);
                }
                Err(error) => log::error!("plugins could not be started: {error}"),
            }
        }
        self.plugins.as_mut()
    }

    /// Starts the plugins and watches their folders: a file that changes
    /// is read again, and the window drawn with what came of it.
    pub(crate) fn watch_plugins(&mut self, cx: &mut Context<Self>) {
        self.plugins_live = true;
        // (Started anew, were it started before a folder was known.)
        self.plugins = None;
        self.plugins_tried = false;
        self.plugin_host();
        self.say_plugin_problems();
        let changed = kou_plugins::watch(self.plugin_folders(), LOOK);
        self.plugin_watch = Some(cx.spawn(async move |this, cx| {
            while changed.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| this.plugins_changed(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// The plugins' files changed: what runs is made to match them.
    pub(crate) fn plugins_changed(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.plugin_host() else {
            return;
        };
        if host.reload() {
            self.plugin_pages = host.pages();
            // A page whose plugin is gone is no page to show.
            if let Some(open) = &self.plugin_page
                && !self.plugin_pages.iter().any(|page| &page.id == open)
            {
                self.plugin_page = None;
            }
            self.say_plugin_problems();
            cx.notify();
        }
    }

    /// Says what is wrong with a plugin where the window says such things,
    /// and takes it back once nothing is.
    fn say_plugin_problems(&mut self) {
        let first = self
            .plugins
            .as_ref()
            .and_then(|host| host.problems().first())
            .map(|problem| {
                let line = problem.message.lines().next().unwrap_or_default();
                SharedString::from(format!("Plugin {}: {line}", problem.plugin))
            });
        match first {
            Some(said) => {
                self.notice = Some(said.clone());
                self.plugin_said = Some(said);
            }
            None => {
                if self.plugin_said.is_some() && self.notice == self.plugin_said {
                    self.notice = None;
                }
                self.plugin_said = None;
            }
        }
    }

    /// Something on a plugin's page was clicked: its handler runs, and
    /// what it asks of the app is done.
    pub(crate) fn plugin_event(&mut self, page: &str, handler: u64, cx: &mut Context<Self>) {
        let Some(host) = self.plugin_host() else {
            return;
        };
        let asked = host.dispatch(page, handler, &json!({}));
        for (name, given) in asked {
            self.plugin_act(&name, &given, cx);
        }
        self.say_plugin_problems();
        cx.notify();
    }

    /// What a plugin may ask the app to do.
    fn plugin_act(&mut self, name: &str, given: &Value, cx: &mut Context<Self>) {
        let text = |key: &str| given.get(key).and_then(Value::as_str);
        match name {
            "usage.refresh" => self.refresh_usage(true, cx),
            "pane.focus" => {
                if let Some(pane) = text("pane") {
                    self.focus_pane(pane, cx);
                }
            }
            "page.show" => {
                if let Some(page) = text("page") {
                    self.show_plugin_page(page, cx);
                }
            }
            "page.close" => {
                self.close_pages();
                self.see_focused();
            }
            "notice" => self.notice = text("text").map(|text| text.to_owned().into()),
            other => {
                self.notice = Some(
                    format!("A plugin asked for \"{other}\", which the app does not do.").into(),
                );
            }
        }
    }

    /// Shows a plugin's page where the workspace is, or goes back from it.
    pub(crate) fn show_plugin_page(&mut self, page: &str, cx: &mut Context<Self>) {
        if page == "usage" {
            return self.toggle_usage(cx);
        }
        let open = self.plugin_page.as_deref() == Some(page);
        self.close_pages();
        if open {
            self.see_focused();
        } else {
            self.plugin_page = Some(page.to_owned());
        }
        cx.notify();
    }

    /// A plugin's page, drawn for `data`: on the pane's ground, scrolling
    /// where it does not fit; and, over it, what is wrong with its plugin
    /// when something is.
    pub(crate) fn plugin_page_element(
        &mut self,
        page: &str,
        data: Value,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = json!({ "theme": theme_json(theme), "data": data });
        let drawn = match self.plugin_host() {
            Some(host) => host.render(page, &input).map_err(|problem| problem.message),
            None => Err("The app's plugins could not be started.".to_owned()),
        };
        let wrong: Vec<String> = self
            .plugins
            .as_ref()
            .map(|host| {
                host.problems()
                    .iter()
                    .map(|problem| format!("{}: {}", problem.plugin, problem.message.trim()))
                    .collect()
            })
            .unwrap_or_default();
        let cx = &*cx;
        let body = match drawn {
            Ok(tree) => {
                let drawing = Drawing {
                    page: SharedString::from(page.to_owned()),
                    theme,
                    made: Cell::new(0),
                };
                Some(element(&tree, &drawing, cx))
            }
            Err(_) => None,
        };
        div()
            .id(SharedString::from(format!("page-{page}")))
            .size_full()
            .relative()
            .overflow_y_scroll()
            .bg(theme.pane)
            .child(grain(theme))
            .when(!wrong.is_empty(), |shown| {
                shown.child(
                    div()
                        .debug_selector(|| "plugin-problems".into())
                        .mx(px(32.))
                        .mt(px(16.))
                        .max_w(px(856.))
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(px(metric::RADIUS_MEDIUM))
                        .border_1()
                        .border_color(theme.error.opacity(0.5))
                        .bg(theme.raised)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .font_family(theme.mono_font.clone())
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_dim)
                        .children(wrong.into_iter().map(SharedString::from)),
                )
            })
            .children(body)
            .into_any_element()
    }
}
