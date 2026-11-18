//! The prompt field: a wrapped, multi-line text input with the platform's
//! editing keys and a real input handler, so composition and non-Latin layouts
//! work as they do in any native field. What a key does to the text is decided
//! in [`crate::buffer`]; this file adds the window: layout, pointer, clipboard.
//!
//! Around the text it keeps what the cockpits' composers keep: the list a `$`
//! or a `/` opens (the project's files, the commands, then a command's
//! values), the pictures the text names by `[Image n]` labels with their
//! strip and their large preview, and the files its `$` references link,
//! as chips. How those look is in [`parts`].

pub(crate) mod parts;

use crate::{
    buffer::{Buffer, Motion},
    commands::{Catalog, Offer, Suggestion},
    draft::{Attached, Draft},
    pictures::{self, Prepared},
    prose::{LINE, PROSE},
};
use gpui::{
    App, AppContext as _, AvailableSpace, Bounds, ClipboardEntry, ClipboardItem, ContentMask,
    Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, ExternalPaths, FocusHandle, Focusable, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, InteractiveElement as _, IntoElement, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
    ScrollEvent, SharedString, Style, Styled as _, TextRun, UTF16Selection, UnderlineStyle, Window,
    WrappedLine, WrappedLineLayout, actions, div, fill, point, relative, size,
};
use kou_thread::{
    Choice,
    files::{FileIndex, FileLink, LinkQuery, file_links, link_label, link_query},
};
use kou_ui::scale::{self, px};
use std::{
    cell::Cell,
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

/// The key context of the field, for key bindings.
pub(crate) const CONTEXT: &str = "ThreadComposer";

/// Lines the field grows to before it scrolls.
const MAX_LINES: usize = 10;
/// Rows a list shows before it scrolls.
const MENU_ROWS: usize = 8;
/// Files a `$` list holds at most, as the web cockpit asks for.
const FILE_LIMIT: usize = 60;
/// How long a notice stays.
const NOTICE: Duration = Duration::from_secs(5);
/// How long what leaves takes to go.
pub(crate) const LEAVING: Duration = kou_ui::motion::BASE;

actions!(
    kou_thread_composer,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
        Left,
        Right,
        WordLeft,
        WordRight,
        LineStart,
        LineEnd,
        DocumentStart,
        DocumentEnd,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectLineStart,
        SelectLineEnd,
        SelectDocumentStart,
        SelectDocumentEnd,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Newline,
        Submit,
        Force,
        Complete,
        Dismiss,
        SearchHistory,
    ]
);

/// What the field tells the view around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComposerEvent {
    /// Enter was pressed on this text. The view clears the field if it sends it.
    Submit(String),
    /// Cmd+Enter (Ctrl+Enter elsewhere) was pressed on this text: force it
    /// into the running turn.
    Force(String),
    /// Enter was pressed on a field with nothing to send.
    Blank,
    /// The field became empty or stopped being empty.
    Emptied(bool),
    /// Up or Down asked for another prompt from the history.
    Recall {
        /// Towards older prompts.
        older: bool,
    },
}

/// A picture the field holds, named in the text by its label.
#[derive(Clone, Debug)]
pub(crate) struct Picture {
    /// Unique within the field, for its card and its focus.
    pub id: u64,
    pub label: String,
    pub state: PictureState,
}

/// Where a picture stands.
#[derive(Clone, Debug)]
pub(crate) enum PictureState {
    /// Being decoded and made ready, off the main thread.
    Reading,
    Ready(Prepared),
}

/// What a list over the field completes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuKind {
    /// A `$` reference: the project's files.
    Files { query: LinkQuery },
    /// A slash command, then its argument.
    Commands,
    /// Earlier prompts, filtered by what is typed.
    History,
}

/// One line of a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuItem {
    /// What the line names: a path, a command, a value, a prompt.
    pub name: String,
    /// What it says beside.
    pub detail: String,
    pub what: What,
}

/// What choosing a line does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum What {
    File {
        directory: bool,
    },
    Command {
        spec: usize,
        alias: Option<&'static str>,
    },
    Value {
        spec: usize,
        value: String,
        current: bool,
    },
    /// Says what a command expects; not a choice.
    Hint,
    Prompt,
}

/// The list open over the field.
#[derive(Clone, Debug)]
pub(crate) struct Menu {
    pub kind: MenuKind,
    pub items: Vec<MenuItem>,
    pub highlighted: usize,
    /// The highlight was moved by hand: Enter then takes it in a list of files.
    pub moved: bool,
    /// The files are still being listed.
    pub loading: bool,
    /// Where in the text what it completes starts, for the list to open from.
    pub anchor: usize,
    /// A new number for each list opened, for its entrance.
    pub id: u64,
}

impl Menu {
    fn choosable(&self) -> bool {
        self.items.iter().any(|item| item.what != What::Hint)
    }
}

/// A short message over the field: a picture refused, a limit reached.
#[derive(Clone, Debug)]
pub(crate) struct Notice {
    pub text: SharedString,
    pub error: bool,
    pub id: u64,
    /// It says which key to press next, and goes when its time is up:
    /// Escape is not taken to send it away.
    pub held: bool,
    /// The picture it is about, by its label: it goes with that picture,
    /// as soon as the text names it no more.
    pub about: Option<String>,
}

/// Something shown that is on its way out, kept until its exit is over.
#[derive(Clone, Debug)]
pub(crate) struct Leaving<T> {
    pub item: T,
    pub since: Instant,
}

/// The prompt field of a thread view.
pub(crate) struct Composer {
    focus: FocusHandle,
    buffer: Buffer,
    placeholder: SharedString,
    /// What the empty field says when it is narrow.
    short_placeholder: SharedString,
    geometry: Option<Geometry>,
    /// How far the text is scrolled once it is taller than the field.
    scroll: Pixels,
    /// Where a wheel step glides the text to, as GPUI's scroll areas glide
    /// (`gpui::SmoothScroll`), in pixels as drawn.
    glide: Option<gpui::ScrollGlide>,
    /// Bring the cursor into view at the next layout.
    reveal: bool,
    /// The column vertical movement tries to keep.
    goal_x: Option<Pixels>,
    /// The cursor sits at the start of a wrapped row rather than the end of the one before.
    downstream: bool,
    selecting: bool,
    /// The text a history recall put in the field, while it is still untouched.
    recalled: Option<String>,

    /// The backend's slash commands.
    commands: Vec<Choice>,
    /// No command is offered at all: the field stands alone, without a backend.
    commands_off: bool,
    /// What else is offered, and the commands in force made of both.
    offer: Offer,
    catalog: Catalog,
    /// The project whose files a `$` links; none, no file list.
    project: Option<PathBuf>,
    index: Option<FileIndex>,
    menu: Option<Menu>,
    menus_opened: u64,
    /// The list was dismissed for the text as it is.
    dismissed: Option<String>,
    /// Earlier prompts for the history list, newest last: text and when.
    history: Vec<(String, String)>,

    pictures: Vec<Picture>,
    pictures_made: u64,
    /// Whether the backend takes pictures: `None` until it says.
    takes_pictures: Option<bool>,
    /// The files the text links, as the last look at it found them.
    links: Vec<FileLink>,
    links_seen: u64,
    /// What the strip and the tray showed last, to see what leaves.
    shown_pictures: Vec<Picture>,
    shown_links: Vec<FileLink>,
    leaving_pictures: Vec<Leaving<Picture>>,
    leaving_links: Vec<Leaving<FileLink>>,
    /// The picture shown large, and whether a click opened it (it stays) or
    /// the cursor after its label (it follows the cursor).
    preview: Option<(u64, bool)>,
    notice: Option<Notice>,
    notices: u64,
    /// A line over the text, set by the view: what the field is editing.
    banner: Option<SharedString>,
    /// Focus of each chip and each picture, by key.
    chip_focus: HashMap<String, FocusHandle>,
    picture_focus: HashMap<u64, FocusHandle>,
    /// Heights of what grows above the text, measured as drawn.
    above_height: Rc<Cell<f32>>,
    /// Where the field was drawn, for the lists and the preview.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The factor the thread view draws at, which the field draws at too.
    scale: Rc<Cell<f32>>,
    /// The thread view's trace, which notes the frames the field is built in.
    trace: crate::trace::Trace,
    /// The field stands in a row of the transcript, as a prompt's editor:
    /// it draws at the factor of where it is drawn, which is the row's.
    inline: bool,
    /// How the view it is the field of looks. See [`crate::nested::Around`].
    around: Rc<crate::nested::Around>,
    /// What draws the list over the field, when the app does (see
    /// [`crate::face`]): the chat's, set by the view.
    pub(crate) chat_face: Option<crate::face::ChatFace>,
    /// The caret's blink.
    blink: kou_ui::Blink,
    /// Any key pressed while the field has the keys lights the caret at once,
    /// whether it moves it, types or does nothing at all.
    _keystrokes: gpui::Subscription,
    /// Where the caret and the first line were last drawn, for the tests.
    #[cfg(test)]
    drawn: Option<Drawn>,
}

/// The caret as last drawn, beside the line it stands on.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct Drawn {
    caret: Option<Bounds<Pixels>>,
    /// How lit the caret was.
    lit: f32,
    /// Where the first line (the placeholder, when the field is empty) was drawn.
    origin: Point<Pixels>,
    line_height: Pixels,
    ascent: Pixels,
    descent: Pixels,
    /// Where the first glyph stands in its line.
    first_glyph: Pixels,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Focusable for Composer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The field's key bindings: the platform's own editing keys.
pub(crate) fn bindings() -> Vec<KeyBinding> {
    let context = Some(CONTEXT);
    let mut keys = vec![
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("shift-backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("shift-home", SelectLineStart, context),
        KeyBinding::new("shift-end", SelectLineEnd, context),
        KeyBinding::new("enter", Submit, context),
        KeyBinding::new("shift-enter", Newline, context),
        KeyBinding::new("alt-enter", Newline, context),
        KeyBinding::new("tab", Complete, context),
        KeyBinding::new("escape", Dismiss, context),
        KeyBinding::new("ctrl-r", SearchHistory, context),
    ];
    if cfg!(target_os = "macos") {
        keys.extend([
            KeyBinding::new("cmd-enter", Force, context),
            KeyBinding::new("alt-backspace", DeleteWordLeft, context),
            KeyBinding::new("alt-delete", DeleteWordRight, context),
            KeyBinding::new("cmd-backspace", DeleteToLineStart, context),
            KeyBinding::new("cmd-delete", DeleteToLineEnd, context),
            KeyBinding::new("alt-left", WordLeft, context),
            KeyBinding::new("alt-right", WordRight, context),
            // (Not Cmd+Left and Cmd+Right: the app walks its tabs with
            // those. Home and End, or Ctrl+A and Ctrl+E, go to the ends of
            // a line.)
            KeyBinding::new("cmd-up", DocumentStart, context),
            KeyBinding::new("cmd-down", DocumentEnd, context),
            KeyBinding::new("alt-shift-left", SelectWordLeft, context),
            KeyBinding::new("alt-shift-right", SelectWordRight, context),
            KeyBinding::new("cmd-shift-left", SelectLineStart, context),
            KeyBinding::new("cmd-shift-right", SelectLineEnd, context),
            KeyBinding::new("cmd-shift-up", SelectDocumentStart, context),
            KeyBinding::new("cmd-shift-down", SelectDocumentEnd, context),
            KeyBinding::new("cmd-a", SelectAll, context),
            KeyBinding::new("cmd-c", Copy, context),
            KeyBinding::new("cmd-x", Cut, context),
            KeyBinding::new("cmd-v", Paste, context),
            KeyBinding::new("cmd-z", Undo, context),
            KeyBinding::new("cmd-shift-z", Redo, context),
            // The Emacs keys every macOS text field answers to.
            KeyBinding::new("ctrl-a", LineStart, context),
            KeyBinding::new("ctrl-e", LineEnd, context),
            KeyBinding::new("ctrl-b", Left, context),
            KeyBinding::new("ctrl-f", Right, context),
            KeyBinding::new("ctrl-p", Up, context),
            KeyBinding::new("ctrl-n", Down, context),
            KeyBinding::new("ctrl-h", Backspace, context),
            KeyBinding::new("ctrl-d", Delete, context),
            KeyBinding::new("ctrl-k", DeleteToLineEnd, context),
        ]);
    } else {
        keys.extend([
            KeyBinding::new("ctrl-enter", Force, context),
            KeyBinding::new("ctrl-backspace", DeleteWordLeft, context),
            KeyBinding::new("ctrl-delete", DeleteWordRight, context),
            KeyBinding::new("ctrl-left", WordLeft, context),
            KeyBinding::new("ctrl-right", WordRight, context),
            KeyBinding::new("ctrl-home", DocumentStart, context),
            KeyBinding::new("ctrl-end", DocumentEnd, context),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, context),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, context),
            KeyBinding::new("ctrl-shift-home", SelectDocumentStart, context),
            KeyBinding::new("ctrl-shift-end", SelectDocumentEnd, context),
            KeyBinding::new("ctrl-a", SelectAll, context),
            KeyBinding::new("ctrl-c", Copy, context),
            KeyBinding::new("ctrl-x", Cut, context),
            KeyBinding::new("ctrl-v", Paste, context),
            KeyBinding::new("ctrl-insert", Copy, context),
            KeyBinding::new("shift-insert", Paste, context),
            KeyBinding::new("shift-delete", Cut, context),
            KeyBinding::new("ctrl-z", Undo, context),
            KeyBinding::new("ctrl-y", Redo, context),
            KeyBinding::new("ctrl-shift-z", Redo, context),
        ]);
    }
    keys
}

/// One visual row of the wrapped text.
#[derive(Clone, Debug)]
struct Row {
    /// Which line (as typed) the row belongs to.
    line: usize,
    /// Offsets of the row within the whole text.
    range: Range<usize>,
    /// Top edge, from the top of the text.
    top: Pixels,
}

/// Where the text was last painted, for hit testing and movement by visual line.
#[derive(Clone, Debug)]
struct Geometry {
    /// The text this is a layout of; anything else makes it stale.
    text_len: usize,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    /// Offset of each typed line in the text, and its layout.
    lines: Vec<(usize, Arc<WrappedLineLayout>)>,
    rows: Vec<Row>,
    /// What the layouts' positions are multiplied by on screen: the lines are
    /// shaped at the unscaled size, so that they wrap where they wrap at 1.
    factor: f32,
}

impl Geometry {
    fn new(
        text_len: usize,
        bounds: Bounds<Pixels>,
        line_height: Pixels,
        shaped: &[WrappedLine],
        factor: f32,
    ) -> Self {
        let mut lines = Vec::with_capacity(shaped.len());
        let mut rows = Vec::new();
        let mut start = 0;
        let mut top = Pixels::ZERO;
        for (index, line) in shaped.iter().enumerate() {
            let layout: Arc<WrappedLineLayout> = Arc::clone(line);
            let mut row_start = 0;
            for boundary in layout.wrap_boundaries() {
                let at =
                    layout.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
                rows.push(Row {
                    line: index,
                    range: start + row_start..start + at,
                    top,
                });
                row_start = at;
                top += line_height;
            }
            rows.push(Row {
                line: index,
                range: start + row_start..start + layout.len(),
                top,
            });
            top += line_height;
            lines.push((start, layout.clone()));
            start += layout.len() + 1;
        }
        Self {
            text_len,
            bounds,
            line_height,
            lines,
            rows,
            factor,
        }
    }

    fn height(&self) -> Pixels {
        self.line_height * self.rows.len().max(1) as f32
    }

    /// The row the cursor at `offset` is drawn on. An offset where the text
    /// wraps is the end of one row and the start of the next; `downstream`
    /// picks the next.
    fn row_of(&self, offset: usize, downstream: bool) -> usize {
        let mut found = 0;
        for (index, row) in self.rows.iter().enumerate() {
            if offset < row.range.start {
                break;
            }
            found = index;
            if offset < row.range.end || (offset == row.range.end && !downstream) {
                break;
            }
        }
        found
    }

    fn x_in_row(&self, row: &Row, offset: usize) -> Pixels {
        let (line_start, layout) = &self.lines[row.line];
        let layout = &layout.unwrapped_layout;
        (layout.x_for_index(offset.saturating_sub(*line_start))
            - layout.x_for_index(row.range.start - line_start))
            * self.factor
    }

    /// Where the cursor at `offset` is, from the top left of the text.
    fn position(&self, offset: usize, downstream: bool) -> Point<Pixels> {
        let Some(row) = self.rows.get(self.row_of(offset, downstream)) else {
            return Point::default();
        };
        let offset = offset.clamp(row.range.start, row.range.end);
        point(self.x_in_row(row, offset), row.top)
    }

    /// The offset nearest to a point given from the top left of the text, and
    /// whether it is the start of a wrapped row.
    fn offset_at(&self, at: Point<Pixels>) -> (usize, bool) {
        if self.rows.is_empty() {
            return (0, false);
        }
        let index = ((at.y / self.line_height).floor().max(0.) as usize).min(self.rows.len() - 1);
        let row = &self.rows[index];
        let (line_start, layout) = &self.lines[row.line];
        let layout = &layout.unwrapped_layout;
        let origin = layout.x_for_index(row.range.start - line_start);
        let local = layout.closest_index_for_x(origin + at.x.max(Pixels::ZERO) / self.factor);
        let offset = (line_start + local).clamp(row.range.start, row.range.end);
        let wrapped = index > 0 && self.rows[index - 1].line == row.line;
        (offset, wrapped && offset == row.range.start)
    }
}

impl Composer {
    pub(crate) fn new(placeholder: SharedString, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            buffer: Buffer::default(),
            short_placeholder: placeholder.clone(),
            placeholder,
            geometry: None,
            scroll: Pixels::ZERO,
            glide: None,
            reveal: false,
            goal_x: None,
            downstream: false,
            selecting: false,
            recalled: None,
            commands: Vec::new(),
            commands_off: false,
            offer: Offer::default(),
            catalog: Catalog::default(),
            project: None,
            index: None,
            menu: None,
            menus_opened: 0,
            dismissed: None,
            history: Vec::new(),
            pictures: Vec::new(),
            pictures_made: 0,
            takes_pictures: None,
            links: Vec::new(),
            links_seen: 0,
            shown_pictures: Vec::new(),
            shown_links: Vec::new(),
            leaving_pictures: Vec::new(),
            leaving_links: Vec::new(),
            preview: None,
            notice: None,
            notices: 0,
            banner: None,
            chip_focus: HashMap::new(),
            picture_focus: HashMap::new(),
            above_height: Rc::new(Cell::new(0.)),
            bounds: Rc::new(Cell::new(Bounds::default())),
            scale: Rc::new(Cell::new(1.)),
            around: Rc::default(),
            chat_face: None,
            trace: None,
            inline: false,
            blink: kou_ui::Blink::new(cx.background_executor().now()),
            _keystrokes: cx.observe_keystrokes(|this: &mut Self, _, window, cx| {
                if this.focus.is_focused(window) {
                    this.blink.restart(cx.background_executor().now());
                    cx.notify();
                }
            }),
            #[cfg(test)]
            drawn: None,
        }
    }

    /// Makes the field one that stands in a row of the transcript (see
    /// [`crate::edit`]): it draws at the factor in effect where the row is
    /// drawn, which is 1 while the row's plan at 1 is made (see
    /// `kou_ui::scale::exact`) and the zoom otherwise.
    pub(crate) fn set_inline(&mut self, inline: bool) {
        self.inline = inline;
    }

    /// Draws at the factor in this cell, which the view around the field
    /// sets, and notes its frames in the view's trace.
    /// How the view it is the field of looks, shared with that view.
    pub(crate) fn set_around(&mut self, around: Rc<crate::nested::Around>) {
        self.around = around;
    }

    /// The theme the field draws with: its view's look's.
    pub(crate) fn themed<'a>(&self, cx: &'a App) -> &'a kou_ui::Theme {
        crate::studio::look_theme(self.around.look.get(), cx)
    }

    /// The theme the field's menus draw with: its view's look's, for what
    /// frames the conversation.
    pub(crate) fn chrome_themed<'a>(&self, cx: &'a App) -> &'a kou_ui::Theme {
        crate::studio::chrome_look_theme(self.around.look.get(), cx)
    }

    /// The text size and line pitch of what is typed.
    fn type_size(&self) -> (f32, f32) {
        if self.around.studio() {
            (crate::studio::FIELD, crate::studio::FIELD_LINE)
        } else {
            (PROSE, LINE_HEIGHT)
        }
    }

    pub(crate) fn set_scale(&mut self, scale: Rc<Cell<f32>>, trace: crate::trace::Trace) {
        self.scale = scale;
        self.trace = trace;
    }

    /// The text in the field.
    pub(crate) fn text(&self) -> &str {
        self.buffer.text()
    }

    /// What the empty field says, at length and when it is narrow.
    pub(crate) fn set_placeholders(
        &mut self,
        placeholder: SharedString,
        short: SharedString,
        cx: &mut Context<Self>,
    ) {
        if self.placeholder != placeholder || self.short_placeholder != short {
            self.placeholder = placeholder;
            self.short_placeholder = short;
            cx.notify();
        }
    }

    /// What the empty field says, at length and short, for tests.
    #[cfg(test)]
    pub(crate) fn placeholders(&self) -> (SharedString, SharedString) {
        (self.placeholder.clone(), self.short_placeholder.clone())
    }

    /// The placeholder for the field's width as last drawn.
    fn shown_placeholder(&self) -> SharedString {
        if self.bounds.get().size.width < px(560.) {
            self.short_placeholder.clone()
        } else {
            self.placeholder.clone()
        }
    }

    /// The slash commands the backend offers.
    pub(crate) fn set_commands(&mut self, commands: Vec<Choice>, cx: &mut Context<Self>) {
        if self.commands != commands {
            self.commands = commands;
            self.rebuild_catalog(cx);
        }
    }

    /// What else the field offers: the view's and the app's commands, the
    /// backend's models and modes, what a command's argument takes.
    pub(crate) fn set_offer(&mut self, mut offer: Offer, cx: &mut Context<Self>) {
        offer.backend.clear();
        if offer != self.offer {
            self.offer = offer;
            self.rebuild_catalog(cx);
        }
    }

    fn rebuild_catalog(&mut self, cx: &mut Context<Self>) {
        let mut offer = self.offer.clone();
        offer.backend = self.commands.clone();
        let catalog = Catalog::of(&offer);
        if catalog != self.catalog {
            self.catalog = catalog;
            if matches!(
                self.menu.as_ref().map(|menu| &menu.kind),
                Some(MenuKind::Commands) | None
            ) {
                self.refresh_menu(cx);
            }
            cx.notify();
        }
    }

    /// Whether no command is offered: a field without a backend.
    pub(crate) fn set_commands_off(&mut self, off: bool, cx: &mut Context<Self>) {
        if self.commands_off != off {
            self.commands_off = off;
            self.refresh_menu(cx);
            cx.notify();
        }
    }

    /// The commands in force.
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The project a `$` links files of.
    pub(crate) fn set_project(&mut self, project: Option<PathBuf>) {
        if self.project != project {
            self.project = project;
            self.index = None;
        }
    }

    /// Whether the backend takes pictures, once it says.
    pub(crate) fn set_takes_pictures(&mut self, takes: Option<bool>) {
        self.takes_pictures = takes;
    }

    /// Earlier prompts for the history list, oldest first: text and when.
    pub(crate) fn set_history(&mut self, history: Vec<(String, String)>) {
        self.history = history;
    }

    /// A line over the text that says what the field is editing.
    pub(crate) fn set_banner(&mut self, banner: Option<SharedString>, cx: &mut Context<Self>) {
        if self.banner != banner {
            self.banner = banner;
            cx.notify();
        }
    }

    /// Says something short over the field, for a few seconds.
    pub(crate) fn tell(
        &mut self,
        text: impl Into<SharedString>,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        self.say(text, error, NOTICE, false, cx);
    }

    /// Says over the field, for as long as `shown`, what holds only that
    /// long: a key that is to be pressed again. Escape passes it by.
    pub(crate) fn tell_for(
        &mut self,
        text: impl Into<SharedString>,
        error: bool,
        shown: Duration,
        cx: &mut Context<Self>,
    ) {
        self.say(text, error, shown, true, cx);
    }

    fn say(
        &mut self,
        text: impl Into<SharedString>,
        error: bool,
        shown: Duration,
        held: bool,
        cx: &mut Context<Self>,
    ) {
        self.notices += 1;
        let id = self.notices;
        self.notice = Some(Notice {
            text: text.into(),
            error,
            id,
            held,
            about: None,
        });
        let timer = cx.background_executor().timer(shown);
        cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |this, cx| {
                if this.notice.as_ref().is_some_and(|notice| notice.id == id) {
                    this.notice = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The notice shown, for tests.
    #[cfg(test)]
    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_ref())
    }

    /// Empty the field after its text was sent: the pictures and links go with it.
    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, |buffer| {
            buffer.take();
        });
        self.recalled = None;
        self.pictures.clear();
        self.links.clear();
        self.preview = None;
        self.menu = None;
        self.dismissed = None;
        cx.notify();
    }

    /// Put a prompt from the history in the field; `None` goes back to an empty one.
    pub(crate) fn recall(&mut self, text: Option<String>, cx: &mut Context<Self>) {
        self.edit(cx, |buffer| buffer.set_text(text.as_deref().unwrap_or("")));
        self.recalled = text;
    }

    /// Puts a draft back in the field, its pictures with it.
    pub(crate) fn restore(&mut self, draft: Draft, cx: &mut Context<Self>) {
        for attached in draft.pictures {
            self.pictures_made += 1;
            self.pictures.push(Picture {
                id: self.pictures_made,
                label: attached.label,
                state: PictureState::Ready(attached.prepared),
            });
        }
        self.recalled = None;
        self.dismissed = Some(draft.text.clone());
        self.edit(cx, |buffer| buffer.set_text(&draft.text));
    }

    /// Puts an earlier prompt back in the field: its text, and its pictures
    /// under the labels the text names them by, made ready again.
    pub(crate) fn reuse(
        &mut self,
        text: &str,
        pictures: Vec<(String, Arc<[u8]>)>,
        cx: &mut Context<Self>,
    ) {
        for (label, bytes) in pictures {
            self.pictures_made += 1;
            let id = self.pictures_made;
            self.pictures.push(Picture {
                id,
                label,
                state: PictureState::Reading,
            });
            let prepared = cx.background_spawn(async move { pictures::prepare(&bytes) });
            cx.spawn(async move |this, cx| {
                let prepared = prepared.await;
                this.update(cx, |this, cx| this.picture_ready(id, prepared, cx))
                    .ok();
            })
            .detach();
        }
        self.recalled = None;
        self.dismissed = Some(text.to_owned());
        self.edit(cx, |buffer| buffer.set_text(text));
    }

    /// The files the text links, as last found.
    #[cfg(test)]
    pub(crate) fn links(&self) -> &[FileLink] {
        &self.links
    }

    /// The draft to send, with the files its text links, or why it cannot go yet.
    pub(crate) fn draft(&self) -> Result<(Draft, Vec<FileLink>), String> {
        let text = self.buffer.text().to_owned();
        let mut pictures = Vec::new();
        for picture in self.referenced() {
            match &picture.state {
                PictureState::Ready(prepared) => pictures.push(Attached {
                    label: picture.label.clone(),
                    prepared: prepared.clone(),
                }),
                PictureState::Reading => {
                    return Err(format!("{} is still being read", picture.label));
                }
            }
        }
        let links = match &self.project {
            Some(project) if text.contains('$') => file_links(&text, project),
            _ => Vec::new(),
        };
        Ok((Draft { text, pictures }, links))
    }

    /// Whether the field shows a recalled prompt the user has not changed.
    fn browsing(&self) -> bool {
        self.recalled.as_deref() == Some(self.buffer.text())
    }

    /// Run an edit, then do what every edit needs: redraw, keep the cursor in
    /// view, follow the text with the lists and the links, tell the input
    /// method and the view.
    fn edit(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Buffer)) {
        let was_empty = self.buffer.is_empty();
        let before = self.buffer.text().len();
        let had_dollar = self.buffer.text().contains('$');
        change(&mut self.buffer);
        self.goal_x = None;
        self.downstream = false;
        self.reveal = true;
        self.refresh_menu(cx);
        let text = self.buffer.text();
        // What was said of a picture is said no longer than the picture is
        // in the prompt.
        if self.notice.as_ref().is_some_and(|notice| {
            notice
                .about
                .as_deref()
                .is_some_and(|label| !text.contains(label))
        }) {
            self.notice = None;
        }
        if text.len() != before || text.contains('$') != had_dollar || !self.links.is_empty() {
            self.scan_links(cx);
        }
        self.follow_cursor();
        if self.buffer.is_empty() != was_empty {
            cx.emit(ComposerEvent::Emptied(self.buffer.is_empty()));
        }
        cx.notify();
    }

    /// Move the cursor without changing the text.
    fn travel(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Buffer)) {
        change(&mut self.buffer);
        self.goal_x = None;
        self.downstream = false;
        self.reveal = true;
        if matches!(
            self.menu.as_ref().map(|menu| &menu.kind),
            Some(MenuKind::Files { .. })
        ) || self.project.is_some() && self.buffer.text().contains('$')
        {
            self.refresh_menu(cx);
        }
        self.follow_cursor();
        cx.notify();
    }

    /// The preview follows the cursor: right after a picture's label it shows
    /// that picture, elsewhere none, unless a click opened one.
    fn follow_cursor(&mut self) {
        if self.preview.is_some_and(|(_, pinned)| pinned) {
            return;
        }
        self.preview = self
            .picture_before_cursor()
            .filter(|picture| matches!(picture.state, PictureState::Ready(_)))
            .map(|picture| (picture.id, false));
    }

    fn geometry(&self) -> Option<&Geometry> {
        self.geometry
            .as_ref()
            .filter(|geometry| geometry.text_len == self.buffer.text().len())
    }

    // ------------------------------------------------------------ lists

    /// The lines of the open list.
    #[cfg(test)]
    pub(crate) fn matches(&self) -> &[MenuItem] {
        self.menu.as_ref().map_or(&[], |menu| &menu.items)
    }

    /// Whether a list is open over the field: commands, files, earlier
    /// prompts.
    pub(crate) fn menu_open(&self) -> bool {
        self.menu
            .as_ref()
            .is_some_and(|menu| !menu.items.is_empty() || menu.loading)
    }

    /// Opens or follows the list the text and the cursor call for.
    fn refresh_menu(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer.text().to_owned();
        if self.dismissed.as_deref() == Some(text.as_str()) {
            self.menu = None;
            return;
        }
        self.dismissed = None;
        let selection = self.buffer.selection();
        if self.project.is_some()
            && selection.is_empty()
            && self.buffer.marked().is_none()
            && let Some(query) = link_query(&text[..selection.end])
        {
            let same = matches!(
                &self.menu,
                Some(Menu { kind: MenuKind::Files { query: open }, .. }) if *open == query
            );
            if !same {
                let items = match &self.menu {
                    Some(menu) if matches!(menu.kind, MenuKind::Files { .. }) => menu.items.clone(),
                    _ => Vec::new(),
                };
                let anchor = query.start;
                self.set_menu(
                    MenuKind::Files {
                        query: query.clone(),
                    },
                    items,
                    anchor,
                    true,
                );
                self.load_files(query, cx);
            }
            return;
        }
        if matches!(
            self.menu.as_ref().map(|menu| &menu.kind),
            Some(MenuKind::History)
        ) {
            let items = self.history_items();
            if items.is_empty() {
                self.menu = None;
            } else {
                self.set_menu(MenuKind::History, items, 0, false);
            }
            return;
        }
        let items = self.command_items(&text);
        if items.is_empty() {
            self.menu = None;
        } else {
            self.set_menu(MenuKind::Commands, items, 0, false);
        }
    }

    /// Shows a list, keeping its entrance and its highlight when it is the
    /// list already open, only with other lines.
    fn set_menu(&mut self, kind: MenuKind, items: Vec<MenuItem>, anchor: usize, loading: bool) {
        let same_kind = |open: &MenuKind| {
            std::mem::discriminant(open) == std::mem::discriminant(&kind)
                && match (open, &kind) {
                    (MenuKind::Files { query: a }, MenuKind::Files { query: b }) => {
                        a.start == b.start
                    }
                    _ => true,
                }
        };
        let current = items
            .iter()
            .position(|item| matches!(item.what, What::Value { current: true, .. }));
        let first = items
            .iter()
            .position(|item| item.what != What::Hint)
            .unwrap_or(0);
        match self.menu.as_mut().filter(|menu| same_kind(&menu.kind)) {
            Some(menu) => {
                let kept = menu
                    .items
                    .get(menu.highlighted)
                    .and_then(|old| items.iter().position(|item| item.name == old.name));
                let changed = menu.items != items;
                menu.kind = kind;
                menu.anchor = anchor;
                menu.loading = loading;
                if changed {
                    menu.highlighted = match (kept, menu.moved) {
                        (Some(kept), true) => kept,
                        _ => current.unwrap_or(first),
                    };
                    if !menu.moved || kept.is_none() {
                        menu.moved = false;
                    }
                    menu.items = items;
                }
            }
            None => {
                self.menus_opened += 1;
                self.menu = Some(Menu {
                    kind,
                    highlighted: current.unwrap_or(first),
                    items,
                    moved: false,
                    loading,
                    anchor,
                    id: self.menus_opened,
                });
            }
        }
    }

    /// The command list's lines for the text.
    fn command_items(&self, raw: &str) -> Vec<MenuItem> {
        if self.commands_off {
            return Vec::new();
        }
        self.catalog
            .suggestions(raw)
            .into_iter()
            .map(|suggestion| match suggestion {
                Suggestion::Command { spec, alias } => {
                    let found = &self.catalog.specs[spec];
                    MenuItem {
                        name: found.name.clone(),
                        detail: found.help.clone(),
                        what: What::Command { spec, alias },
                    }
                }
                Suggestion::Value { spec, value } => MenuItem {
                    name: value.label.clone(),
                    detail: value.detail.clone(),
                    what: What::Value {
                        spec,
                        value: value.value,
                        current: value.current,
                    },
                },
                Suggestion::Hint { spec } => {
                    let found = &self.catalog.specs[spec];
                    MenuItem {
                        name: format!("/{} {}", found.name, found.args.clone().unwrap_or_default()),
                        detail: found.help.clone(),
                        what: What::Hint,
                    }
                }
            })
            .collect()
    }

    /// The history list's lines: the newest first, those that hold what is typed.
    fn history_items(&self) -> Vec<MenuItem> {
        let query = self.buffer.text().trim().to_lowercase();
        let mut found: Vec<(u32, usize)> = self
            .history
            .iter()
            .enumerate()
            .rev()
            .map(|(at, (prompt, _))| (crate::commands::score(&query, prompt), at))
            .filter(|(score, _)| *score > 0)
            .collect();
        if !query.is_empty() {
            found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        }
        let mut seen = std::collections::HashSet::new();
        found
            .into_iter()
            .filter(|(_, at)| seen.insert(self.history[*at].0.clone()))
            .take(50)
            .map(|(_, at)| MenuItem {
                name: self.history[at].0.clone(),
                detail: self.history[at].1.clone(),
                what: What::Prompt,
            })
            .collect()
    }

    /// Lists the files a `$` reference completes to, off the main thread,
    /// listing the project first if no list is there yet.
    fn load_files(&mut self, query: LinkQuery, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let index = self
            .index
            .get_or_insert_with(|| FileIndex::new(project))
            .clone();
        let typed = query.query.clone();
        cx.spawn(async move |this, cx| {
            // The first listing runs on a thread of the index's own; this
            // looks for it now and then rather than being woken from there.
            let listing = loop {
                if let Some(listing) = index.listing() {
                    break listing;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(15))
                    .await;
            };
            let found = cx
                .background_spawn(async move { listing.matches(&typed, FILE_LIMIT) })
                .await;
            this.update(cx, |this, cx| this.files_found(query, found, cx))
                .ok();
        })
        .detach();
    }

    fn files_found(
        &mut self,
        query: LinkQuery,
        found: Vec<kou_thread::files::FileMatch>,
        cx: &mut Context<Self>,
    ) {
        let open = matches!(
            &self.menu,
            Some(Menu { kind: MenuKind::Files { query: open }, .. }) if *open == query
        );
        if !open {
            return;
        }
        if found.is_empty() {
            // A $ nothing completes, such as $HOME, is text.
            self.menu = None;
            cx.notify();
            return;
        }
        let items = found
            .into_iter()
            .map(|found| MenuItem {
                detail: if found.directory {
                    "folder".to_owned()
                } else {
                    String::new()
                },
                name: found.path,
                what: What::File {
                    directory: found.directory,
                },
            })
            .collect();
        let anchor = query.start;
        self.set_menu(MenuKind::Files { query }, items, anchor, false);
        cx.notify();
    }

    fn step_menu(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        let count = menu.items.len();
        if count == 0 {
            return;
        }
        let mut next = menu.highlighted;
        for _ in 0..count {
            next = if down {
                (next + 1) % count
            } else {
                (next + count - 1) % count
            };
            if menu.items[next].what != What::Hint {
                break;
            }
        }
        menu.highlighted = next;
        menu.moved = true;
        cx.notify();
    }

    /// The pointer is over a line: it is the highlighted one.
    pub(crate) fn hover_menu(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu
            && menu.highlighted != index
            && menu
                .items
                .get(index)
                .is_some_and(|item| item.what != What::Hint)
        {
            menu.highlighted = index;
            menu.moved = true;
            cx.notify();
        }
    }

    /// Takes line `index` of the list into the field. With `go`, a command
    /// that needs nothing more runs, and a value runs its command.
    pub(crate) fn accept(&mut self, index: usize, go: bool, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.clone() else {
            return;
        };
        let Some(item) = menu.items.get(index).cloned() else {
            return;
        };
        match (&menu.kind, &item.what) {
            (MenuKind::Files { query }, What::File { directory }) => {
                let cursor = self.buffer.cursor();
                let after = &self.buffer.text()[cursor..];
                let end = if query.quoted {
                    match after.find('"') {
                        Some(close) if !after[..close].contains('\n') => cursor + close + 1,
                        _ => cursor,
                    }
                } else {
                    cursor + after.find(char::is_whitespace).unwrap_or(after.len())
                };
                let label = link_label(&item.name);
                let insert = if *directory {
                    label.clone()
                } else {
                    format!("{label} ")
                };
                self.menu = None;
                let start = query.start;
                self.edit(cx, |buffer| buffer.replace(start..end, &insert));
                if *directory && label.ends_with('"') {
                    // Inside the quotes, where the folder's entries complete.
                    let at = start + label.len() - 1;
                    self.travel(cx, |buffer| buffer.move_to(at));
                }
            }
            (MenuKind::Commands, What::Command { spec, .. }) => {
                let spec = self.catalog.specs[*spec].clone();
                if go && spec.runs_at_once() {
                    let text = format!("/{}", spec.name);
                    self.dismissed = Some(text.clone());
                    self.edit(cx, |buffer| buffer.set_text(&text));
                    cx.emit(ComposerEvent::Submit(text));
                } else {
                    let space = spec.args.is_some() || spec.open;
                    let text = format!("/{}{}", spec.name, if space { " " } else { "" });
                    self.edit(cx, |buffer| buffer.set_text(&text));
                }
            }
            (MenuKind::Commands, What::Value { spec, value, .. }) => {
                let text = format!("/{} {value}", self.catalog.specs[*spec].name);
                self.dismissed = Some(text.clone());
                self.edit(cx, |buffer| buffer.set_text(&text));
                if go {
                    cx.emit(ComposerEvent::Submit(text));
                }
            }
            (MenuKind::History, What::Prompt) => {
                self.menu = None;
                self.dismissed = Some(item.name.clone());
                self.edit(cx, |buffer| buffer.set_text(&item.name));
            }
            _ => {}
        }
        cx.notify();
    }

    fn dismiss_menu(&mut self, cx: &mut Context<Self>) {
        self.dismissed = Some(self.buffer.text().to_owned());
        self.menu = None;
        cx.notify();
    }

    // ------------------------------------------------------------ pictures

    /// The pictures the text names, each once, in the order they came, at
    /// most as many as a prompt takes.
    pub(crate) fn referenced(&self) -> Vec<&Picture> {
        let text = self.buffer.text();
        self.pictures
            .iter()
            .filter(|picture| text.contains(&picture.label))
            .take(pictures::MAX_PICTURES)
            .collect()
    }

    /// The picture whose label ends right at the cursor.
    fn picture_before_cursor(&self) -> Option<&Picture> {
        let selection = self.buffer.selection();
        if !selection.is_empty() {
            return None;
        }
        let before = &self.buffer.text()[..selection.start];
        self.pictures
            .iter()
            .rev()
            .find(|picture| before.ends_with(&picture.label))
    }

    /// The label the next picture gets: one past any the text or the field has.
    fn next_label(&self) -> String {
        let most = pictures::label_numbers(self.buffer.text())
            .into_iter()
            .chain(
                self.pictures
                    .iter()
                    .flat_map(|picture| pictures::label_numbers(&picture.label)),
            )
            .max()
            .unwrap_or(0);
        pictures::label(most + 1)
    }

    /// Whether another picture may come, saying why not when it may not.
    fn room_for_picture(&mut self, cx: &mut Context<Self>) -> bool {
        if self.takes_pictures == Some(false) {
            self.tell(
                "This agent takes no pictures: describe it in words, or link the file with $.",
                true,
                cx,
            );
            return false;
        }
        if self.referenced().len() >= pictures::MAX_PICTURES {
            self.tell(
                format!(
                    "A prompt takes at most {} pictures.",
                    pictures::MAX_PICTURES
                ),
                true,
                cx,
            );
            return false;
        }
        true
    }

    /// Attaches a picture's bytes: its label goes in at the cursor at once,
    /// and the picture is made ready off the main thread.
    pub(crate) fn attach(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if !self.room_for_picture(cx) {
            return;
        }
        let label = self.next_label();
        self.pictures_made += 1;
        let id = self.pictures_made;
        self.pictures.push(Picture {
            id,
            label: label.clone(),
            state: PictureState::Reading,
        });
        let cursor = self.buffer.selection();
        let text = self.buffer.text();
        let space_before = cursor.start > 0 && !text[..cursor.start].ends_with(char::is_whitespace);
        let space_after = !text[cursor.end..].starts_with(char::is_whitespace);
        let insert = format!(
            "{}{label}{}",
            if space_before { " " } else { "" },
            if space_after { " " } else { "" }
        );
        self.edit(cx, |buffer| buffer.insert(&insert));
        let prepared = cx.background_spawn(async move { pictures::prepare(&bytes) });
        cx.spawn(async move |this, cx| {
            let prepared = prepared.await;
            this.update(cx, |this, cx| this.picture_ready(id, prepared, cx))
                .ok();
        })
        .detach();
    }

    fn picture_ready(
        &mut self,
        id: u64,
        prepared: Result<Prepared, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.pictures.iter().position(|picture| picture.id == id) else {
            return;
        };
        match prepared {
            Ok(prepared) => {
                // (Nothing is said of a picture taken out before it was read.)
                let label = self.pictures[at].label.clone();
                if let Some(note) = &prepared.note
                    && self.buffer.text().contains(&label)
                {
                    self.tell(format!("{label}: {note}."), false, cx);
                    if let Some(notice) = &mut self.notice {
                        notice.about = Some(label);
                    }
                }
                self.pictures[at].state = PictureState::Ready(prepared);
                self.follow_cursor();
            }
            Err(reason) => {
                let picture = self.pictures.remove(at);
                self.strip_label(&picture.label, cx);
                self.tell(format!("The picture was not attached: {reason}."), true, cx);
            }
        }
        cx.notify();
    }

    /// Attaches files: pictures as pictures, anything else as a `$` link.
    pub(crate) fn attach_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut links = Vec::new();
        for path in paths {
            if pictures::is_picture_path(&path) {
                match std::fs::metadata(&path) {
                    Ok(metadata) if metadata.len() as usize > pictures::MAX_SOURCE => {
                        self.tell(
                            format!(
                                "{} is {}; pictures of up to {} are read.",
                                file_name(&path),
                                pictures::size(metadata.len() as usize),
                                pictures::size(pictures::MAX_SOURCE)
                            ),
                            true,
                            cx,
                        );
                    }
                    Ok(_) => match std::fs::read(&path) {
                        Ok(bytes) => self.attach(bytes, cx),
                        Err(error) => self.tell(
                            format!("{} could not be read: {error}.", file_name(&path)),
                            true,
                            cx,
                        ),
                    },
                    Err(error) => self.tell(
                        format!("{} could not be read: {error}.", file_name(&path)),
                        true,
                        cx,
                    ),
                }
            } else {
                links.push(self.name_in_project(&path));
            }
        }
        if !links.is_empty() {
            let text = self.buffer.text();
            let cursor = self.buffer.selection();
            let before = cursor.start > 0 && !text[..cursor.start].ends_with(char::is_whitespace);
            let insert = format!(
                "{}{} ",
                if before { " " } else { "" },
                links
                    .iter()
                    .map(|name| link_label(name))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            self.dismissed = None;
            self.edit(cx, |buffer| buffer.insert(&insert));
        }
    }

    /// How a `$` link names a path: from the project when it is inside it.
    fn name_in_project(&self, path: &Path) -> String {
        if let Some(project) = &self.project
            && let Ok(inside) = path.strip_prefix(project)
            && !inside.as_os_str().is_empty()
        {
            let mut name = inside.to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                name.push('/');
            }
            return name;
        }
        path.to_string_lossy().into_owned()
    }

    /// Takes a picture out: every mention of its label leaves the text, as one
    /// step that undo takes back. The picture stays known for that undo.
    pub(crate) fn remove_picture(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(label) = self
            .pictures
            .iter()
            .find(|picture| picture.id == id)
            .map(|picture| picture.label.clone())
        else {
            return;
        };
        if self.preview.is_some_and(|(shown, _)| shown == id) {
            self.preview = None;
        }
        self.strip_label(&label, cx);
    }

    fn strip_label(&mut self, label: &str, cx: &mut Context<Self>) {
        let text = self.buffer.text();
        if !text.contains(label) {
            return;
        }
        // The label and one space beside it, so no double space is left.
        let mut stripped = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(label) {
            let mut before = &rest[..at];
            let mut after = &rest[at + label.len()..];
            if after.starts_with(' ') {
                after = &after[1..];
            } else if before.ends_with(' ') {
                before = &before[..before.len() - 1];
            }
            stripped.push_str(before);
            rest = after;
        }
        stripped.push_str(rest);
        let cursor = self.buffer.cursor().min(stripped.len());
        self.edit(cx, |buffer| {
            buffer.set_text(&stripped);
            buffer.move_to(cursor);
        });
    }

    /// Shows a picture large until Escape or a click closes it.
    pub(crate) fn open_preview(&mut self, id: u64, cx: &mut Context<Self>) {
        self.preview = Some((id, true));
        cx.notify();
    }

    pub(crate) fn close_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview.take().is_some() {
            cx.notify();
        }
    }

    // ------------------------------------------------------------ links

    /// Reads the text's links off the main thread; the tray follows.
    fn scan_links(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            return;
        };
        self.links_seen += 1;
        let seen = self.links_seen;
        let text = self.buffer.text().to_owned();
        if !text.contains('$') {
            if !self.links.is_empty() {
                self.links.clear();
                cx.notify();
            }
            return;
        }
        let found = cx.background_spawn(async move { file_links(&text, &project) });
        cx.spawn(async move |this, cx| {
            let found = found.await;
            this.update(cx, |this, cx| {
                if this.links_seen == seen && this.links != found {
                    this.links = found;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Takes a link out of the text: the reference and a space beside it.
    pub(crate) fn remove_link(&mut self, label: &str, cx: &mut Context<Self>) {
        self.strip_label(label, cx);
    }

    /// Puts the cursor right after the first mention of `label`.
    pub(crate) fn cursor_after(
        &mut self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.buffer.text().find(label) else {
            return;
        };
        window.focus(&self.focus, cx);
        self.travel(cx, |buffer| buffer.move_to(at + label.len()));
    }

    // ------------------------------------------------------------ keys

    fn motion(&mut self, motion: Motion, extend: bool, cx: &mut Context<Self>) {
        self.travel(cx, |buffer| buffer.motion(motion, extend));
    }

    /// Start or end of the row as wrapped, falling back to the line as typed.
    fn row_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        let cursor = self.buffer.cursor();
        // An empty field has no rows; the lines as typed (none) answer for it.
        let target = self.geometry().and_then(|geometry| {
            let row = geometry
                .rows
                .get(geometry.row_of(cursor, self.downstream))?;
            let wrapped_start = row.range.start > geometry.lines[row.line].0;
            Some(if end {
                (row.range.end, false)
            } else {
                (row.range.start, wrapped_start)
            })
        });
        match target {
            Some((offset, downstream)) => {
                self.travel(cx, |buffer| {
                    if extend {
                        buffer.select_to(offset);
                    } else {
                        buffer.move_to(offset);
                    }
                });
                self.downstream = downstream;
            }
            None => self.motion(
                if end {
                    Motion::LineEnd
                } else {
                    Motion::LineStart
                },
                extend,
                cx,
            ),
        }
    }

    /// Up or down one row as wrapped, keeping the column.
    fn vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        let cursor = self.buffer.cursor();
        let (target, downstream, goal) = match self.geometry() {
            Some(geometry) => {
                let at = geometry.position(cursor, self.downstream);
                let goal = self.goal_x.unwrap_or(at.x);
                let row = geometry.row_of(cursor, self.downstream);
                if down && row + 1 >= geometry.rows.len() {
                    (self.buffer.text().len(), false, goal)
                } else if !down && row == 0 {
                    (0, false, goal)
                } else {
                    let row = if down { row + 1 } else { row - 1 };
                    let y = geometry.rows[row].top + geometry.line_height / 2.;
                    let (offset, downstream) = geometry.offset_at(point(goal, y));
                    (offset, downstream, goal)
                }
            }
            // Not painted yet: the lines as typed have to do.
            None => {
                let text = self.buffer.text();
                let start = crate::buffer::line_start(text, cursor);
                let column = text[start..cursor].chars().count();
                let target = if down {
                    let end = crate::buffer::line_end(text, cursor);
                    if end == text.len() {
                        text.len()
                    } else {
                        let next_end = crate::buffer::line_end(text, end + 1);
                        text[end + 1..next_end]
                            .char_indices()
                            .nth(column)
                            .map_or(next_end, |(index, _)| end + 1 + index)
                    }
                } else if start == 0 {
                    0
                } else {
                    let previous = crate::buffer::line_start(text, start - 1);
                    text[previous..start - 1]
                        .char_indices()
                        .nth(column)
                        .map_or(start - 1, |(index, _)| previous + index)
                };
                (target, false, px(0.))
            }
        };
        let keep_goal = self.geometry().is_some();
        self.travel(cx, |buffer| {
            if extend {
                buffer.select_to(target);
            } else {
                buffer.move_to(target);
            }
        });
        self.downstream = downstream;
        self.goal_x = keep_goal.then_some(goal);
    }

    fn list_takes_keys(&self) -> bool {
        self.menu.as_ref().is_some_and(Menu::choosable)
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.list_takes_keys() {
            self.step_menu(false, cx);
        } else if self.buffer.is_empty() || self.browsing() {
            cx.emit(ComposerEvent::Recall { older: true });
        } else {
            self.vertical(false, false, cx);
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.list_takes_keys() {
            self.step_menu(true, cx);
        } else if self.browsing() {
            cx.emit(ComposerEvent::Recall { older: false });
        } else {
            self.vertical(true, false, cx);
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &self.menu
            && menu.choosable()
        {
            // In a list of files Enter is the field's until a line was
            // chosen, so a prompt that ends in a path is sent as it is.
            if matches!(menu.kind, MenuKind::Files { .. }) && !menu.moved {
                self.dismissed = Some(self.buffer.text().to_owned());
                self.menu = None;
            } else {
                self.accept(menu.highlighted, true, cx);
                return;
            }
        }
        // Enter while composing belongs to the input method.
        if self.buffer.marked().is_some() {
            cx.propagate();
            return;
        }
        if !self.buffer.text().trim().is_empty() {
            cx.emit(ComposerEvent::Submit(self.buffer.text().to_owned()));
        } else {
            cx.emit(ComposerEvent::Blank);
        }
    }

    fn force(&mut self, _: &Force, _: &mut Window, cx: &mut Context<Self>) {
        // An empty field leaves the key to what is around it: a permission's answer.
        if self.buffer.text().trim().is_empty() || self.buffer.marked().is_some() {
            cx.propagate();
            return;
        }
        self.menu = None;
        cx.emit(ComposerEvent::Force(self.buffer.text().to_owned()));
    }

    fn complete(&mut self, _: &Complete, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &self.menu
            && menu.choosable()
        {
            self.accept(menu.highlighted, false, cx);
        } else {
            // Tab is not text here: it walks the controls.
            cx.propagate();
        }
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.dismiss_menu(cx);
        } else if self.preview.is_some() {
            self.close_preview(cx);
        } else if self.notice.as_ref().is_some_and(|notice| !notice.held) {
            self.notice = None;
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    fn search_history(&mut self, _: &SearchHistory, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.menu.as_ref().map(|menu| &menu.kind),
            Some(MenuKind::History)
        ) {
            self.menu = None;
            cx.notify();
            return;
        }
        let items = self.history_items();
        if items.is_empty() {
            let text = if self.history.is_empty() {
                "No earlier prompts yet: what you send here, and in kou's other cockpits, comes back here."
            } else {
                "No earlier prompt holds what is typed."
            };
            self.tell(text, false, cx);
            return;
        }
        self.menu = None;
        self.dismissed = None;
        self.set_menu(MenuKind::History, items, 0, false);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let selection = self.buffer.selection();
        if selection.is_empty() && self.buffer.marked().is_none() {
            // Right after a picture's label, Backspace takes the whole label,
            // and the picture with it.
            if let Some(label) = self
                .picture_before_cursor()
                .map(|picture| picture.label.clone())
            {
                let end = selection.start;
                self.edit(cx, |buffer| buffer.replace(end - label.len()..end, ""));
                return;
            }
            // At the very start, it takes the last picture.
            if selection.start == 0
                && let Some(last) = self.referenced().last().map(|picture| picture.id)
            {
                self.remove_picture(last, cx);
                self.travel(cx, |buffer| buffer.move_to(0));
                return;
            }
        }
        self.edit(cx, |buffer| buffer.delete(Motion::Left));
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        // An empty field has nowhere to go: the key scrolls the transcript instead.
        if self.buffer.is_empty() {
            cx.propagate();
        } else {
            self.row_edge(false, false, cx);
        }
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.is_empty() {
            cx.propagate();
        } else {
            self.row_edge(true, false, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.selection().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.selected_text().to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.selection().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.selected_text().to_owned(),
            ));
            self.edit(cx, |buffer| buffer.insert(""));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let (mut images, mut paths, mut text) = (Vec::new(), Vec::new(), String::new());
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => images.push(image.bytes.clone()),
                ClipboardEntry::ExternalPaths(found) => paths.extend(found.paths().iter().cloned()),
                ClipboardEntry::String(string) => text.push_str(string.text()),
            }
        }
        let typed = text.trim();
        // An office program puts a picture of what was copied beside its
        // text: the text is what was meant, unless it only names the files.
        let names_only = typed.is_empty()
            || typed.lines().all(|line| {
                let line = line.trim();
                paths
                    .iter()
                    .any(|path| path.to_string_lossy() == line || file_name(path) == line)
                    || Path::new(line).is_absolute() && pictures::is_picture_path(Path::new(line))
            });
        if !images.is_empty() && names_only {
            for bytes in images {
                self.attach(bytes, cx);
            }
            return;
        }
        if images.is_empty() && !paths.is_empty() && names_only {
            self.attach_paths(paths, cx);
            return;
        }
        // A copied file's address or path, of a picture, attaches it.
        if images.is_empty()
            && let Some(path) = picture_path(typed)
        {
            self.attach_paths(vec![path], cx);
            return;
        }
        if !text.is_empty() {
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            self.edit(cx, |buffer| buffer.insert(&text));
        }
    }

    /// Files dropped on the field: pictures attach, other files are linked.
    pub(crate) fn drop_paths(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.attach_paths(paths.paths().to_vec(), cx);
    }

    fn offset_at(&self, position: Point<Pixels>) -> Option<(usize, bool)> {
        let geometry = self.geometry()?;
        let local = position - geometry.bounds.origin + point(Pixels::ZERO, self.scroll);
        Some(geometry.offset_at(local))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((offset, downstream)) = self.offset_at(event.position) else {
            return;
        };
        self.selecting = true;
        self.travel(cx, |buffer| match event.click_count {
            1 if event.modifiers.shift => buffer.select_to(offset),
            1 => buffer.move_to(offset),
            2 => buffer.select(buffer.word_at(offset)),
            _ => buffer.select(buffer.line_at(offset)),
        });
        self.downstream = downstream && event.click_count == 1;
        // A click places the cursor where it is; nothing needs scrolling into view.
        self.reveal = false;
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if let Some((offset, _)) = self.offset_at(event.position) {
            self.travel(cx, |buffer| buffer.select_to(offset));
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn wheel(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(geometry) = self.geometry() else {
            return;
        };
        let max = (geometry.height() - geometry.bounds.size.height).max(Pixels::ZERO);
        if max <= Pixels::ZERO {
            return;
        }
        let delta = event.delta.pixel_delta(window.line_height()).y;
        // A wheel's line steps glide, as everything around the field does; a
        // trackpad's pixels are followed as they come.
        let smooth = gpui::SmoothScroll::of(cx);
        let lines = matches!(event.delta, gpui::ScrollDelta::Lines(_));
        if smooth.enabled && lines {
            let now = cx.background_executor().now();
            let mut glide = self
                .glide
                .unwrap_or_else(|| gpui::ScrollGlide::new(self.scroll.as_f32()));
            let target = (glide.target() - delta.as_f32()).clamp(0., max.as_f32());
            glide.glide_to(target, now, smooth.duration);
            self.glide = Some(glide);
            cx.notify();
        } else {
            self.glide = None;
            let scroll = (self.scroll - delta).clamp(Pixels::ZERO, max);
            if scroll != self.scroll {
                self.scroll = scroll;
                cx.notify();
            }
        }
        // The field is a scroll area of its own: the transcript behind it stays put.
        cx.stop_propagation();
    }

    /// The ranges of the text drawn apart: picture labels and links, with
    /// their colour, and whether bold.
    fn marks(&self, theme: &kou_ui::Theme) -> Vec<(Range<usize>, Hsla, bool)> {
        let text = self.buffer.text();
        let mut marks = Vec::new();
        for picture in &self.pictures {
            let mut from = 0;
            while let Some(at) = text[from..].find(&picture.label) {
                let start = from + at;
                marks.push((start..start + picture.label.len(), theme.accent, true));
                from = start + picture.label.len();
            }
        }
        for link in &self.links {
            if text.get(link.range.clone()) == Some(link.label.as_str()) {
                marks.push((link.range.clone(), theme.done, false));
            }
        }
        marks.sort_by_key(|(range, _, _)| range.start);
        // No two overlap: the first wins.
        let mut kept: Vec<(Range<usize>, Hsla, bool)> = Vec::new();
        for mark in marks {
            if kept.last().is_none_or(|last| last.0.end <= mark.0.start) {
                kept.push(mark);
            }
        }
        kept
    }
}

/// A file's name, for a message.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The picture file a pasted line names: a `file://` address or a path.
fn picture_path(text: &str) -> Option<PathBuf> {
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    let path = match text.strip_prefix("file://") {
        Some(rest) => PathBuf::from(percent_decode(rest)),
        None => {
            let unquoted = text.trim_matches(['\'', '"']);
            match unquoted.strip_prefix("~/") {
                Some(rest) => dirs_home()?.join(rest),
                None => PathBuf::from(unquoted.replace("\\ ", " ")),
            }
        }
    };
    (path.is_absolute() && pictures::is_picture_path(&path) && path.is_file()).then_some(path)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// `%20` and its kind, as a file address writes them.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |byte: u8| (byte as char).to_digit(16);
        if bytes[at] == b'%'
            && at + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[at + 1]), hex(bytes[at + 2]))
        {
            out.push((high * 16 + low) as u8);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl EntityInputHandler for Composer {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        // The platform asks in good faith, but a reversed range must not be a panic.
        let range = range.start..range.end.max(range.start);
        actual_range.replace(self.buffer.range_to_utf16(&range));
        Some(self.buffer.text()[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(&self.buffer.selection()),
            reversed: self.buffer.reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.buffer
            .marked()
            .map(|range| self.buffer.range_to_utf16(&range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.buffer.unmark();
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|range| self.buffer.range_from_utf16(&range));
        self.edit(cx, |buffer| match range {
            Some(range) => buffer.replace(range, text),
            None => buffer.insert(text),
        });
        window.notify_text_input_changed(&self.focus);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        selected_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|range| self.buffer.range_from_utf16(&range));
        // The selection is given in UTF-16 units of the new text.
        let selected = selected_utf16.map(|selected| {
            let to_bytes = |units: usize| {
                let mut seen = 0;
                for (index, ch) in text.char_indices() {
                    if seen >= units {
                        return index;
                    }
                    seen += ch.len_utf16();
                }
                text.len()
            };
            to_bytes(selected.start)..to_bytes(selected.end)
        });
        self.edit(cx, |buffer| buffer.compose(range, text, selected));
        window.notify_text_input_changed(&self.focus);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        let geometry = self.geometry()?;
        let start = geometry.position(range.start, true);
        let end = geometry.position(range.end, false);
        let origin = geometry.bounds.origin + start - point(Pixels::ZERO, self.scroll);
        // A range that wraps is reported by its first row; the candidate window sits under it.
        let width = if end.y == start.y {
            (end.x - start.x).max(px(1.))
        } else {
            (geometry.bounds.size.width - start.x).max(px(1.))
        };
        Some(Bounds::new(origin, size(width, geometry.line_height)))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let (offset, _) = self.offset_at(position)?;
        Some(self.buffer.offset_to_utf16(offset))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.buffer.range_from_utf16(&range_utf16);
        self.travel(cx, |buffer| buffer.select(range));
        window.notify_text_selection_changed(&self.focus);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.buffer.offset_to_utf16(self.buffer.text().len()))
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let factor = if self.inline {
            scale::factor()
        } else {
            self.scale.get()
        };
        crate::trace::field_built(&self.trace, factor);
        let field = scale::with_factor(factor, || self.field(window, cx));
        crate::dock::scaled(factor, field)
    }
}

impl Composer {
    fn field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let now = cx.background_executor().now();
        self.track_presence(now, cx);
        let above = self.above_element(cx);
        let bounds = self.bounds.clone();
        let menu = self.menu_element(window, cx);
        let preview = self.preview_element(cx);
        let theme = self.themed(cx);
        let (size, line) = self.type_size();
        let wide = px(560.);

        let field = div()
            .id("composer")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(|this, _: &Delete, _, cx| {
                this.edit(cx, |buffer| buffer.delete(Motion::Right))
            }))
            .on_action(cx.listener(|this, _: &DeleteWordLeft, _, cx| {
                this.edit(cx, |buffer| buffer.delete(Motion::WordLeft))
            }))
            .on_action(cx.listener(|this, _: &DeleteWordRight, _, cx| {
                this.edit(cx, |buffer| buffer.delete(Motion::WordRight))
            }))
            .on_action(cx.listener(|this, _: &DeleteToLineStart, _, cx| {
                this.edit(cx, |buffer| buffer.delete(Motion::LineStart))
            }))
            .on_action(cx.listener(|this, _: &DeleteToLineEnd, _, cx| {
                this.edit(cx, |buffer| buffer.delete(Motion::LineEnd))
            }))
            .on_action(cx.listener(|this, _: &Left, _, cx| this.motion(Motion::Left, false, cx)))
            .on_action(cx.listener(|this, _: &Right, _, cx| this.motion(Motion::Right, false, cx)))
            .on_action(
                cx.listener(|this, _: &WordLeft, _, cx| this.motion(Motion::WordLeft, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &WordRight, _, cx| this.motion(Motion::WordRight, false, cx)),
            )
            .on_action(cx.listener(|this, _: &LineStart, _, cx| this.row_edge(false, false, cx)))
            .on_action(cx.listener(|this, _: &LineEnd, _, cx| this.row_edge(true, false, cx)))
            .on_action(
                cx.listener(|this, _: &DocumentStart, _, cx| this.motion(Motion::Start, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DocumentEnd, _, cx| this.motion(Motion::End, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectLeft, _, cx| this.motion(Motion::Left, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectRight, _, cx| this.motion(Motion::Right, true, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectWordLeft, _, cx| {
                this.motion(Motion::WordLeft, true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWordRight, _, cx| {
                this.motion(Motion::WordRight, true, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectLineStart, _, cx| this.row_edge(false, true, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectLineEnd, _, cx| this.row_edge(true, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDocumentStart, _, cx| {
                this.motion(Motion::Start, true, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectDocumentEnd, _, cx| {
                    this.motion(Motion::End, true, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.vertical(false, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.vertical(true, true, cx)))
            .on_action(
                cx.listener(|this, _: &SelectAll, _, cx| this.travel(cx, Buffer::select_all)),
            )
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.edit(cx, |buffer| {
                    buffer.undo();
                })
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.edit(cx, |buffer| {
                    buffer.redo();
                })
            }))
            .on_action(
                cx.listener(|this, _: &Newline, _, cx| this.edit(cx, |buffer| buffer.insert("\n"))),
            )
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::force))
            .on_action(cx.listener(Self::complete))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::search_history))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll(cx.listener(Self::wheel))
            .cursor(CursorStyle::IBeam)
            .w_full()
            // `.composer textarea`: the prose's size, a little air on the right.
            .pr(px(4.))
            .flex()
            .items_start()
            .text_size(px(size))
            .line_height(px(line))
            .text_color(theme.fg)
            .child(div().flex_1().min_w_0().child(ComposerText {
                composer: cx.entity(),
            }));

        div()
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .child(
                gpui::canvas(
                    move |drawn, window, _| {
                        // The placeholder and the lists depend on the width.
                        let narrow = |width: Pixels| width < wide;
                        let was = bounds.replace(drawn);
                        if narrow(was.size.width) != narrow(drawn.size.width) {
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .children(menu)
            .children(preview)
            .child(above)
            .child(field)
    }
}

/// The field's line pitch: the transcript's.
const LINE_HEIGHT: f32 = LINE;
/// The caret's width.
const CARET: f32 = 1.5;

/// The element that shapes, paints and hit-tests the field's text.
struct ComposerText {
    composer: Entity<Composer>,
}

/// What prepaint hands to paint.
struct Painted {
    lines: Vec<WrappedLine>,
    /// The runs the lines were shaped in.
    runs: Vec<TextRun>,
    geometry: Geometry,
    scroll: Pixels,
    selection: Vec<Bounds<Pixels>>,
    cursor: Option<Bounds<Pixels>>,
}

impl ComposerText {
    /// The text to shape and its runs: the buffer, or the placeholder when it
    /// is empty. Picture labels and links are drawn apart, and what the input
    /// method composes is underlined.
    fn shaped_text(&self, window: &Window, cx: &App) -> (SharedString, Vec<TextRun>) {
        let composer = self.composer.read(cx);
        let theme = composer.themed(cx);
        let style = window.text_style();
        let run = |len: usize| TextRun {
            len,
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        if composer.buffer.is_empty() {
            let text = composer.shown_placeholder();
            let mut run = run(text.len());
            run.color = theme.fg_muted;
            return (text, vec![run]);
        }
        let text = SharedString::from(composer.buffer.text().to_owned());
        let marks = composer.marks(theme);
        let marked = composer.buffer.marked();
        let mut cuts = vec![0, text.len()];
        for (range, _, _) in &marks {
            cuts.extend([range.start, range.end]);
        }
        if let Some(marked) = &marked {
            cuts.extend([marked.start, marked.end]);
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut runs = Vec::with_capacity(cuts.len());
        for edge in cuts.windows(2) {
            let (from, to) = (edge[0], edge[1]);
            if from >= to || to > text.len() {
                continue;
            }
            let mut piece = run(to - from);
            if let Some((_, color, bold)) = marks
                .iter()
                .find(|(range, _, _)| range.start <= from && to <= range.end)
            {
                piece.color = *color;
                if *bold {
                    piece.font.weight = FontWeight::SEMIBOLD;
                }
            }
            if marked
                .as_ref()
                .is_some_and(|marked| marked.start <= from && to <= marked.end)
            {
                piece.underline = Some(UnderlineStyle {
                    color: Some(style.color),
                    thickness: px(1.),
                    wavy: false,
                });
            }
            runs.push(piece);
        }
        (text, runs)
    }
}

impl IntoElement for ComposerText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ComposerText {
    /// The width the text was measured in, before its box was rounded.
    type RequestLayoutState = Rc<Cell<Option<Pixels>>>;
    type PrepaintState = Option<Painted>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (text, runs) = self.shaped_text(window, cx);
        // Shaped at the unscaled size and drawn scaled: see `kou_ui::scale::ScaledText`.
        let factor = scale::factor();
        let font_size =
            scale::steady(window.text_style().font_size.to_pixels(window.rem_size()) / factor);
        let line_height = window.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let measured_in = Rc::new(Cell::new(None));
        let seen = measured_in.clone();
        // The placeholder keeps to one line, as a field's does.
        let empty = self.composer.read(cx).buffer.is_empty();
        let layout = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
            });
            seen.set(width);
            if empty {
                return size(width.unwrap_or(Pixels::ZERO), line_height);
            }
            let rows = window
                .text_system()
                .shape_text(
                    text.clone(),
                    font_size,
                    &runs,
                    width.map(|width| scale::steady(width / factor)),
                    None,
                )
                .map(|lines| {
                    lines
                        .iter()
                        .map(|line| line.wrap_boundaries().len() + 1)
                        .sum::<usize>()
                })
                .unwrap_or(1);
            size(
                width.unwrap_or(Pixels::ZERO),
                line_height * rows.clamp(1, MAX_LINES) as f32,
            )
        });
        (layout, measured_in)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        measured_in: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Painted> {
        let (text, runs) = self.shaped_text(window, cx);
        let factor = scale::factor();
        let font_size =
            scale::steady(window.text_style().font_size.to_pixels(window.rem_size()) / factor);
        let line_height = window.line_height();
        // The box is rounded to whole pixels after the text was measured, at
        // a zoom to a different fraction every frame: wrapped in the width it
        // was measured in, the text keeps its lines while the zoom moves.
        let width = measured_in
            .get()
            .filter(|width| (*width - bounds.size.width).abs() <= px(1.))
            .unwrap_or(bounds.size.width);
        let empty = self.composer.read(cx).buffer.is_empty();
        // The placeholder starts where the first letter typed will, under the
        // caret, and keeps to one line: the short one when the long one does
        // not fit.
        let room = scale::steady(width / factor);
        let shape = |text: SharedString, runs: &[TextRun], wrap: Option<Pixels>| {
            window
                .text_system()
                .shape_text(text, font_size, runs, wrap, None)
                .ok()
                .map(|lines| lines.into_iter().collect::<Vec<WrappedLine>>())
        };
        let mut runs = runs;
        let lines: Vec<WrappedLine> = if empty {
            let mut lines = shape(text, &runs, None)?;
            let wide = lines.iter().any(|line| line.width() > room);
            let short = self.composer.read(cx).short_placeholder.clone();
            if wide && let Some(run) = runs.first().cloned() {
                runs = vec![TextRun {
                    len: short.len(),
                    ..run
                }];
                lines = shape(short, &runs, None)?;
            }
            lines
        } else {
            shape(text, &runs, Some(room))?
        };

        let composer = self.composer.read(cx);
        let text_len = composer.buffer.text().len();
        // The placeholder is not text: an empty field has one empty row.
        let geometry = if empty {
            Geometry::new(0, bounds, line_height, &[], factor)
        } else {
            Geometry::new(text_len, bounds, line_height, &lines, factor)
        };
        let cursor_offset = composer.buffer.cursor();
        let selected = composer.buffer.selection();
        let downstream = composer.downstream;
        let at = geometry.position(cursor_offset, downstream);

        let max_scroll = (geometry.height() - bounds.size.height).max(Pixels::ZERO);
        let mut scroll = composer.scroll.clamp(Pixels::ZERO, max_scroll);
        // On its way where a wheel step sent it: a frame more until it is there.
        let now = cx.background_executor().now();
        let gliding = composer
            .glide
            .filter(|glide| glide.is_moving(now) && !composer.reveal);
        if let Some(glide) = gliding {
            scroll = px(glide.position(now)).clamp(Pixels::ZERO, max_scroll);
            window.request_animation_frame();
        } else if let Some(glide) = composer.glide.filter(|_| !composer.reveal) {
            scroll = px(glide.target()).clamp(Pixels::ZERO, max_scroll);
        }
        if composer.reveal {
            scroll = scroll
                .min(at.y)
                .max(at.y + line_height - bounds.size.height)
                .clamp(Pixels::ZERO, max_scroll);
        }
        let shift = bounds.origin - point(Pixels::ZERO, scroll);

        let mut selection = Vec::new();
        if !selected.is_empty() {
            for (index, row) in geometry.rows.iter().enumerate() {
                let from = selected.start.max(row.range.start);
                let to = selected.end.min(row.range.end);
                if from > to || selected.end < row.range.start || selected.start > row.range.end {
                    continue;
                }
                let left = geometry.x_in_row(row, from);
                let mut right = geometry.x_in_row(row, to);
                // A selected line break shows as a sliver past the last character.
                // A row that merely wraps has no line break to show.
                let breaks = geometry
                    .rows
                    .get(index + 1)
                    .is_some_and(|next| next.line != row.line);
                if breaks && selected.end > row.range.end {
                    right += px(5.);
                }
                if right > left {
                    selection.push(Bounds::new(
                        shift + point(left, row.top),
                        size(right - left, line_height),
                    ));
                }
            }
        }
        // The caret is as tall as the font, from its ascent to its descent,
        // and stands where the line's text does, centred in the line.
        let cursor = selected.is_empty().then(|| {
            let (ascent, descent) = lines
                .first()
                .map_or((font_size * 0.8, font_size * 0.2), |line| {
                    (line.ascent(), line.descent())
                });
            let height = ((ascent + descent) * factor).min(line_height);
            Bounds::new(
                shift + point(at.x, at.y + (line_height - height) / 2.),
                size(px(CARET * factor), height),
            )
        });

        Some(Painted {
            lines,
            runs,
            geometry,
            scroll,
            selection,
            cursor,
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        painted: &mut Option<Painted>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.composer.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.composer.clone()),
            cx,
        );
        let Some(painted) = painted.take() else {
            return;
        };
        let theme = self.composer.read(cx).themed(cx);
        let (selection_color, cursor_color) = (theme.fg.opacity(0.22), theme.fg);
        let line_height = painted.geometry.line_height;

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in &painted.selection {
                window.paint_quad(fill(*quad, selection_color));
            }
            let mut top = bounds.origin.y - painted.scroll;
            let factor = painted.geometry.factor;
            let mut line_start = 0;
            for line in &painted.lines {
                let height = line.size(line_height).height;
                if top + height >= bounds.top() && top <= bounds.bottom() {
                    let origin = point(bounds.origin.x, top);
                    if factor == 1. {
                        line.paint(
                            origin,
                            line_height,
                            gpui::TextAlign::Left,
                            Some(bounds),
                            window,
                            cx,
                        )
                        .ok();
                    } else {
                        scale::paint_line(
                            line,
                            &painted.runs,
                            line_start,
                            origin,
                            line_height,
                            factor,
                            window,
                            cx,
                        );
                    }
                }
                top += height;
                line_start += line.len() + 1;
            }
            let caret = painted.cursor.filter(|_| focus.is_focused(window));
            let lit = self.composer.update(cx, |composer, cx| {
                if caret.is_none() {
                    composer.blink.hold();
                    return 0.;
                }
                let now = cx.background_executor().now();
                let buffer = &composer.buffer;
                let selected = buffer.selection();
                let place = [
                    buffer.text().len(),
                    buffer.cursor(),
                    selected.start,
                    selected.end,
                    usize::from(composer.downstream),
                ]
                .iter()
                .fold(0u64, |hash, part| {
                    (hash ^ *part as u64).wrapping_mul(0x0100_0000_01b3)
                });
                composer.blink.at(place, now);
                let blinks = window.is_window_active() && !cx.reduce_motion();
                composer.blink.lit(blinks, now, window, cx)
            });
            if let Some(cursor) = caret
                && lit > 0.
            {
                window.paint_quad(fill(cursor, cursor_color.opacity(lit)));
            }
            #[cfg(test)]
            if let Some(line) = painted.lines.first() {
                let drawn = Drawn {
                    caret,
                    lit,
                    origin: point(bounds.origin.x, bounds.origin.y - painted.scroll),
                    line_height,
                    ascent: line.ascent(),
                    descent: line.descent(),
                    first_glyph: line
                        .unwrapped_layout
                        .runs
                        .first()
                        .and_then(|run| run.glyphs.first())
                        .map_or(Pixels::ZERO, |glyph| glyph.position.x),
                };
                self.composer
                    .update(cx, |composer, _| composer.drawn = Some(drawn));
            }
        });

        let (geometry, scroll) = (painted.geometry, painted.scroll);
        self.composer.update(cx, |composer, cx| {
            composer.geometry = Some(geometry);
            composer.scroll = scroll;
            // Once there, or once the cursor moved the text, the glide is over.
            let now = cx.background_executor().now();
            if composer.reveal || composer.glide.is_some_and(|glide| !glide.is_moving(now)) {
                composer.glide = None;
            }
            composer.reveal = false;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn composer(cx: &mut TestAppContext) -> (Entity<Composer>, &mut VisualTestContext) {
        cx.update(|cx| {
            cx.set_global(kou_ui::Theme::new(true));
            cx.bind_keys(bindings());
        });
        let (composer, cx) = cx.add_window_view(|window, cx| {
            let composer = Composer::new("Message".into(), cx);
            window.focus(&composer.focus, cx);
            composer
        });
        cx.run_until_parked();
        (composer, cx)
    }

    fn text(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> String {
        composer.read_with(cx, |composer, _| composer.text().to_owned())
    }

    fn events(
        composer: &Entity<Composer>,
        cx: &mut VisualTestContext,
    ) -> std::rc::Rc<std::cell::RefCell<Vec<ComposerEvent>>> {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = seen.clone();
        cx.update(|_, cx| {
            cx.subscribe(composer, move |_, event: &ComposerEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
            .detach();
        });
        seen
    }

    #[gpui::test]
    fn typing_goes_through_the_input_handler_in_any_script(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        let seen = events(&composer, cx);
        cx.simulate_input("сделай");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("git");
        assert_eq!(text(&composer, cx), "сделай\ngit");
        assert_eq!(seen.borrow().as_slice(), [ComposerEvent::Emptied(false)]);

        cx.simulate_keystrokes("enter");
        assert_eq!(
            seen.borrow().last(),
            Some(&ComposerEvent::Submit("сделай\ngit".into()))
        );
        // The view decides whether the text was sent; until then it stays.
        assert_eq!(text(&composer, cx), "сделай\ngit");
        composer.update(cx, |composer, cx| composer.clear(cx));
        assert_eq!(seen.borrow().last(), Some(&ComposerEvent::Emptied(true)));
    }

    #[gpui::test]
    fn the_platform_keys_edit_select_copy_and_undo(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        cx.simulate_input("one two three");
        let (word_left, select_word_left, select_all, copy, cut, paste, undo, redo) =
            if cfg!(target_os = "macos") {
                (
                    "alt-left",
                    "alt-shift-left",
                    "cmd-a",
                    "cmd-c",
                    "cmd-x",
                    "cmd-v",
                    "cmd-z",
                    "cmd-shift-z",
                )
            } else {
                (
                    "ctrl-left",
                    "ctrl-shift-left",
                    "ctrl-a",
                    "ctrl-c",
                    "ctrl-x",
                    "ctrl-v",
                    "ctrl-z",
                    "ctrl-shift-z",
                )
            };
        cx.simulate_keystrokes(word_left);
        cx.simulate_keystrokes(select_word_left);
        cx.simulate_keystrokes(cut);
        assert_eq!(text(&composer, cx), "one three");
        cx.simulate_keystrokes("end");
        cx.simulate_keystrokes(paste);
        assert_eq!(text(&composer, cx), "one threetwo ");
        cx.simulate_keystrokes(undo);
        assert_eq!(text(&composer, cx), "one three");
        cx.simulate_keystrokes(redo);
        assert_eq!(text(&composer, cx), "one threetwo ");
        cx.simulate_keystrokes(select_all);
        cx.simulate_keystrokes(copy);
        cx.simulate_keystrokes("backspace");
        assert_eq!(text(&composer, cx), "");
        cx.simulate_keystrokes(paste);
        cx.simulate_keystrokes("home shift-right shift-right shift-right delete");
        assert_eq!(text(&composer, cx), " threetwo ");
    }

    #[gpui::test]
    fn the_field_grows_to_ten_lines_then_scrolls_with_the_cursor(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        let height = |cx: &mut VisualTestContext| {
            composer.read_with(cx, |composer, _| {
                let geometry = composer.geometry.as_ref().expect("painted");
                (
                    geometry.bounds.size.height,
                    geometry.height(),
                    composer.scroll,
                )
            })
        };
        cx.simulate_input("a");
        cx.run_until_parked();
        let (one, _, _) = height(cx);
        assert_eq!(one, px(LINE_HEIGHT));
        for _ in 0..3 {
            cx.simulate_keystrokes("shift-enter");
            cx.simulate_input("b");
        }
        cx.run_until_parked();
        assert_eq!(height(cx).0, px(LINE_HEIGHT * 4.));
        for _ in 0..10 {
            cx.simulate_keystrokes("shift-enter");
            cx.simulate_input("c");
        }
        cx.run_until_parked();
        let (shown, content, scroll) = height(cx);
        assert_eq!(shown, px(LINE_HEIGHT * MAX_LINES as f32));
        assert_eq!(content, px(LINE_HEIGHT * 14.));
        // The cursor is on the last line, so the field is scrolled to its end.
        assert_eq!(scroll, content - shown);
        // Back at the top, the first line is in view again.
        let top = if cfg!(target_os = "macos") {
            "cmd-up"
        } else {
            "ctrl-home"
        };
        cx.simulate_keystrokes(top);
        cx.run_until_parked();
        assert_eq!(height(cx).2, px(0.));
    }

    #[gpui::test]
    fn a_wheel_step_glides_the_text_as_the_scroll_areas_around_it_do(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        cx.update(|_, cx| cx.set_reduce_motion(false));
        cx.simulate_input("a");
        for _ in 0..14 {
            cx.simulate_keystrokes("shift-enter");
            cx.simulate_input("c");
        }
        cx.run_until_parked();
        let (scroll, middle) = composer.read_with(cx, |composer, _| {
            let geometry = composer.geometry.as_ref().expect("painted");
            (composer.scroll, geometry.bounds.center())
        });
        assert!(scroll > px(0.), "at the end of its text");
        cx.simulate_event(ScrollEvent {
            position: middle,
            delta: gpui::ScrollDelta::Lines(point(0., 2.)),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
            source: Default::default(),
        });
        cx.run_until_parked();
        let first = composer.read_with(cx, |composer, _| composer.scroll);
        assert_eq!(first, scroll, "a step starts where the text is");
        // Half way through the glide it is on its way; at its end, there.
        cx.executor()
            .advance_clock(gpui::SmoothScroll::DEFAULT_DURATION / 2);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        let half = composer.read_with(cx, |composer, _| composer.scroll);
        assert!(half < scroll, "{half:?} from {scroll:?}");
        cx.executor()
            .advance_clock(gpui::SmoothScroll::DEFAULT_DURATION);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        let end = composer.read_with(cx, |composer, _| (composer.scroll, composer.glide));
        assert!(end.0 < half, "{end:?}");
        assert!(end.1.is_none(), "the glide is over");
    }

    #[gpui::test]
    fn up_and_down_move_by_wrapped_row_and_keep_the_column(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        cx.simulate_resize(size(px(200.), px(400.)));
        let long = "word ".repeat(30);
        cx.simulate_input(long.trim_end());
        cx.run_until_parked();
        let (rows, cursor) = composer.read_with(cx, |composer, _| {
            (
                composer.geometry.as_ref().expect("painted").rows.clone(),
                composer.buffer.cursor(),
            )
        });
        assert!(rows.len() > 2, "the text wraps: {rows:?}");
        assert_eq!(cursor, long.trim_end().len());

        cx.simulate_keystrokes("up");
        let cursor = composer.read_with(cx, |composer, _| composer.buffer.cursor());
        let last = rows.len() - 1;
        assert!(
            rows[last - 1].range.contains(&cursor),
            "{cursor} is on the row above: {:?}",
            rows[last - 1]
        );
        cx.simulate_keystrokes("down");
        assert_eq!(
            composer.read_with(cx, |composer, _| composer.buffer.cursor()),
            long.trim_end().len()
        );

        // The start of a wrapped row is not the end of the row above it.
        let edge = "home";
        cx.simulate_keystrokes(edge);
        cx.run_until_parked();
        composer.read_with(cx, |composer, _| {
            assert_eq!(composer.buffer.cursor(), rows[last].range.start);
            let geometry = composer.geometry.as_ref().expect("painted");
            let at = geometry.position(composer.buffer.cursor(), composer.downstream);
            assert_eq!((at.x, at.y), (px(0.), rows[last].top));
        });
    }

    #[gpui::test]
    fn every_key_is_harmless_in_an_empty_field(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        cx.run_until_parked();
        let os = cx.update(|_, cx| cx.platform_info().os);
        let keys = bindings()
            .iter()
            .flat_map(|binding| binding.keystrokes().to_vec())
            .map(|keystroke| keystroke.unparse(os))
            .filter(|key| key != "enter")
            .collect::<Vec<_>>();
        assert!(
            keys.len() > 40,
            "the platform's editing keys are bound: {keys:?}"
        );
        for key in &keys {
            cx.simulate_keystrokes(key);
        }
        // Only the line break keys can have left anything, and undo may have taken it back.
        assert!(text(&composer, cx).chars().all(|c| c == '\n'));
        // And again with text, so every motion and deletion has something to chew on.
        composer.update(cx, |composer, cx| composer.clear(cx));
        cx.simulate_input("одна строка");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("и вторая");
        for key in &keys {
            cx.simulate_keystrokes(key);
        }
        composer.read_with(cx, |composer, _| {
            let selection = composer.buffer.selection();
            assert!(selection.end <= composer.text().len());
            assert!(composer.text().is_char_boundary(selection.start));
        });
    }

    #[gpui::test]
    fn a_click_places_the_cursor_and_a_double_click_takes_the_word(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        cx.simulate_input("alpha beta gamma");
        cx.run_until_parked();
        let origin = composer.read_with(cx, |composer, _| {
            composer.geometry.as_ref().expect("painted").bounds.origin
        });
        cx.simulate_click(origin + point(px(1.), px(8.)), gpui::Modifiers::none());
        assert_eq!(
            composer.read_with(cx, |composer, _| composer.buffer.cursor()),
            0
        );

        let middle = composer.read_with(cx, |composer, _| {
            let geometry = composer.geometry.as_ref().expect("painted");
            geometry.bounds.origin + point(geometry.position(8, false).x, px(8.))
        });
        cx.simulate_event(MouseDownEvent {
            position: middle,
            modifiers: gpui::Modifiers::none(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position: middle,
            modifiers: gpui::Modifiers::none(),
            button: MouseButton::Left,
            click_count: 2,
        });
        composer.read_with(cx, |composer, _| {
            assert_eq!(composer.buffer.selected_text(), "beta");
        });
    }

    #[gpui::test]
    fn a_slash_offers_the_backends_commands(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        let seen = events(&composer, cx);
        let command = |name: &str, description: &str| Choice {
            id: name.into(),
            name: name.into(),
            description: Some(description.into()),
        };
        // No commands, no menu: a slash is just a slash.
        cx.simulate_input("/");
        assert!(!composer.read_with(cx, |composer, _| composer.menu_open()));
        composer.update(cx, |composer, cx| {
            composer.clear(cx);
            composer.set_commands(
                vec![
                    command("compact", "Summarise the conversation"),
                    command("review", "Review the working tree"),
                    command("resume", "Pick up an earlier session"),
                ],
                cx,
            );
        });

        cx.simulate_input("/re");
        cx.run_until_parked();
        composer.read_with(cx, |composer, _| {
            assert!(composer.menu_open());
            let names: Vec<&str> = composer
                .matches()
                .iter()
                .map(|choice| choice.name.as_str())
                .collect();
            assert_eq!(names, ["review", "resume"]);
        });
        assert!(cx.debug_bounds("slash-command-1").is_some());

        cx.simulate_keystrokes("down enter");
        assert_eq!(text(&composer, cx), "/resume ");
        assert!(!composer.read_with(cx, |composer, _| composer.menu_open()));
        // Choosing a command sends nothing; Enter on the finished line does.
        assert!(
            !seen
                .borrow()
                .iter()
                .any(|event| matches!(event, ComposerEvent::Submit(_)))
        );
        cx.simulate_input("last");
        cx.simulate_keystrokes("enter");
        assert_eq!(
            seen.borrow().last(),
            Some(&ComposerEvent::Submit("/resume last".into()))
        );

        // Escape closes the menu and leaves the text.
        composer.update(cx, |composer, cx| composer.clear(cx));
        cx.simulate_input("/c");
        assert!(composer.read_with(cx, |composer, _| composer.menu_open()));
        cx.simulate_keystrokes("escape");
        assert!(!composer.read_with(cx, |composer, _| composer.menu_open()));
        assert_eq!(text(&composer, cx), "/c");
    }

    #[gpui::test]
    fn up_in_an_empty_field_asks_for_history(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        let seen = events(&composer, cx);
        cx.simulate_keystrokes("up");
        assert_eq!(
            seen.borrow().as_slice(),
            [ComposerEvent::Recall { older: true }]
        );
        composer.update(cx, |composer, cx| {
            composer.recall(Some("earlier prompt".into()), cx)
        });
        assert_eq!(text(&composer, cx), "earlier prompt");
        // Still untouched: Up and Down keep walking the history.
        cx.simulate_keystrokes("up down");
        assert_eq!(
            seen.borrow()[2..],
            [
                ComposerEvent::Recall { older: true },
                ComposerEvent::Recall { older: false }
            ]
        );
        // Once edited it is a draft: the arrows move the cursor.
        cx.simulate_input("!");
        let before = seen.borrow().len();
        cx.simulate_keystrokes("up");
        assert_eq!(seen.borrow().len(), before);
        assert_eq!(
            composer.read_with(cx, |composer, _| composer.buffer.cursor()),
            0
        );
    }

    fn drawn(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> Drawn {
        cx.run_until_parked();
        composer.read_with(cx, |composer, _| composer.drawn.expect("drawn"))
    }

    /// The caret stands where the first letter is or will be, as tall as the
    /// font from its ascent to its descent and where the line's text stands.
    fn assert_caret_on_its_line(drawn: Drawn, x: Pixels) {
        assert_caret_on_its_line_at(drawn, x, 1.);
    }

    /// The same, drawn at `factor` (the text shaped at its unscaled size).
    fn assert_caret_on_its_line_at(drawn: Drawn, x: Pixels, factor: f32) {
        let caret = drawn.caret.expect("a caret");
        let height = (drawn.ascent + drawn.descent) * factor;
        assert!((caret.size.height - height).abs() < px(0.01), "{drawn:?}");
        let top = drawn.origin.y + (drawn.line_height - height) / 2.;
        assert!((caret.origin.y - top).abs() < px(0.01), "{drawn:?}");
        assert!((caret.origin.x - x).abs() < px(0.01), "{drawn:?}");
        assert!(caret.size.height < drawn.line_height, "{drawn:?}");
    }

    #[gpui::test]
    fn the_caret_stands_where_the_first_letter_will_and_is_as_tall_as_the_text(
        cx: &mut TestAppContext,
    ) {
        let (composer, cx) = composer(cx);
        cx.update(|window, _| window.activate_window());
        // Empty: the placeholder starts at the field's edge, under the caret.
        let empty = drawn(&composer, cx);
        assert_caret_on_its_line(empty, empty.origin.x + empty.first_glyph);
        let edge = composer.read_with(cx, |composer, _| {
            composer.geometry.as_ref().expect("painted").bounds.origin.x
        });
        assert_eq!(empty.origin.x, edge, "the placeholder is not pushed aside");
        // Typed: at its end, and at its start where the first letter is.
        cx.simulate_input("Ask");
        let typed = drawn(&composer, cx);
        assert!(typed.caret.expect("a caret").origin.x > typed.origin.x + px(10.));
        assert_eq!(
            typed.origin, empty.origin,
            "the text starts where the placeholder did"
        );
        cx.simulate_keystrokes("home");
        let home = drawn(&composer, cx);
        assert_caret_on_its_line(home, home.origin.x + home.first_glyph);
        assert_eq!(
            home.caret.map(|caret| caret.size),
            empty.caret.map(|caret| caret.size)
        );
        // Zoomed, as in a canvas node: the same geometry at the zoom.
        composer.update(cx, |composer, cx| {
            composer.scale.set(1.5);
            cx.notify();
        });
        let zoomed = drawn(&composer, cx);
        assert_caret_on_its_line_at(zoomed, zoomed.origin.x + zoomed.first_glyph * 1.5, 1.5);
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a backspace"
        } else {
            "ctrl-a backspace"
        });
        let cleared = drawn(&composer, cx);
        assert_caret_on_its_line_at(cleared, cleared.origin.x + cleared.first_glyph * 1.5, 1.5);
    }

    #[gpui::test]
    fn the_caret_blinks_only_while_it_has_the_keys_and_lights_at_a_key(cx: &mut TestAppContext) {
        use kou_ui::motion::caret::{DARK, FADE, LIT};
        let (composer, cx) = composer(cx);
        cx.update(|window, _| window.activate_window());
        assert_eq!(drawn(&composer, cx).lit, 1.);
        // Dark after its lit time and the fade out.
        cx.executor().advance_clock(LIT + FADE + DARK / 2);
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert_eq!(drawn(&composer, cx).lit, 0.);
        // A key lights it at once.
        cx.simulate_keystrokes("left");
        assert_eq!(drawn(&composer, cx).lit, 1.);
        // Without the keys it is not drawn and asks for nothing: no frame,
        // however long it waits.
        cx.update(|window, cx| {
            window.blur(cx);
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert_eq!(drawn(&composer, cx).caret, None);
        // What the blur itself asked for is drawn.
        for _ in 0..3 {
            cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
        }
        for _ in 0..20 {
            cx.executor().advance_clock(LIT);
            cx.run_until_parked();
            let frames = cx.update(|window, cx| window.simulate_next_frame(cx));
            assert_eq!(frames, 0, "an unfocused field asks for no frame");
        }
    }

    #[gpui::test]
    fn composition_marks_text_until_it_is_committed(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        composer.update_in(cx, |composer, window, cx| {
            composer.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
            assert_eq!(composer.marked_text_range(window, cx), Some(0..2));
            composer.replace_and_mark_text_in_range(None, "你", Some(1..1), window, cx);
            assert_eq!(composer.text(), "你");
            assert_eq!(composer.marked_text_range(window, cx), Some(0..1));
            composer.replace_text_in_range(None, "你好", window, cx);
            assert_eq!(composer.text(), "你好");
            assert_eq!(composer.marked_text_range(window, cx), None);
            assert_eq!(composer.text_length_utf16(window, cx), Some(2));
            assert_eq!(
                composer
                    .selected_text_range(false, window, cx)
                    .map(|selection| selection.range),
                Some(2..2)
            );
        });
        cx.run_until_parked();
        // Painted: the input method can ask where a character is.
        composer.update_in(cx, |composer, window, cx| {
            let bounds = composer.geometry.as_ref().expect("painted").bounds;
            let first = composer
                .bounds_for_range(0..1, bounds, window, cx)
                .expect("bounds");
            let second = composer
                .bounds_for_range(1..2, bounds, window, cx)
                .expect("bounds");
            assert!(second.origin.x > first.origin.x);
            assert_eq!(first.size.height, px(LINE_HEIGHT));
        });
    }

    #[gpui::test]
    fn what_is_said_of_a_picture_goes_when_the_picture_leaves_the_prompt(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        let said = |composer: &Entity<Composer>, cx: &mut VisualTestContext| {
            composer.read_with(cx, |composer, _| composer.notice().map(str::to_owned))
        };
        composer.update(cx, |composer, cx| {
            composer.attach(
                crate::pictures::tests::picture(40, 30, image::ImageFormat::Bmp),
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(
            said(&composer, cx).as_deref(),
            Some("[Image 1]: converted from BMP.")
        );
        // Text typed beside the picture leaves it said.
        cx.simulate_input("x");
        assert!(said(&composer, cx).is_some());
        // The picture taken out, it is said no more: not when its time is up.
        composer.update(cx, |composer, cx| {
            let id = composer.pictures[0].id;
            composer.remove_picture(id, cx);
        });
        assert_eq!(said(&composer, cx), None);
        // What is not about a picture stays through an edit.
        composer.update(cx, |composer, cx| {
            composer.tell("Something else.", false, cx)
        });
        cx.simulate_input("y");
        assert_eq!(said(&composer, cx).as_deref(), Some("Something else."));
    }
}
