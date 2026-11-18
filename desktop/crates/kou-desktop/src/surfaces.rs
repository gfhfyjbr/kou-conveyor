//! Surfaces: the small UIs an agent shows the user with kou's `UI` tool
//! (generative UI: metrics, charts, tables, diffs, inputs, buttons, and
//! layouts of the agent's own), drawn in the chat where the call stands.
//!
//! The tool (the `genui` plugin of kou's runner) writes each version of a
//! surface to a file of the workspace, `.harness/ui/<session>/<surface>/
//! v<N>.json`, and names it on the first line of what it answers. The chat
//! finds that line ([`kou_thread::ToolCall::surface`]) and asks the app for
//! the call's card; the app reads the version and has the `genui` plugin
//! (`plugins/genui`, a script like the Usage page's) build what it shows.
//!
//! The newest version of a surface is the live one: its sources are the
//! workspace's files as they are now, looked at again every [`POLL`], and
//! its buttons and inputs work. A button's event reaches the agent as a
//! message of the user's, `[UI <surface> v<N>] <name> <json>`. An earlier
//! version shows what its call read, and answers to nothing.
//!
//! A surface reads only files of its own workspace, never `.git` or a
//! `.env`, as the tool that made it does.

use crate::{
    plugin_view::{Drawing, On, element, theme_json},
    plugins::{Keys, SharedHost, typing},
    shell::Shell,
};
use gpui::{App, Context, FocusHandle, Focusable as _, IntoElement as _, SharedString, Window};
use kou_plugins::Node;
use kou_thread::ToolCall;
use kou_thread_view::{ToolAids, ToolFace, ToolView};
use serde_json::{Map, Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io::{Read as _, Seek as _, SeekFrom},
    path::{Component, Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The view of the `genui` plugin that draws a surface.
const VIEW: &str = "genui";
/// How often the files of the surfaces on screen are looked at.
pub(crate) const POLL: Duration = Duration::from_millis(1500);
/// A surface drawn this recently is on screen.
const ON_SCREEN: Duration = Duration::from_secs(4);
/// How much of a version's file, a source and a Code's file is read.
const MAX_VERSION: u64 = 8 << 20;
const MAX_SOURCE: u64 = 2 << 20;
const MAX_CODE: u64 = 64 << 10;
/// How many surfaces are remembered: past this, the ones drawn longest ago
/// are let go.
const KEPT: usize = 96;

/// When a file was written and how long it is: it has changed when either
/// has. `None` for no file.
type Mark = Option<(SystemTime, u64)>;

fn mark(path: &Path) -> Mark {
    let meta = std::fs::metadata(path).ok().filter(|meta| meta.is_file())?;
    Some((meta.modified().ok()?, meta.len()))
}

/// A file a surface reads: a source of its data, or what a Code shows.
struct Watched {
    /// The source's name, or the component's id.
    name: String,
    code: bool,
    format: String,
    lines: Option<String>,
    /// Where it is, or why it is not read.
    path: Result<PathBuf, &'static str>,
    /// As it was when last read.
    mark: Mark,
    /// The script has its text as it was then.
    sent: bool,
}

/// A surface as it is shown in a chat.
struct Shown {
    pane: String,
    call: String,
    root: PathBuf,
    live: bool,
    seen: Instant,
    files: Vec<Watched>,
    /// What it was last drawn for: the scripts' epoch and the theme.
    stamp: (u64, u64),
    /// Something it is drawn from has changed since.
    stale: bool,
    tree: Option<Rc<Node>>,
    /// A file of its is not there yet.
    waiting: bool,
}

/// The surfaces the chats show. Shared by the window, which looks at their
/// files and does what their buttons ask, and the chats, which draw them.
#[derive(Default)]
pub(crate) struct Surfaces {
    /// The versions read, by file: one never changes once written.
    versions: HashMap<PathBuf, Rc<Value>>,
    /// The ones that could not be read, and when: tried again in a while.
    missing: HashMap<PathBuf, Instant>,
    shown: HashMap<String, Shown>,
}

/// A path a surface names, in its workspace: the file, or why not.
fn inside(root: &Path, file: &str) -> Result<PathBuf, &'static str> {
    let relative = Path::new(file);
    let mut parts = Vec::new();
    for part in relative.components() {
        match part {
            Component::Normal(part) => parts.push(part.to_string_lossy()),
            _ => return Err("the file leads outside the workspace"),
        }
    }
    let hidden = parts.first().is_some_and(|first| first == ".git")
        || parts
            .iter()
            .any(|part| part == ".env" || part.starts_with(".env."));
    if parts.is_empty() || hidden {
        return Err("a surface does not show .git or .env files");
    }
    let full = root.join(relative);
    // (A link may lead out of it, too.)
    if let (Ok(real), Ok(root)) = (full.canonicalize(), root.canonicalize())
        && !real.starts_with(root)
    {
        return Err("the file leads outside the workspace");
    }
    Ok(full)
}

/// A time as the tool writes it: `2026-11-14T09:30:00Z`.
fn iso(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // (Days since 1970 to a date: Howard Hinnant's civil-from-days.)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

fn mark_text(mark: Mark) -> String {
    match mark {
        Some((time, len)) => {
            let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
            format!("{}.{}:{len}", since.as_secs(), since.subsec_nanos())
        }
        None => String::new(),
    }
}

/// A source's file as the script is given it: its text, or why there is
/// none. A table too long to read whole keeps its newest rows.
fn read_source(path: &Path, format: &str, found: Mark) -> Value {
    let Some((modified, len)) = found else {
        return if path.is_dir() {
            json!({ "error": "a folder, not a file" })
        } else {
            json!({ "error": "no such file" })
        };
    };
    let about = |mut told: Map<String, Value>| {
        told.insert("size".into(), json!(len));
        told.insert("mtime".into(), json!(iso(modified)));
        told.insert("mark".into(), json!(mark_text(found)));
        Value::Object(told)
    };
    let failed = |error: String| about(Map::from_iter([("error".to_owned(), json!(error))]));
    let Ok(mut file) = std::fs::File::open(path) else {
        return failed("it could not be read".into());
    };
    let mut bytes = Vec::new();
    if len > MAX_SOURCE {
        let rows = matches!(format, "jsonl" | "csv" | "tsv");
        if !rows {
            return failed(format!("bigger than {} MB", MAX_SOURCE >> 20));
        }
        // A table's first line names its columns: it is kept.
        if format != "jsonl" {
            let mut head = vec![0; 64 << 10];
            let got = file.read(&mut head).unwrap_or(0);
            head.truncate(got);
            if let Some(end) = head.iter().position(|byte| *byte == b'\n') {
                bytes.extend_from_slice(&head[..=end]);
            }
        }
        // The newest rows are at the end: those, from a whole line.
        let mut tail = Vec::new();
        if file.seek(SeekFrom::Start(len - MAX_SOURCE)).is_err()
            || file.read_to_end(&mut tail).is_err()
        {
            return failed("it could not be read".into());
        }
        let from = tail
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(tail.len(), |end| end + 1);
        bytes.extend_from_slice(&tail[from..]);
    } else if file.take(MAX_SOURCE + 1).read_to_end(&mut bytes).is_err() {
        return failed("it could not be read".into());
    }
    about(Map::from_iter([(
        "text".to_owned(),
        json!(String::from_utf8_lossy(&bytes)),
    )]))
}

/// The lines a Code shows of its file: all of them, or `10-40` of them.
fn read_code(path: &Path, shown: &str, lines: Option<&str>) -> Value {
    let Ok(file) = std::fs::File::open(path) else {
        return json!({ "error": format!("There is no {shown}.") });
    };
    let mut bytes = Vec::new();
    if file.take(MAX_CODE).read_to_end(&mut bytes).is_err() {
        return json!({ "error": format!("{shown} could not be read.") });
    }
    let text = String::from_utf8_lossy(&bytes);
    let range = lines.and_then(|lines| {
        let (from, to) = lines.trim().split_once('-').unwrap_or((lines.trim(), ""));
        let from: usize = from.parse().ok()?;
        let to = if to.is_empty() {
            from
        } else {
            to.parse().ok()?
        };
        Some((from.max(1), to.max(from)))
    });
    match range {
        Some((from, to)) => {
            let picked: Vec<&str> = text.lines().skip(from - 1).take(to + 1 - from).collect();
            json!({ "text": picked.join("\n") })
        }
        None => json!({ "text": text }),
    }
}

/// What tells two themes apart, cheaply.
fn theme_key(theme: &kou_ui::Theme) -> u64 {
    let palette = &theme.palette;
    (u64::from(theme.dark) << 63)
        ^ (u64::from(palette.accent) << 32)
        ^ (u64::from(palette.pane) << 8)
        ^ u64::from(palette.fg)
}

impl Surfaces {
    /// The version in `path` of the workspace `root`, read once.
    fn version(&mut self, root: &Path, path: &str) -> Option<Rc<Value>> {
        let full = root.join(path);
        if let Some(known) = self.versions.get(&full) {
            return Some(known.clone());
        }
        let now = Instant::now();
        if self
            .missing
            .get(&full)
            .is_some_and(|tried| now.duration_since(*tried) < POLL)
        {
            return None;
        }
        let read = || {
            let mut text = String::new();
            std::fs::File::open(&full)
                .ok()?
                .take(MAX_VERSION)
                .read_to_string(&mut text)
                .ok()?;
            let version: Value = serde_json::from_str(&text).ok()?;
            // (What the tool writes, and nothing that merely lies there.)
            version
                .get("kou")
                .and_then(Value::as_str)
                .is_some_and(|format| format.starts_with("genui/"))
                .then(|| Rc::new(version))
        };
        match read() {
            Some(version) => {
                if self.versions.len() >= KEPT * 4 {
                    self.versions.clear();
                }
                self.missing.remove(&full);
                self.versions.insert(full, version.clone());
                Some(version)
            }
            None => {
                if self.missing.len() >= KEPT * 4 {
                    self.missing.clear();
                }
                self.missing.insert(full, now);
                None
            }
        }
    }

    /// The pane a surface is shown in, the call that made it, and its
    /// workspace.
    fn place(&self, instance: &str) -> Option<(String, String, PathBuf)> {
        let shown = self.shown.get(instance)?;
        Some((shown.pane.clone(), shown.call.clone(), shown.root.clone()))
    }

    /// Where each surface is shown.
    fn places(&self) -> Vec<(String, String)> {
        self.shown
            .values()
            .map(|shown| (shown.pane.clone(), shown.call.clone()))
            .collect()
    }

    /// What a surface is drawn from has changed: it is built again.
    fn touch(&mut self, instance: &str) {
        if let Some(shown) = self.shown.get_mut(instance) {
            shown.stale = true;
        }
    }

    /// Looks at the files of the live surfaces on screen: where the ones
    /// whose files changed are shown.
    fn look(&mut self, now: Instant) -> Vec<(String, String)> {
        let mut changed = Vec::new();
        for shown in self.shown.values_mut() {
            if !shown.live || shown.stale || now.duration_since(shown.seen) > ON_SCREEN {
                continue;
            }
            let moved = shown
                .files
                .iter()
                .any(|file| file.path.as_ref().is_ok_and(|path| mark(path) != file.mark));
            if moved {
                shown.stale = true;
                changed.push((shown.pane.clone(), shown.call.clone()));
            }
        }
        changed
    }

    /// Lets go of the surfaces drawn longest ago, past [`KEPT`].
    fn trim(&mut self) {
        if self.shown.len() <= KEPT {
            return;
        }
        let mut seen: Vec<Instant> = self.shown.values().map(|shown| shown.seen).collect();
        seen.sort_unstable();
        let keep_from = seen[seen.len() - KEPT * 3 / 4];
        self.shown.retain(|_, shown| shown.seen >= keep_from);
    }
}

/// The files a version reads: its sources, and what its Code components
/// show of files.
fn files_of(version: &Value, root: &Path) -> Vec<Watched> {
    let mut files = Vec::new();
    if let Some(sources) = version.get("sources").and_then(Value::as_object) {
        for (name, decl) in sources {
            let Some(file) = decl.get("file").and_then(Value::as_str) else {
                continue;
            };
            files.push(Watched {
                name: name.clone(),
                code: false,
                format: decl
                    .get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("text")
                    .to_owned(),
                lines: None,
                path: inside(root, file),
                mark: None,
                sent: false,
            });
        }
    }
    let components = version.get("components").and_then(Value::as_array);
    for component in components.into_iter().flatten() {
        let text = |key: &str| component.get(key).and_then(Value::as_str);
        if text("component") != Some("Code") || component.get("text").is_some() {
            continue;
        }
        if let (Some(id), Some(file)) = (text("id"), text("file")) {
            files.push(Watched {
                name: id.to_owned(),
                code: true,
                format: file.to_owned(),
                lines: text("lines").map(str::to_owned),
                path: inside(root, file),
                mark: None,
                sent: false,
            });
        }
    }
    files
}

/// What the script is given to draw a surface from, its files read where
/// they have changed since it was last given them.
fn input_of(shown: &mut Shown, instance: &str, version: &Value, theme: &kou_ui::Theme) -> Value {
    let (mut sources, mut code) = (Map::new(), Map::new());
    shown.waiting = false;
    for file in &mut shown.files {
        let path = match &file.path {
            Ok(path) => path,
            Err(why) => {
                let told = json!({ "error": why });
                if file.code {
                    code.insert(file.name.clone(), told);
                } else if shown.live {
                    sources.insert(file.name.clone(), told);
                }
                continue;
            }
        };
        let found = mark(path);
        if file.code {
            // (What it shows of its file is small, and read as it is now
            // for an earlier version too, as everywhere else.)
            file.mark = found;
            code.insert(
                file.name.clone(),
                read_code(path, &file.format, file.lines.as_deref()),
            );
        } else if shown.live {
            shown.waiting |= found.is_none();
            let told = if file.sent && found == file.mark && found.is_some() {
                json!({ "mark": mark_text(found) })
            } else {
                read_source(path, &file.format, found)
            };
            file.mark = found;
            file.sent = told.get("error").is_none();
            sources.insert(file.name.clone(), told);
        }
    }
    json!({
        "theme": theme_json(theme),
        "data": {
            "key": instance,
            "state": version,
            "live": shown.live,
            "sources": sources,
            "code": code,
        },
    })
}

/// What a chat needs to draw the surfaces of its transcript.
struct Chat {
    pane: String,
    root: PathBuf,
    surfaces: Rc<RefCell<Surfaces>>,
    plugins: SharedHost,
    focus: FocusHandle,
    on: Rc<dyn Fn(&str) -> On>,
    keys: Keys,
}

impl Chat {
    /// The card of `call`, if it made a surface that can be drawn.
    fn face(&self, call: &ToolCall, aids: &ToolAids, cx: &App) -> Option<ToolFace> {
        let named = call.surface().filter(|named| !named.deleted)?;
        // (Never while something else has the plugins: the call is an
        // ordinary card for that frame.)
        let mut hosted = self.plugins.try_borrow_mut().ok()?;
        let epoch = hosted.epoch;
        let host = hosted.host.as_mut().filter(|host| host.has(VIEW))?;
        let mut surfaces = self.surfaces.try_borrow_mut().ok()?;
        let version = surfaces.version(&self.root, &named.path)?;
        let instance = format!("{}:{}", self.pane, named.path);
        let theme = kou_ui::theme(cx);
        let stamp = (epoch, theme_key(theme));
        surfaces.trim();
        let shown = surfaces
            .shown
            .entry(instance.clone())
            .or_insert_with(|| Shown {
                pane: self.pane.clone(),
                call: call.id.clone(),
                root: self.root.clone(),
                live: aids.newest,
                seen: Instant::now(),
                files: files_of(&version, &self.root),
                stamp,
                stale: true,
                tree: None,
                waiting: false,
            });
        shown.seen = Instant::now();
        shown.call.clone_from(&call.id);
        if shown.live != aids.newest {
            shown.live = aids.newest;
            shown.stale = true;
        }
        if shown.stamp.0 != epoch {
            // The scripts were read again: they hold nothing of before.
            shown.files.iter_mut().for_each(|file| file.sent = false);
        }
        if shown.stale || shown.stamp != stamp || shown.tree.is_none() {
            let mut tree = host
                .render_as(
                    VIEW,
                    &instance,
                    &input_of(shown, &instance, &version, theme),
                )
                .ok();
            // The script no longer has a file it was given: once more,
            // with the file.
            if tree.as_ref().is_some_and(|tree| tree.flag("again")) {
                shown.files.iter_mut().for_each(|file| file.sent = false);
                tree = host
                    .render_as(
                        VIEW,
                        &instance,
                        &input_of(shown, &instance, &version, theme),
                    )
                    .ok();
            }
            shown.tree = tree;
            shown.stamp = stamp;
            shown.stale = false;
        }
        let tree = shown.tree.clone()?;
        let (live, waiting) = (shown.live, shown.waiting);
        let followed = shown.files.iter().any(|file| !file.code);
        drop(surfaces);
        let typed = typing(&hosted, VIEW, &instance, &self.focus, &self.keys);
        let drawing = Drawing {
            page: SharedString::from(instance.clone()),
            theme,
            made: Cell::new(0),
            on: (self.on)(&instance),
            aids: Some(aids),
            root: Some(&self.root),
            typing: typed,
            slots: RefCell::default(),
            dropped: None,
        };
        let title = version
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&named.surface);
        Some(ToolFace {
            title: title.to_owned().into(),
            tag: format!("{} · v{}", named.surface, named.version).into(),
            status: match (live, waiting, followed) {
                (false, ..) => "snapshot",
                (true, true, _) => "waiting",
                (true, false, true) => "live",
                (true, false, false) => "",
            }
            .into(),
            live: live && !waiting,
            body: element(&tree, &drawing).into_any_element(),
        })
    }
}

impl Shell {
    /// What draws the surfaces of the chat in `pane`, whose agent works in
    /// `root`.
    pub(crate) fn surface_view(&mut self, pane: &str, root: &Path, cx: &Context<Self>) -> ToolView {
        self.start_plugins();
        let shell = cx.weak_entity();
        let chat = Chat {
            pane: pane.to_owned(),
            root: root.to_owned(),
            surfaces: self.surfaces.clone(),
            plugins: self.plugins.clone(),
            focus: self.plugin_focus.clone(),
            on: Rc::new(move |instance: &str| {
                let (shell, instance) = (shell.clone(), instance.to_owned());
                Rc::new(
                    move |handler: u64, event: Value, window: &mut Window, cx: &mut App| {
                        shell
                            .update(cx, |this, cx| {
                                this.plugin_event(VIEW, &instance, handler, event, window, cx);
                            })
                            .ok();
                    },
                ) as On
            }),
            keys: self.plugin_keys(cx),
        };
        Rc::new(move |call, aids, cx| chat.face(call, aids, cx))
    }

    /// Looks at the files of the surfaces on screen every [`POLL`].
    pub(crate) fn watch_surfaces(&mut self, cx: &mut Context<Self>) {
        self.surface_watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this
                    .update(cx, |this, cx| this.look_at_surfaces(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// The surfaces whose files have changed are drawn again.
    pub(crate) fn look_at_surfaces(&mut self, cx: &mut Context<Self>) {
        let changed = self.surfaces.borrow_mut().look(Instant::now());
        for (pane, call) in changed {
            self.redraw_call(&pane, &call, cx);
        }
    }

    fn redraw_call(&mut self, pane: &str, call: &str, cx: &mut Context<Self>) {
        if let Some(talking) = self.threads.get(pane) {
            talking
                .view()
                .update(cx, |view, cx| view.tool_changed(call, cx));
        }
    }

    /// The surface drawn for `instance` is built and measured again: what
    /// the user did to it changed what it shows. (Nothing for a page.)
    pub(crate) fn redraw_surface(&mut self, instance: &str, cx: &mut Context<Self>) {
        let place = {
            let mut surfaces = self.surfaces.borrow_mut();
            surfaces.touch(instance);
            surfaces.place(instance)
        };
        if let Some((pane, call, _)) = place {
            self.redraw_call(&pane, &call, cx);
        }
    }

    /// Every surface is built again: the scripts that draw them changed.
    pub(crate) fn redraw_surfaces(&mut self, cx: &mut Context<Self>) {
        let places = self.surfaces.borrow().places();
        for (pane, call) in places {
            self.redraw_call(&pane, &call, cx);
        }
    }

    /// A button of a surface tells the agent of its chat: `text` goes to it
    /// as a message of the user's.
    pub(crate) fn surface_said(&mut self, instance: &str, text: &str, cx: &mut Context<Self>) {
        let Some((pane, ..)) = self.surfaces.borrow().place(instance) else {
            return;
        };
        if !self.send_text(&pane, text, true, cx) {
            self.notice = Some("This chat's agent is gone: nothing was sent.".into());
        }
    }

    /// Opens a file of a surface's workspace with what the system opens
    /// such files with.
    pub(crate) fn surface_open(&mut self, instance: &str, file: &str, cx: &mut Context<Self>) {
        let Some((_, _, root)) = self.surfaces.borrow().place(instance) else {
            return;
        };
        match inside(&root, file) {
            Ok(path) if path.exists() => cx.open_with_system(&path),
            Ok(_) => self.notice = Some(format!("There is no {file}.").into()),
            Err(why) => self.notice = Some(format!("{file}: {why}.").into()),
        }
    }

    /// A field of a surface is no longer typed in: the keys go back to the
    /// chat's own field.
    pub(crate) fn surface_done_typing(
        &mut self,
        instance: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((pane, ..)) = self.surfaces.borrow().place(instance) else {
            return;
        };
        if let Some(talking) = self.threads.get(&pane) {
            let field = talking.view().read(cx).focus_handle(cx);
            window.focus(&field, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kou_plugins::{Host, Source};

    /// A version as the tool writes it: a migration for the user to
    /// approve, over a file of benchmark runs.
    fn version() -> Value {
        json!({
            "kou": "genui/v0", "surface": "migration", "version": 2, "session": "s-1",
            "title": "Schema migration", "glance": "",
            "sources": { "runs": { "file": "bench/runs.jsonl", "format": "jsonl" } },
            "data": { "mode": "batch" },
            "components": [
                { "id": "root", "component": "Column", "children":
                    ["kpis", "what", "box", "comment", "mode", "buttons", "trend", "table", "page"] },
                { "id": "kpis", "component": "Row", "children": ["p50", "p99"] },
                { "id": "p50", "component": "Metric", "label": "p50", "unit": "ms", "better": "lower",
                  "digits": 1, "value": { "path": "/sources/runs/last/p50_ms" } },
                { "id": "p99", "component": "Metric", "label": "p99", "unit": "ms", "better": "lower",
                  "digits": 1, "value": { "path": "/sources/runs/last/p99_ms" } },
                { "id": "what", "component": "P", "text": "{/sources/runs/count} runs so far",
                  "size": "large", "weight": "semibold", "color": "accent" },
                { "id": "box", "component": "Div", "direction": "row", "gap": 8, "padding": 12,
                  "background": "bg-2", "border": true, "radius": 8, "align": "center",
                  "children": ["tag", "space", "note"] },
                { "id": "tag", "component": "Badge", "text": "draft", "tone": "warn" },
                { "id": "space", "component": "Spacer" },
                { "id": "note", "component": "Container", "title": "Mind", "tone": "info",
                  "children": ["rule", "said"] },
                { "id": "rule", "component": "Divider", "label": "downtime" },
                { "id": "said", "component": "Text", "text": "about 40 s", "style": "caption" },
                { "id": "comment", "component": "TextField", "label": "Comment",
                  "placeholder": "when to run it", "value": { "path": "/comment" } },
                { "id": "mode", "component": "Select", "label": "fsync",
                  "value": { "path": "/mode" }, "options": ["always", "batch", "off"] },
                { "id": "buttons", "component": "Row", "children": ["apply", "cancel"] },
                { "id": "apply", "component": "Button", "label": "Apply", "style": "primary", "once": true,
                  "action": { "event": { "name": "apply", "data": {
                      "comment": { "path": "/comment" }, "mode": { "path": "/mode" } } } } },
                { "id": "cancel", "component": "Button", "label": "Cancel",
                  "action": { "event": { "name": "cancel" } } },
                { "id": "trend", "component": "Chart", "kind": "line",
                  "data": { "path": "/sources/runs/rows" }, "x": "commit",
                  "y": ["p50_ms", "p99_ms"], "unit": "ms", "alt": "p50 and p99 by commit" },
                { "id": "table", "component": "Table", "rows": { "path": "/sources/runs/rows" },
                  "reverse": true, "limit": 2, "columns": ["commit", "p99_ms"] },
                { "id": "page", "component": "Html", "html": "<b>x</b>", "alt": "a map of the modules" },
            ],
            "snapshot": {
                "sources": { "runs": {
                    "rows": [{ "commit": "a1", "p50_ms": 9.0, "p99_ms": 50.0 }],
                    "last": { "commit": "a1", "p50_ms": 9.0, "p99_ms": 50.0 }, "count": 1 } },
                "sourceInfo": { "runs": { "path": "bench/runs.jsonl", "format": "jsonl" } },
                "diffs": {},
            },
            "text": "◇ Schema migration",
        })
    }

    const RUNS: &str = "{\"commit\": \"a1\", \"p50_ms\": 9.0, \"p99_ms\": 50.0}\n\
        {\"commit\": \"b2\", \"p50_ms\": 8.5, \"p99_ms\": 44.0}\n\
        {\"commit\": \"c3\", \"p50_ms\": 8.2, \"p99_ms\": 41.6}\n";

    fn input(live: bool, runs: Value) -> Value {
        json!({
            "theme": theme_json(&kou_ui::Theme::new(true)),
            "data": { "key": "p1:v2", "state": version(), "live": live,
                      "sources": { "runs": runs }, "code": {} },
        })
    }

    #[test]
    fn the_plugin_draws_a_surface_from_its_files_and_does_what_is_asked_of_it() {
        let mut host = Host::new(vec![Source::Embedded(crate::plugins::BUILT_IN)]).unwrap();
        assert_eq!(host.problems(), []);
        assert!(host.has(VIEW), "the plugin's view");
        assert!(host.pages().iter().all(|page| page.id != VIEW), "no page");
        let live = input(true, json!({ "text": RUNS, "mark": "1:3", "size": 150 }));
        let tree = host.render_as(VIEW, "p1:v2", &live).unwrap();
        let said = tree.said();
        // The file as it is now: its newest row, and the change from the
        // row before it, which is good news where lower is better.
        assert!(said.contains("P99 41.6 ms ▼5%"), "{said}");
        assert!(said.contains("3 runs so far"), "{said}");
        assert_eq!(tree.find("gu-what").unwrap().number("size"), Some(16.));
        // A box of the agent's own arranging, in the theme's colours.
        let made = tree.find("gu-box").unwrap();
        assert_eq!(made.text("dir"), Some("row"));
        // (Its grounds are counted from the card's: `bg-2` is a step up.)
        assert_eq!(made.text("bg"), Some("hover"));
        assert_eq!(
            (made.number("p"), made.number("radius")),
            (Some(12.), Some(8.))
        );
        assert!(said.contains("draft") && said.contains("Mind") && said.contains("downtime"));
        // The chart's marks are the app's to draw: two series of three.
        let plot = tree.find("gu-trend-plot").unwrap();
        assert_eq!(plot.kind, "plot");
        let series = plot.props["series"].as_array().unwrap();
        assert_eq!(series.len(), 2);
        let values: Vec<f64> = series[1]["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_f64().unwrap())
            .collect();
        assert_eq!(values, [50.0, 44.0, 41.6]);
        assert_eq!(series[1]["shown"][2], "41.6 ms");
        assert_eq!(plot.props["labels"], json!(["a1", "b2", "c3"]));
        assert_eq!(
            (plot.number("lo"), plot.number("hi")),
            (Some(0.), Some(60.))
        );
        // The table's newest rows first, as many as it asks for.
        assert!(
            said.contains("COMMIT P99_MS c3 41.6 b2 44 1 more row before these"),
            "{said}"
        );
        // A page of the agent's own is said, not drawn.
        assert!(said.contains("HTML a map of the modules"), "{said}");
        // Typing in a field and choosing are kept as the surface's own.
        let field = tree.find("gu-comment").unwrap();
        assert_eq!(
            (field.kind.as_str(), field.text("placeholder")),
            ("input", Some("when to run it"))
        );
        let typed = field.handler("onChange").unwrap();
        assert_eq!(
            host.dispatch_as(VIEW, "p1:v2", typed, &json!({ "value": "after 18:00" })),
            []
        );
        let tree = host.render_as(VIEW, "p1:v2", &live).unwrap();
        assert_eq!(
            tree.find("gu-comment").unwrap().text("value"),
            Some("after 18:00")
        );
        let off = tree.find("gu-mode-2").unwrap().handler("onClick").unwrap();
        host.dispatch_as(VIEW, "p1:v2", off, &json!({}));
        let tree = host.render_as(VIEW, "p1:v2", &live).unwrap();
        assert_eq!(tree.find("gu-mode-2").unwrap().text("bg"), Some("selected"));
        // The button tells the agent, with what was typed and chosen read
        // at the click; and works once.
        let apply = tree.find("gu-apply").unwrap();
        assert_eq!(
            (apply.kind.as_str(), apply.flag("primary")),
            ("button", true)
        );
        let asked = host.dispatch_as(VIEW, "p1:v2", apply.handler("onClick").unwrap(), &json!({}));
        assert_eq!(
            asked,
            [(
                "surface.send".to_owned(),
                json!({
                    "text": "[UI migration v2] apply {\"comment\":\"after 18:00\",\"mode\":\"off\"}",
                    "name": "apply",
                })
            )]
        );
        let tree = host.render_as(VIEW, "p1:v2", &live).unwrap();
        let apply = tree.find("gu-apply").unwrap();
        assert_eq!(apply.text("label"), Some("Apply ✓"));
        assert!(apply.flag("disabled") && apply.handler("onClick").is_none());
        // An event with nothing to say is its name alone.
        let cancel = tree.find("gu-cancel").unwrap().handler("onClick").unwrap();
        assert_eq!(
            host.dispatch_as(VIEW, "p1:v2", cancel, &json!({}))[0].1["text"],
            "[UI migration v2] cancel"
        );
        // The file unchanged is not given again: the script has it.
        let same = input(true, json!({ "mark": "1:3", "size": 150 }));
        let tree = host.render_as(VIEW, "p1:v2", &same).unwrap();
        assert!(tree.said().contains("P99 41.6 ms") && !tree.flag("again"));
        // One it does not have, it asks for.
        let other = input(true, json!({ "mark": "2:9", "size": 150 }));
        assert!(host.render_as(VIEW, "p1:v2", &other).unwrap().flag("again"));
        // A file that is not there yet is said, and the rest stands.
        let none = input(true, json!({ "error": "no such file" }));
        let said = host.render_as(VIEW, "p1:v2", &none).unwrap().said();
        assert!(
            said.contains("Waiting for bench/runs.jsonl") && said.contains("P99 —"),
            "{said}"
        );
        // An earlier version shows what its call read, and answers to
        // nothing: another surface, with nothing typed into it.
        let snapshot = host
            .render_as(VIEW, "p1:v1", &input(false, Value::Null))
            .unwrap();
        let said = snapshot.said();
        assert!(
            said.contains("P99 50.0 ms") && said.contains("1 runs so far"),
            "{said}"
        );
        let apply = snapshot.find("gu-apply").unwrap();
        assert!(apply.flag("disabled") && apply.handler("onClick").is_none());
        let field = snapshot.find("gu-comment").unwrap();
        assert!(field.flag("disabled") && field.handler("onChange").is_none());
        assert_eq!(host.problems(), []);
    }

    /// The data model is the browser cockpit's own file, copied: a surface
    /// reads the same here as there and in the text the agent got back.
    #[test]
    fn the_data_model_is_the_browser_cockpit_s_file() {
        let here = include_str!("../../../plugins/genui/shared/data.js");
        let there = include_str!("../../../../examples/plugins/genui/web/data.js");
        assert!(
            here == there,
            "desktop/plugins/genui/shared/data.js is not examples/plugins/genui/web/data.js: \
             copy it again"
        );
    }

    #[test]
    fn a_surface_reads_only_files_of_its_workspace() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("work");
        std::fs::create_dir_all(root.join("bench")).unwrap();
        std::fs::write(root.join("bench/runs.jsonl"), "{}\n").unwrap();
        std::fs::write(folder.path().join("secret"), "no").unwrap();
        assert_eq!(
            inside(&root, "bench/runs.jsonl"),
            Ok(root.join("bench/runs.jsonl"))
        );
        // One that is not there yet is fine: it is shown once it appears.
        assert!(inside(&root, "bench/later.jsonl").is_ok());
        for out in ["../secret", "/etc/passwd", "bench/../../secret", ""] {
            assert!(inside(&root, out).is_err(), "{out}");
        }
        for hidden in [".git/config", ".env", "deploy/.env.production"] {
            assert!(inside(&root, hidden).is_err(), "{hidden}");
        }
        // A link out of the workspace leads nowhere either.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(folder.path().join("secret"), root.join("link")).unwrap();
            assert!(inside(&root, "link").is_err());
        }
    }

    #[test]
    fn a_source_is_given_as_text_and_a_long_table_keeps_its_newest_rows() {
        let folder = tempfile::tempdir().unwrap();
        let small = folder.path().join("runs.jsonl");
        std::fs::write(&small, "{\"p99\": 41}\n").unwrap();
        let told = read_source(&small, "jsonl", mark(&small));
        assert_eq!(told["text"], "{\"p99\": 41}\n");
        assert_eq!(told["size"], 12);
        assert!(told["mtime"].as_str().unwrap().ends_with('Z'), "{told}");
        assert_eq!(
            read_source(&folder.path().join("none"), "jsonl", None)["error"],
            "no such file"
        );
        assert_eq!(
            read_source(folder.path(), "jsonl", mark(folder.path()))["error"],
            "a folder, not a file"
        );
        // Longer than is read: the header and the end, from a whole line.
        let long = folder.path().join("long.csv");
        let mut text = String::from("n,name\n");
        let mut n = 0;
        while text.len() as u64 <= MAX_SOURCE + 4096 {
            text.push_str(&format!("{n},row number {n}\n"));
            n += 1;
        }
        std::fs::write(&long, &text).unwrap();
        let told = read_source(&long, "csv", mark(&long));
        let read = told["text"].as_str().unwrap();
        assert!(read.len() as u64 <= MAX_SOURCE + 16, "{}", read.len());
        let mut lines = read.lines();
        assert_eq!(lines.next(), Some("n,name"));
        let first = lines.next().unwrap();
        assert!(
            first.split_once(',').unwrap().0.parse::<u64>().is_ok(),
            "{first}"
        );
        assert_eq!(
            read.lines().last().unwrap(),
            format!("{},row number {}", n - 1, n - 1)
        );
        // And what is no table is not read in part.
        let told = read_source(&long, "json", mark(&long));
        assert_eq!(told["error"], "bigger than 2 MB");
    }

    #[test]
    fn a_code_shows_the_lines_it_names() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("main.go");
        std::fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(
            read_code(&file, "main.go", Some("2-3"))["text"],
            "two\nthree"
        );
        assert_eq!(read_code(&file, "main.go", Some("4"))["text"], "four");
        assert_eq!(
            read_code(&file, "main.go", None)["text"],
            "one\ntwo\nthree\nfour\n"
        );
        assert_eq!(
            read_code(&folder.path().join("no.go"), "no.go", None)["error"],
            "There is no no.go."
        );
    }

    #[test]
    fn a_time_reads_as_the_tool_writes_it() {
        let at = |seconds: u64| iso(UNIX_EPOCH + Duration::from_secs(seconds));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_794_648_645), "2026-11-14T09:30:45Z");
    }
}
