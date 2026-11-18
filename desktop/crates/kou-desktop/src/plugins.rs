//! The app's plugins, as the window runs them: scripts that draw parts of
//! its interface and are read again as their files change (see
//! `kou-plugins`). The Usage page is drawn by one, and so are the surfaces
//! an agent shows in a chat (see [`crate::surfaces`]).
//!
//! Plugins come, in this order, from the program itself (the built-in
//! ones), from the checkout the program was built from while that is still
//! on disk (the same built-in ones, read live, so that a change to one
//! shows as it is saved), and from `plugins/` in the app's folder (the
//! user's). A later one of the same name takes an earlier one's place.
//!
//! The window is not the only one to draw with them: a chat draws the
//! surfaces of its own transcript, while the window is not at hand. So the
//! interpreter is shared ([`SharedHost`]), and what a drawing does when it
//! is used comes back to the window through [`Shell::plugin_event`].

pub(crate) use crate::plugin_view::Keys;
use crate::{
    plugin_view::{Drawing, On, Typing, element, theme_json},
    shell::Shell,
};
use gpui::{
    AnyElement, ClipboardItem, Context, FocusHandle, IntoElement, KeyDownEvent, ParentElement as _,
    SharedString, Window, div,
};
use kou_canvas::field::{Field, Outcome};
use kou_plugins::{Host, Source};
use kou_ui::Theme;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

/// The plugins that are part of the program.
pub(crate) const BUILT_IN: &[kou_plugins::File] = &[
    (
        "studio",
        "plugin.json",
        include_str!("../../../plugins/studio/plugin.json"),
    ),
    (
        "studio",
        "desktop/studio.js",
        include_str!("../../../plugins/studio/desktop/studio.js"),
    ),
    (
        "appearance",
        "plugin.json",
        include_str!("../../../plugins/appearance/plugin.json"),
    ),
    (
        "appearance",
        "desktop/appearance.js",
        include_str!("../../../plugins/appearance/desktop/appearance.js"),
    ),
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
    (
        "composer",
        "plugin.json",
        include_str!("../../../plugins/composer/plugin.json"),
    ),
    (
        "composer",
        "desktop/composer.js",
        include_str!("../../../plugins/composer/desktop/composer.js"),
    ),
    (
        "genui",
        "plugin.json",
        include_str!("../../../plugins/genui/plugin.json"),
    ),
    (
        "genui",
        "shared/data.js",
        include_str!("../../../plugins/genui/shared/data.js"),
    ),
    (
        "genui",
        "desktop/genui.js",
        include_str!("../../../plugins/genui/desktop/genui.js"),
    ),
    (
        "chat",
        "plugin.json",
        include_str!("../../../plugins/chat/plugin.json"),
    ),
    (
        "chat",
        "desktop/chat.js",
        include_str!("../../../plugins/chat/desktop/chat.js"),
    ),
    (
        "rail",
        "plugin.json",
        include_str!("../../../plugins/rail/plugin.json"),
    ),
    (
        "rail",
        "desktop/rail.js",
        include_str!("../../../plugins/rail/desktop/rail.js"),
    ),
    (
        "workspace",
        "plugin.json",
        include_str!("../../../plugins/workspace/plugin.json"),
    ),
    (
        "workspace",
        "desktop/workspace.js",
        include_str!("../../../plugins/workspace/desktop/workspace.js"),
    ),
    (
        "screens",
        "plugin.json",
        include_str!("../../../plugins/screens/plugin.json"),
    ),
    (
        "screens",
        "desktop/screens.js",
        include_str!("../../../plugins/screens/desktop/screens.js"),
    ),
    (
        "canvas",
        "plugin.json",
        include_str!("../../../plugins/canvas/plugin.json"),
    ),
    (
        "canvas",
        "desktop/canvas.js",
        include_str!("../../../plugins/canvas/desktop/canvas.js"),
    ),
];

/// Draws `view` of the window's own parts (see [`Shell::chrome`]), with
/// `on` what its elements do when used.
fn chrome(
    plugins: &SharedHost,
    view: &str,
    instance: &str,
    data: Value,
    slots: impl FnOnce() -> Vec<(&'static str, AnyElement)>,
    theme: &Theme,
    on: On,
) -> Option<AnyElement> {
    let tree = chrome_tree(plugins, view, instance, data, theme)?;
    Some(drawn(&tree, &page_of(view, instance), slots(), theme, on))
}

/// What a drawing of `view` for `instance` is called: its elements are
/// known by it.
pub(crate) fn page_of(view: &str, instance: &str) -> SharedString {
    SharedString::from(if instance.is_empty() {
        view.to_owned()
    } else {
        format!("{view}-{instance}")
    })
}

/// `tree` as the window's elements.
pub(crate) fn drawn(
    tree: &kou_plugins::Node,
    page: &SharedString,
    slots: Vec<(&'static str, AnyElement)>,
    theme: &Theme,
    on: On,
) -> AnyElement {
    let drawing = Drawing {
        page: page.clone(),
        theme,
        made: Cell::new(0),
        on,
        aids: None,
        root: None,
        typing: None,
        slots: RefCell::new(slots),
        dropped: None,
    };
    element(tree, &drawing)
}

/// The drawing of `view` for `instance`, from `data`: the one made before
/// when it shows what it showed, without putting anything into words for
/// the host or working out a hash of it. `None` when no plugin has the
/// view, its script fails, or the plugins are busy.
fn chrome_tree(
    plugins: &SharedHost,
    view: &str,
    instance: &str,
    data: Value,
    theme: &Theme,
) -> Option<Rc<kou_plugins::Node>> {
    let mut hosted = plugins.try_borrow_mut().ok()?;
    if !hosted.host.as_ref().is_some_and(|host| host.has(view)) {
        return None;
    }
    let epoch = hosted.epoch;
    let known = hosted
        .chromed
        .get(view)
        .and_then(|drawn| drawn.get(instance))
        .filter(|drawn| {
            drawn.epoch == epoch && drawn.palette == theme.palette && drawn.data == data
        })
        .map(|drawn| drawn.tree.clone());
    if known.is_some() {
        return known;
    }
    let host = hosted.host.as_mut()?;
    let input = json!({ "theme": theme_json(theme), "data": &data });
    let tree = host.render_strict(view, instance, &input).ok()?;
    let drawn = hosted.chromed.entry(view.to_owned()).or_default();
    if drawn.len() > 256 {
        drawn.clear();
    }
    drawn.insert(
        instance.to_owned(),
        Chromed {
            data,
            epoch,
            palette: theme.palette,
            tree: tree.clone(),
        },
    );
    Some(tree)
}

/// What draws tooltips' bodies: the `workspace` plugin's `tip` view. A tip
/// it cannot draw is drawn as the app always did.
pub(crate) fn tip_face(plugins: SharedHost) -> kou_ui::TipFace {
    Rc::new(move |text, cx| {
        // (A tip answers to nothing.)
        let on: On = Rc::new(|_, _, _, _| {});
        let data = json!({ "text": text.as_ref() });
        chrome(&plugins, "tip", "", data, Vec::new, kou_ui::theme(cx), on)
    })
}

/// An icon as a script names it: by its file's name (`chevron-right`).
pub(crate) fn icon_file(icon: kou_ui::Icon) -> &'static str {
    icon.path()
        .trim_start_matches("icons/")
        .trim_end_matches(".svg")
}

/// How often the plugins' folders are looked at for changes.
const LOOK: Duration = Duration::from_millis(200);

/// The field of a drawing that is being typed in: whose it is, and the
/// text as it stands.
pub(crate) struct Editing {
    pub page: String,
    pub instance: String,
    /// Which of the drawing's fields: its `key`.
    pub key: String,
    pub field: Field,
}

/// The interpreter and what goes with it, as the window and the chats share
/// them.
#[derive(Default)]
pub(crate) struct Hosted {
    /// `None` until first asked for, and when it could not be started.
    pub host: Option<Host>,
    tried: bool,
    /// Pristine JS for required UI when a live override fails or omits a view.
    builtins: Option<Host>,
    /// Goes up when the scripts were read again: what was drawn by the old
    /// ones is drawn anew.
    pub epoch: u64,
    pub editing: Option<Editing>,
    /// The prompt fields a plugin draws the frame of, by their keys: what
    /// each does when its frame asks (`field.attach`, ...). Said again each
    /// time a field is drawn.
    pub fields: HashMap<String, kou_thread_view::FieldAct>,
    /// The canvases a plugin draws the chrome of, by their keys: what each
    /// does when its chrome asks (`canvas.fit`, ...).
    pub canvases: HashMap<String, kou_canvas::FaceAct>,
    /// What was drawn for each node's part of a canvas, by part and
    /// instance, and from what: the part's stamp, the scripts' epoch and the
    /// theme. Looked up at each frame of a pan or a zoom, where nothing is
    /// put into words again for as long as the stamp holds.
    pub faced: HashMap<&'static str, HashMap<String, Faced>>,
    /// How many times such a part was made from its data, rather than
    /// looked up: what a pan or a zoom must not add to.
    pub faced_made: u64,
    /// The last drawing of each of the window's own parts, by view and
    /// instance, and what it was made from (see [`Shell::chrome`]).
    chromed: HashMap<String, HashMap<String, Chromed>>,
}

/// A drawing of a part of the window's own, and what it was made from.
struct Chromed {
    data: Value,
    epoch: u64,
    palette: kou_ui::Palette,
    tree: Rc<kou_plugins::Node>,
}

/// A drawing of a canvas's part, and what it was made from.
pub(crate) struct Faced {
    stamp: u64,
    epoch: u64,
    palette: kou_ui::Palette,
    tree: Rc<kou_plugins::Node>,
}

/// The view of the `composer` plugin that draws a prompt field's frame.
const FIELD: &str = "composer";

pub(crate) type SharedHost = Rc<RefCell<Hosted>>;

/// The field of `page`'s drawing for `instance` that is being typed in, if
/// one is.
pub(crate) fn typing<'a>(
    hosted: &'a Hosted,
    page: &str,
    instance: &str,
    focus: &'a FocusHandle,
    keys: &Keys,
) -> Option<Typing<'a>> {
    hosted
        .editing
        .as_ref()
        .filter(|editing| editing.page == page && editing.instance == instance)
        .map(|editing| Typing {
            key: &editing.key,
            field: &editing.field,
            focus,
            keys: keys.clone(),
        })
}

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

    /// Starts the plugins the first time they are asked for. False when
    /// the interpreter could not be started.
    pub(crate) fn start_plugins(&mut self) -> bool {
        let shared = self.plugins.clone();
        let mut hosted = shared.borrow_mut();
        if hosted.host.is_none() && !hosted.tried {
            hosted.tried = true;
            let sources = [Source::Embedded(BUILT_IN)]
                .into_iter()
                .chain(self.plugin_folders().into_iter().map(Source::Folder))
                .collect();
            match Host::new(sources) {
                Ok(host) => {
                    self.plugin_pages = host.pages();
                    hosted.host = Some(host);
                }
                Err(error) => log::error!("plugins could not be started: {error}"),
            }
        }
        hosted.host.is_some()
    }

    /// Starts the plugins and watches their folders: a file that changes
    /// is read again, and the window drawn with what came of it.
    pub(crate) fn watch_plugins(&mut self, cx: &mut Context<Self>) {
        self.plugins_live = true;
        {
            // (Started anew, were it started before a folder was known.)
            let mut hosted = self.plugins.borrow_mut();
            hosted.host = None;
            hosted.tried = false;
            hosted.epoch += 1;
        }
        self.start_plugins();
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
        self.watch_surfaces(cx);
    }

    /// The plugins' files changed: what runs is made to match them.
    pub(crate) fn plugins_changed(&mut self, cx: &mut Context<Self>) {
        if !self.start_plugins() {
            return;
        }
        let shared = self.plugins.clone();
        let pages = {
            let mut hosted = shared.borrow_mut();
            let changed = hosted.host.as_mut().is_some_and(Host::reload);
            if changed {
                hosted.epoch += 1;
            }
            changed.then(|| hosted.host.as_ref().map(Host::pages).unwrap_or_default())
        };
        let Some(pages) = pages else {
            return;
        };
        self.plugin_pages = pages;
        // A page whose plugin is gone is no page to show.
        if let Some(open) = &self.plugin_page
            && !self.plugin_pages.iter().any(|page| &page.id == open)
        {
            self.plugin_page = None;
        }
        self.say_plugin_problems();
        self.redraw_surfaces(cx);
        // What plugins draw stands in views of their own too (a chat's
        // prompt field): all of the window is drawn again.
        cx.refresh_windows();
        cx.notify();
    }

    /// Says what is wrong with a plugin where the window says such things,
    /// and takes it back once nothing is.
    fn say_plugin_problems(&mut self) {
        let first = self
            .plugins
            .borrow()
            .host
            .as_ref()
            .and_then(|host| host.problems().first().cloned())
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

    /// What the elements of `page`'s drawing for `instance` do when used:
    /// they tell the window.
    pub(crate) fn plugin_on(&self, page: &str, instance: &str, cx: &Context<Self>) -> On {
        let (shell, page, instance) = (cx.weak_entity(), page.to_owned(), instance.to_owned());
        Rc::new(move |handler, event, window, cx| {
            shell
                .update(cx, |this, cx| {
                    this.plugin_event(&page, &instance, handler, event, window, cx);
                })
                .ok();
        })
    }

    /// Where the keys of a field being typed in go: to the window.
    pub(crate) fn plugin_keys(&self, cx: &Context<Self>) -> Keys {
        let shell = cx.weak_entity();
        Rc::new(move |event, window, cx| {
            shell
                .update(cx, |this, cx| this.plugin_edit_key(event, window, cx))
                .ok();
        })
    }

    /// Something a plugin drew was used: its handler runs, and what it asks
    /// of the app is done. A field that was pressed is typed in instead.
    pub(crate) fn plugin_event(
        &mut self,
        page: &str,
        instance: &str,
        handler: u64,
        event: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = event.get("$edit") {
            return self.begin_plugin_edit(page, instance, field, window, cx);
        }
        // What was being typed is kept first: a button may send it.
        self.commit_plugin_edit(window, cx);
        self.plugin_dispatch(page, instance, handler, &event, window, cx);
    }

    fn plugin_dispatch(
        &mut self,
        page: &str,
        instance: &str,
        handler: u64,
        event: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let asked = match self.plugins.borrow_mut().host.as_mut() {
            Some(host) => host.dispatch_as(page, instance, handler, event),
            None => return,
        };
        self.plugins.borrow_mut().chromed.remove(page);
        match page {
            "rail" => self.rail_shows = None,
            "tabs" => self.tabs_shows = None,
            _ => {}
        }
        for (name, given) in asked {
            self.plugin_act(instance, &name, &given, window, cx);
        }
        self.say_plugin_problems();
        self.redraw_surface(instance, cx);
        cx.notify();
    }

    /// What a plugin may ask the app to do.
    fn plugin_act(
        &mut self,
        instance: &str,
        name: &str,
        given: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.appearance_act(name, given, window, cx) {
            return;
        }
        let text = |key: &str| given.get(key).and_then(Value::as_str);
        // What a prompt field's frame asks of its field.
        if let Some(asked) = name.strip_prefix("field.") {
            let field = self.plugins.borrow().fields.get(instance).cloned();
            if let Some(field) = field {
                field(asked, window, cx);
            }
            return;
        }
        // What a canvas's chrome asks of its canvas.
        if let Some(asked) = name.strip_prefix("canvas.") {
            // (A part drawn for one of a canvas's things is `canvas/thing`.)
            let canvas = instance.split('/').next().unwrap_or_default();
            let act = self.plugins.borrow().canvases.get(canvas).cloned();
            if let Some(act) = act {
                act(asked, given, window, cx);
            }
            return;
        }
        // What the parts of the window's own ask (the rail, ...).
        if self.chrome_act(name, given, window, cx) {
            return;
        }
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
            "clipboard.copy" => {
                if let Some(text) = text("text") {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
                    self.notice = Some("Copied".into());
                }
            }
            // What a surface asks: see `surfaces.rs`.
            "surface.send" => {
                if let Some(text) = text("text") {
                    self.surface_said(instance, text, cx);
                }
            }
            "file.open" => {
                if let Some(path) = text("path") {
                    self.surface_open(instance, path, cx);
                }
            }
            other => {
                self.notice = Some(
                    format!("A plugin asked for \"{other}\", which the app does not do.").into(),
                );
            }
        }
    }

    /// A field a plugin drew was pressed: it is typed in where it stands,
    /// from what it held, all of it selected.
    fn begin_plugin_edit(
        &mut self,
        page: &str,
        instance: &str,
        field: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_plugin_edit(window, cx);
        let text = |key: &str| field.get(key).and_then(Value::as_str).unwrap_or_default();
        let multiline = field.get("multiline").and_then(Value::as_bool) == Some(true);
        self.plugins.borrow_mut().editing = Some(Editing {
            page: page.to_owned(),
            instance: instance.to_owned(),
            key: text("key").to_owned(),
            field: Field::new(text("value"), multiline),
        });
        // Taking the keys elsewhere keeps what was typed.
        if self.plugin_blur.is_none() {
            self.plugin_blur = Some(cx.on_blur(&self.plugin_focus, window, |this, window, cx| {
                this.commit_plugin_edit(window, cx);
            }));
        }
        window.focus(&self.plugin_focus, cx);
        self.redraw_surface(instance, cx);
        cx.notify();
    }

    /// Keeps what was typed in a plugin's field: the field's own handler is
    /// given it, as the drawing stands now.
    pub(crate) fn commit_plugin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.plugins.borrow_mut().editing.take() else {
            return;
        };
        let handler = self
            .plugins
            .borrow()
            .host
            .as_ref()
            .and_then(|host| host.drawn(&edit.page, &edit.instance))
            .and_then(|tree| {
                tree.walk()
                    .into_iter()
                    .find(|(_, node)| node.kind == "input" && node.text("key") == Some(&edit.key))
                    .and_then(|(_, node)| node.handler("onChange"))
            });
        match handler {
            Some(handler) => self.plugin_dispatch(
                &edit.page,
                &edit.instance,
                handler,
                &json!({ "value": edit.field.text() }),
                window,
                cx,
            ),
            // (The field is gone from the drawing: nothing to keep it in.)
            None => {
                self.redraw_surface(&edit.instance, cx);
                cx.notify();
            }
        }
    }

    /// A key in a plugin's field that is being typed in.
    pub(crate) fn plugin_edit_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let (outcome, instance) = {
            let mut hosted = self.plugins.borrow_mut();
            let Some(edit) = hosted.editing.as_mut() else {
                return;
            };
            (
                edit.field.key(&event.keystroke, clipboard),
                edit.instance.clone(),
            )
        };
        match outcome {
            Outcome::Commit => {
                self.commit_plugin_edit(window, cx);
                self.surface_done_typing(&instance, window, cx);
            }
            Outcome::Cancel => {
                self.plugins.borrow_mut().editing = None;
                self.surface_done_typing(&instance, window, cx);
            }
            Outcome::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Outcome::Edited | Outcome::Moved => {}
            // Keys the field has no use for go on to the window.
            Outcome::Ignored => return,
        }
        cx.stop_propagation();
        self.redraw_surface(&instance, cx);
        cx.notify();
    }

    /// What draws the frame of a prompt field, in a chat and on the empty
    /// page: the `composer` plugin, which arranges the field's own parts
    /// (see `plugins/composer`). A field it cannot draw is the view's own.
    pub(crate) fn field_face(&mut self, cx: &Context<Self>) -> kou_thread_view::FieldFace {
        self.start_plugins();
        let (plugins, shell) = (self.plugins.clone(), cx.weak_entity());
        Rc::new(move |parts, _, cx| {
            // (Never while something else has the plugins: the view draws
            // the field itself then.)
            let mut hosted = plugins.try_borrow_mut().ok()?;
            let theme = kou_ui::theme(cx);
            let key = parts.key.to_string();
            let names: Vec<&str> = parts.slots.iter().map(|(name, _)| *name).collect();
            let input = json!({
                "theme": theme_json(theme),
                "data": {
                    "look": parts.look,
                    "focused": parts.focused,
                    "busy": parts.busy,
                    "closed": parts.closed,
                    "narrow": parts.narrow,
                    "standalone": parts.standalone,
                    "slots": names,
                    // How its own controls stand, for a frame that draws them.
                    "controls": parts.controls,
                },
            });
            let host = hosted.host.as_mut().filter(|host| host.has(FIELD))?;
            let tree = host.render_strict(FIELD, &key, &input).ok()?;
            if hosted.fields.len() > 256 {
                hosted.fields.clear();
            }
            hosted.fields.insert(key.clone(), parts.act.clone());
            let (shell, instance) = (shell.clone(), key);
            let drawing = Drawing {
                page: parts.key.clone(),
                theme,
                made: Cell::new(0),
                on: Rc::new(move |handler, event, window, cx| {
                    shell
                        .update(cx, |this, cx| {
                            this.plugin_event(FIELD, &instance, handler, event, window, cx);
                        })
                        .ok();
                }),
                aids: None,
                root: None,
                typing: None,
                slots: RefCell::new(parts.slots),
                dropped: Some(parts.dropped),
            };
            Some(element(&tree, &drawing))
        })
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
        self.start_plugins();
        let input = json!({ "theme": theme_json(theme), "data": data });
        let shared = self.plugins.clone();
        let (drawn, wrong) = {
            let mut hosted = shared.borrow_mut();
            match hosted.host.as_mut() {
                Some(host) => {
                    let drawn = host.render(page, &input).ok();
                    let wrong: Vec<String> = host
                        .problems()
                        .iter()
                        .map(|problem| format!("{}: {}", problem.plugin, problem.message.trim()))
                        .collect();
                    (drawn, wrong)
                }
                None => (None, Vec::new()),
            }
        };
        let hosted = shared.borrow();
        let keys = self.plugin_keys(cx);
        let body = drawn.map(|tree| {
            let drawing = Drawing {
                page: SharedString::from(page.to_owned()),
                theme,
                made: Cell::new(0),
                on: self.plugin_on(page, "", cx),
                aids: None,
                root: None,
                typing: typing(&hosted, page, "", &self.plugin_focus, &keys),
                slots: RefCell::default(),
                dropped: None,
            };
            element(&tree, &drawing)
        });
        drop(hosted);
        self.core_chrome(
            "pluginPage",
            json!({ "page": page, "problems": wrong, "body": body.is_some() }),
            || body.map(|body| vec![("body", body)]).unwrap_or_default(),
            theme,
            cx,
        )
    }

    /// Draws `view`, a part of the window's own (the rail, ...), for
    /// `instance` (one of the things it is drawn for, or `""`) from `data`,
    /// with the parts that stay the app's own to draw made by `slots`.
    /// `None` when no plugin has the view, its script fails, or the
    /// plugins are busy with something else: the app then draws the part
    /// itself, as it did before there were plugins.
    pub(crate) fn chrome(
        &self,
        view: &str,
        instance: &str,
        data: Value,
        slots: impl FnOnce() -> Vec<(&'static str, AnyElement)>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let on = self.plugin_on(view, instance, cx);
        chrome(&self.plugins, view, instance, data, slots, theme, on)
    }

    /// Required UI has one implementation: JS. A broken override falls back
    /// to the embedded script, with events dispatched to that same interpreter.
    pub(crate) fn core_chrome(
        &self,
        view: &str,
        data: Value,
        slots: impl FnOnce() -> Vec<(&'static str, AnyElement)>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        if let Some(tree) = chrome_tree(&self.plugins, view, "", data.clone(), theme) {
            return drawn(
                &tree,
                &page_of(view, ""),
                slots(),
                theme,
                self.plugin_on(view, "", cx),
            );
        }
        let tree = {
            let mut hosted = self.plugins.borrow_mut();
            if hosted.builtins.is_none() {
                hosted.builtins = Host::new(vec![Source::Embedded(BUILT_IN)]).ok();
            }
            hosted.builtins.as_mut().and_then(|host| {
                host.render_strict(view, "", &json!({"theme": theme_json(theme), "data": data}))
                    .ok()
            })
        };
        let Some(tree) = tree else {
            log::error!("embedded UI could not render {view}");
            return div()
                .child(format!("Could not load interface: {view}"))
                .into_any_element();
        };
        let (shell, view) = (cx.weak_entity(), view.to_owned());
        let page = page_of(&view, "");
        let on = Rc::new(
            move |handler, event: Value, window: &mut Window, cx: &mut gpui::App| {
                shell
                    .update(cx, |this, cx| {
                        this.commit_plugin_edit(window, cx);
                        let actions = this
                            .plugins
                            .borrow_mut()
                            .builtins
                            .as_mut()
                            .map(|host| host.dispatch_as(&view, "", handler, &event))
                            .unwrap_or_default();
                        for (name, given) in actions {
                            this.plugin_act("", &name, &given, window, cx);
                        }
                        cx.notify();
                    })
                    .ok();
            },
        );
        drawn(&tree, &page, slots(), theme, on)
    }

    /// The drawing of `view` for `instance` from `data`, without making it
    /// into elements: for a part of the window drawn as a view of its own
    /// (see `workspace.rs`), which makes it into elements itself.
    pub(crate) fn chrome_drawing(
        &self,
        view: &str,
        instance: &str,
        data: Value,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Option<crate::workspace::Drawn> {
        let tree = chrome_tree(&self.plugins, view, instance, data, theme)?;
        Some(crate::workspace::Drawn {
            tree,
            page: page_of(view, instance),
            on: self.plugin_on(view, instance, cx),
        })
    }

    /// The same for a part drawn where the window is not at hand (in a
    /// view of its own), with `shell` the window it tells when used.
    pub(crate) fn chrome_of(
        shell: &gpui::Entity<Shell>,
        view: &str,
        data: Value,
        theme: &Theme,
        cx: &gpui::App,
    ) -> Option<AnyElement> {
        let (weak, page) = (shell.downgrade(), view.to_owned());
        let on: On = Rc::new(move |handler, event, window, cx| {
            weak.update(cx, |this, cx| {
                this.plugin_event(&page, "", handler, event, window, cx);
            })
            .ok();
        });
        let plugins = shell.read(cx).plugins.clone();
        chrome(&plugins, view, "", data, Vec::new, theme, on)
    }

    /// What draws a canvas's chrome (the bar in its corner, its menus, ...):
    /// the `canvas` plugin (see `plugins/canvas`). A part it cannot draw is
    /// the canvas's own.
    pub(crate) fn canvas_face(&mut self, cx: &Context<Self>) -> kou_canvas::Face {
        self.start_plugins();
        let (plugins, shell) = (self.plugins.clone(), cx.weak_entity());
        Rc::new(move |parts, cx| {
            let theme = kou_ui::theme(cx);
            let instance = if parts.instance.is_empty() {
                parts.key.to_string()
            } else {
                format!("{}/{}", parts.key, parts.instance)
            };
            let tree = {
                let mut hosted = plugins.try_borrow_mut().ok()?;
                if !hosted.canvases.contains_key(parts.key.as_ref()) {
                    if hosted.canvases.len() > 64 {
                        hosted.canvases.clear();
                    }
                    hosted
                        .canvases
                        .insert(parts.key.to_string(), parts.act.clone());
                }
                let epoch = hosted.epoch;
                // A node's part that shows what it showed: its drawing, looked
                // up rather than made from words again.
                let known = parts.stamp.and_then(|stamp| {
                    let faced = hosted.faced.get(parts.part)?.get(&instance)?;
                    (faced.stamp == stamp && faced.epoch == epoch && faced.palette == theme.palette)
                        .then(|| faced.tree.clone())
                });
                match known {
                    Some(tree) => tree,
                    None => {
                        let host = hosted.host.as_mut().filter(|host| host.has(parts.part))?;
                        let input = json!({
                            "theme": theme_json(theme),
                            "data": parts.data.as_ref(),
                        });
                        let tree = host.render_strict(parts.part, &instance, &input).ok()?;
                        if let Some(stamp) = parts.stamp {
                            hosted.faced_made += 1;
                            let faced = hosted.faced.entry(parts.part).or_default();
                            if faced.len() > 4096 {
                                faced.clear();
                            }
                            faced.insert(
                                instance.clone(),
                                Faced {
                                    stamp,
                                    epoch,
                                    palette: theme.palette,
                                    tree: tree.clone(),
                                },
                            );
                        }
                        tree
                    }
                }
            };
            let page = if parts.instance.is_empty() {
                SharedString::from(instance.clone())
            } else {
                SharedString::from(format!("{}-{instance}", parts.part))
            };
            let (shell, view) = (shell.clone(), parts.part);
            let drawing = Drawing {
                page,
                theme,
                made: Cell::new(0),
                on: Rc::new(move |handler, event, window, cx| {
                    shell
                        .update(cx, |this, cx| {
                            this.plugin_event(view, &instance, handler, event, window, cx);
                        })
                        .ok();
                }),
                aids: None,
                root: None,
                typing: None,
                slots: RefCell::new(parts.slots),
                dropped: None,
            };
            Some(element(&tree, &drawing))
        })
    }

    /// Does what a part of the window's own, drawn by a plugin, asks for:
    /// false when `name` is nothing such a part asks.
    fn chrome_act(
        &mut self,
        name: &str,
        given: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let text = |key: &str| given.get(key).and_then(Value::as_str).map(str::to_owned);
        let (space, session, pane) = (text("space"), text("session"), text("pane"));
        let index = given
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize);
        match name {
            // The rail.
            "rail.hide" | "rail.show" => {
                if self.store.rail_hidden == (name == "rail.show") {
                    self.toggle_rail(cx);
                }
            }
            "agents.seen" => self.see_everything(cx),
            "agents.idle" => {
                self.idle_open = !self.idle_open;
                cx.notify();
            }
            "settings.toggle" => self.toggle_settings(cx),
            "space.add" => self.add_space(window, cx),
            "space.fold" | "space.archive" => {
                let Some(found) = space.as_deref().and_then(|id| self.store.space_mut(id)) else {
                    return true;
                };
                if name == "space.fold" {
                    found.collapsed = !found.collapsed;
                } else {
                    found.archive_open = !found.archive_open;
                }
                self.save(cx);
            }
            "space.start" => {
                if let Some(space) = space {
                    self.add_session(&space, cx);
                }
            }
            "space.rename" => {
                if let Some(space) = space {
                    self.edit_name(crate::shell::Named::Space(space), window, cx);
                }
            }
            "session.select" => {
                if let Some(session) = session {
                    self.select_session(&session, cx);
                }
            }
            "session.rename" => {
                if let Some(session) = session {
                    self.edit_name(crate::shell::Named::Session(session), window, cx);
                }
            }
            "session.archive" => {
                if let Some(session) = session {
                    self.set_archived(&session, true, window, cx);
                }
            }
            "session.delete" => {
                if let Some(session) = session {
                    self.delete_session(&session, cx);
                }
            }
            // The tab strip.
            "tab.rename" | "tab.close" => {
                if let (Some(session), Some(index)) = (session, index) {
                    if name == "tab.rename" {
                        self.edit_tab(&session, index, window, cx);
                    } else {
                        self.close_tab(&session, index, window, cx);
                    }
                }
            }
            "tab.waiting" => self.show_pending_tab(cx),
            "tab.closeWaiting" => self.close_pending_tab(cx),
            "split" => {
                let axis = match text("axis").as_deref() {
                    Some("down") => crate::state::Axis::Column,
                    _ => crate::state::Axis::Row,
                };
                self.split(axis, window, cx);
            }
            "layout.set" => match text("layout").as_deref() {
                Some("canvas") => self.set_layout(crate::state::Layout::Canvas, cx),
                Some("panes") => self.set_layout(crate::state::Layout::Panes, cx),
                _ => {}
            },
            "newMenu.toggle" => self.toggle_new_menu(cx),
            "newMenu.close" => {
                if std::mem::take(&mut self.new_menu) {
                    cx.notify();
                }
            }
            "new.agent" => {
                self.new_menu = false;
                self.pick(None, cx);
            }
            "new.shell" => self.new_shell(window, cx),
            // A pane's header, and the half of a split being chosen for.
            "pane.switch" => {
                if let Some(pane) = pane {
                    self.switch_interface(&pane, window, cx);
                }
            }
            "pane.restart" => {
                if let Some(pane) = pane {
                    self.restart(&pane, window, cx);
                }
            }
            "pane.close" => {
                if let Some(pane) = pane {
                    self.request_close(&pane, window, cx);
                }
            }
            "pane.account" => {
                if let Some(pane) = pane {
                    self.move_to_best_account(&pane, window, cx);
                }
            }
            "picker.close" => {
                self.picker = None;
                cx.notify();
            }
            // The panel a tab is named in.
            "tabEditor.commit" => self.commit_tab_edit(cx),
            "tabEditor.icon" => {
                if let Some(edit) = self.tab_edit.as_mut() {
                    edit.icon = text("icon");
                    cx.notify();
                }
            }
            // The picker: a harness made the one the field starts, or started.
            "harness.choose" | "harness.start" => {
                let harness = text("harness");
                let kind = crate::screens::kind_named(text("kind").as_deref());
                let (Some(harness), Some(kind)) = (harness, kind) else {
                    return true;
                };
                let account = text("account");
                if name == "harness.choose" {
                    match account {
                        Some(account) => self.choose_account(&harness, Some(account), kind, cx),
                        None => self.choose_asked(&harness, kind, cx),
                    }
                } else {
                    // Said with an account (or none), the menu shuts and
                    // the pane runs under it.
                    if given.get("account").is_some() {
                        self.ask_menu = false;
                        self.launch_profile = account;
                    }
                    self.start(&harness, kind, window, cx);
                }
            }
            "askMenu.toggle" => self.toggle_ask_menu(cx),
            "adapter.install" => {
                if let Some(harness) = text("harness") {
                    self.install_adapter(&harness, window, cx);
                }
            }
            "session.reopen" => {
                if let (Some(pane), Some(id)) = (pane, text("id")) {
                    self.reopen(&pane, Some(id), window, cx);
                }
            }
            // The settings.
            "settings.cursor" => self.toggle_cursor(cx),
            "settings.orchestration" => {
                use crate::state::Orchestration as Way;
                let way = match text("way").as_deref() {
                    Some("skills") => Way::Skills,
                    Some("mcp") => Way::Mcp,
                    Some("off") => Way::Off,
                    _ => return true,
                };
                self.set_orchestration(way, cx);
            }
            "settings.notify" => {
                let asks = given.get("asks").and_then(Value::as_bool) == Some(true);
                self.toggle_notice(asks, cx);
            }
            "settings.allowNotices" => crate::notify::open_system_settings(),
            "settings.accent" => {
                let value = given
                    .get("value")
                    .and_then(Value::as_u64)
                    .map(|rgb| rgb as u32);
                self.set_accent(value, cx);
            }
            "settings.scan" => self.scan(cx),
            "account.add" => {
                if let Some(harness) = text("harness") {
                    self.open_account_form(&harness, None, window, cx);
                }
            }
            "account.edit" => {
                if let Some(account) = text("account")
                    && let Some(harness) = self
                        .store
                        .profile(&account)
                        .map(|profile| profile.harness.clone())
                {
                    self.open_account_form(&harness, Some(&account), window, cx);
                }
            }
            "account.signIn" => {
                if let Some(account) = text("account") {
                    self.sign_in(&account, window, cx);
                }
            }
            "account.form.commit" => self.commit_account_form(window, cx),
            "account.form.folder" => self.account_from_folder(window, cx),
            "account.form.cancel" => self.close_account_form(cx),
            "settings.balance" => {
                self.store.balance = given.get("on").and_then(Value::as_bool).unwrap_or(true);
                self.save(cx);
            }
            "settings.claudeKeep" | "settings.codexKeep" => {
                let Some(on) = given.get("on").and_then(Value::as_bool) else {
                    return true;
                };
                if name == "settings.codexKeep" {
                    self.store.codex_keep = on;
                } else {
                    self.store.claude_keep = on;
                }
                self.save(cx);
            }
            "settings.claude" | "settings.codex" => {
                let direct = given
                    .get("direct")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.set_direct(
                    if name == "settings.codex" {
                        "codex"
                    } else {
                        "claude"
                    },
                    direct,
                    cx,
                );
            }
            "account.remove" => {
                if let Some(account) = text("account") {
                    self.remove_account(&account, cx);
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod core_ui_tests {
    use super::*;
    use crate::state::Store;
    use gpui::TestAppContext;

    #[gpui::test]
    fn js_state_redraws_a_cached_sidebar_without_a_rust_state_change(cx: &mut TestAppContext) {
        const COUNTER: &[kou_plugins::File] = &[
            (
                "rail",
                "plugin.json",
                r#"{"name":"rail","desktop":{"script":"main.js"}}"#,
            ),
            (
                "rail",
                "main.js",
                r#"kou.view({id:'rail',render(ctx){
                const count = (ctx.state && ctx.state.count) || 0;
                return ui.button(String(count), {id:'js-counter',onClick(api){api.setState({count:count+1})}});
            }})"#,
            ),
        ];
        cx.update(|cx| {
            cx.set_global(Theme::new(true));
            crate::shell::bind_keys(cx);
        });
        let mut store = Store::default();
        let space = store.add_space("/fixture/js-state".into());
        let session = store.add_session(&space).unwrap();
        store.selected = Some(session);
        let (shell, cx) = cx.add_window_view(|_, cx| Shell::new(store, None, cx));
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(800.)));
        shell.update(cx, |shell, cx| {
            let mut hosted = shell.plugins.borrow_mut();
            hosted.host = Some(
                Host::new(vec![Source::Embedded(BUILT_IN), Source::Embedded(COUNTER)]).unwrap(),
            );
            hosted.epoch += 1;
            cx.notify();
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..2 {
                cx.update(|window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                });
            }
        };
        draw(cx);
        let before = shell.read_with(cx, |shell, _| serde_json::to_value(&shell.store).unwrap());
        let counter = cx.debug_bounds("js-counter").unwrap();
        cx.simulate_click(counter.center(), gpui::Modifiers::none());
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(serde_json::to_value(&shell.store).unwrap(), before);
            assert_eq!(
                shell
                    .plugins
                    .borrow()
                    .host
                    .as_ref()
                    .unwrap()
                    .drawn("rail", "")
                    .unwrap()
                    .said(),
                "1"
            );
        });
    }

    #[gpui::test]
    fn broken_overrides_keep_settings_and_theme_actions_available(cx: &mut TestAppContext) {
        const BROKEN: &[kou_plugins::File] = &[
            (
                "appearance",
                "plugin.json",
                r#"{"name":"appearance","desktop":{"script":"main.js"}}"#,
            ),
            (
                "appearance",
                "main.js",
                "kou.view({id:'appearance',render(){throw Error('broken override')}})",
            ),
            (
                "screens",
                "plugin.json",
                r#"{"name":"screens","desktop":{"script":"main.js"}}"#,
            ),
            (
                "screens",
                "main.js",
                "kou.view({id:'settingsFrame',render(){throw Error('broken frame')}})",
            ),
        ];
        cx.update(|cx| {
            cx.set_global(Theme::new(true));
            crate::shell::bind_keys(cx);
        });
        let (shell, cx) = cx.add_window_view(|_, cx| Shell::new(Store::default(), None, cx));
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(1100.)));
        shell.update(cx, |shell, cx| {
            shell.plugins.borrow_mut().host = Some(
                Host::new(vec![Source::Embedded(BUILT_IN), Source::Embedded(BROKEN)]).unwrap(),
            );
            shell.show_settings(cx);
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..2 {
                cx.update(|window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                });
            }
        };
        draw(cx);
        let general = cx.debug_bounds("settings-page-general").unwrap();
        cx.simulate_click(general.center(), gpui::Modifiers::none());
        draw(cx);
        let cursor = cx.debug_bounds("settings-cursor").unwrap();
        let before = shell.read_with(cx, |shell, _| shell.store.smooth_cursor);
        cx.simulate_click(cursor.center(), gpui::Modifiers::none());
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.smooth_cursor),
            !before
        );
        let appearance = cx.debug_bounds("settings-page-appearance").unwrap();
        cx.simulate_click(appearance.center(), gpui::Modifiers::none());
        draw(cx);
        let create = cx.debug_bounds("theme-create").unwrap();
        cx.simulate_click(create.center(), gpui::Modifiers::none());
        draw(cx);
        let field = cx.debug_bounds("theme-name").unwrap();
        cx.simulate_click(field.center(), gpui::Modifiers::none());
        cx.simulate_keystrokes("T e s t enter");
        draw(cx);
        let save = cx.debug_bounds("theme-save").unwrap();
        cx.simulate_click(save.center(), gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.store.themes[0].theme.name, "Test");
            let hosted = shell.plugins.borrow();
            assert!(hosted.builtins.as_ref().unwrap().problems().is_empty());
            assert!(!hosted.host.as_ref().unwrap().problems().is_empty());
        });
    }
}
