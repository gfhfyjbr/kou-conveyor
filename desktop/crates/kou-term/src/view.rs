//! The GPUI view of a terminal: draws the grid and its pictures, and turns keys, text
//! and the mouse into what the program on the other side expects.
//!
//! On a zoomable canvas the view can follow a zoom factor ([`TerminalView::follow_zoom`]):
//! it then lays the grid out as if the view were its unzoomed size and draws everything
//! scaled, so the program's screen keeps its columns and rows while the canvas zooms.
//!
//! The history scrolls by pixels: a trackpad moves it as the fingers do, and a wheel's
//! line steps glide there the way GPUI's own scroll areas glide ([`gpui::ScrollGlide`],
//! [`gpui::SmoothScroll`]). The emulator scrolls its viewport by whole rows, so the view
//! keeps the part of a row ([`Scrollback`]) and draws the grid that much lower, with the
//! row above the viewport in the gap at the top. It comes to rest on a whole row.

use crate::{
    blink::{Blink, CursorBlink},
    boxdraw::{self, BoxOp},
    keys,
    mouse::{self, MouseAction},
    smear::Smear,
    style::{self, TerminalStyle},
};
use crate::{
    render::{CellCopy, CursorShape, PLACEHOLDER, PictureCopy, RenderState, ScreenCopy},
    terminal::Terminal,
    vt::Vt,
};
use ghostty_vt_sys as sys;
use gpui::{
    App, BorderStyle, Bounds, ClipboardItem, ContentMask, Context, Corners, CursorStyle,
    DispatchPhase, Edges, Entity, EntityInputHandler, FocusHandle, Focusable, Font, FontId,
    GlyphId, Hitbox, HitboxBehavior, Hsla, InputHandler, InteractiveElement as _, IntoElement,
    KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, PathBuilder, Pixels, Point, Render, ScrollEvent, ScrollGlide, Size,
    SmoothScroll, Styled as _, Subscription, Task, TextAlign, TextRun, TouchPhase, UTF16Selection,
    UnderlineStyle, Window, canvas, div, fill, font, point, px, quad, size, transparent_black,
};
use std::{
    cell::{Cell, RefCell},
    mem,
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

/// The most wheel steps one scroll event may turn into, so a flick of the trackpad cannot
/// flood the program with reports.
const MAX_WHEEL_STEPS: i32 = 30;

/// How long a trackpad must stay still before a view it left between two rows settles on
/// the nearer one. Its momentum sends an event every few milliseconds; a pause this long
/// means the gesture is over.
const SETTLE_AFTER: Duration = Duration::from_millis(80);

/// The same while fingers rest on the trackpad: long enough to hold still mid-gesture,
/// and a bound for when they lift somewhere the view does not hear of it.
const SETTLE_WHILE_TOUCHING: Duration = Duration::from_millis(600);

/// The zoom factors a handle may hold; anything outside is clamped, and a value that is
/// not a number reads as 1.
const ZOOM_RANGE: (f32, f32) = (0.05, 8.);

/// How far, in device pixels at the current zoom, the view's unzoomed size may drift
/// before the grid is measured again. Layout rounds a zoomed node to whole device pixels,
/// which divided by a small zoom is several unzoomed pixels; without this margin a column
/// could come and go between zoom steps.
const ZOOM_SLACK: f32 = 1.5;

/// Font sizes are rasterised in steps of half a device pixel up to this size and in
/// steps of [`RASTER_RATIO`] above it, so a zoom gesture adds a few dozen sizes to the
/// glyph atlas rather than one per frame.
const RASTER_LINEAR_LIMIT: f32 = 12.5;
const RASTER_RATIO: f32 = 1.04;

/// A view of a [`Terminal`]: the grid, its pictures, the cursor and the selection, with
/// keyboard, text and mouse input wired to the program.
///
/// While focused it takes the keys a terminal program expects (plain and Shift keys,
/// Ctrl with a character, Alt with a key) before the application's key bindings see them,
/// so a binding on Tab or Escape elsewhere cannot break the program in the pane. Command
/// shortcuts, and chords such as Ctrl+Tab or Ctrl+Shift+T, go to the application's
/// bindings first and reach the program only when nothing claims them.
pub struct TerminalView {
    terminal: Entity<Terminal>,
    style: TerminalStyle,
    focus: FocusHandle,
    metrics: Option<Metrics>,
    geometry: Option<Grid>,
    render: Option<Rc<RefCell<RenderState>>>,
    /// The canvas's zoom, read on every frame, while the view follows one.
    zoom: Option<Rc<Cell<f32>>>,
    /// The unzoomed size the grid was last measured from while following a zoom.
    measured: Option<Size<Pixels>>,
    /// When the program was last told a new size, and the wait for the next
    /// telling while the size keeps changing; see [`RESIZE_PACE`].
    resized_at: Option<std::time::Instant>,
    resize_pending: Option<gpui::Task<()>>,
    /// Text an input method is still composing, shown at the cursor until it commits.
    composing: Option<String>,
    scroll_remainder: Pixels,
    /// Where the view is in the history between whole rows, and where it is going.
    scrollback: Scrollback,
    selecting: Option<Selecting>,
    /// What the last selection gesture selected by, for how its text is copied.
    selected_by: SelectBy,
    /// The button whose press was reported to the program, until its release is.
    reported_button: Option<u8>,
    reported_cell: Option<(usize, usize)>,
    alt_is_meta: bool,
    /// The focus state last told to the program.
    focused: bool,
    #[cfg(any(test, feature = "capture"))]
    forced_focus: Option<bool>,
    /// Where the cursor is on its way between cells; paint advances it.
    smear: Rc<RefCell<Smear>>,
    /// How a blinking cursor fades out and back; `None`, it does not blink.
    cursor_blink: Option<CursorBlink>,
    blink: Blink,
    _subscriptions: Vec<Subscription>,
}

/// What a click and drag selects by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectBy {
    Cell,
    Word,
    Line,
}

/// Which half of a cell the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// A selection being dragged out, anchored in the whole history (row 0 is the oldest
/// line) so it stays put while the view scrolls.
#[derive(Clone, Copy, Debug)]
struct Selecting {
    by: SelectBy,
    anchor: (usize, usize),
    side: Side,
}

/// Where the view is in the history while it scrolls by pixels or glides. The emulator
/// moves its viewport by whole rows, so the part of a row is the view's to keep.
///
/// A position counts rows back from the live screen: the emulator's display offset plus
/// [`Scrollback::fraction`]. Whatever moves the view (a wheel step, a trackpad, the
/// settling after one) sets a glide, and the next frame puts the emulator's viewport and
/// the fraction where the glide is before the screen is copied, so that what is drawn,
/// the cell under the pointer and the selection agree. It always comes to rest on a whole
/// row.
#[derive(Default)]
struct Scrollback {
    /// Where the view is going, while it moves or waits for the next frame to place it.
    glide: Option<ScrollGlide>,
    /// How much of a row past the emulator's display offset the view is drawn, towards
    /// older output: the rows are drawn that much lower, and the row above the viewport
    /// shows in the gap at the top.
    fraction: f32,
    /// The display offset the view last left the emulator at.
    display: usize,
    /// Whether fingers are on the trackpad: a view between rows waits longer to settle.
    touching: bool,
    /// Brings a view a trackpad left between rows to rest on the nearer one.
    settle: Option<Task<()>>,
}

impl Scrollback {
    /// Where the view is at `now`, or where the next frame puts it.
    fn position(&self, now: Instant) -> f32 {
        self.glide
            .map_or(self.display as f32 + self.fraction, |glide| {
                glide.position(now)
            })
    }

    /// Whether the view is on its way somewhere or between two rows.
    fn unsettled(&self) -> bool {
        self.glide.is_some() || self.fraction > 0.
    }

    /// Takes in where the emulator's viewport is. Output that arrives while the view is
    /// scrolled back keeps the rows it shows where they are, so the display offset grows:
    /// the glide moves with them, and one bound for the live screen still goes there. Any
    /// other change (the history trimmed, a reflow) ends the glide and the part of a row.
    fn follow(&mut self, display: usize, now: Instant, duration: Duration) {
        if display == self.display {
            return;
        }
        if display > self.display {
            let moved = (display - self.display) as f32;
            if let Some(glide) = &mut self.glide {
                let to_live = glide.target() == 0.;
                glide.shift(moved);
                if to_live {
                    glide.glide_to(0., now, duration);
                }
            }
        } else {
            self.glide = None;
            self.fraction = 0.;
        }
        self.display = display;
    }

    /// Puts the emulator's viewport and the fraction where the glide is at `now`, before
    /// a frame copies the screen. Returns whether the glide goes on after this frame.
    fn advance(&mut self, vt: &mut Vt, now: Instant, duration: Duration) -> bool {
        self.follow(vt.display_offset(), now, duration);
        let Some(glide) = self.glide else {
            return false;
        };
        self.place(vt, glide.position(now));
        if glide.is_moving(now) {
            return true;
        }
        self.glide = None;
        false
    }

    /// Puts the view `position` rows back, within the history.
    fn place(&mut self, vt: &mut Vt, position: f32) {
        let history = vt.history_rows();
        let position = position.clamp(0., history as f32);
        let mut whole = position.floor();
        let mut fraction = position - whole;
        // What rounding leaves of a row is no part of a row.
        if fraction < 1e-3 {
            fraction = 0.;
        } else if fraction > 1. - 1e-3 {
            whole += 1.;
            fraction = 0.;
        }
        let whole = (whole as usize).min(history);
        let current = vt.display_offset();
        if whole != current {
            vt.scroll(whole as isize - current as isize);
        }
        self.display = vt.display_offset();
        self.fraction = if self.display == whole && whole < history {
            fraction
        } else {
            0.
        };
    }

    /// Leaves the view still on the row the emulator's viewport is at: something moved it
    /// by whole rows at once (keys, a selection dragged past an edge, typing).
    fn jumped(&mut self, vt: &Vt) {
        self.glide = None;
        self.fraction = 0.;
        self.settle = None;
        self.display = vt.display_offset();
    }

    /// A wheel's step of `rows` (positive towards older output): a glide on to a whole
    /// row, or there at once while smooth scrolling is off.
    fn step(&mut self, rows: f32, history: usize, now: Instant, smooth: SmoothScroll) {
        self.settle = None;
        let position = self.position(now);
        let from = self.glide.map_or(position, |glide| glide.target());
        let target = (from + rows).round().clamp(0., history as f32);
        let mut glide = self.glide.unwrap_or_else(|| ScrollGlide::new(position));
        let duration = if smooth.enabled {
            smooth.duration
        } else {
            Duration::ZERO
        };
        glide.glide_to(target, now, duration);
        self.glide = Some(glide);
    }

    /// A trackpad's movement of `rows`, applied as it comes; `phase` says whether fingers
    /// went down or lifted. Returns how long to wait, with no further movement, before
    /// settling on a row.
    fn trackpad(&mut self, rows: f32, phase: TouchPhase, history: usize, now: Instant) -> Duration {
        self.settle = None;
        match phase {
            TouchPhase::Started => self.touching = true,
            TouchPhase::Ended | TouchPhase::Cancelled => self.touching = false,
            TouchPhase::Moved => {}
        }
        let position = (self.position(now) + rows).clamp(0., history as f32);
        self.glide = Some(ScrollGlide::new(position));
        if self.touching {
            SETTLE_WHILE_TOUCHING
        } else {
            SETTLE_AFTER
        }
    }

    /// Starts the glide onto the nearer row for a view a trackpad left between two.
    /// Returns whether there was one to start.
    fn settle(&mut self, now: Instant, smooth: SmoothScroll) -> bool {
        self.settle = None;
        if self.glide.is_some() || self.fraction == 0. {
            return false;
        }
        let position = self.position(now);
        let mut glide = ScrollGlide::new(position);
        let duration = if smooth.enabled {
            smooth.duration
        } else {
            Duration::ZERO
        };
        glide.glide_to(position.round(), now, duration);
        self.glide = Some(glide);
        true
    }
}

impl TerminalView {
    /// A view of `terminal` drawn in `style`.
    pub fn new(
        terminal: Entity<Terminal>,
        style: TerminalStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle().tab_index(0).tab_stop(true);
        terminal.read(cx).set_style(&style);

        let render = match RenderState::new() {
            Ok(render) => Some(Rc::new(RefCell::new(render))),
            Err(error) => {
                log::error!("the terminal cannot be drawn: {error:#}");
                None
            }
        };

        let weak = cx.weak_entity();
        let intercept_focus = focus.clone();
        let subscriptions = vec![
            cx.observe(&terminal, |_, _, cx| cx.notify()),
            cx.on_focus(&focus, window, |this, window, cx| {
                this.focus_changed(window.is_window_active(), cx);
            }),
            cx.on_blur(&focus, window, |this, _, cx| this.focus_changed(false, cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                let focused = this.focus.is_focused(window) && window.is_window_active();
                this.focus_changed(focused, cx);
            }),
            // Pictures live in the window's GPU atlas until they are taken out.
            cx.on_release_in(window, |this, window, _| {
                if let Some(render) = this.render.take() {
                    let mut render = render.borrow_mut();
                    for image in render.release_all() {
                        let _ = window.drop_image(image);
                    }
                }
            }),
            // Key bindings are matched before key listeners run, so a listener alone
            // would lose Tab, Escape or Enter to any binding the application has on them.
            cx.intercept_keystrokes(move |event, window, cx| {
                if !intercept_focus.is_focused(window) || !keys::is_claimed(&event.keystroke) {
                    return;
                }
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let handled =
                    view.update(cx, |view, cx| view.handle_keystroke(&event.keystroke, cx));
                if handled {
                    cx.stop_propagation();
                }
            }),
        ];

        Self {
            terminal,
            style,
            focus,
            metrics: None,
            geometry: None,
            render,
            zoom: None,
            measured: None,
            resized_at: None,
            resize_pending: None,
            composing: None,
            scroll_remainder: px(0.),
            scrollback: Scrollback::default(),
            selecting: None,
            selected_by: SelectBy::Cell,
            reported_button: None,
            reported_cell: None,
            alt_is_meta: true,
            focused: false,
            #[cfg(any(test, feature = "capture"))]
            forced_focus: None,
            smear: Rc::default(),
            cursor_blink: None,
            blink: Blink::new(cx.background_executor().now()),
            _subscriptions: subscriptions,
        }
    }

    /// Lets the cursor blink along `curve` while the view has the keys, the
    /// program asks for a blinking cursor (or leaves the default, which
    /// blinks) and the system does not ask for less motion; `None` keeps it
    /// steady. It is lit at once whenever it moves or a key is pressed.
    pub fn set_cursor_blink(&mut self, curve: Option<CursorBlink>, cx: &mut Context<Self>) {
        self.cursor_blink = curve;
        cx.notify();
    }

    /// Replaces the style, for a theme or font change.
    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        self.terminal.read(cx).set_style(&style);
        self.style = style;
        self.metrics = None;
        cx.notify();
    }

    /// Follows a zoom factor, or stops following one with `None`.
    ///
    /// While a handle is set, its value is read on every frame and everything is drawn
    /// scaled by it: glyphs, cells, the cursor, the selection, underlines and pictures.
    /// The grid keeps its size: its columns and rows are what the view's bounds divided
    /// by the zoom hold at the unscaled cell size, so zooming sends the program no
    /// resize. Pointer positions map to cells at any zoom. `None` draws at the view's
    /// own size again.
    pub fn follow_zoom(&mut self, zoom: Option<Rc<Cell<f32>>>, cx: &mut Context<Self>) {
        self.zoom = zoom;
        self.measured = None;
        cx.notify();
    }

    /// The terminal this view shows.
    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    /// The style in use.
    pub fn style(&self) -> &TerminalStyle {
        &self.style
    }

    /// Whether Alt (Option on macOS) sends Meta, the default, or types the layout's
    /// alternate characters. Layouts that reach brackets or the at sign through Option
    /// need the latter.
    pub fn set_alt_is_meta(&mut self, alt_is_meta: bool) {
        self.alt_is_meta = alt_is_meta;
    }

    /// The window rectangle of a cell as last drawn, at the zoom it was drawn at, or
    /// `None` before the first frame or for a cell outside the grid. For anchoring
    /// something to a position in the pane.
    pub fn cell_bounds(&self, column: usize, line: usize) -> Option<Bounds<Pixels>> {
        let grid = self.geometry?;
        (column < grid.columns && line < grid.lines)
            .then(|| grid.cell_bounds(column, line + grid.above))
    }

    /// Draws the cursor as if the view were focused (or not), whatever the window's
    /// state: a hidden capture window is never active, and its cursor would always be
    /// hollow.
    #[cfg(any(test, feature = "capture"))]
    pub fn force_focused(&mut self, focused: Option<bool>) {
        self.forced_focus = focused;
    }

    /// Copies the selection to the clipboard. Returns whether there was one.
    pub fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.terminal.read(cx).lock().selection_text();
        match text {
            Some(mut text) => {
                // A line taken whole carries its line break, as it was selected.
                if self.selected_by == SelectBy::Line && !text.ends_with('\n') {
                    text.push('\n');
                }
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    /// Selects everything: the history and the screen.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        let mut vt = terminal.lock();
        let everything = vt.everything();
        vt.set_selection(everything);
        drop(vt);
        self.selected_by = SelectBy::Cell;
        cx.notify();
    }

    /// Shows `text` at the cursor as an input method's unfinished composition would be,
    /// for captures of that state.
    #[cfg(any(test, feature = "capture"))]
    pub fn force_composition(&mut self, text: Option<String>) {
        self.composing = text;
    }

    /// Pastes the clipboard's text into the program.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text, cx);
        }
    }

    /// How many rows of history the view is scrolled back by; zero shows the live screen.
    /// While it glides or rests between two rows, the whole rows it is past.
    pub fn scrollback_offset(&self, cx: &App) -> usize {
        self.terminal.read(cx).lock().display_offset()
    }

    /// Scrolls the view by `lines` rows of history at once; positive is towards older
    /// output.
    pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        let mut vt = terminal.lock();
        vt.scroll(lines as isize);
        self.scrollback.jumped(&vt);
        drop(vt);
        cx.notify();
    }

    fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.follow_output(cx);
        self.terminal.read(cx).paste(text);
    }

    /// Typing returns the view to the live screen and drops the selection, as the text
    /// under it is about to change.
    fn follow_output(&mut self, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        let mut vt = terminal.lock();
        let scrolled = vt.display_offset() != 0 || self.scrollback.unsettled();
        if scrolled {
            vt.scroll_to_bottom();
            self.scrollback.jumped(&vt);
        }
        let selected = vt.selection().is_some();
        if selected {
            vt.set_selection(None);
        }
        drop(vt);
        if scrolled || selected {
            cx.notify();
        }
    }

    fn send(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.follow_output(cx);
        self.terminal.read(cx).write(bytes);
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        let terminal = self.terminal.read(cx);
        if terminal.modes().focus_events {
            terminal.write(if focused { b"\x1b[I" } else { b"\x1b[O" });
        }
        cx.notify();
    }

    /// Acts on a key press. Returns whether the key was used, in which case it must not
    /// travel further.
    fn handle_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        // A key lights the cursor at once, whatever it does.
        self.blink.restart(cx.background_executor().now());
        if is_copy(keystroke) {
            self.copy(cx);
            return true;
        }
        if is_paste(keystroke) {
            self.paste(cx);
            return true;
        }
        if is_select_all(keystroke) {
            self.select_all(cx);
            return true;
        }

        let terminal = self.terminal.read(cx);
        let modes = terminal.modes();
        let modifiers = keystroke.modifiers;
        let shift_only =
            modifiers.shift && !modifiers.control && !modifiers.alt && !modifiers.platform;
        // The alternate screen has no history; there these keys belong to the program.
        if shift_only && !modes.alt_screen {
            let mut vt = terminal.lock();
            let page = vt.size().1.saturating_sub(1).max(1) as isize;
            let handled = match keystroke.key.as_str() {
                "pageup" => {
                    vt.scroll(page);
                    true
                }
                "pagedown" => {
                    vt.scroll(-page);
                    true
                }
                "home" => {
                    vt.scroll_to_top();
                    true
                }
                "end" => {
                    vt.scroll_to_bottom();
                    true
                }
                _ => false,
            };
            if handled {
                self.scrollback.jumped(&vt);
            }
            drop(vt);
            if handled {
                cx.notify();
                return true;
            }
        }

        // Nothing is listening any more: leave the keys to the application, which may
        // want Enter to restart the program or close the pane.
        if terminal.exit_status().is_some() {
            return false;
        }
        if let Some(bytes) = keys::encode(keystroke, terminal.key_mode(self.alt_is_meta)) {
            self.send(&bytes, cx);
            return true;
        }
        if let Some(bytes) = platform_editing_key(keystroke) {
            self.send(bytes, cx);
            return true;
        }
        false
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.handle_keystroke(&event.keystroke, cx) {
            cx.stop_propagation();
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(grid) = self.geometry else {
            return;
        };
        let (column, row, side) = grid.cell_at(event.position);
        let line = grid.viewport_line(row);
        let terminal = self.terminal.read(cx);
        let modes = terminal.modes();

        // Shift keeps the mouse for selecting even when the program tracks it.
        if modes.mouse.tracking() && !event.modifiers.shift {
            if let Some(button) = button_code(event.button) {
                let line = line.max(0) as usize;
                self.reported_button = Some(button);
                self.reported_cell = Some((column, line));
                let action = MouseAction::Press(button);
                if let Some(bytes) =
                    mouse::report(action, column, line, event.modifiers, modes.mouse)
                {
                    terminal.write(&bytes);
                }
            }
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        let mut vt = terminal.lock();
        let Some(cell) = viewport_cell(&vt, column, line) else {
            return;
        };
        let Some(here) = vt.screen_point(&cell) else {
            return;
        };
        let existing = vt.selection();
        let selecting = if event.modifiers.shift
            && let Some(existing) = existing
            && let Some(start) = ordered_start(&vt, existing)
        {
            // Shift extends the selection there is, from its start.
            Selecting {
                by: SelectBy::Cell,
                anchor: start,
                side: Side::Left,
            }
        } else {
            let by = match event.click_count {
                0 | 1 => SelectBy::Cell,
                2 => SelectBy::Word,
                _ => SelectBy::Line,
            };
            Selecting {
                by,
                anchor: here,
                side,
            }
        };
        let selection = selection_to(&vt, selecting, here, side);
        vt.set_selection(selection);
        drop(vt);
        self.selected_by = selecting.by;
        self.selecting = Some(selecting);
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, hovered: bool, cx: &mut Context<Self>) {
        let Some(grid) = self.geometry else {
            return;
        };
        let (column, row, side) = grid.cell_at(event.position);
        let line = grid.viewport_line(row);
        let terminal = self.terminal.read(cx);

        if let Some(selecting) = self.selecting {
            if event.pressed_button != Some(MouseButton::Left) {
                // The release happened where no event reached this view.
                self.selecting = None;
                cx.notify();
                return;
            }
            let mut vt = terminal.lock();
            // Dragging past an edge pulls more of the history into view.
            if event.position.y < grid.top() {
                vt.scroll(1);
                self.scrollback.jumped(&vt);
            } else if event.position.y > grid.bottom() {
                vt.scroll(-1);
                self.scrollback.jumped(&vt);
            }
            if let Some(here) =
                viewport_cell(&vt, column, line).and_then(|cell| vt.screen_point(&cell))
            {
                let selection = selection_to(&vt, selecting, here, side);
                vt.set_selection(selection);
            }
            drop(vt);
            cx.notify();
            return;
        }

        let modes = terminal.modes();
        if !modes.mouse.tracking() || event.modifiers.shift {
            return;
        }
        if !hovered && self.reported_button.is_none() {
            return;
        }
        let line = line.max(0) as usize;
        if self.reported_cell == Some((column, line)) {
            return;
        }
        self.reported_cell = Some((column, line));
        let action = MouseAction::Motion(self.reported_button);
        if let Some(bytes) = mouse::report(action, column, line, event.modifiers, modes.mouse) {
            terminal.write(&bytes);
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        if self.selecting.is_some() && event.button == MouseButton::Left {
            self.selecting = None;
            cx.notify();
            return;
        }
        let Some(button) = button_code(event.button) else {
            return;
        };
        if self.reported_button != Some(button) {
            return;
        }
        self.reported_button = None;
        let Some(grid) = self.geometry else {
            return;
        };
        let (column, row, _) = grid.cell_at(event.position);
        let line = grid.viewport_line(row).max(0) as usize;
        let action = MouseAction::Release(button);
        let mode = terminal.modes().mouse;
        if let Some(bytes) = mouse::report(action, column, line, event.modifiers, mode) {
            terminal.write(&bytes);
        }
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let Some(grid) = self.geometry else {
            return;
        };
        // In screen pixels, so the content follows the fingers at any zoom.
        let cell_height = grid.cell.height;
        let modes = self.terminal.read(cx).modes();
        let to_program = !event.modifiers.shift
            && (modes.mouse.tracking() || (modes.alt_screen && modes.alternate_scroll));
        if !to_program {
            self.scroll_history(event, cell_height, cx);
            return;
        }

        // A program that takes the wheel gets whole steps, as many as the deltas add up to.
        self.scroll_remainder += event.delta.pixel_delta(cell_height).y;
        let lines = (self.scroll_remainder / cell_height).trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= cell_height * lines as f32;
        let steps = lines.abs().min(MAX_WHEEL_STEPS);
        let up = lines > 0;

        let terminal = self.terminal.read(cx);
        if modes.mouse.tracking() {
            let (column, row, _) = grid.cell_at(event.position);
            let line = grid.viewport_line(row).max(0) as usize;
            let action = MouseAction::Wheel { up };
            if let Some(bytes) = mouse::report(action, column, line, event.modifiers, modes.mouse) {
                terminal.write(&bytes.repeat(steps as usize));
            }
        } else {
            // A full-screen program has no history to scroll; the wheel moves its cursor.
            let key = Keystroke {
                key: if up { "up" } else { "down" }.into(),
                ..Default::default()
            };
            if let Some(bytes) = keys::encode(&key, terminal.key_mode(self.alt_is_meta)) {
                terminal.write(&bytes.repeat(steps as usize));
            }
        }
    }

    /// Scrolls the history: a precise delta (a trackpad) by its pixels as they come, a
    /// wheel's line steps by gliding to the rows they lead to. The next frame places the
    /// view (see [`Scrollback`]).
    fn scroll_history(&mut self, event: &ScrollEvent, cell_height: Pixels, cx: &mut Context<Self>) {
        let smooth = SmoothScroll::of(cx);
        let now = cx.background_executor().now();
        let history = {
            let terminal = self.terminal.read(cx);
            let vt = terminal.lock();
            self.scrollback
                .follow(vt.display_offset(), now, smooth.duration);
            vt.history_rows()
        };
        if event.delta.precise() {
            let rows = event.delta.pixel_delta(cell_height).y / cell_height;
            let wait = self
                .scrollback
                .trackpad(rows, event.touch_phase, history, now);
            self.scrollback.settle = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                this.update(cx, |view, cx| {
                    let smooth = SmoothScroll::of(cx);
                    let now = cx.background_executor().now();
                    if view.scrollback.settle(now, smooth) {
                        cx.notify();
                    }
                })
                .ok();
            }));
        } else {
            self.scroll_remainder += event.delta.pixel_delta(cell_height).y;
            let lines = (self.scroll_remainder / cell_height).trunc();
            if lines == 0. {
                return;
            }
            self.scroll_remainder -= cell_height * lines;
            self.scrollback.step(lines, history, now, smooth);
        }
        cx.notify();
    }

    /// The zoom to draw at this frame: 1 without a handle.
    fn zoom(&self) -> f32 {
        let value = self.zoom.as_ref().map_or(1., |zoom| zoom.get());
        if value.is_finite() {
            value.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1)
        } else {
            1.
        }
    }

    /// The size the grid is measured from: the view's own, or while following a zoom its
    /// unzoomed size, held still against layout rounding.
    fn measured_size(&mut self, bounds: Size<Pixels>, zoom: f32, scale: f32) -> Size<Pixels> {
        if self.zoom.is_none() {
            self.measured = None;
            return bounds;
        }
        let unzoomed = size(bounds.width / zoom, bounds.height / zoom);
        let slack = px(ZOOM_SLACK / (zoom * scale.max(1.)));
        let close = |a: Pixels, b: Pixels| (a - b).abs() <= slack + a.abs() * 1e-4;
        match self.measured {
            Some(previous)
                if close(previous.width, unzoomed.width)
                    && close(previous.height, unzoomed.height) =>
            {
                previous
            }
            _ => {
                self.measured = Some(unzoomed);
                unzoomed
            }
        }
    }

    /// Measures, resizes the terminal to fit and brings the copy of its screen up to
    /// date.
    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Frame {
        let scale = window.scale_factor();
        if self
            .metrics
            .as_ref()
            .is_none_or(|metrics| metrics.scale != scale)
        {
            self.metrics = Some(Metrics::measure(&self.style, window));
        }
        let metrics = self.metrics.clone().expect("measured above");
        let zoom = self.zoom();
        let measured = self.measured_size(bounds.size, zoom, scale);

        let padding = self.style.padding;
        let available = size(
            measured.width - padding * 2.,
            measured.height - padding * 2.,
        );
        let columns = (available.width / metrics.cell.width).floor() as usize;
        let lines = (available.height / metrics.cell.height).floor() as usize;
        // While an edge is dragged the size changes on every frame, and a shell
        // redraws its prompt for every size it is told: told two hundred sizes a
        // second, it leaves pieces of two hundred prompts behind. The program is
        // told a new size at once when the size was at rest, and no more often
        // than `RESIZE_PACE` while it keeps changing, ending with the last one.
        let tell = if self.terminal.read(cx).size() == (columns, lines) {
            true
        } else {
            // The executor's clock, which a test moves by hand.
            let now = cx.background_executor().now();
            let since = self.resized_at.map(|at| now.duration_since(at));
            match since {
                Some(since) if since < RESIZE_PACE => {
                    if self.resize_pending.is_none() {
                        let wait = RESIZE_PACE - since;
                        self.resize_pending = Some(cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(wait).await;
                            this.update(cx, |this, cx| {
                                this.resize_pending = None;
                                cx.notify();
                            })
                            .ok();
                        }));
                    }
                    false
                }
                _ => {
                    self.resized_at = Some(now);
                    self.resize_pending = None;
                    true
                }
            }
        };

        let smooth = SmoothScroll::of(cx);
        let now = cx.background_executor().now();
        let terminal = self.terminal.read(cx);
        if let Some(interval) = window.refresh_interval(cx) {
            terminal.set_frame_interval(interval);
        }
        let snap = |value: Pixels| px((value.as_f32() * scale).round() / scale);
        // A pane that is collapsed or not laid out yet must not squeeze the program
        // into two columns; it keeps its size until there is room to show it.
        if tell && columns >= 2 && lines >= 1 {
            terminal.resize_cells(
                columns,
                lines,
                (metrics.cell.width.as_f32() * scale).round() as u16,
                (metrics.cell.height.as_f32() * scale).round() as u16,
            );
        }
        let cell_pixels = {
            let size = terminal.grid_pixels();
            (f32::from(size.cell_width), f32::from(size.cell_height))
        };

        let mut gliding = false;
        if let Some(render) = &self.render {
            let mut vt = terminal.lock();
            // The view moves before the screen is copied: the copy is of where it is drawn.
            gliding = self.scrollback.advance(&mut vt, now, smooth.duration);
            render.borrow_mut().update(&vt);
        }
        let (grid_columns, grid_lines, above) = self
            .render
            .as_ref()
            .map(|render| {
                let render = render.borrow();
                let copy = render.copy();
                (copy.columns, copy.lines - copy.above, copy.above)
            })
            .unwrap_or_default();
        // Without the row above the viewport there is nothing to show in a gap at the top.
        let fraction = if above > 0 {
            self.scrollback.fraction
        } else {
            0.
        };
        let origin = point(
            snap(bounds.origin.x) + padding * zoom,
            snap(bounds.origin.y) + padding * zoom,
        );
        let grid = Grid {
            bounds,
            origin,
            cell: size(metrics.cell.width * zoom, metrics.cell.height * zoom),
            columns: grid_columns,
            lines: grid_lines,
            above,
            fraction,
            back: self.scrollback.display as f32 + fraction,
            zoom,
            scale,
        };
        self.geometry = Some(grid);
        let focused = self.focus.is_focused(window) && window.is_window_active();
        #[cfg(any(test, feature = "capture"))]
        let focused = self.forced_focus.unwrap_or(focused);
        let cursor = self
            .render
            .as_ref()
            .and_then(|render| render.borrow().copy().cursor);
        let cursor_lit = match (self.cursor_blink, cursor) {
            (Some(curve), Some(cursor))
                if focused
                    && cursor.blinking
                    && cursor.shape != CursorShape::HollowBlock
                    && !cx.reduce_motion() =>
            {
                self.blink.at((cursor.line, cursor.column), now);
                self.blink.lit(curve, now, cx.entity_id(), window, cx)
            }
            _ => {
                self.blink.hold();
                1.
            }
        };
        Frame {
            render: self.render.clone(),
            metrics,
            grid,
            cell_pixels,
            style: self.style.clone(),
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            focused,
            composing: self.composing.clone(),
            focus: self.focus.clone(),
            smear: self.smear.clone(),
            flight: None,
            cursor_lit,
            still: cx.reduce_motion(),
            gliding,
        }
    }

    /// The window rectangle of the cursor's cell, for placing input-method windows.
    fn cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let grid = self.geometry?;
        let render = self.render.as_ref()?.borrow();
        let cursor = render.copy().cursor?;
        Some(grid.cell_bounds(cursor.column, cursor.line))
    }
}

/// The cell at `column` on viewport row `line`, where row -1 is the row above the
/// viewport that a view drawn between rows shows at its top.
fn viewport_cell(vt: &Vt, column: usize, line: isize) -> Option<sys::GhosttyGridRef> {
    if let Ok(line) = usize::try_from(line) {
        return vt.viewport_ref(column, line);
    }
    let (_, top) = vt
        .viewport_ref(0, 0)
        .and_then(|cell| vt.screen_point(&cell))?;
    let row = top.checked_sub(line.unsigned_abs())?;
    vt.screen_ref(column.min(vt.size().0.saturating_sub(1)), row)
}

/// The start of a selection in reading order, in whole-history coordinates.
fn ordered_start(vt: &Vt, selection: sys::GhosttySelection) -> Option<(usize, usize)> {
    let start = vt.screen_point(&selection.start)?;
    let end = vt.screen_point(&selection.end)?;
    Some(if (start.1, start.0) <= (end.1, end.0) {
        start
    } else {
        end
    })
}

/// The selection from a gesture's anchor to the cell under the pointer.
fn selection_to(
    vt: &Vt,
    selecting: Selecting,
    here: (usize, usize),
    side: Side,
) -> Option<sys::GhosttySelection> {
    let anchor = vt.screen_ref(selecting.anchor.0, selecting.anchor.1)?;
    let head = vt.screen_ref(here.0, here.1)?;
    match selecting.by {
        SelectBy::Cell => {
            let columns = vt.size().0;
            let (start, end) =
                cell_span((selecting.anchor, selecting.side), (here, side), columns)?;
            Some(sys::GhosttySelection {
                start: vt.screen_ref(start.0, start.1)?,
                end: vt.screen_ref(end.0, end.1)?,
                rectangle: false,
            })
        }
        SelectBy::Word => {
            let (first, last) = if (selecting.anchor.1, selecting.anchor.0) <= (here.1, here.0) {
                (anchor, head)
            } else {
                (head, anchor)
            };
            vt.words_between(first, last)
        }
        SelectBy::Line => {
            let from = vt.line_at(anchor)?;
            let to = vt.line_at(head)?;
            let ahead = (selecting.anchor.1, selecting.anchor.0) <= (here.1, here.0);
            Some(if ahead {
                sys::GhosttySelection {
                    start: from.start,
                    end: to.end,
                    rectangle: false,
                }
            } else {
                sys::GhosttySelection {
                    start: to.start,
                    end: from.end,
                    rectangle: false,
                }
            })
        }
    }
}

/// The cells a drag covers from one point to another, inclusive, in whole-history
/// coordinates `(column, row)`; `None` when it covers none. A drag that starts on the
/// right half of a cell leaves that cell out, and one that ends on the left half of a
/// cell leaves that one out, so a click without a drag selects nothing.
fn cell_span(
    from: ((usize, usize), Side),
    to: ((usize, usize), Side),
    columns: usize,
) -> Option<((usize, usize), (usize, usize))> {
    let key = |point: (usize, usize)| (point.1, point.0);
    let (first, last) = if key(from.0) <= key(to.0) {
        (from, to)
    } else {
        (to, from)
    };
    let columns = columns.max(1);
    let linear = |point: (usize, usize)| point.1 * columns + point.0.min(columns - 1);
    let mut start = linear(first.0);
    let mut end = linear(last.0);
    if first.1 == Side::Right {
        start += 1;
    }
    if last.1 == Side::Left {
        end = end.checked_sub(1)?;
    }
    (start <= end).then(|| {
        (
            (start % columns, start / columns),
            (end % columns, end / columns),
        )
    })
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prepare_view = cx.entity();
        let paint_view = cx.entity();
        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        prepare_view.update(cx, |view, cx| view.prepare(bounds, window, cx))
                    },
                    move |_, frame, window, cx| paint(paint_view, frame, window, cx),
                )
                .size_full(),
            )
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        // The only text an input method can ask about is what it is composing itself.
        let composing = self.composing.as_ref()?;
        let units: Vec<u16> = composing.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        adjusted_range.replace(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = composing_length(&self.composing);
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.composing
            .is_some()
            .then(|| 0..composing_length(&self.composing))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.composing.take().is_some() {
            cx.notify();
        }
    }

    fn paste(&mut self, item: ClipboardItem, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            self.paste_text(&text, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composing.take().is_some() {
            cx.notify();
        }
        if !text.is_empty() {
            self.send(text.as_bytes(), cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composing = (!new_text.is_empty()).then(|| new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds()
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

fn composing_length(composing: &Option<String>) -> usize {
    composing
        .as_ref()
        .map_or(0, |text| text.encode_utf16().count())
}

/// Hands the platform's text input to the view's [`EntityInputHandler`], with one change
/// from GPUI's own adapter: a held key repeats instead of opening the accent menu, which
/// is what holding a key means in a terminal.
struct TerminalInputHandler {
    view: Entity<TerminalView>,
    bounds: Bounds<Pixels>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.view.update(cx, |view, cx| {
            view.selected_text_range(ignore_disabled_input, window, cx)
        })
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.view
            .update(cx, |view, cx| view.marked_text_range(window, cx))
    }

    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.view.update(cx, |view, cx| {
            view.text_for_range(range, adjusted_range, window, cx)
        })
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.replace_text_in_range(range, text, window, cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(range, new_text, new_selected_range, window, cx);
        });
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.view
            .update(cx, |view, cx| view.unmark_text(window, cx));
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| {
            EntityInputHandler::paste(view, item, window, cx)
        });
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds;
        self.view.update(cx, |view, cx| {
            view.bounds_for_range(range, bounds, window, cx)
        })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.view.update(cx, |view, cx| {
            view.character_index_for_point(point, window, cx)
        })
    }

    fn element_bounds(&mut self, _: &mut Window, _: &mut App) -> Option<Bounds<Pixels>> {
        Some(self.bounds)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}

/// The platform's copy shortcut: Cmd+C on macOS, Ctrl+Shift+C elsewhere, where Ctrl+C
/// alone has to stay the interrupt.
fn is_copy(keystroke: &Keystroke) -> bool {
    is_clipboard_shortcut(keystroke, "c")
}

/// The platform's paste shortcut: Cmd+V on macOS, Ctrl+Shift+V elsewhere.
fn is_paste(keystroke: &Keystroke) -> bool {
    is_clipboard_shortcut(keystroke, "v")
}

/// The platform's select-all shortcut: Cmd+A on macOS, Ctrl+Shift+A elsewhere.
fn is_select_all(keystroke: &Keystroke) -> bool {
    is_clipboard_shortcut(keystroke, "a")
}

fn is_clipboard_shortcut(keystroke: &Keystroke, key: &str) -> bool {
    let modifiers = keystroke.modifiers;
    if !keystroke.key.eq_ignore_ascii_case(key) || modifiers.alt || modifiers.function {
        return false;
    }
    if cfg!(target_os = "macos") {
        modifiers.platform && !modifiers.control && !modifiers.shift
    } else {
        modifiers.control && modifiers.shift && !modifiers.platform
    }
}

/// Line editing the way macOS text fields do it, for shells and prompts: Cmd+Left and
/// Cmd+Right jump to the ends of the line and Cmd+Backspace clears back to its start.
/// These only apply when the application has not bound the chords to something else.
fn platform_editing_key(keystroke: &Keystroke) -> Option<&'static [u8]> {
    let modifiers = keystroke.modifiers;
    if !cfg!(target_os = "macos")
        || !modifiers.platform
        || modifiers.control
        || modifiers.alt
        || modifiers.shift
    {
        return None;
    }
    match keystroke.key.as_str() {
        "left" => Some(b"\x01"),
        "right" => Some(b"\x05"),
        "backspace" => Some(b"\x15"),
        _ => None,
    }
}

fn button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
}

/// Font measurements for one style at one scale factor, unzoomed.
#[derive(Clone)]
struct Metrics {
    scale: f32,
    font_size: Pixels,
    /// Regular, bold, italic, bold italic.
    fonts: [Font; 4],
    font_ids: [FontId; 4],
    /// The size of a cell, a whole number of device pixels each way.
    cell: Size<Pixels>,
    /// From the top of a row to the text baseline.
    baseline: Pixels,
    /// From the baseline down to the top of an underline.
    underline_offset: Pixels,
    /// From the top of a row to the top of a strikethrough.
    strike_offset: Pixels,
    /// Thickness of underlines, strikethroughs and cursor outlines.
    stroke: Pixels,
}

impl Metrics {
    fn measure(style: &TerminalStyle, window: &Window) -> Self {
        let scale = window.scale_factor();
        let text_system = window.text_system();
        let regular = font(style.font_family.clone());
        let fonts = [
            regular.clone(),
            regular.clone().bold(),
            regular.clone().italic(),
            regular.bold().italic(),
        ];
        let font_ids = [
            text_system.resolve_font(&fonts[0]),
            text_system.resolve_font(&fonts[1]),
            text_system.resolve_font(&fonts[2]),
            text_system.resolve_font(&fonts[3]),
        ];
        let font_size = style.font_size;
        // Whole device pixels, so every glyph of a kind lands on the same subpixel and
        // the edges of neighbouring cells coincide exactly.
        let snap = |value: Pixels| px((value.as_f32() * scale).round().max(1.) / scale);
        let advance = text_system
            .advance(font_ids[0], font_size, 'm')
            .map(|advance| advance.width)
            .unwrap_or(font_size * 0.6);
        let ascent = text_system.ascent(font_ids[0], font_size);
        // Platforms disagree on the sign of the descent.
        let descent = text_system.descent(font_ids[0], font_size).abs();
        let cell = size(
            snap(advance),
            snap((font_size * style.line_height).max(ascent + descent)),
        );
        let baseline = snap((cell.height - ascent - descent) / 2. + ascent);
        let stroke = snap(font_size / 13.);
        let x_height = text_system.x_height(font_ids[0], font_size);
        Self {
            scale,
            font_size,
            fonts,
            font_ids,
            cell,
            baseline,
            underline_offset: snap(descent * 0.4),
            strike_offset: snap(baseline - x_height / 2. - stroke / 2.),
            stroke,
        }
    }
}

/// The least time between two sizes told to the program while its view keeps changing
/// size: long enough that a drag is a handful of resizes, short enough to look live.
const RESIZE_PACE: std::time::Duration = std::time::Duration::from_millis(90);

/// The font size glyphs are rasterised at for `size` drawn at `zoom`: exact at zoom 1,
/// otherwise rounded to half a device pixel below [`RASTER_LINEAR_LIMIT`] device pixels
/// and to a power of [`RASTER_RATIO`] above it. The cells keep their exact zoomed size;
/// only the glyphs are up to 2% off it, so a continuous zoom reuses a few dozen sizes
/// from GPUI's glyph atlas instead of adding one per frame.
fn raster_size(size: Pixels, zoom: f32, scale: f32) -> Pixels {
    if zoom == 1. {
        return size;
    }
    let scale = scale.max(0.5);
    let device = (size.as_f32() * zoom * scale).max(0.5);
    let quantised = if device <= RASTER_LINEAR_LIMIT {
        ((device * 2.).round() / 2.).max(0.5)
    } else {
        let steps = (device / RASTER_LINEAR_LIMIT).ln() / RASTER_RATIO.ln();
        RASTER_LINEAR_LIMIT * RASTER_RATIO.powf(steps.round())
    };
    px(quantised / scale)
}

/// Where the grid is drawn this frame, for turning a pointer position into a cell and a
/// cell into a rectangle.
///
/// Its rows are those of the screen copy: the viewport's, after the [`Grid::above`] rows
/// before it. They are drawn [`Grid::fraction`] of a row lower than the viewport's rest,
/// clipped to the text area; at rest the row above the viewport is out of sight.
#[derive(Clone, Copy)]
struct Grid {
    bounds: Bounds<Pixels>,
    /// The top-left corner of the viewport's first cell at rest: the top-left of the text
    /// area.
    origin: Point<Pixels>,
    /// The size of a cell on screen: the unzoomed cell times the zoom, not rounded.
    cell: Size<Pixels>,
    columns: usize,
    /// The viewport's rows.
    lines: usize,
    /// How many rows come before the viewport's first: the row above it, when there is
    /// history above.
    above: usize,
    /// How much of a row lower than at rest the rows are drawn, while the view is between
    /// two rows of the history.
    fraction: f32,
    /// How many rows back from the live screen the view is, fraction included.
    back: f32,
    zoom: f32,
    /// Device pixels per window pixel.
    scale: f32,
}

impl Grid {
    fn snap(&self, value: Pixels) -> Pixels {
        px((value.as_f32() * self.scale).round() / self.scale)
    }

    /// The left edge of a column, on a device pixel. Neighbouring cells share their edge,
    /// so backgrounds meet without seams at any zoom; at zoom 1 every cell is the same
    /// whole number of device pixels wide.
    fn x(&self, column: usize) -> Pixels {
        self.snap(self.origin.x + self.cell.width * column as f32)
    }

    /// The top edge of a row, on a device pixel.
    fn y(&self, row: usize) -> Pixels {
        let rows = row as f32 - self.above as f32 + self.fraction;
        self.snap(self.origin.y + self.cell.height * rows)
    }

    /// The top of the text area, where the viewport's first row rests.
    fn top(&self) -> Pixels {
        self.snap(self.origin.y)
    }

    /// The bottom of the text area, where the viewport's last row ends at rest.
    fn bottom(&self) -> Pixels {
        self.snap(self.origin.y + self.cell.height * self.lines as f32)
    }

    /// The viewport row of a row; -1 for the row above the viewport.
    fn viewport_line(&self, row: usize) -> isize {
        row as isize - self.above as isize
    }

    /// The rows that show in the text area: the row above the viewport only while the view
    /// is between two rows.
    fn shown_rows(&self) -> Range<usize> {
        let first = if self.fraction > 0. { 0 } else { self.above };
        first..self.above + self.lines
    }

    /// The clip for the rows while the view is between two rows, when part of a row sticks
    /// out above the text area and part of another below it; `None` at rest.
    fn rows_mask(&self) -> Option<ContentMask<Pixels>> {
        (self.fraction > 0.).then(|| ContentMask {
            bounds: Bounds::from_corners(
                point(self.bounds.left(), self.top()),
                point(self.bounds.right(), self.bottom()),
            ),
        })
    }

    fn cell_bounds(&self, column: usize, row: usize) -> Bounds<Pixels> {
        Bounds::from_corners(
            point(self.x(column), self.y(row)),
            point(self.x(column + 1), self.y(row + 1)),
        )
    }

    /// The text area: where the viewport's cells rest.
    fn area(&self) -> Bounds<Pixels> {
        Bounds::from_corners(
            point(self.x(0), self.top()),
            point(self.x(self.columns), self.bottom()),
        )
    }

    /// The cell under `position`, clamped into the text area, as a column, a row and the
    /// half of the cell that was hit.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let x = (position.x - self.origin.x) / self.cell.width;
        let y = ((position.y - self.origin.y) / self.cell.height)
            .clamp(0., (self.lines as f32 - 1e-3).max(0.))
            - self.fraction
            + self.above as f32;
        let last_column = self.columns.saturating_sub(1);
        let (column, side) = if x < 0. {
            (0, Side::Left)
        } else if x >= self.columns as f32 {
            (last_column, Side::Right)
        } else {
            let side = if x.fract() < 0.5 {
                Side::Left
            } else {
                Side::Right
            };
            (x as usize, side)
        };
        let row = (y.max(0.) as usize).min((self.above + self.lines).saturating_sub(1));
        (column, row, side)
    }
}

/// Everything one frame draws, handed from layout to paint.
struct Frame {
    render: Option<Rc<RefCell<RenderState>>>,
    metrics: Metrics,
    grid: Grid,
    /// The size of a cell in the terminal's pixels, which pictures are measured in.
    cell_pixels: (f32, f32),
    style: TerminalStyle,
    hitbox: Hitbox,
    focused: bool,
    composing: Option<String>,
    focus: FocusHandle,
    smear: Rc<RefCell<Smear>>,
    /// The cursor's four corners while it travels between cells. While it does,
    /// rows do not draw it and the character under it keeps its own colour.
    flight: Option<[Point<Pixels>; 4]>,
    /// How lit the cursor is in its blink, from 1 to 0.
    cursor_lit: f32,
    /// The user asked the system for less motion.
    still: bool,
    /// The history is gliding: another frame is wanted.
    gliding: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Underline {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
    Strike,
}

/// A shaped glyph with the column of the cell it belongs to.
struct PlacedGlyph {
    font: FontId,
    id: GlyphId,
    x: Pixels,
    y: Pixels,
    column: usize,
    emoji: bool,
}

/// Scratch space reused from row to row.
#[derive(Default)]
struct RowBuffers {
    text: String,
    /// The column of the cell each byte of `text` came from.
    byte_columns: Vec<usize>,
    runs: Vec<(usize, usize)>,
    foreground: Vec<Hsla>,
    wide: Vec<bool>,
    /// Whether a column shows nothing, so a neighbour's glyph may spill into it.
    blank: Vec<bool>,
    backgrounds: Vec<(usize, usize, Hsla)>,
    selected: Vec<(usize, usize, ())>,
    underlines: Vec<(usize, usize, (Underline, Hsla))>,
    strikes: Vec<(usize, usize, Hsla)>,
    boxes: Vec<(usize, char)>,
    box_ops: Vec<BoxOp>,
    glyphs: Vec<PlacedGlyph>,
}

impl RowBuffers {
    fn clear(&mut self) {
        self.text.clear();
        self.byte_columns.clear();
        self.runs.clear();
        self.foreground.clear();
        self.wide.clear();
        self.blank.clear();
        self.backgrounds.clear();
        self.selected.clear();
        self.underlines.clear();
        self.strikes.clear();
        self.boxes.clear();
        self.glyphs.clear();
    }
}

/// Extends the last span when `value` continues it, otherwise starts a new one.
fn push_span<T: PartialEq + Copy>(spans: &mut Vec<(usize, usize, T)>, column: usize, value: T) {
    match spans.last_mut() {
        Some((_, end, last)) if *end == column && *last == value => *end = column + 1,
        _ => spans.push((column, column + 1, value)),
    }
}

/// A cell's colours as drawn: foreground and background after inverse and faint.
fn cell_colors(cell: &CellCopy, screen: &ScreenCopy) -> (Hsla, Hsla) {
    let mut foreground = cell.foreground;
    let mut background = cell.background.unwrap_or(screen.background);
    if cell.has(sys::GHOSTTY_STYLE_FLAG_INVERSE) {
        mem::swap(&mut foreground, &mut background);
    }
    if cell.has(sys::GHOSTTY_STYLE_FLAG_FAINT) {
        foreground = style::mix(foreground, background, style::DIM_AMOUNT);
    }
    (foreground, background)
}

/// Where the cursor rests this frame, unzoomed and relative to the grid's origin: its
/// cell, narrowed to the bar or the underline it is drawn as. `None` when it is not drawn
/// filled. The cursor's flight is computed in these units so that neither a zoom step
/// nor the pane moving starts one.
fn cursor_rest(frame: &Frame, screen: &ScreenCopy) -> Option<[f32; 4]> {
    let cursor = screen.cursor?;
    if !frame.focused {
        return None;
    }
    let cell = frame.metrics.cell;
    let wide = screen
        .cell(cursor.line, cursor.column)
        .is_some_and(CellCopy::is_wide);
    let (width, height) = (cell.width.as_f32(), cell.height.as_f32());
    let x = width * cursor.column as f32;
    // From the top of the live screen, which moves with the rows: scrolling the history
    // carries the cursor along rather than sending it on a flight.
    let line = frame.grid.viewport_line(cursor.line) as f32;
    let y = height * (line - frame.grid.back);
    let width = if wide { width * 2. } else { width };
    let thin = frame.metrics.stroke.as_f32() * 2.;
    match cursor.shape {
        CursorShape::Block => Some([x, y, width, height]),
        CursorShape::Beam => Some([x, y, thin.min(width), height]),
        CursorShape::Underline => Some([x, y + height - thin, width, thin]),
        CursorShape::HollowBlock => None,
    }
}

fn paint(view: Entity<TerminalView>, mut frame: Frame, window: &mut Window, cx: &mut App) {
    let bounds = frame.grid.bounds;
    let Some(render) = frame.render.clone() else {
        window.paint_quad(fill(bounds, frame.style.background));
        return;
    };
    for image in render.borrow_mut().take_retired() {
        let _ = window.drop_image(image);
    }
    let render = render.borrow();
    let screen = render.copy();
    let grid = frame.grid;

    // Advance the cursor's flight before the rows are drawn: they need to know
    // whether it has landed.
    frame.flight = match (
        cursor_rest(&frame, screen),
        frame.style.cursor_motion,
        frame.still,
    ) {
        (Some(rest), Some(motion), false) => frame
            .smear
            .borrow_mut()
            .step(
                rest,
                frame.metrics.cell.width.as_f32(),
                std::time::Instant::now(),
                &motion,
            )
            .map(|corners| {
                corners.map(|(x, y)| {
                    let back = grid.back * frame.metrics.cell.height.as_f32();
                    point(
                        grid.origin.x + px(x * grid.zoom),
                        grid.origin.y + px((y + back) * grid.zoom),
                    )
                })
            }),
        _ => {
            // Hidden, unfocused or unanimated: when it shows again it is simply there.
            frame.smear.borrow_mut().reset();
            None
        }
    };
    let background = screen.background;
    window.paint_quad(fill(bounds, background));
    let below_background = screen
        .pictures
        .iter()
        .any(|picture| picture.layer == sys::GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND);
    let below_text = screen
        .pictures
        .iter()
        .any(|picture| picture.layer == sys::GHOSTTY_IMAGE_LAYER_BELOW_TEXT);
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        let mut buffers = RowBuffers::default();
        if below_background {
            // A layer of its own, so cell backgrounds cover these pictures.
            window.paint_layer(bounds, |window| {
                paint_pictures(
                    &frame,
                    screen,
                    sys::GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND,
                    window,
                );
            });
        }
        if below_text {
            // Backgrounds, then the pictures that go between them and the text.
            window.paint_layer(bounds, |window| {
                window.with_content_mask(grid.rows_mask(), |window| {
                    for line in grid.shown_rows() {
                        paint_backgrounds(&frame, screen, line, background, &mut buffers, window);
                    }
                });
                paint_pictures(&frame, screen, sys::GHOSTTY_IMAGE_LAYER_BELOW_TEXT, window);
            });
        }
        // One layer: every quad is drawn before every glyph, in the order painted, which
        // is the stacking a terminal wants, and no primitive pays for overlap tracking.
        // Pictures above the text are textures, which GPUI draws after glyphs.
        window.paint_layer(bounds, |window| {
            window.with_content_mask(grid.rows_mask(), |window| {
                for line in grid.shown_rows() {
                    paint_row(
                        &frame,
                        screen,
                        line,
                        background,
                        below_text,
                        &mut buffers,
                        window,
                    );
                }
            });
            if let Some(corners) = frame.flight {
                let mut shape = PathBuilder::fill();
                shape.move_to(corners[0]);
                for corner in &corners[1..] {
                    shape.line_to(*corner);
                }
                shape.close();
                if let Ok(path) = shape.build() {
                    window.paint_path(path, screen.cursor_color);
                }
                // Another frame while it is in the air, at whatever rate the display has.
                window.request_animation_frame();
            }
            paint_pictures(&frame, screen, sys::GHOSTTY_IMAGE_LAYER_ABOVE_TEXT, window);
        });
        paint_composition(&frame, screen, background, window, cx);
    });
    drop(render);
    if frame.gliding {
        // The history is on its way somewhere: another frame, at the display's rate.
        window.request_animation_frame();
    }

    window.handle_input(
        &frame.focus,
        TerminalInputHandler {
            view: view.clone(),
            bounds,
        },
        cx,
    );
    let tracking = view.read(cx).terminal.read(cx).modes().mouse.tracking();
    let cursor_style = if tracking {
        CursorStyle::Arrow
    } else {
        CursorStyle::IBeam
    };
    window.set_cursor_style(cursor_style, &frame.hitbox);
    register_mouse(view, frame.hitbox, window);
}

/// Draws the pictures of one layer, each clipped to the part of the grid it covers.
fn paint_pictures(frame: &Frame, screen: &ScreenCopy, layer: std::ffi::c_int, window: &mut Window) {
    let grid = frame.grid;
    let area = grid.area();
    // Screen pixels per terminal pixel: the terminal measures pictures in device pixels
    // of the cell it was told about, the screen draws that cell at its zoomed size.
    let per_x = grid.cell.width.as_f32() / frame.cell_pixels.0.max(1.);
    let per_y = grid.cell.height.as_f32() / frame.cell_pixels.1.max(1.);
    for picture in screen
        .pictures
        .iter()
        .filter(|picture| picture.layer == layer)
    {
        let Some((visible, whole)) = picture_bounds(picture, grid, per_x, per_y, area) else {
            continue;
        };
        let parts = if layer == sys::GHOSTTY_IMAGE_LAYER_ABOVE_TEXT {
            uncovered_parts(screen, grid, visible)
        } else {
            vec![visible]
        };
        for part in parts {
            if let Err(error) = window.paint_image(
                part,
                whole,
                Corners::default(),
                picture.image.clone(),
                0,
                false,
            ) {
                log::debug!("painting a terminal picture failed: {error:#}");
            }
        }
    }
}

/// Whether a cell shows text of its own, which a picture drawn above the text leaves
/// uncovered: text written over a picture cuts it, as it would cut a picture made of
/// cells.
fn shows_text(cell: &CellCopy) -> bool {
    !cell.is_spacer()
        && cell.character != ' '
        && cell.character != PLACEHOLDER
        && !cell.character.is_control()
        && !cell.has(sys::GHOSTTY_STYLE_FLAG_INVISIBLE)
}

/// The parts of `visible` (a picture's rectangle on screen) not over a cell that shows
/// text, as few rectangles as possible: runs of free cells along each line, with lines
/// whose runs are the same merged. A picture over blank cells is one rectangle.
fn uncovered_parts(
    screen: &ScreenCopy,
    grid: Grid,
    visible: Bounds<Pixels>,
) -> Vec<Bounds<Pixels>> {
    let (first_column, first_line, _) = grid.cell_at(visible.origin);
    let (last_column, last_line, _) = grid.cell_at(point(
        visible.right() - px(0.01),
        visible.bottom() - px(0.01),
    ));
    let covered = |line: usize, column: usize| {
        screen.cell(line, column).is_some_and(|cell| {
            shows_text(cell)
                // The right half of a wide character belongs to its left half.
                || (cell.is_spacer()
                    && column > 0
                    && screen.cell(line, column - 1).is_some_and(shows_text))
        })
    };
    let any_text = (first_line..=last_line)
        .any(|line| (first_column..=last_column).any(|column| covered(line, column)));
    if !any_text {
        return vec![visible];
    }

    // Free runs of each line, as column ranges.
    let mut lines: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
    for line in first_line..=last_line {
        let mut runs = Vec::new();
        let mut start = None;
        for column in first_column..=last_column + 1 {
            let free = column <= last_column && !covered(line, column);
            match (free, start) {
                (true, None) => start = Some(column),
                (false, Some(from)) => {
                    runs.push((from, column));
                    start = None;
                }
                _ => {}
            }
        }
        lines.push((line, runs));
    }
    let mut parts = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let (top_line, runs) = &lines[index];
        let mut bottom_line = *top_line;
        while index + 1 < lines.len() && lines[index + 1].1 == *runs {
            index += 1;
            bottom_line = lines[index].0;
        }
        for &(start, end) in runs {
            let cells = Bounds::from_corners(
                point(grid.x(start), grid.y(*top_line)),
                point(grid.x(end), grid.y(bottom_line + 1)),
            );
            let part = cells.intersect(&visible);
            if part.size.width > px(0.) && part.size.height > px(0.) {
                parts.push(part);
            }
        }
        index += 1;
    }
    parts
}

/// Where a picture lands on screen: the part to draw (its placement clipped to the grid)
/// and where the whole texture would lie for that part to show the picture's source
/// rectangle.
fn picture_bounds(
    picture: &PictureCopy,
    grid: Grid,
    per_x: f32,
    per_y: f32,
    area: Bounds<Pixels>,
) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
    let left = grid.origin.x.as_f32()
        + grid.cell.width.as_f32() * picture.column as f32
        + picture.offset_x as f32 * per_x;
    let top = grid.origin.y.as_f32()
        + grid.cell.height.as_f32() * (picture.line as f32 + grid.fraction)
        + picture.offset_y as f32 * per_y;
    let width = picture.width as f32 * per_x;
    let height = picture.height as f32 * per_y;
    let placed = Bounds::new(point(px(left), px(top)), size(px(width), px(height)));
    let visible = placed.intersect(&area);
    if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
        return None;
    }
    let [source_x, source_y, source_width, source_height] =
        picture.source.map(|value| value as f32);
    let texture = picture.image.size(0);
    let scale = picture.texture_scale;
    // Screen pixels per picture pixel along each axis.
    let along_x = width / source_width;
    let along_y = height / source_height;
    let whole = Bounds::new(
        point(px(left - source_x * along_x), px(top - source_y * along_y)),
        size(
            px(texture.width.0 as f32 / scale * along_x),
            px(texture.height.0 as f32 / scale * along_y),
        ),
    );
    Some((visible, whole))
}

fn register_mouse(view: Entity<TerminalView>, hitbox: Hitbox, window: &mut Window) {
    window.on_mouse_event({
        let view = view.clone();
        let hitbox = hitbox.clone();
        move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                view.update(cx, |view, cx| view.mouse_down(event, window, cx));
            }
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        let hitbox = hitbox.clone();
        move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let hovered = hitbox.is_hovered(window);
                view.update(cx, |view, cx| view.mouse_move(event, hovered, cx));
            }
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |view, cx| view.mouse_up(event, cx));
            }
        }
    });
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
            view.update(cx, |view, cx| view.scroll(event, cx));
            cx.stop_propagation();
        }
    });
}

/// The cell backgrounds of one row, merged into spans.
fn background_spans(screen: &ScreenCopy, line: usize, background: Hsla, buffers: &mut RowBuffers) {
    buffers.backgrounds.clear();
    for (column, cell) in screen.row(line).iter().enumerate() {
        let (_, cell_background) = cell_colors(cell, screen);
        if cell_background != background {
            push_span(&mut buffers.backgrounds, column, cell_background);
        }
    }
}

fn paint_background_spans(frame: &Frame, line: usize, buffers: &RowBuffers, window: &mut Window) {
    let grid = frame.grid;
    for &(start, end, color) in &buffers.backgrounds {
        let bounds = Bounds::from_corners(
            point(grid.x(start), grid.y(line)),
            point(grid.x(end), grid.y(line + 1)),
        );
        window.paint_quad(fill(bounds, color));
    }
}

fn paint_backgrounds(
    frame: &Frame,
    screen: &ScreenCopy,
    line: usize,
    background: Hsla,
    buffers: &mut RowBuffers,
    window: &mut Window,
) {
    background_spans(screen, line, background, buffers);
    paint_background_spans(frame, line, buffers, window);
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    frame: &Frame,
    screen: &ScreenCopy,
    line: usize,
    background: Hsla,
    backgrounds_painted: bool,
    buffers: &mut RowBuffers,
    window: &mut Window,
) {
    let Frame {
        metrics,
        grid,
        style,
        ..
    } = frame;
    let zoom = grid.zoom;
    let columns = screen.columns;
    let cells = screen.row(line);
    let top = grid.y(line);
    let bottom = grid.y(line + 1);

    // A filled block cursor shows the character under it in the background colour.
    let block_cursor = match screen.cursor {
        Some(cursor)
            if cursor.line == line
                && cursor.shape == CursorShape::Block
                && frame.focused
                && frame.flight.is_none()
                && frame.cursor_lit > 0. =>
        {
            Some(cursor.column)
        }
        _ => None,
    };

    buffers.clear();
    let mut ink_end = 0;
    for (column, cell) in cells.iter().enumerate() {
        let (mut foreground, cell_background) = cell_colors(cell, screen);
        if !backgrounds_painted && cell_background != background {
            push_span(&mut buffers.backgrounds, column, cell_background);
        }
        if screen.selection[line].is_some_and(|(start, end)| (start..=end).contains(&column)) {
            push_span(&mut buffers.selected, column, ());
        }

        let spacer = cell.is_spacer();
        let under_cursor =
            block_cursor.is_some_and(|cursor| cursor == column || (spacer && cursor + 1 == column));
        if under_cursor {
            // As the block fades, the character under it takes its own colour back.
            foreground = style::mix(foreground, background, frame.cursor_lit);
        }
        buffers.foreground.push(foreground);
        buffers.wide.push(cell.is_wide());
        if spacer {
            buffers.blank.push(false);
            continue;
        }

        let hidden = cell.has(sys::GHOSTTY_STYLE_FLAG_INVISIBLE);
        if !hidden {
            let underline = match cell.underline() {
                sys::GHOSTTY_SGR_UNDERLINE_SINGLE => Some(Underline::Single),
                sys::GHOSTTY_SGR_UNDERLINE_DOUBLE => Some(Underline::Double),
                sys::GHOSTTY_SGR_UNDERLINE_CURLY => Some(Underline::Curly),
                sys::GHOSTTY_SGR_UNDERLINE_DOTTED => Some(Underline::Dotted),
                sys::GHOSTTY_SGR_UNDERLINE_DASHED => Some(Underline::Dashed),
                _ => None,
            };
            let span = if cell.is_wide() { 2 } else { 1 };
            if let Some(underline) = underline {
                let color = cell.underline_color.unwrap_or(foreground);
                for covered in column..(column + span).min(columns) {
                    push_span(&mut buffers.underlines, covered, (underline, color));
                }
            }
            if cell.has(sys::GHOSTTY_STYLE_FLAG_STRIKETHROUGH) {
                for covered in column..(column + span).min(columns) {
                    push_span(&mut buffers.strikes, covered, foreground);
                }
            }
        }

        let character = cell.character;
        let drawn = !hidden && boxdraw::is_drawn(character);
        if drawn {
            buffers.boxes.push((column, character));
        }
        // A picture's placeholder cell shows the picture, not a glyph.
        let empty =
            hidden || character == ' ' || character == PLACEHOLDER || character.is_control();
        buffers.blank.push(empty);
        // Drawn characters occupy their cell but contribute no glyph to shape.
        let blank = empty || drawn;
        let start = buffers.text.len();
        buffers.text.push(if blank { ' ' } else { character });
        if !blank {
            if let Some(marks) = &cell.extra {
                buffers.text.extend(marks.iter());
            }
            ink_end = buffers.text.len();
        }
        let added = buffers.text.len() - start;
        buffers
            .byte_columns
            .extend(std::iter::repeat_n(column, added));
        let variant = usize::from(cell.has(sys::GHOSTTY_STYLE_FLAG_BOLD))
            | usize::from(cell.has(sys::GHOSTTY_STYLE_FLAG_ITALIC)) << 1;
        match buffers.runs.last_mut() {
            Some((length, last)) if *last == variant => *length += added,
            _ => buffers.runs.push((added, variant)),
        }
    }

    // Quads, back to front: cell backgrounds, the selection, the cursor, drawn characters.
    paint_background_spans(frame, line, buffers, window);
    for &(start, end, ()) in &buffers.selected {
        let bounds = Bounds::from_corners(point(grid.x(start), top), point(grid.x(end), bottom));
        window.paint_quad(fill(bounds, style.selection));
    }
    if let Some(cursor) = screen.cursor
        && cursor.line == line
        && frame.flight.is_none()
    {
        let wide = buffers.wide.get(cursor.column).copied().unwrap_or(false);
        let end = cursor.column + if wide { 2 } else { 1 };
        let bounds = Bounds::from_corners(
            point(grid.x(cursor.column), top),
            point(grid.x(end), bottom),
        );
        paint_cursor(frame, screen, bounds, cursor.shape, window);
    }
    paint_boxes(frame, line, buffers, window);
    for &(start, end, (kind, color)) in &buffers.underlines {
        paint_decoration(
            frame,
            grid.x(start),
            grid.x(end) - grid.x(start),
            top,
            kind,
            color,
            window,
        );
    }
    for &(start, end, color) in &buffers.strikes {
        let width = grid.x(end) - grid.x(start);
        paint_decoration(
            frame,
            grid.x(start),
            width,
            top,
            Underline::Strike,
            color,
            window,
        );
    }

    // Blank cells at the end of the row need no shaping.
    if ink_end == 0 {
        return;
    }
    buffers.text.truncate(ink_end);
    let mut remaining = ink_end;
    let runs: Vec<TextRun> = buffers
        .runs
        .iter()
        .map_while(|&(length, variant)| {
            let length = length.min(remaining);
            remaining -= length;
            (length > 0).then(|| TextRun {
                len: length,
                font: metrics.fonts[variant].clone(),
                // Colour is applied per glyph below; keeping it out of the runs lets a
                // line that only changed colour reuse its cached layout.
                color: style.foreground,
                background_color: None,
                underline: None,
                strikethrough: None,
            })
        })
        .collect();
    // Shaped at the unzoomed size whatever the zoom, so a zoom gesture reuses the cached
    // layout of every line; positions are scaled below.
    let layout = window
        .text_system()
        .layout_line(&buffers.text, metrics.font_size, &runs, None);

    for run in &layout.runs {
        for glyph in &run.glyphs {
            let Some(&column) = buffers.byte_columns.get(glyph.index) else {
                continue;
            };
            buffers.glyphs.push(PlacedGlyph {
                font: run.font_id,
                id: glyph.id,
                x: glyph.position.x,
                y: glyph.position.y,
                column,
                emoji: glyph.is_emoji,
            });
        }
    }

    // The shaper's own advances are ignored: each cell's glyphs are placed relative to
    // the cell, so a wide or missing glyph cannot shift the rest of the row off the grid.
    let baseline = grid.snap(top + metrics.baseline * zoom);
    let cell_width = metrics.cell.width;
    let mut at = 0;
    while at < buffers.glyphs.len() {
        let column = buffers.glyphs[at].column;
        let mut next = at + 1;
        while next < buffers.glyphs.len() && buffers.glyphs[next].column == column {
            next += 1;
        }
        let start_x = buffers.glyphs[at].x;
        let end_x = buffers
            .glyphs
            .get(next)
            .map_or(layout.width, |glyph| glyph.x);
        let advance = end_x - start_x;
        let wide = buffers.wide[column];
        let own_cells = if wide { 2 } else { 1 };
        // The shaper may have merged the characters of the following cells into this
        // glyph (a flag, an emoji joined by zero-width joiners, a ligature): the cluster
        // then has all of those cells to draw in.
        let cells = buffers
            .glyphs
            .get(next)
            .map_or(own_cells, |glyph| glyph.column.saturating_sub(column))
            .max(own_cells);
        let room = cell_width * cells;
        let primary = metrics.font_ids.contains(&buffers.glyphs[at].font);
        // Glyphs from a fallback font, and wide ones, bring their own advance.
        let foreign = (!primary || wide) && advance > px(0.);
        // A symbol wider than its cells may spill into a blank that follows it, which is
        // how prompts and bullets are usually written; next to another character it is
        // scaled down to fit instead of overprinting it.
        let squeeze = if foreign
            && advance > room + px(0.5)
            && !buffers.blank.get(column + cells).copied().unwrap_or(false)
        {
            room / advance
        } else {
            1.
        };
        let font_size = raster_size(metrics.font_size * squeeze, zoom, metrics.scale);
        // Narrower than its own cell: centred in it.
        let own_width = cell_width * own_cells;
        let inset = if foreign && advance * squeeze < own_width {
            let free = (own_width - advance * squeeze).as_f32() / 2.;
            px((free * metrics.scale).round() / metrics.scale)
        } else {
            px(0.)
        };
        let color = buffers.foreground[column];
        for glyph in &buffers.glyphs[at..next] {
            let origin = point(
                grid.x(column) + (inset + (glyph.x - start_x) * squeeze) * zoom,
                baseline + glyph.y * squeeze * zoom,
            );
            let painted = if glyph.emoji {
                window.paint_emoji(origin, glyph.font, glyph.id, font_size)
            } else {
                window.paint_glyph(origin, glyph.font, glyph.id, font_size, color)
            };
            if let Err(error) = painted {
                log::debug!("painting a terminal glyph failed: {error:#}");
            }
        }
        at = next;
    }
}

fn paint_cursor(
    frame: &Frame,
    screen: &ScreenCopy,
    bounds: Bounds<Pixels>,
    shape: CursorShape,
    window: &mut Window,
) {
    let stroke = frame.metrics.stroke * frame.grid.zoom;
    if frame.cursor_lit <= 0. {
        return;
    }
    let color = screen.cursor_color.opacity(frame.cursor_lit);
    // An unfocused view shows where the cursor is without claiming to take input.
    let shape = if frame.focused {
        shape
    } else {
        CursorShape::HollowBlock
    };
    match shape {
        CursorShape::Block => window.paint_quad(fill(bounds, color)),
        CursorShape::Beam => {
            let width = (stroke * 2.).min(bounds.size.width);
            window.paint_quad(fill(
                Bounds::new(bounds.origin, size(width, bounds.size.height)),
                color,
            ));
        }
        CursorShape::Underline => {
            let height = stroke * 2.;
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x, bounds.bottom() - height),
                    size(bounds.size.width, height),
                ),
                color,
            ));
        }
        CursorShape::HollowBlock => window.paint_quad(quad(
            bounds,
            Corners::default(),
            transparent_black(),
            Edges::all(stroke),
            color,
            BorderStyle::Solid,
        )),
    }
}

fn paint_boxes(frame: &Frame, line: usize, buffers: &mut RowBuffers, window: &mut Window) {
    if buffers.boxes.is_empty() {
        return;
    }
    let grid = frame.grid;
    let scale = grid.scale;
    let device = |value: Pixels| (value.as_f32() * scale).round() as i32;
    let logical = |device: i32| px(device as f32 / scale);
    // Every cell edge is a whole device pixel (see `Grid::x`), so each cell is drawn in
    // its own whole-pixel box; at a zoom neighbouring cells may differ by a pixel.
    let origin_y = device(grid.y(line));
    let height = device(grid.y(line + 1)) - origin_y;

    for &(column, character) in &buffers.boxes {
        let x = device(grid.x(column));
        let width = device(grid.x(column + 1)) - x;
        if width <= 0 || height <= 0 {
            continue;
        }
        buffers.box_ops.clear();
        boxdraw::ops(character, width, height, &mut buffers.box_ops);
        let color = buffers.foreground[column];
        let cell = Bounds::from_corners(
            point(logical(x), logical(origin_y)),
            point(logical(x + width), logical(origin_y + height)),
        );
        for op in &buffers.box_ops {
            match *op {
                BoxOp::Rect {
                    x0,
                    y0,
                    x1,
                    y1,
                    alpha,
                } => {
                    let bounds = Bounds::from_corners(
                        point(logical(x + x0), logical(origin_y + y0)),
                        point(logical(x + x1), logical(origin_y + y1)),
                    );
                    window.paint_quad(fill(bounds, color.opacity(alpha)));
                }
                BoxOp::Arc {
                    right,
                    down,
                    thickness,
                } => paint_arc(
                    cell, x, origin_y, width, height, right, down, thickness, color, scale, window,
                ),
                BoxOp::Diagonal { rising, thickness } => {
                    let (from, to) = if rising {
                        (cell.bottom_left(), cell.top_right())
                    } else {
                        (cell.origin, cell.bottom_right())
                    };
                    let mut path = PathBuilder::stroke(logical(thickness));
                    path.move_to(from);
                    path.line_to(to);
                    if let Ok(path) = path.build() {
                        window.with_content_mask(Some(ContentMask { bounds: cell }), |window| {
                            window.paint_path(path, color);
                        });
                    }
                }
            }
        }
    }
}

/// A rounded corner: one quarter of a bordered, round-cornered quad whose other three
/// quarters lie outside the cell and are clipped away.
#[allow(clippy::too_many_arguments)]
fn paint_arc(
    cell: Bounds<Pixels>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    right: bool,
    down: bool,
    thickness: i32,
    color: Hsla,
    scale: f32,
    window: &mut Window,
) {
    let logical = |device: i32| px(device as f32 / scale);
    // The centred strokes the arc has to meet at the cell's edges.
    let stroke_x = (width - thickness) / 2;
    let stroke_y = (height - thickness) / 2;
    let reach_x = if right {
        width - stroke_x
    } else {
        stroke_x + thickness
    };
    let reach_y = if down {
        height - stroke_y
    } else {
        stroke_y + thickness
    };
    let radius = reach_x.min(reach_y);
    // Far enough past the cell that the quad's other corners and edges are never seen.
    let overshoot = width + height;
    let (x0, x1) = if right {
        (x + stroke_x, x + width + overshoot)
    } else {
        (x - overshoot, x + stroke_x + thickness)
    };
    let (y0, y1) = if down {
        (y + stroke_y, y + height + overshoot)
    } else {
        (y - overshoot, y + stroke_y + thickness)
    };
    let stroke = logical(thickness);
    let none = px(0.);
    let mut radii = Corners::default();
    match (right, down) {
        (true, true) => radii.top_left = logical(radius),
        (false, true) => radii.top_right = logical(radius),
        (false, false) => radii.bottom_right = logical(radius),
        (true, false) => radii.bottom_left = logical(radius),
    }
    let borders = Edges {
        top: if down { stroke } else { none },
        bottom: if down { none } else { stroke },
        left: if right { stroke } else { none },
        right: if right { none } else { stroke },
    };
    let bounds = Bounds::from_corners(
        point(logical(x0), logical(y0)),
        point(logical(x1), logical(y1)),
    );
    window.with_content_mask(Some(ContentMask { bounds: cell }), |window| {
        window.paint_quad(quad(
            bounds,
            radii,
            transparent_black(),
            borders,
            color,
            BorderStyle::Solid,
        ));
    });
}

fn paint_decoration(
    frame: &Frame,
    x: Pixels,
    width: Pixels,
    top: Pixels,
    kind: Underline,
    color: Hsla,
    window: &mut Window,
) {
    let metrics = &frame.metrics;
    let zoom = frame.grid.zoom;
    let stroke = metrics.stroke * zoom;
    let underline_top = frame
        .grid
        .snap(top + (metrics.baseline + metrics.underline_offset) * zoom);
    let bar = |y: Pixels, x: Pixels, width: Pixels| {
        fill(Bounds::new(point(x, y), size(width, stroke)), color)
    };
    match kind {
        Underline::Single => window.paint_quad(bar(underline_top, x, width)),
        Underline::Double => {
            window.paint_quad(bar(underline_top - stroke, x, width));
            window.paint_quad(bar(underline_top + stroke, x, width));
        }
        Underline::Curly => window.paint_underline(
            point(x, underline_top - stroke),
            width,
            &UnderlineStyle {
                thickness: stroke,
                color: Some(color),
                wavy: true,
            },
        ),
        Underline::Dotted | Underline::Dashed => {
            let dash = if kind == Underline::Dotted {
                stroke
            } else {
                stroke * 3.
            };
            if dash <= px(0.) {
                return;
            }
            let mut offset = px(0.);
            while offset < width {
                window.paint_quad(bar(underline_top, x + offset, dash.min(width - offset)));
                offset += dash * 2.;
            }
        }
        Underline::Strike => window.paint_quad(bar(
            frame.grid.snap(top + metrics.strike_offset * zoom),
            x,
            width,
        )),
    }
}

/// Text an input method is composing: drawn over the grid at the cursor, underlined,
/// until it is committed and goes to the program.
fn paint_composition(
    frame: &Frame,
    screen: &ScreenCopy,
    background: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(text) = frame.composing.as_ref() else {
        return;
    };
    let metrics = &frame.metrics;
    let grid = frame.grid;
    let (line, column) = screen
        .cursor
        .map_or((0, 0), |cursor| (cursor.line, cursor.column));
    let run = TextRun {
        len: text.len(),
        font: metrics.fonts[0].clone(),
        color: frame.style.foreground,
        background_color: None,
        underline: Some(UnderlineStyle {
            thickness: metrics.stroke * grid.zoom,
            color: Some(frame.style.foreground),
            wavy: false,
        }),
        strikethrough: None,
    };
    let font_size = raster_size(metrics.font_size, grid.zoom, metrics.scale);
    let shaped = window
        .text_system()
        .shape_line(text.clone().into(), font_size, &[run], None);
    // Keep the composition inside the view when the cursor is near the right edge.
    let x = grid
        .x(column)
        .min(grid.bounds.right() - shaped.width())
        .max(grid.bounds.left());
    let height = grid.y(line + 1) - grid.y(line);
    let origin = point(x, grid.y(line));
    window.paint_quad(fill(
        Bounds::new(origin, size(shaped.width(), height)),
        background,
    ));
    if let Err(error) = shaped.paint(origin, height, TextAlign::Left, None, window, cx) {
        log::debug!("painting the composition failed: {error:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_covers_the_cells_between_its_halves() {
        let cell = |column, row| (column, row);
        // From the right half of "b" (column 1) to the left half of "e" (column 4): "cd".
        assert_eq!(
            cell_span((cell(1, 0), Side::Right), (cell(4, 0), Side::Left), 10),
            Some((cell(2, 0), cell(3, 0)))
        );
        // Backwards gives the same.
        assert_eq!(
            cell_span((cell(4, 0), Side::Left), (cell(1, 0), Side::Right), 10),
            Some((cell(2, 0), cell(3, 0)))
        );
        // A click without a drag covers nothing; a drag across one cell covers it.
        assert_eq!(
            cell_span((cell(3, 0), Side::Left), (cell(3, 0), Side::Left), 10),
            None
        );
        assert_eq!(
            cell_span((cell(3, 0), Side::Left), (cell(3, 0), Side::Right), 10),
            Some((cell(3, 0), cell(3, 0)))
        );
        // Across a line break: from the last cell's right half to the next line.
        assert_eq!(
            cell_span((cell(9, 0), Side::Right), (cell(2, 1), Side::Right), 10),
            Some((cell(0, 1), cell(2, 1)))
        );
        assert_eq!(
            cell_span((cell(0, 0), Side::Left), (cell(0, 1), Side::Left), 10),
            Some((cell(0, 0), cell(9, 0)))
        );
    }

    #[test]
    fn glyph_sizes_are_exact_at_zoom_one_and_few_across_a_gesture() {
        let base = px(13.);
        assert_eq!(raster_size(base, 1., 2.), base);
        let mut sizes = Vec::new();
        let mut zoom = 0.1f32;
        while zoom <= 4. {
            let raster = raster_size(base, zoom, 1.);
            let wanted = 13. * zoom;
            let error = (raster.as_f32() - wanted).abs();
            assert!(
                error <= 0.25 + wanted * 0.021,
                "zoom {zoom}: {raster:?} for {wanted}"
            );
            sizes.push((raster.as_f32() * 1000.).round() as i32);
            // A gesture at 240 Hz moves the zoom by well under a percent a frame.
            zoom *= 1.002;
        }
        sizes.dedup();
        assert!(
            sizes.len() < 70,
            "{} distinct sizes over a sweep from 0.1 to 4",
            sizes.len()
        );
        // Never smaller for a larger zoom.
        assert!(sizes.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn cell_edges_are_whole_device_pixels_and_shared() {
        let grid = Grid {
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.))),
            origin: point(px(3.5), px(4.)),
            cell: size(px(8. * 0.37), px(18. * 0.37)),
            columns: 80,
            lines: 24,
            above: 0,
            fraction: 0.,
            back: 0.,
            zoom: 0.37,
            scale: 2.,
        };
        for column in 0..=80 {
            let edge = grid.x(column).as_f32() * 2.;
            assert_eq!(edge, edge.round());
            let exact = 3.5 + 8. * 0.37 * column as f32;
            assert!((grid.x(column).as_f32() - exact).abs() <= 0.25 + 1e-4);
        }
        assert_eq!(
            grid.cell_bounds(4, 2).right(),
            grid.cell_bounds(5, 2).left()
        );
        // The pointer maps back to the cell it is over, halves included.
        let middle = grid.cell_bounds(17, 9).center();
        assert_eq!(grid.cell_at(middle).0, 17);
        assert_eq!(grid.cell_at(middle).1, 9);
    }

    /// A grid of five rows of 10 by 20 pixels from (10, 10), with the row above the
    /// viewport, drawn `fraction` of a row lower than at rest.
    fn grid_between_rows(fraction: f32) -> Grid {
        Grid {
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(200.), px(120.))),
            origin: point(px(10.), px(10.)),
            cell: size(px(10.), px(20.)),
            columns: 18,
            lines: 5,
            above: 1,
            fraction,
            back: 3. + fraction,
            zoom: 1.,
            scale: 1.,
        }
    }

    #[test]
    fn rows_between_two_are_drawn_lower_with_the_row_above_in_the_gap() {
        let grid = grid_between_rows(0.25);
        // The viewport's first row starts a quarter of a row down, under the row above it.
        assert_eq!(grid.y(1), px(15.));
        assert_eq!(grid.y(0), px(-5.));
        assert_eq!(grid.shown_rows(), 0..6);
        assert!(grid.rows_mask().is_some());
        // The text area stays where it is.
        assert_eq!(
            grid.area(),
            Bounds::from_corners(point(px(10.), px(10.)), point(px(190.), px(110.)))
        );
        // The pointer finds the row drawn under it, the one above the viewport included,
        // and over the padding the nearest row in the text area.
        assert_eq!(grid.cell_at(point(px(15.), px(12.))).1, 0);
        assert_eq!(grid.viewport_line(0), -1);
        assert_eq!(grid.cell_at(point(px(15.), px(16.))).1, 1);
        assert_eq!(grid.cell_at(point(px(15.), px(108.))).1, 5);
        assert_eq!(grid.cell_at(point(px(15.), px(2.))).1, 0);

        // At rest the row above is out of sight and nothing is clipped.
        let rest = grid_between_rows(0.);
        assert_eq!(rest.y(1), px(10.));
        assert_eq!(rest.shown_rows(), 1..6);
        assert!(rest.rows_mask().is_none());
        assert_eq!(rest.cell_at(point(px(15.), px(2.))).1, 1);
        assert_eq!(rest.cell_at(point(px(15.), px(29.))).1, 1);
        assert_eq!(rest.cell_at(point(px(15.), px(31.))).1, 2);
    }

    #[test]
    fn the_history_glides_between_rows_and_rests_on_one() {
        let mut harness = crate::vt::tests::Harness::new(10, 4);
        let lines: Vec<String> = (0..30).map(|line| format!("L{line}")).collect();
        harness.write(lines.join("\r\n"));
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let smooth = SmoothScroll::default();
        let duration = smooth.duration;
        let mut scrollback = Scrollback::default();

        // A wheel step of three rows glides there, between rows on the way.
        let history = harness.vt.history_rows();
        scrollback.step(3., history, start, smooth);
        assert!(scrollback.advance(&mut harness.vt, at(30), duration));
        let partway = scrollback.display as f32 + scrollback.fraction;
        assert!(partway > 0.5 && partway < 3., "{partway}");
        assert!(scrollback.fraction > 0.);
        assert_eq!(harness.vt.display_offset(), partway.floor() as usize);
        assert!(!scrollback.advance(&mut harness.vt, at(200), duration));
        assert_eq!((harness.vt.display_offset(), scrollback.fraction), (3, 0.));

        // Output that arrives while the view is scrolled back keeps the rows in view, and a
        // glide on its way keeps its place among them.
        scrollback.step(2., history, at(200), smooth);
        scrollback.advance(&mut harness.vt, at(230), duration);
        let shown = harness.vt.display_offset();
        harness.write("\r\nM1\r\nM2");
        assert_eq!(harness.vt.display_offset(), shown + 2);
        assert!(!scrollback.advance(&mut harness.vt, at(400), duration));
        assert_eq!(harness.vt.display_offset(), 7);

        // A glide back to the live screen gets there however much output arrives on the way.
        scrollback.step(-7., harness.vt.history_rows(), at(400), smooth);
        scrollback.advance(&mut harness.vt, at(420), duration);
        harness.write("\r\nM3");
        // The next frame finds the output and goes on to the live screen from there.
        assert!(scrollback.advance(&mut harness.vt, at(424), duration));
        assert!(scrollback.glide.is_some_and(|glide| glide.target() == 0.));
        assert!(!scrollback.advance(&mut harness.vt, at(700), duration));
        assert_eq!((harness.vt.display_offset(), scrollback.fraction), (0, 0.));

        // A trackpad moves it by parts of rows at once, and once it stops it settles on the
        // nearer row.
        let history = harness.vt.history_rows();
        let wait = scrollback.trackpad(2.4, TouchPhase::Moved, history, at(700));
        assert_eq!(wait, SETTLE_AFTER);
        assert!(!scrollback.advance(&mut harness.vt, at(700), duration));
        assert_eq!(harness.vt.display_offset(), 2);
        assert!((scrollback.fraction - 0.4).abs() < 1e-4);
        assert!(scrollback.settle(at(800), smooth));
        assert!(!scrollback.advance(&mut harness.vt, at(1000), duration));
        assert_eq!((harness.vt.display_offset(), scrollback.fraction), (2, 0.));

        // With fingers down it waits longer, then settles all the same.
        let wait = scrollback.trackpad(0.5, TouchPhase::Started, history, at(1000));
        assert_eq!(wait, SETTLE_WHILE_TOUCHING);
        scrollback.advance(&mut harness.vt, at(1000), duration);
        assert!(scrollback.settle(at(1600), smooth));
        assert!(!scrollback.advance(&mut harness.vt, at(1800), duration));
        assert_eq!((harness.vt.display_offset(), scrollback.fraction), (3, 0.));

        // Switched off, a step is there at once.
        let off = SmoothScroll {
            enabled: false,
            ..SmoothScroll::default()
        };
        scrollback.step(3., history, at(1100), off);
        assert!(!scrollback.advance(&mut harness.vt, at(1100), duration));
        assert_eq!((harness.vt.display_offset(), scrollback.fraction), (6, 0.));
    }
}
