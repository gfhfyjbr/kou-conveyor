//! The emulator: a libghostty-vt terminal, the effects it reports while it parses, and
//! the queries the rest of the crate makes of it.
//!
//! A [`Vt`] lives behind a mutex shared by the reader thread, which feeds it, and the
//! interface thread, which reads it. Its callbacks run on the reader thread inside
//! [`Vt::write`] and only ever send messages; they never touch the mutex.

use crate::terminal::{Input, Internal, Shared};
use crate::{files::FileGuard, osc::OscEvent, style::TerminalStyle};
use ghostty_vt_sys as sys;
use std::{
    ffi::{c_int, c_void},
    path::PathBuf,
    ptr,
    sync::{Arc, Once, atomic::Ordering, mpsc},
};

/// Lines of history kept above the screen. libghostty-vt prunes whole pages, so a few
/// dozen more may stay.
pub(crate) const SCROLLBACK_LINES: usize = 10_000;

/// Image memory per screen for the Kitty graphics protocol: kitty's own default. The
/// library drops the oldest images first when a new one would not fit.
pub(crate) const IMAGE_STORAGE_LIMIT: u64 = 320_000_000;

/// The largest PNG side the decoder accepts; anything larger is refused before decoding.
const MAX_PNG_SIDE: u32 = 10_000;

/// The terminal's answer to XTVERSION.
const XTVERSION: &str = concat!("kou ", env!("CARGO_PKG_VERSION"));

/// Terminal modes the crate reads, by number.
pub(crate) mod mode {
    use ghostty_vt_sys::ghostty_mode;
    pub const CURSOR_KEYS: u16 = ghostty_mode(1, false);
    pub const MOUSE_X10: u16 = ghostty_mode(9, false);
    pub const MOUSE_NORMAL: u16 = ghostty_mode(1000, false);
    pub const MOUSE_BUTTON: u16 = ghostty_mode(1002, false);
    pub const MOUSE_ANY: u16 = ghostty_mode(1003, false);
    pub const FOCUS_EVENT: u16 = ghostty_mode(1004, false);
    pub const MOUSE_UTF8: u16 = ghostty_mode(1005, false);
    pub const MOUSE_SGR: u16 = ghostty_mode(1006, false);
    pub const ALTERNATE_SCROLL: u16 = ghostty_mode(1007, false);
    pub const BRACKETED_PASTE: u16 = ghostty_mode(2004, false);
}

/// The modes that decide how input is encoded, read in one go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Modes {
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub alt_screen: bool,
    pub alternate_scroll: bool,
    pub kitty_keyboard: u8,
    pub mouse: crate::mouse::MouseMode,
}

/// Where the effects of parsing go. Boxed inside [`Vt`] so its address, which the
/// library holds as the callbacks' userdata, never moves.
pub(crate) struct Effects {
    replies: mpsc::Sender<Input>,
    events: async_channel::Sender<Internal>,
    shared: Arc<Shared>,
}

impl Effects {
    fn send(&self, event: Internal) {
        let _ = self.events.try_send(event);
    }
}

/// A libghostty-vt terminal.
pub(crate) struct Vt {
    raw: sys::GhosttyTerminal,
    effects: Box<Effects>,
    /// Keeps commands that name a file the emulator cannot safely open away from it.
    files: FileGuard,
    filtered: Vec<u8>,
}

// SAFETY: the handle is only used through `&mut Vt` or `&Vt`, which the crate only ever
// reaches through a mutex; the library has no thread affinity.
unsafe impl Send for Vt {}

impl Drop for Vt {
    fn drop(&mut self) {
        // SAFETY: the handle came from `ghostty_terminal_new` and is freed once.
        unsafe { sys::ghostty_terminal_free(self.raw) };
    }
}

/// Points the library's process-wide PNG decoder at [`decode_png`]. The library asks for
/// this to happen before any terminal exists, which the `Once` in [`Vt::new`] ensures.
fn install_png_decoder() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let decoder: sys::GhosttySysDecodePngFn = decode_png;
        // SAFETY: a valid function pointer with the documented signature.
        unsafe { sys::ghostty_sys_set(sys::GHOSTTY_SYS_OPT_DECODE_PNG, decoder as *const c_void) };
    });
}

impl Vt {
    /// A terminal of `columns` by `lines` cells in `style`'s colours, reporting what it
    /// parses to `events` and writing its replies to `replies`.
    pub(crate) fn new(
        columns: u16,
        lines: u16,
        style: &TerminalStyle,
        replies: mpsc::Sender<Input>,
        events: async_channel::Sender<Internal>,
        shared: Arc<Shared>,
    ) -> anyhow::Result<Self> {
        install_png_decoder();
        let mut raw: sys::GhosttyTerminal = ptr::null_mut();
        // SAFETY: `raw` is a valid out pointer; a null allocator selects the default.
        let result = unsafe { sys::ghostty_terminal_new(ptr::null(), &mut raw, columns, lines) };
        anyhow::ensure!(
            result == sys::GHOSTTY_SUCCESS && !raw.is_null(),
            "libghostty-vt could not create a terminal ({result})"
        );
        let mut vt = Self {
            raw,
            effects: Box::new(Effects {
                replies,
                events,
                shared,
            }),
            files: FileGuard::default(),
            filtered: Vec::new(),
        };
        vt.configure();
        vt.set_colors(style);
        // The cursor blinks until a program asks for a steady one (DEC mode 12, the
        // default Ghostty itself and xterm's DECSCUSR 0 have); whether it is drawn
        // blinking is the view's to say.
        vt.write(b"\x1b[?12h");
        Ok(vt)
    }

    fn set(&mut self, option: c_int, value: *const c_void) {
        // SAFETY: each caller passes the value type its option documents.
        let result = unsafe { sys::ghostty_terminal_set(self.raw, option, value) };
        if result != sys::GHOSTTY_SUCCESS {
            log::warn!("libghostty-vt refused option {option}: {result}");
        }
    }

    fn configure(&mut self) {
        let userdata: *const Effects = &*self.effects;
        self.set(sys::GHOSTTY_TERMINAL_OPT_USERDATA, userdata.cast());
        macro_rules! callback {
            ($option:ident, $kind:ty, $function:expr) => {{
                let function: $kind = $function;
                self.set(sys::$option, function as *const c_void);
            }};
        }
        callback!(
            GHOSTTY_TERMINAL_OPT_WRITE_PTY,
            sys::GhosttyTerminalWritePtyFn,
            on_write_pty
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_BELL,
            sys::GhosttyTerminalBellFn,
            on_bell
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
            sys::GhosttyTerminalTitleChangedFn,
            on_title_changed
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
            sys::GhosttyTerminalPwdChangedFn,
            on_pwd_changed
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_RESET,
            sys::GhosttyTerminalResetFn,
            on_reset
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_SIZE,
            sys::GhosttyTerminalSizeFn,
            on_size
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
            sys::GhosttyTerminalColorSchemeFn,
            on_color_scheme
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_XTVERSION,
            sys::GhosttyTerminalXtversionFn,
            on_xtversion
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_RENDER_HOLD,
            sys::GhosttyTerminalRenderHoldFn,
            on_render_hold
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION,
            sys::GhosttyTerminalDesktopNotificationFn,
            on_notification
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT,
            sys::GhosttyTerminalProgressReportFn,
            on_progress
        );
        callback!(
            GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
            sys::GhosttyTerminalClipboardWriteFn,
            on_clipboard_write
        );

        let terminfo = b"xterm-256color";
        let terminfo = sys::GhosttyString {
            ptr: terminfo.as_ptr(),
            len: terminfo.len(),
        };
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_TERMINFO_NAME,
            (&raw const terminfo).cast(),
        );

        // The default is about one page of history; the line limit decides instead.
        let unlimited = usize::MAX;
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
            (&raw const unlimited).cast(),
        );
        let lines = SCROLLBACK_LINES;
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
            (&raw const lines).cast(),
        );

        // Pictures: kitty's quota, and every way of sending one a local program has. The
        // library refuses files under /proc, /sys and /dev (but /dev/shm), anything that
        // is not a regular file, and deletes temporary files only inside the temporary
        // directories.
        let limit = IMAGE_STORAGE_LIMIT;
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
            (&raw const limit).cast(),
        );
        let enabled = true;
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_FILE,
            (&raw const enabled).cast(),
        );
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM,
            (&raw const enabled).cast(),
        );
        let temp = std::env::temp_dir();
        let temp = temp.to_string_lossy();
        let temp = sys::GhosttyString {
            ptr: temp.as_ptr(),
            len: temp.len(),
        };
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE,
            (&raw const temp).cast(),
        );
    }

    /// Makes the style's colours the terminal's defaults; a program's own OSC 4, 10,
    /// 11 and 12 changes stay on top of them.
    pub(crate) fn set_colors(&mut self, style: &TerminalStyle) {
        let foreground = rgb(style.foreground);
        let background = rgb(style.background);
        let cursor = rgb(style.cursor);
        let mut palette = [sys::GhosttyColorRgb::default(); 256];
        for (index, color) in palette.iter_mut().enumerate() {
            *color = if index < 16 {
                rgb(style.ansi[index])
            } else {
                let (r, g, b) = crate::style::indexed_rgb_bytes(index as u8);
                sys::GhosttyColorRgb { r, g, b }
            };
        }
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
            (&raw const foreground).cast(),
        );
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
            (&raw const background).cast(),
        );
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
            (&raw const cursor).cast(),
        );
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
            palette.as_ptr().cast(),
        );
        self.effects
            .shared
            .dark
            .store(crate::style::is_dark(style.background), Ordering::Relaxed);
    }

    /// The raw handle, for the render state and the image storage.
    pub(crate) fn raw(&self) -> sys::GhosttyTerminal {
        self.raw
    }

    /// Feeds output of the program to the parser.
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        let mut filtered = std::mem::take(&mut self.filtered);
        let bytes = self.files.filter(bytes, &mut filtered);
        if !bytes.is_empty() {
            // SAFETY: a valid handle and a byte slice that outlives the call.
            unsafe { sys::ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len()) };
        }
        self.filtered = filtered;
    }

    /// Resizes the grid, recording the size of a cell in device pixels for pictures and
    /// size reports.
    pub(crate) fn resize(&mut self, columns: u16, lines: u16, cell_width: u32, cell_height: u32) {
        // SAFETY: a valid handle; the sizes are checked by the caller to be non-zero.
        let result = unsafe {
            sys::ghostty_terminal_resize(self.raw, columns, lines, cell_width, cell_height)
        };
        if result != sys::GHOSTTY_SUCCESS {
            log::warn!("libghostty-vt could not resize to {columns}x{lines}: {result}");
        }
    }

    fn get<T: Default>(&self, data: c_int) -> Option<T> {
        let mut value = T::default();
        // SAFETY: each caller names the output type its data key documents.
        let result = unsafe { sys::ghostty_terminal_get(self.raw, data, (&raw mut value).cast()) };
        (result == sys::GHOSTTY_SUCCESS).then_some(value)
    }

    /// The grid size as `(columns, lines)`.
    pub(crate) fn size(&self) -> (usize, usize) {
        let columns: u16 = self.get(sys::GHOSTTY_TERMINAL_DATA_COLS).unwrap_or(0);
        let lines: u16 = self.get(sys::GHOSTTY_TERMINAL_DATA_ROWS).unwrap_or(0);
        (usize::from(columns), usize::from(lines))
    }

    /// The cursor's column and line in the active area.
    #[cfg(test)]
    pub(crate) fn cursor(&self) -> (usize, usize) {
        let x: u16 = self.get(sys::GHOSTTY_TERMINAL_DATA_CURSOR_X).unwrap_or(0);
        let y: u16 = self.get(sys::GHOSTTY_TERMINAL_DATA_CURSOR_Y).unwrap_or(0);
        (usize::from(x), usize::from(y))
    }

    /// Whether a mode is set.
    pub(crate) fn mode(&self, mode: u16) -> bool {
        let mut config = sys::GhosttyTerminalModeConfig { mode, value: false };
        // SAFETY: the documented in/out type for this data key.
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.raw,
                sys::GHOSTTY_TERMINAL_DATA_MODE,
                (&raw mut config).cast(),
            )
        };
        result == sys::GHOSTTY_SUCCESS && config.value
    }

    /// Whether the alternate screen is showing.
    pub(crate) fn alt_screen(&self) -> bool {
        self.get::<c_int>(sys::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN)
            == Some(sys::GHOSTTY_TERMINAL_SCREEN_ALTERNATE)
    }

    /// The modes that decide how keys, the mouse and pastes are encoded.
    pub(crate) fn modes(&self) -> Modes {
        let any = self.mode(mode::MOUSE_ANY);
        let button = self.mode(mode::MOUSE_BUTTON);
        let normal = self.mode(mode::MOUSE_NORMAL) || self.mode(mode::MOUSE_X10);
        Modes {
            app_cursor: self.mode(mode::CURSOR_KEYS),
            bracketed_paste: self.mode(mode::BRACKETED_PASTE),
            focus_events: self.mode(mode::FOCUS_EVENT),
            alt_screen: self.alt_screen(),
            alternate_scroll: self.mode(mode::ALTERNATE_SCROLL),
            kitty_keyboard: self
                .get::<u8>(sys::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS)
                .unwrap_or(0),
            mouse: crate::mouse::MouseMode {
                click: normal || button || any,
                drag: button || any,
                motion: any,
                sgr: self.mode(mode::MOUSE_SGR),
                utf8: self.mode(mode::MOUSE_UTF8),
            },
        }
    }

    /// How far the view is scrolled back into the history, in rows.
    pub(crate) fn display_offset(&self) -> usize {
        let bar: sys::GhosttyTerminalScrollbar = self
            .get(sys::GHOSTTY_TERMINAL_DATA_SCROLLBAR)
            .unwrap_or_default();
        bar.total.saturating_sub(bar.offset + bar.len) as usize
    }

    /// How far back the view can scroll: the rows of history above the screen.
    pub(crate) fn history_rows(&self) -> usize {
        let bar: sys::GhosttyTerminalScrollbar = self
            .get(sys::GHOSTTY_TERMINAL_DATA_SCROLLBAR)
            .unwrap_or_default();
        bar.total.saturating_sub(bar.len) as usize
    }

    /// Scrolls the view by `lines` rows of history; positive is towards older output.
    pub(crate) fn scroll(&mut self, lines: isize) {
        self.scroll_viewport(sys::GhosttyTerminalScrollViewport {
            tag: sys::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA,
            value: sys::GhosttyTerminalScrollViewportValue { delta: -lines },
        });
    }

    /// Scrolls the view to the oldest line of history.
    pub(crate) fn scroll_to_top(&mut self) {
        self.scroll_viewport(sys::GhosttyTerminalScrollViewport {
            tag: sys::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP,
            value: sys::GhosttyTerminalScrollViewportValue { row: 0 },
        });
    }

    /// Returns the view to the live screen.
    pub(crate) fn scroll_to_bottom(&mut self) {
        self.scroll_viewport(sys::GhosttyTerminalScrollViewport {
            tag: sys::GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM,
            value: sys::GhosttyTerminalScrollViewportValue { row: 0 },
        });
    }

    fn scroll_viewport(&mut self, behavior: sys::GhosttyTerminalScrollViewport) {
        // SAFETY: a valid handle and a fully initialised behaviour.
        unsafe { sys::ghostty_terminal_scroll_viewport(self.raw, behavior) };
    }

    /// The cell at a viewport position, for selections.
    pub(crate) fn viewport_ref(&self, column: usize, line: usize) -> Option<sys::GhosttyGridRef> {
        let (columns, lines) = self.size();
        let point = point(
            sys::GHOSTTY_POINT_TAG_VIEWPORT,
            column.min(columns.saturating_sub(1)),
            line.min(lines.saturating_sub(1)),
        );
        self.grid_ref(point)
    }

    fn grid_ref(&self, point: sys::GhosttyPoint) -> Option<sys::GhosttyGridRef> {
        let mut out = sys::GhosttyGridRef::default();
        // SAFETY: a valid handle and out pointer.
        let result = unsafe { sys::ghostty_terminal_grid_ref(self.raw, point, &mut out) };
        (result == sys::GHOSTTY_SUCCESS).then_some(out)
    }

    /// A position in the whole history (row 0 is the oldest line) that stays put while
    /// the view scrolls, for anchoring a selection.
    pub(crate) fn screen_point(&self, cell: &sys::GhosttyGridRef) -> Option<(usize, usize)> {
        let mut out = sys::GhosttyPointCoordinate::default();
        // SAFETY: a ref from this terminal, used before any mutation.
        let result = unsafe {
            sys::ghostty_terminal_point_from_grid_ref(
                self.raw,
                cell,
                sys::GHOSTTY_POINT_TAG_SCREEN,
                &mut out,
            )
        };
        (result == sys::GHOSTTY_SUCCESS).then_some((usize::from(out.x), out.y as usize))
    }

    /// The cell at a position in the whole history.
    pub(crate) fn screen_ref(&self, column: usize, row: usize) -> Option<sys::GhosttyGridRef> {
        self.grid_ref(point(sys::GHOSTTY_POINT_TAG_SCREEN, column, row))
    }

    /// Replaces the selection; `None` clears it.
    pub(crate) fn set_selection(&mut self, selection: Option<sys::GhosttySelection>) {
        match selection {
            Some(selection) => self.set(
                sys::GHOSTTY_TERMINAL_OPT_SELECTION,
                (&raw const selection).cast(),
            ),
            None => self.set(sys::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::null()),
        }
    }

    /// The current selection, if there is one.
    pub(crate) fn selection(&self) -> Option<sys::GhosttySelection> {
        self.get(sys::GHOSTTY_TERMINAL_DATA_SELECTION)
    }

    /// The words from one cell to another, whole words at both ends.
    pub(crate) fn words_between(
        &self,
        start: sys::GhosttyGridRef,
        end: sys::GhosttyGridRef,
    ) -> Option<sys::GhosttySelection> {
        let options = sys::GhosttyTerminalSelectWordBetweenOptions {
            size: size_of::<sys::GhosttyTerminalSelectWordBetweenOptions>(),
            start,
            end,
            boundary_codepoints: ptr::null(),
            boundary_codepoints_len: 0,
        };
        let mut out = sys::GhosttySelection::default();
        // SAFETY: valid options and out pointer.
        let result =
            unsafe { sys::ghostty_terminal_select_word_between(self.raw, &options, &mut out) };
        (result == sys::GHOSTTY_SUCCESS).then_some(out)
    }

    /// The line around a cell, following soft wraps.
    pub(crate) fn line_at(&self, cell: sys::GhosttyGridRef) -> Option<sys::GhosttySelection> {
        let options = sys::GhosttyTerminalSelectLineOptions {
            size: size_of::<sys::GhosttyTerminalSelectLineOptions>(),
            r#ref: cell,
            whitespace: ptr::null(),
            whitespace_len: 0,
            semantic_prompt_boundary: false,
        };
        let mut out = sys::GhosttySelection::default();
        // SAFETY: valid options and out pointer.
        let result = unsafe { sys::ghostty_terminal_select_line(self.raw, &options, &mut out) };
        (result == sys::GHOSTTY_SUCCESS).then_some(out)
    }

    /// Everything: the history and the screen.
    pub(crate) fn everything(&self) -> Option<sys::GhosttySelection> {
        let mut out = sys::GhosttySelection::default();
        // SAFETY: a valid out pointer.
        let result = unsafe { sys::ghostty_terminal_select_all(self.raw, &mut out) };
        (result == sys::GHOSTTY_SUCCESS).then_some(out)
    }

    /// The text of a selection, or of the whole history and screen when `None`: one
    /// line per row (or per logical line when `unwrap`), trailing blanks removed.
    fn format(&self, selection: Option<sys::GhosttySelection>, unwrap: bool) -> String {
        let mut formatter: sys::GhosttyFormatter = ptr::null_mut();
        // SAFETY: plain calls on a valid handle; the formatter and the buffer it returns
        // are freed before returning.
        unsafe {
            if sys::ghostty_formatter_terminal_new(ptr::null(), &mut formatter, self.raw)
                != sys::GHOSTTY_SUCCESS
                || formatter.is_null()
            {
                return String::new();
            }
            let plain: c_int = sys::GHOSTTY_FORMATTER_FORMAT_PLAIN;
            sys::ghostty_formatter_set(
                formatter,
                sys::GHOSTTY_FORMATTER_OPT_EMIT,
                (&raw const plain).cast(),
            );
            sys::ghostty_formatter_set(
                formatter,
                sys::GHOSTTY_FORMATTER_OPT_UNWRAP,
                (&raw const unwrap).cast(),
            );
            let trim = true;
            sys::ghostty_formatter_set(
                formatter,
                sys::GHOSTTY_FORMATTER_OPT_TRIM,
                (&raw const trim).cast(),
            );
            if let Some(selection) = selection.as_ref() {
                sys::ghostty_formatter_set(
                    formatter,
                    sys::GHOSTTY_FORMATTER_OPT_SELECTION,
                    (selection as *const sys::GhosttySelection).cast(),
                );
            }
            let mut bytes: *mut u8 = ptr::null_mut();
            let mut length = 0usize;
            let result = sys::ghostty_formatter_format_alloc(
                formatter,
                ptr::null(),
                &mut bytes,
                &mut length,
            );
            let text = if result == sys::GHOSTTY_SUCCESS && !bytes.is_null() {
                let text =
                    String::from_utf8_lossy(std::slice::from_raw_parts(bytes, length)).into_owned();
                sys::ghostty_free(ptr::null(), bytes, length);
                text
            } else {
                String::new()
            };
            sys::ghostty_formatter_free(formatter);
            without_placeholders(&text)
        }
    }

    /// The screen as text, one line per row, as [`crate::Terminal::text`] describes.
    pub(crate) fn text(&self, lines: Option<usize>, scrollback: bool) -> String {
        let selection = if scrollback {
            None
        } else {
            let (columns, rows) = self.size();
            let start = self.grid_ref(point(sys::GHOSTTY_POINT_TAG_ACTIVE, 0, 0));
            let end = self.grid_ref(point(
                sys::GHOSTTY_POINT_TAG_ACTIVE,
                columns.saturating_sub(1),
                rows.saturating_sub(1),
            ));
            match (start, end) {
                (Some(start), Some(end)) => Some(sys::GhosttySelection {
                    start,
                    end,
                    rectangle: false,
                }),
                _ => return String::new(),
            }
        };
        let text = self.format(selection, false);
        let mut rows: Vec<&str> = text.split('\n').map(|row| row.trim_end()).collect();
        while rows.last().is_some_and(|row| row.is_empty()) {
            rows.pop();
        }
        if let Some(lines) = lines {
            let skip = rows.len().saturating_sub(lines);
            rows.drain(..skip);
        }
        rows.join("\n")
    }

    /// The selected text the way it is copied: soft-wrapped lines joined, trailing
    /// blanks removed. `None` when nothing is selected.
    pub(crate) fn selection_text(&self) -> Option<String> {
        let selection = self.selection()?;
        let text = self.format(Some(selection), true);
        (!text.is_empty()).then_some(text)
    }
}

fn point(tag: c_int, column: usize, row: usize) -> sys::GhosttyPoint {
    sys::GhosttyPoint {
        tag,
        value: sys::GhosttyPointValue {
            coordinate: sys::GhosttyPointCoordinate {
                x: column.min(u16::MAX as usize) as u16,
                y: row.min(u32::MAX as usize) as u32,
            },
        },
    }
}

fn rgb(color: gpui::Hsla) -> sys::GhosttyColorRgb {
    let (r, g, b) = crate::style::rgb_bytes(color);
    sys::GhosttyColorRgb { r, g, b }
}

/// Removes the Unicode placeholders of Kitty pictures (U+10EEEE and the diacritics that
/// number its row and column) from text read out of the terminal: they are bookkeeping,
/// not text. The cell they stood in reads as a space.
pub(crate) fn without_placeholders(text: &str) -> String {
    const PLACEHOLDER: char = '\u{10EEEE}';
    if !text.contains(PLACEHOLDER) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character != PLACEHOLDER {
            out.push(character);
            continue;
        }
        out.push(' ');
        while chars.peek().is_some_and(|next| is_combining(*next)) {
            chars.next();
        }
    }
    out
}

/// Combining marks, the alphabet the placeholder protocol numbers rows and columns in.
fn is_combining(character: char) -> bool {
    matches!(
        u32::from(character),
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05C7
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06ED
            | 0x0711
            | 0x0730..=0x074A
            | 0x07EB..=0x07F3
            | 0x0816..=0x082D
            | 0x0951..=0x0954
            | 0x0F82..=0x0F87
            | 0x135D..=0x135F
            | 0x17DD
            | 0x193A
            | 0x1A17..=0x1A18
            | 0x1A75..=0x1A7C
            | 0x1B6B..=0x1B73
            | 0x1CD0..=0x1CF9
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20F0
            | 0x2CEF..=0x2CF1
            | 0x2DE0..=0x2DFF
            | 0xA66F..=0xA67D
            | 0xA6F0..=0xA6F1
            | 0xA8E0..=0xA8F1
            | 0xAAB0..=0xAABE
            | 0xAAC1
            | 0xFE20..=0xFE2F
            | 0x10A0F
            | 0x10A38
            | 0x1D185..=0x1D1AD
            | 0x1D242..=0x1D244
    )
}

/// The effects behind a callback's userdata.
///
/// # Safety
///
/// `userdata` must be the pointer [`Vt::configure`] registered, whose `Effects` lives as
/// long as the terminal calling back.
unsafe fn effects<'a>(userdata: *mut c_void) -> &'a Effects {
    // SAFETY: guaranteed by the caller.
    unsafe { &*userdata.cast::<Effects>() }
}

/// Borrowed bytes from the library as a string.
///
/// # Safety
///
/// `value` must point at `len` readable bytes, or be null.
unsafe fn string(value: sys::GhosttyString) -> String {
    if value.ptr.is_null() || value.len == 0 {
        return String::new();
    }
    // SAFETY: guaranteed by the caller.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(value.ptr, value.len) })
        .into_owned()
}

unsafe extern "C" fn on_write_pty(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if data.is_null() || len == 0 {
        return;
    }
    // SAFETY: the library passes `len` readable bytes and our own userdata.
    let (effects, bytes) = unsafe { (effects(userdata), std::slice::from_raw_parts(data, len)) };
    // Replies queue behind whatever was typed before them, as they would on a wire.
    let _ = effects.replies.send(Input::Bytes(bytes.to_vec()));
}

unsafe extern "C" fn on_bell(_: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: our own userdata.
    unsafe { effects(userdata) }.send(Internal::Bell);
}

unsafe extern "C" fn on_title_changed(terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    let mut value = sys::GhosttyString {
        ptr: ptr::null(),
        len: 0,
    };
    // SAFETY: reading from the terminal that called back is allowed; the borrowed title
    // is copied at once.
    let title = unsafe {
        sys::ghostty_terminal_get(
            terminal,
            sys::GHOSTTY_TERMINAL_DATA_TITLE,
            (&raw mut value).cast(),
        );
        string(value)
    };
    // SAFETY: our own userdata.
    unsafe { effects(userdata) }.send(Internal::Title((!title.is_empty()).then_some(title)));
}

unsafe extern "C" fn on_pwd_changed(terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    let mut value = sys::GhosttyString {
        ptr: ptr::null(),
        len: 0,
    };
    // SAFETY: as in `on_title_changed`.
    let pwd = unsafe {
        sys::ghostty_terminal_get(
            terminal,
            sys::GHOSTTY_TERMINAL_DATA_PWD,
            (&raw mut value).cast(),
        );
        string(value)
    };
    // OSC 7 carries a URI; OSC 9;9 and OSC 1337 CurrentDir a bare path.
    let path = crate::osc::path_from_uri(&pwd)
        .or_else(|| (!pwd.is_empty()).then(|| PathBuf::from(pwd.trim_matches('"'))));
    if let Some(path) = path {
        // SAFETY: our own userdata.
        unsafe { effects(userdata) }.send(Internal::Osc(OscEvent::Cwd(path)));
    }
}

unsafe extern "C" fn on_reset(_: sys::GhosttyTerminal, userdata: *mut c_void) {
    // A full reset clears the title without calling `on_title_changed`.
    // SAFETY: our own userdata.
    unsafe { effects(userdata) }.send(Internal::Title(None));
}

unsafe extern "C" fn on_size(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttySizeReportSize,
) -> bool {
    // SAFETY: our own userdata and the library's out pointer.
    let (effects, out) = unsafe { (effects(userdata), &mut *out) };
    let size = *effects.shared.size.lock();
    *out = sys::GhosttySizeReportSize {
        rows: size.lines,
        columns: size.columns,
        cell_width: u32::from(size.cell_width),
        cell_height: u32::from(size.cell_height),
    };
    true
}

unsafe extern "C" fn on_color_scheme(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut c_int,
) -> bool {
    // SAFETY: our own userdata and the library's out pointer.
    unsafe {
        let dark = effects(userdata).shared.dark.load(Ordering::Relaxed);
        *out = if dark {
            sys::GHOSTTY_COLOR_SCHEME_DARK
        } else {
            sys::GHOSTTY_COLOR_SCHEME_LIGHT
        };
    }
    true
}

unsafe extern "C" fn on_xtversion(_: sys::GhosttyTerminal, _: *mut c_void) -> sys::GhosttyString {
    sys::GhosttyString {
        ptr: XTVERSION.as_ptr(),
        len: XTVERSION.len(),
    }
}

unsafe extern "C" fn on_render_hold(_: sys::GhosttyTerminal, userdata: *mut c_void, held: bool) {
    // SAFETY: our own userdata.
    unsafe { effects(userdata) }.shared.set_render_hold(held);
}

unsafe extern "C" fn on_notification(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    notification: *const sys::GhosttyTerminalDesktopNotification,
) {
    // SAFETY: the library's borrowed request, copied at once, and our own userdata.
    let (title, body, effects) = unsafe {
        let notification = &*notification;
        (
            string(notification.title),
            string(notification.body),
            effects(userdata),
        )
    };
    crate::osc::emit_notification(&title, &body, &mut |event| {
        effects.send(Internal::Osc(event));
    });
}

unsafe extern "C" fn on_progress(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    report: *const sys::GhosttyTerminalProgressReport,
) {
    // SAFETY: the library's borrowed report and our own userdata.
    let (report, effects) = unsafe { (*report, effects(userdata)) };
    let value = u8::try_from(report.progress)
        .ok()
        .map(|value| value.min(100));
    let progress = match report.state {
        sys::GHOSTTY_TERMINAL_PROGRESS_STATE_REMOVE => None,
        // Busy, extent unknown: there is no percentage to report.
        sys::GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE => Some(0),
        _ => Some(value.unwrap_or(0)),
    };
    effects.send(Internal::Osc(OscEvent::Progress(progress)));
}

unsafe extern "C" fn on_clipboard_write(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const sys::GhosttyClipboardWrite,
) {
    // SAFETY: the library's borrowed request, valid for this call, and our own userdata.
    unsafe {
        let write = &*write;
        let contents = if write.contents.is_null() {
            &[][..]
        } else {
            std::slice::from_raw_parts(write.contents, write.contents_len)
        };
        let text = contents.iter().find_map(|content| {
            let mime = string(content.mime);
            (mime.is_empty() || mime.starts_with("text/plain")).then(|| string(content.data))
        });
        let result = match text {
            Some(text) => {
                effects(userdata).send(Internal::Clipboard(text));
                sys::GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS
            }
            None => sys::GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED,
        };
        if let Some(reply) = write.reply {
            let answer = sys::GhosttyClipboardWriteReply {
                size: size_of::<sys::GhosttyClipboardWriteReply>(),
                result,
                remember: false,
            };
            reply(write, &answer);
        }
    }
}

/// Decodes a PNG sent with the Kitty graphics protocol into RGBA pixels in memory from
/// the library's allocator, which then owns them. Refuses absurd sizes before decoding.
unsafe extern "C" fn decode_png(
    _: *mut c_void,
    allocator: *const sys::GhosttyAllocator,
    data: *const u8,
    data_len: usize,
    out: *mut sys::GhosttySysImage,
) -> bool {
    if data.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: the library passes `data_len` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data, data_len) };
    let Some((width, height, pixels)) = decode_rgba(bytes) else {
        return false;
    };
    // SAFETY: memory from the library's allocator, filled and handed over whole.
    unsafe {
        let buffer = sys::ghostty_alloc(allocator, pixels.len());
        if buffer.is_null() {
            return false;
        }
        ptr::copy_nonoverlapping(pixels.as_ptr(), buffer, pixels.len());
        *out = sys::GhosttySysImage {
            width,
            height,
            data: buffer,
            data_len: pixels.len(),
        };
    }
    true
}

/// A PNG as width, height and RGBA bytes, or `None` when it is not one or is too big.
pub(crate) fn decode_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    use image::{ImageDecoder as _, ImageFormat, ImageReader, Limits};
    let mut reader = ImageReader::with_format(std::io::Cursor::new(bytes), ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_PNG_SIDE);
    limits.max_image_height = Some(MAX_PNG_SIDE);
    limits.max_alloc = Some(IMAGE_STORAGE_LIMIT / 2);
    reader.limits(limits);
    let decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let image = image::DynamicImage::from_decoder(decoder).ok()?;
    Some((width, height, image.into_rgba8().into_raw()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        render::RenderState,
        terminal::{GridPixels, Shared},
    };
    use std::{ffi::CString, io::Write as _};

    /// A terminal with nothing on the other side: what it answers lands in `replies`.
    pub(crate) struct Harness {
        pub vt: Vt,
        replies: mpsc::Receiver<Input>,
        /// Kept so the callbacks' channel stays open.
        _events: async_channel::Receiver<Internal>,
    }

    impl Harness {
        pub(crate) fn new(columns: u16, lines: u16) -> Self {
            let (replies_tx, replies) = mpsc::channel();
            let (events_tx, events) = async_channel::unbounded();
            let shared = Arc::new(Shared::new(GridPixels {
                columns,
                lines,
                cell_width: 10,
                cell_height: 20,
            }));
            let mut vt = Vt::new(
                columns,
                lines,
                &TerminalStyle::default(),
                replies_tx,
                events_tx,
                shared,
            )
            .expect("a terminal");
            vt.resize(columns, lines, 10, 20);
            Self {
                vt,
                replies,
                _events: events,
            }
        }

        pub(crate) fn write(&mut self, output: impl AsRef<[u8]>) -> &mut Self {
            self.vt.write(output.as_ref());
            self
        }

        /// Everything answered since the last call.
        pub(crate) fn replies(&self) -> String {
            let mut out = Vec::new();
            for input in self.replies.try_iter() {
                if let Input::Bytes(bytes) = input {
                    out.extend(bytes);
                }
            }
            String::from_utf8_lossy(&out).into_owned()
        }

        pub(crate) fn pictures(&self) -> Vec<(i32, i32, u32, u32)> {
            let mut render = RenderState::new().expect("a render state");
            render.update(&self.vt);
            render
                .copy()
                .pictures
                .iter()
                .map(|picture| (picture.column, picture.line, picture.width, picture.height))
                .collect()
        }
    }

    pub(crate) fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let value = chunk.iter().enumerate().fold(0u32, |value, (index, byte)| {
                value | u32::from(*byte) << (16 - 8 * index)
            });
            for index in 0..4 {
                if index <= chunk.len() {
                    out.push(ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A PNG of the given size, red with a blue first row.
    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |_, y| {
            if y == 0 {
                image::Rgba([0, 0, 255, 255])
            } else {
                image::Rgba([255, 0, 0, 255])
            }
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode a PNG");
        bytes.into_inner()
    }

    /// A zlib stream holding `data` in stored (uncompressed) deflate blocks.
    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let chunks: Vec<&[u8]> = data.chunks(65_535).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            out.push(u8::from(index + 1 == chunks.len()));
            let length = chunk.len() as u16;
            out.extend_from_slice(&length.to_le_bytes());
            out.extend_from_slice(&(!length).to_le_bytes());
            out.extend_from_slice(chunk);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for byte in data {
            a = (a + u32::from(*byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        out.extend_from_slice(&((b << 16) | a).to_be_bytes());
        out
    }

    fn command(keys: &str, payload: &[u8]) -> String {
        format!("\x1b_G{keys};{}\x1b\\", base64(payload))
    }

    fn ok(id: u32) -> String {
        format!("\x1b_Gi={id};OK\x1b\\")
    }

    #[test]
    fn text_trims_blanks_and_empty_rows() {
        let mut terminal = Harness::new(20, 5);
        terminal.write("one  \r\n\r\n  three\r\n");
        assert_eq!(terminal.vt.text(None, false), "one\n\n  three");
    }

    #[test]
    fn text_keeps_the_last_lines() {
        let mut terminal = Harness::new(20, 5);
        terminal.write("a\r\nb\r\nc\r\nd");
        assert_eq!(terminal.vt.text(Some(2), false), "c\nd");
        assert_eq!(terminal.vt.text(Some(10), false), "a\nb\nc\nd");
        assert_eq!(terminal.vt.text(Some(0), false), "");
    }

    #[test]
    fn text_with_and_without_scrollback() {
        let mut terminal = Harness::new(20, 4);
        let output: String = (1..=8).map(|line| format!("line {line}\r\n")).collect();
        terminal.write(output);
        assert_eq!(terminal.vt.text(None, false), "line 6\nline 7\nline 8");
        assert_eq!(
            terminal.vt.text(None, true),
            "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8"
        );
        assert_eq!(
            terminal.vt.text(Some(4), true),
            "line 5\nline 6\nline 7\nline 8"
        );
    }

    #[test]
    fn text_does_not_follow_the_view_into_history() {
        let mut terminal = Harness::new(20, 4);
        let output: String = (1..=8).map(|line| format!("line {line}\r\n")).collect();
        terminal.write(output);
        terminal.vt.scroll_to_top();
        assert!(terminal.vt.display_offset() > 0);
        assert_eq!(terminal.vt.text(None, false), "line 6\nline 7\nline 8");
    }

    #[test]
    fn text_of_wide_and_combining_characters() {
        let mut terminal = Harness::new(20, 2);
        terminal.write("e\u{301} \u{4f60}\u{597d} \u{1f600}!");
        assert_eq!(
            terminal.vt.text(None, false),
            "e\u{301} \u{4f60}\u{597d} \u{1f600}!"
        );
    }

    #[test]
    fn text_ignores_styling_and_the_alternate_screen_shows_through() {
        let mut terminal = Harness::new(20, 3);
        terminal.write("\x1b[1;31mred\x1b[0m plain");
        assert_eq!(terminal.vt.text(None, false), "red plain");
        terminal.write("\x1b[?1049h\x1b[Hfull screen");
        assert_eq!(terminal.vt.text(None, false), "full screen");
        terminal.write("\x1b[?1049l");
        assert_eq!(terminal.vt.text(None, false), "red plain");
    }

    #[test]
    fn text_holds_no_picture_placeholders() {
        assert_eq!(
            without_placeholders("a\u{10EEEE}\u{305}\u{30D}b \u{10EEEE}"),
            "a b  "
        );
        let mut terminal = Harness::new(20, 2);
        terminal.write("x\u{10EEEE}\u{305}\u{305}y");
        assert_eq!(terminal.vt.text(None, false), "x y");
    }

    #[test]
    fn the_scroll_viewport_reports_its_offset() {
        let mut terminal = Harness::new(20, 4);
        let output: String = (1..=30).map(|line| format!("line {line}\r\n")).collect();
        terminal.write(output);
        assert_eq!(terminal.vt.display_offset(), 0);
        terminal.vt.scroll(5);
        assert_eq!(terminal.vt.display_offset(), 5);
        terminal.vt.scroll(-2);
        assert_eq!(terminal.vt.display_offset(), 3);
        terminal.vt.scroll_to_bottom();
        assert_eq!(terminal.vt.display_offset(), 0);
    }

    #[test]
    fn words_lines_and_everything_select_what_they_say() {
        let mut terminal = Harness::new(30, 3);
        terminal.write("cargo test --workspace\r\nsecond line");
        let cell = terminal.vt.viewport_ref(7, 0).expect("a cell");
        let word = terminal.vt.words_between(cell, cell);
        terminal.vt.set_selection(word);
        assert_eq!(terminal.vt.selection_text().as_deref(), Some("test"));

        let line = terminal.vt.line_at(cell);
        terminal.vt.set_selection(line);
        assert_eq!(
            terminal.vt.selection_text().as_deref(),
            Some("cargo test --workspace")
        );

        let everything = terminal.vt.everything();
        terminal.vt.set_selection(everything);
        assert_eq!(
            terminal.vt.selection_text().as_deref(),
            Some("cargo test --workspace\nsecond line")
        );
        terminal.vt.set_selection(None);
        assert_eq!(terminal.vt.selection_text(), None);
    }

    #[test]
    fn a_selection_in_scrolled_back_history() {
        let mut terminal = Harness::new(20, 4);
        let output: String = (1..=8).map(|line| format!("line {line}\r\n")).collect();
        terminal.write(output);
        terminal.vt.scroll_to_top();
        // The top row of the view now shows the oldest line.
        let cell = terminal.vt.viewport_ref(0, 0).expect("a cell");
        assert_eq!(terminal.vt.screen_point(&cell), Some((0, 0)));
        let line = terminal.vt.line_at(cell);
        terminal.vt.set_selection(line);
        assert_eq!(terminal.vt.selection_text().as_deref(), Some("line 1"));
    }

    #[test]
    fn modes_follow_the_program() {
        let mut terminal = Harness::new(20, 4);
        let modes = terminal.vt.modes();
        assert!(!modes.app_cursor && !modes.bracketed_paste && !modes.mouse.tracking());
        terminal.write("\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[>1u");
        let modes = terminal.vt.modes();
        assert!(modes.app_cursor && modes.bracketed_paste && modes.focus_events);
        assert!(modes.mouse.click && modes.mouse.drag && !modes.mouse.motion && modes.mouse.sgr);
        assert_eq!(modes.kitty_keyboard, 1);
        terminal.write("\x1b[?1049h");
        assert!(terminal.vt.modes().alt_screen);
    }

    #[test]
    fn size_reports_match_the_cell_size() {
        let mut terminal = Harness::new(20, 4);
        // The size the program is told is what the callbacks report.
        *terminal.vt.effects.shared.size.lock() = GridPixels {
            columns: 20,
            lines: 4,
            cell_width: 10,
            cell_height: 20,
        };
        terminal.write("\x1b[14t\x1b[16t\x1b[18t");
        assert_eq!(
            terminal.replies(),
            "\x1b[4;80;200t\x1b[6;20;10t\x1b[8;4;20t"
        );
    }

    #[test]
    fn a_query_is_answered_in_every_format() {
        let mut terminal = Harness::new(40, 10);
        // Raw RGB and RGBA, a PNG, and RGBA compressed with zlib.
        terminal.write(command("a=q,i=1,s=2,v=1,f=24", &[0; 6]));
        terminal.write(command("a=q,i=2,s=1,v=2,f=32", &[0; 8]));
        terminal.write(command("a=q,i=3,f=100", &png(3, 2)));
        terminal.write(command("a=q,i=4,s=2,v=2,f=32,o=z", &zlib(&[7; 16])));
        assert_eq!(
            terminal.replies(),
            format!("{}{}{}{}", ok(1), ok(2), ok(3), ok(4))
        );
        // A query stores nothing.
        terminal.write(command("a=p,i=1", &[]));
        assert!(terminal.replies().starts_with("\x1b_Gi=1;ENOENT"));
    }

    #[test]
    fn errors_are_answered_and_quiet_mode_is_honoured() {
        let mut terminal = Harness::new(40, 10);
        // Too little data for the size.
        terminal.write(command("a=q,i=5,s=4,v=4,f=32", &[0; 3]));
        let reply = terminal.replies();
        assert!(
            reply.starts_with("\x1b_Gi=5;E") && !reply.contains("OK"),
            "{reply:?}"
        );
        // Not a PNG.
        terminal.write(command("a=q,i=6,f=100", b"not a png"));
        let reply = terminal.replies();
        assert!(reply.starts_with("\x1b_Gi=6;E"), "{reply:?}");
        // q=1 hides OK, q=2 hides errors too; without an id nothing is answered.
        terminal.write(command("a=q,i=7,s=1,v=1,f=24,q=1", &[0; 3]));
        terminal.write(command("a=q,i=8,f=100,q=2", b"garbage"));
        terminal.write(command("a=q,s=1,v=1,f=24", &[0; 3]));
        assert_eq!(terminal.replies(), "");
    }

    #[test]
    fn a_chunked_transmission_is_answered_once_at_the_end() {
        let mut terminal = Harness::new(40, 10);
        let data = png(16, 8);
        let encoded = base64(&data);
        let pieces: Vec<&str> = encoded
            .as_bytes()
            .chunks(32)
            .map(|chunk| std::str::from_utf8(chunk).expect("ASCII"))
            .collect();
        for (index, piece) in pieces.iter().enumerate() {
            let more = u8::from(index + 1 < pieces.len());
            let keys = if index == 0 {
                format!("a=T,i=9,f=100,m={more}")
            } else {
                format!("m={more}")
            };
            terminal.write(format!("\x1b_G{keys};{piece}\x1b\\"));
            if index + 1 < pieces.len() {
                assert_eq!(terminal.replies(), "", "answered before the last chunk");
            }
        }
        assert_eq!(terminal.replies(), ok(9));
        assert_eq!(terminal.pictures().len(), 1);
    }

    #[test]
    fn files_are_read_and_devices_and_pipes_are_refused() {
        let directory = tempfile::tempdir().expect("a directory");
        let path = directory.path().join("picture.png");
        std::fs::write(&path, png(4, 4)).expect("write the picture");
        let mut terminal = Harness::new(40, 10);
        terminal.write(command(
            "a=q,i=10,t=f,f=100",
            path.to_string_lossy().as_bytes(),
        ));
        assert_eq!(terminal.replies(), ok(10));

        terminal.write(command("a=q,i=11,t=f,f=100", b"/dev/zero"));
        let reply = terminal.replies();
        assert!(reply.starts_with("\x1b_Gi=11;E"), "{reply:?}");

        // A pipe nobody writes to would block a reader forever.
        let fifo = directory.path().join("pipe");
        let name = CString::new(fifo.to_string_lossy().as_bytes()).expect("a path");
        // SAFETY: a valid path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        terminal.write(command(
            "a=q,i=12,t=f,f=100",
            fifo.to_string_lossy().as_bytes(),
        ));
        let reply = terminal.replies();
        assert!(reply.starts_with("\x1b_Gi=12;E"), "{reply:?}");

        terminal.write(command("a=q,i=13,t=f,f=100", b"/nonexistent/picture.png"));
        let reply = terminal.replies();
        assert!(reply.starts_with("\x1b_Gi=13;E"), "{reply:?}");
    }

    #[test]
    fn temporary_files_are_deleted_after_reading() {
        let mut file = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .suffix(".png")
            .tempfile_in(std::env::temp_dir())
            .expect("a temporary file");
        file.write_all(&png(4, 4)).expect("write");
        let (_, path) = file.keep().expect("keep the file");
        let mut terminal = Harness::new(40, 10);
        terminal.write(command(
            "a=t,i=14,t=t,f=100",
            path.to_string_lossy().as_bytes(),
        ));
        assert_eq!(terminal.replies(), ok(14));
        assert!(!path.exists(), "the terminal deleted {}", path.display());
    }

    #[test]
    fn shared_memory_is_read_and_unlinked() {
        let name = format!("/kou-term-{}", std::process::id());
        let c_name = CString::new(name.clone()).expect("a name");
        let pixels = [9u8; 4 * 4 * 4];
        // SAFETY: plain POSIX shared memory calls on an object this test owns.
        unsafe {
            let fd = libc::shm_open(c_name.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600);
            assert!(fd >= 0, "shm_open: {}", std::io::Error::last_os_error());
            assert_eq!(libc::ftruncate(fd, pixels.len() as libc::off_t), 0);
            let map = libc::mmap(
                std::ptr::null_mut(),
                pixels.len(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            assert_ne!(map, libc::MAP_FAILED);
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), map.cast(), pixels.len());
            libc::munmap(map, pixels.len());
            libc::close(fd);
        }
        let mut terminal = Harness::new(40, 10);
        terminal.write(command(
            &format!("a=t,i=15,t=s,f=32,s=4,v=4,S={}", pixels.len()),
            name.as_bytes(),
        ));
        assert_eq!(terminal.replies(), ok(15));
        // SAFETY: as above; opening must fail because the terminal unlinked it.
        let reopened = unsafe { libc::shm_open(c_name.as_ptr(), libc::O_RDONLY, 0) };
        if reopened >= 0 {
            // SAFETY: clean up after a failed expectation.
            unsafe {
                libc::close(reopened);
                libc::shm_unlink(c_name.as_ptr());
            }
            panic!("the shared memory object was left behind");
        }
    }

    #[test]
    fn a_placement_moves_the_cursor_as_kitty_does() {
        let mut terminal = Harness::new(40, 10);
        terminal.write(command("a=T,i=20,f=100,c=10,r=4", &png(8, 8)));
        assert_eq!(terminal.replies(), ok(20));
        // Past the right edge of the picture, on its last row.
        assert_eq!(terminal.vt.cursor(), (10, 3));
        assert_eq!(terminal.pictures(), vec![(0, 0, 100, 80)]);

        // C=1 leaves the cursor where it was; x, y, w, h choose a part of the picture;
        // X and Y offset it inside its first cell.
        terminal.write("\x1b[6;1H");
        terminal.write(command(
            "a=T,i=21,f=100,C=1,x=2,y=2,w=4,h=4,X=3,Y=5,q=2",
            &png(8, 8),
        ));
        assert_eq!(terminal.vt.cursor(), (0, 5));
        let mut render = RenderState::new().expect("a render state");
        render.update(&terminal.vt);
        let picture = render
            .copy()
            .pictures
            .iter()
            .find(|picture| picture.line == 5)
            .cloned()
            .expect("the second picture");
        assert_eq!(picture.source, [2, 2, 4, 4]);
        assert_eq!((picture.offset_x, picture.offset_y), (3, 5));
        assert_eq!((picture.width, picture.height), (4, 4));
    }

    #[test]
    fn negative_z_goes_below_the_text() {
        let mut terminal = Harness::new(40, 10);
        terminal.write(command("a=T,i=22,f=100,z=-1,q=2", &png(8, 8)));
        terminal.write(command("a=T,i=23,f=100,z=-1073741825,q=2", &png(8, 8)));
        terminal.write(command("a=T,i=24,f=100,q=2", &png(8, 8)));
        let mut render = RenderState::new().expect("a render state");
        render.update(&terminal.vt);
        let mut layers: Vec<c_int> = render.copy().pictures.iter().map(|p| p.layer).collect();
        layers.sort();
        assert_eq!(
            layers,
            vec![
                ghostty_vt_sys::GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND,
                ghostty_vt_sys::GHOSTTY_IMAGE_LAYER_BELOW_TEXT,
                ghostty_vt_sys::GHOSTTY_IMAGE_LAYER_ABOVE_TEXT
            ]
        );
    }

    #[test]
    fn pictures_scroll_into_the_history_and_back() {
        let mut terminal = Harness::new(20, 6);
        terminal.write(command("a=T,i=30,f=100,c=4,r=2,q=2", &png(8, 8)));
        assert_eq!(terminal.pictures(), vec![(0, 0, 40, 40)]);
        // The cursor is on the picture's last row; four lines reach the bottom of the
        // screen and one more scrolls it, and the picture with it: half of it is left.
        terminal.write("\r\n".repeat(5));
        assert_eq!(terminal.pictures(), vec![(0, -1, 40, 40)]);
        terminal.write("\r\n".repeat(10));
        assert_eq!(terminal.pictures(), vec![]);
        // Scrolled back to, it is there again.
        terminal.vt.scroll_to_top();
        assert_eq!(terminal.pictures(), vec![(0, 0, 40, 40)]);
    }

    #[test]
    fn pictures_belong_to_their_screen_and_go_with_a_clear() {
        let mut terminal = Harness::new(20, 6);
        terminal.write(command("a=T,i=31,f=100,c=4,r=2,q=2", &png(8, 8)));
        terminal.write("\x1b[?1049h");
        assert_eq!(terminal.pictures(), vec![]);
        terminal.write(command("a=T,i=32,f=100,c=2,r=1,q=2", &png(8, 8)));
        assert_eq!(terminal.pictures().len(), 1);
        terminal.write("\x1b[?1049l");
        assert_eq!(terminal.pictures(), vec![(0, 0, 40, 40)]);
        terminal.write("\x1b[2J");
        assert_eq!(terminal.pictures(), vec![]);
    }

    #[test]
    fn deletes_by_all_id_cell_and_cursor() {
        let mut terminal = Harness::new(40, 10);
        let place = |terminal: &mut Harness, id: u32, row: u16, column: u16| {
            terminal.write(format!("\x1b[{};{}H", row + 1, column + 1));
            terminal.write(command(
                &format!("a=T,i={id},f=100,c=2,r=1,q=2"),
                &png(4, 4),
            ));
        };
        for (id, column) in [(40, 0), (41, 5), (42, 10), (43, 15)] {
            place(&mut terminal, id, 0, column);
        }
        assert_eq!(terminal.pictures().len(), 4);
        // By image id.
        terminal.write("\x1b_Ga=d,d=i,i=40\x1b\\");
        assert_eq!(terminal.pictures().len(), 3);
        // By a cell it covers (1-based column and row).
        terminal.write("\x1b_Ga=d,d=p,x=7,y=1\x1b\\");
        assert_eq!(terminal.pictures().len(), 2);
        // By the cursor's cell.
        terminal.write("\x1b[1;11H\x1b_Ga=d,d=c\x1b\\");
        assert_eq!(terminal.pictures().len(), 1);
        // Everything, and the image data with it.
        terminal.write("\x1b_Ga=d,d=A\x1b\\");
        assert_eq!(terminal.pictures(), vec![]);
        terminal.write(command("a=p,i=43", &[]));
        assert!(terminal.replies().contains("ENOENT"));
    }

    #[test]
    fn the_image_quota_is_kittys() {
        let terminal = Harness::new(20, 4);
        let limit: u64 = terminal
            .vt
            .get(ghostty_vt_sys::GHOSTTY_TERMINAL_DATA_KITTY_IMAGE_STORAGE_LIMIT)
            .expect("a limit");
        assert_eq!(limit, IMAGE_STORAGE_LIMIT);
    }

    #[test]
    fn broken_and_hostile_commands_do_not_stop_the_terminal() {
        let mut terminal = Harness::new(40, 6);
        // Garbage keys, a payload that is not base64, an absurd size, a command that is
        // cancelled half-way, and one that never ends until the next escape.
        terminal.write("\x1b_Gzz=!!,a=T;%%%\x1b\\");
        terminal.write(command("a=T,i=50,s=100000,v=100000,f=32", &[1; 16]));
        terminal.write("\x1b_Ga=T,i=51,f=100;AAAA\x18after cancel ");
        terminal.write("\x1b_Ga=T,i=52,m=1;");
        terminal.write("A".repeat(1 << 20));
        terminal.write("\x1b\\still here");
        // A transmission left open takes the next command as its continuation, as the
        // protocol says; a final empty chunk closes it.
        terminal.write("\x1b_Gm=0;\x1b\\");
        let text = terminal.vt.text(None, false);
        assert!(
            text.contains("after cancel") && text.contains("still here"),
            "{text:?}"
        );
        assert_eq!(terminal.pictures(), vec![]);
        // And a picture still works afterwards.
        terminal.write(command("a=T,i=53,f=100,q=2", &png(4, 4)));
        assert_eq!(terminal.pictures().len(), 1);
    }

    #[test]
    fn png_decoding_refuses_what_is_not_a_reasonable_png() {
        assert_eq!(decode_rgba(b"nope"), None);
        let (width, height, pixels) = decode_rgba(&png(3, 2)).expect("a PNG");
        assert_eq!((width, height, pixels.len()), (3, 2, 24));
        assert_eq!(&pixels[..4], &[0, 0, 255, 255]);
    }
}
