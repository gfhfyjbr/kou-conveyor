//! Raw bindings to libghostty-vt 1.0.0, the terminal state and parser library of the
//! Ghostty terminal (MIT), built from the vendored release by `build.rs`.
//!
//! Only the part of the C API that `kou-term` uses is declared, written by hand from
//! the headers of the pinned release (`include/ghostty/vt/*.h`). The library's C ABI is
//! stable from 1.0. `tests` checks every struct here against the sizes the library
//! itself reports through `ghostty_type_json`.
//!
//! Enumerations are C `int`s; their values are plain constants here.

#![allow(non_camel_case_types, non_upper_case_globals, missing_docs)]

use std::ffi::{c_char, c_int, c_void};

/// A result code: `GHOSTTY_SUCCESS` or a negative error.
pub type GhosttyResult = c_int;
pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
pub const GHOSTTY_NO_VALUE: GhosttyResult = -4;

macro_rules! opaque {
    ($($name:ident => $handle:ident),* $(,)?) => {$(
        #[repr(C)]
        pub struct $name {
            _private: [u8; 0],
        }
        pub type $handle = *mut $name;
    )*};
}

opaque! {
    GhosttyTerminalImpl => GhosttyTerminal,
    GhosttyRenderStateImpl => GhosttyRenderState,
    GhosttyRenderStateRowIteratorImpl => GhosttyRenderStateRowIterator,
    GhosttyRenderStateRowCellsImpl => GhosttyRenderStateRowCells,
    GhosttyRenderStateImageIteratorImpl => GhosttyRenderStateImageIterator,
    GhosttyKittyGraphicsImpl => GhosttyKittyGraphics,
    GhosttyFormatterImpl => GhosttyFormatter,
    GhosttyTrackedGridRefImpl => GhosttyTrackedGridRef,
}

/// A borrowed image in the Kitty graphics storage.
#[repr(C)]
pub struct GhosttyKittyGraphicsImageImpl {
    _private: [u8; 0],
}
pub type GhosttyKittyGraphicsImage = *const GhosttyKittyGraphicsImageImpl;

/// An allocator. Only ever passed through: `NULL` means the library's default.
#[repr(C)]
pub struct GhosttyAllocator {
    pub ctx: *mut c_void,
    pub vtable: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyString {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyStyleColorValue {
    pub palette: u8,
    pub rgb: GhosttyColorRgb,
}

pub const GHOSTTY_STYLE_COLOR_NONE: u8 = 0;
pub const GHOSTTY_STYLE_COLOR_PALETTE: u8 = 1;
pub const GHOSTTY_STYLE_COLOR_RGB: u8 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyleColor {
    pub tag: u8,
    pub value: GhosttyStyleColorValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyle {
    pub fg_color: GhosttyStyleColor,
    pub bg_color: GhosttyStyleColor,
    pub underline_color: GhosttyStyleColor,
    pub flags: u16,
    pub reserved: u16,
}

pub const GHOSTTY_STYLE_FLAG_BOLD: u16 = 1 << 0;
pub const GHOSTTY_STYLE_FLAG_ITALIC: u16 = 1 << 1;
pub const GHOSTTY_STYLE_FLAG_FAINT: u16 = 1 << 2;
pub const GHOSTTY_STYLE_FLAG_BLINK: u16 = 1 << 3;
pub const GHOSTTY_STYLE_FLAG_INVERSE: u16 = 1 << 4;
pub const GHOSTTY_STYLE_FLAG_INVISIBLE: u16 = 1 << 5;
pub const GHOSTTY_STYLE_FLAG_STRIKETHROUGH: u16 = 1 << 6;
pub const GHOSTTY_STYLE_FLAG_OVERLINE: u16 = 1 << 7;
pub const GHOSTTY_STYLE_UNDERLINE_SHIFT: u16 = 8;
pub const GHOSTTY_STYLE_UNDERLINE_MASK: u16 = 0x7 << GHOSTTY_STYLE_UNDERLINE_SHIFT;

pub const GHOSTTY_SGR_UNDERLINE_NONE: u16 = 0;
pub const GHOSTTY_SGR_UNDERLINE_SINGLE: u16 = 1;
pub const GHOSTTY_SGR_UNDERLINE_DOUBLE: u16 = 2;
pub const GHOSTTY_SGR_UNDERLINE_CURLY: u16 = 3;
pub const GHOSTTY_SGR_UNDERLINE_DOTTED: u16 = 4;
pub const GHOSTTY_SGR_UNDERLINE_DASHED: u16 = 5;

#[repr(C)]
pub struct GhosttyCodepointBuffer {
    pub ptr: *mut u32,
    pub cap: usize,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttyPointCoordinate {
    pub x: u16,
    pub y: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyPointValue {
    pub coordinate: GhosttyPointCoordinate,
    pub _padding: [u64; 2],
}

pub const GHOSTTY_POINT_TAG_ACTIVE: c_int = 0;
pub const GHOSTTY_POINT_TAG_VIEWPORT: c_int = 1;
pub const GHOSTTY_POINT_TAG_SCREEN: c_int = 2;
pub const GHOSTTY_POINT_TAG_HISTORY: c_int = 3;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPoint {
    pub tag: c_int,
    pub value: GhosttyPointValue,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyGridRef {
    pub node: *mut c_void,
    pub x: u16,
    pub y: u16,
}

impl Default for GhosttyGridRef {
    fn default() -> Self {
        Self {
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttySelection {
    pub start: GhosttyGridRef,
    pub end: GhosttyGridRef,
    pub rectangle: bool,
}

#[repr(C)]
pub struct GhosttyTerminalSelectWordOptions {
    pub size: usize,
    pub r#ref: GhosttyGridRef,
    pub boundary_codepoints: *const u32,
    pub boundary_codepoints_len: usize,
}

#[repr(C)]
pub struct GhosttyTerminalSelectWordBetweenOptions {
    pub size: usize,
    pub start: GhosttyGridRef,
    pub end: GhosttyGridRef,
    pub boundary_codepoints: *const u32,
    pub boundary_codepoints_len: usize,
}

#[repr(C)]
pub struct GhosttyTerminalSelectLineOptions {
    pub size: usize,
    pub r#ref: GhosttyGridRef,
    pub whitespace: *const u32,
    pub whitespace_len: usize,
    pub semantic_prompt_boundary: bool,
}

pub const GHOSTTY_SELECTION_ORDER_FORWARD: c_int = 0;
pub const GHOSTTY_SELECTION_ORDER_REVERSE: c_int = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyTerminalScrollViewportValue {
    pub delta: isize,
    pub row: usize,
    pub delta_prompt: isize,
    pub _padding: [u64; 2],
}

pub const GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP: c_int = 0;
pub const GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM: c_int = 1;
pub const GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA: c_int = 2;
pub const GHOSTTY_TERMINAL_SCROLL_VIEWPORT_ROW: c_int = 3;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalScrollViewport {
    pub tag: c_int,
    pub value: GhosttyTerminalScrollViewportValue,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttyTerminalScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

/// A mode number: the low fifteen bits are the number, the top bit says it is an ANSI
/// mode rather than a DEC private one.
pub type GhosttyMode = u16;

/// `ghostty_mode_new` from `modes.h`, which is an inline function there.
pub const fn ghostty_mode(value: u16, ansi: bool) -> GhosttyMode {
    (value & 0x7fff) | ((ansi as u16) << 15)
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyTerminalModeConfig {
    pub mode: GhosttyMode,
    pub value: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttySizeReportSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyTerminalDesktopNotification {
    pub title: GhosttyString,
    pub body: GhosttyString,
}

pub const GHOSTTY_TERMINAL_PROGRESS_STATE_REMOVE: c_int = 0;
pub const GHOSTTY_TERMINAL_PROGRESS_STATE_SET: c_int = 1;
pub const GHOSTTY_TERMINAL_PROGRESS_STATE_ERROR: c_int = 2;
pub const GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE: c_int = 3;
pub const GHOSTTY_TERMINAL_PROGRESS_STATE_PAUSE: c_int = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyTerminalProgressReport {
    pub state: c_int,
    pub progress: i8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyClipboardContent {
    pub mime: GhosttyString,
    pub data: GhosttyString,
}

pub const GHOSTTY_CLIPBOARD_LOCATION_STANDARD: c_int = 0;
pub const GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS: c_int = 0;
pub const GHOSTTY_CLIPBOARD_WRITE_RESULT_DENIED: c_int = 1;
pub const GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED: c_int = 2;

#[repr(C)]
pub struct GhosttyClipboardWriteReply {
    pub size: usize,
    pub result: c_int,
    pub remember: bool,
}

pub type GhosttyClipboardWriteReplyFn = unsafe extern "C" fn(
    write: *const GhosttyClipboardWrite,
    reply: *const GhosttyClipboardWriteReply,
);

#[repr(C)]
pub struct GhosttyClipboardWrite {
    pub location: c_int,
    pub contents: *const GhosttyClipboardContent,
    pub contents_len: usize,
    pub name: GhosttyString,
    pub granted: bool,
    pub can_remember: bool,
    pub ctx: *const c_void,
    pub reply: Option<GhosttyClipboardWriteReplyFn>,
}

pub const GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_APC: c_int = 0;
pub const GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_OSC: c_int = 1;
pub const GHOSTTY_OSC_TERMINATOR_ST: c_int = 0;
pub const GHOSTTY_OSC_TERMINATOR_BEL: c_int = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalUnknownOscSequence {
    pub truncated: bool,
    pub content: GhosttyString,
    pub terminator: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyTerminalUnknownSequenceValue {
    pub osc: GhosttyTerminalUnknownOscSequence,
    pub _padding: [u64; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalUnknownSequence {
    pub tag: c_int,
    pub value: GhosttyTerminalUnknownSequenceValue,
}

pub const GHOSTTY_COLOR_SCHEME_LIGHT: c_int = 0;
pub const GHOSTTY_COLOR_SCHEME_DARK: c_int = 1;

#[repr(C)]
pub struct GhosttySysImage {
    pub width: u32,
    pub height: u32,
    pub data: *mut u8,
    pub data_len: usize,
}

// Callbacks. They run synchronously inside `ghostty_terminal_vt_write` and must not
// write to the same terminal.
pub type GhosttyTerminalWritePtyFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
);
pub type GhosttyTerminalBellFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
pub type GhosttyTerminalTitleChangedFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
pub type GhosttyTerminalPwdChangedFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
pub type GhosttyTerminalResetFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
pub type GhosttyTerminalSizeFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_size: *mut GhosttySizeReportSize,
) -> bool;
pub type GhosttyTerminalColorSchemeFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_scheme: *mut c_int,
) -> bool;
pub type GhosttyTerminalXtversionFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void) -> GhosttyString;
pub type GhosttyTerminalRenderHoldFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, held: bool);
pub type GhosttyTerminalDesktopNotificationFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    notification: *const GhosttyTerminalDesktopNotification,
);
pub type GhosttyTerminalProgressReportFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    report: *const GhosttyTerminalProgressReport,
);
pub type GhosttyTerminalClipboardWriteFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    write: *const GhosttyClipboardWrite,
);
pub type GhosttyTerminalUnknownSequenceFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    sequence: *const GhosttyTerminalUnknownSequence,
);
pub type GhosttySysDecodePngFn = unsafe extern "C" fn(
    userdata: *mut c_void,
    allocator: *const GhosttyAllocator,
    data: *const u8,
    data_len: usize,
    out: *mut GhosttySysImage,
) -> bool;

// GhosttyTerminalOption
pub const GHOSTTY_TERMINAL_OPT_USERDATA: c_int = 1;
pub const GHOSTTY_TERMINAL_OPT_WRITE_PTY: c_int = 2;
pub const GHOSTTY_TERMINAL_OPT_BELL: c_int = 3;
pub const GHOSTTY_TERMINAL_OPT_XTVERSION: c_int = 5;
pub const GHOSTTY_TERMINAL_OPT_TITLE_CHANGED: c_int = 6;
pub const GHOSTTY_TERMINAL_OPT_SIZE: c_int = 7;
pub const GHOSTTY_TERMINAL_OPT_COLOR_SCHEME: c_int = 8;
pub const GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND: c_int = 12;
pub const GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND: c_int = 13;
pub const GHOSTTY_TERMINAL_OPT_COLOR_CURSOR: c_int = 14;
pub const GHOSTTY_TERMINAL_OPT_COLOR_PALETTE: c_int = 15;
pub const GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT: c_int = 16;
pub const GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_FILE: c_int = 17;
pub const GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE: c_int = 18;
pub const GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM: c_int = 19;
pub const GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES: c_int = 20;
pub const GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES_KITTY: c_int = 21;
pub const GHOSTTY_TERMINAL_OPT_SELECTION: c_int = 22;
pub const GHOSTTY_TERMINAL_OPT_PWD_CHANGED: c_int = 26;
pub const GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE: c_int = 27;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES: c_int = 28;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES: c_int = 29;
pub const GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION: c_int = 30;
pub const GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT: c_int = 31;
pub const GHOSTTY_TERMINAL_OPT_MODE: c_int = 35;
pub const GHOSTTY_TERMINAL_OPT_UNKNOWN_SEQUENCE: c_int = 36;
pub const GHOSTTY_TERMINAL_OPT_UNKNOWN_MAX_BYTES: c_int = 37;
pub const GHOSTTY_TERMINAL_OPT_TERMINFO_NAME: c_int = 38;
pub const GHOSTTY_TERMINAL_OPT_RENDER_HOLD: c_int = 42;
pub const GHOSTTY_TERMINAL_OPT_RESET: c_int = 44;

// GhosttyTerminalData
pub const GHOSTTY_TERMINAL_DATA_COLS: c_int = 1;
pub const GHOSTTY_TERMINAL_DATA_ROWS: c_int = 2;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_X: c_int = 3;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_Y: c_int = 4;
pub const GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN: c_int = 6;
pub const GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS: c_int = 8;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBAR: c_int = 9;
pub const GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING: c_int = 11;
pub const GHOSTTY_TERMINAL_DATA_TITLE: c_int = 12;
pub const GHOSTTY_TERMINAL_DATA_PWD: c_int = 13;
pub const GHOSTTY_TERMINAL_DATA_TOTAL_ROWS: c_int = 14;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS: c_int = 15;
pub const GHOSTTY_TERMINAL_DATA_KITTY_IMAGE_STORAGE_LIMIT: c_int = 26;
pub const GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS: c_int = 30;
pub const GHOSTTY_TERMINAL_DATA_SELECTION: c_int = 31;
pub const GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE: c_int = 32;
pub const GHOSTTY_TERMINAL_DATA_MODE: c_int = 37;

pub const GHOSTTY_TERMINAL_SCREEN_PRIMARY: c_int = 0;
pub const GHOSTTY_TERMINAL_SCREEN_ALTERNATE: c_int = 1;

// GhosttyCursorStyle
pub const GHOSTTY_CURSOR_STYLE_BAR: c_int = 0;
pub const GHOSTTY_CURSOR_STYLE_BLOCK: c_int = 1;
pub const GHOSTTY_CURSOR_STYLE_UNDERLINE: c_int = 2;
pub const GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW: c_int = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttyRenderStateCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyRenderStateColors {
    pub size: usize,
    pub background: GhosttyColorRgb,
    pub foreground: GhosttyColorRgb,
    pub cursor: GhosttyColorRgb,
    pub cursor_has_value: bool,
    pub palette: [GhosttyColorRgb; 256],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttyRenderStateRowSelection {
    pub size: usize,
    pub start_x: u16,
    pub end_x: u16,
}

pub const GHOSTTY_RENDER_STATE_DIRTY_FALSE: c_int = 0;
pub const GHOSTTY_RENDER_STATE_DIRTY_PARTIAL: c_int = 1;
pub const GHOSTTY_RENDER_STATE_DIRTY_FULL: c_int = 2;

/// Rows above and below the viewport that a render state copies too
/// (`GhosttyRenderStateOverscan`): set with
/// [`GHOSTTY_RENDER_STATE_OPT_OVERSCAN`], and read back, as the rows it
/// found, with [`GHOSTTY_RENDER_STATE_DATA_OVERSCAN`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyRenderStateOverscan {
    pub above: u16,
    pub below: u16,
}

// GhosttyRenderStateOption
pub const GHOSTTY_RENDER_STATE_OPT_OVERSCAN: c_int = 2;

// GhosttyRenderStateData
pub const GHOSTTY_RENDER_STATE_DATA_OVERSCAN: c_int = 20;
pub const GHOSTTY_RENDER_STATE_DATA_COLS: c_int = 1;
pub const GHOSTTY_RENDER_STATE_DATA_ROWS: c_int = 2;
pub const GHOSTTY_RENDER_STATE_DATA_DIRTY: c_int = 3;
pub const GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR: c_int = 4;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR: c_int = 18;
pub const GHOSTTY_RENDER_STATE_DATA_COLORS: c_int = 19;
pub const GHOSTTY_RENDER_STATE_DATA_IMAGE_ITERATOR: c_int = 22;
pub const GHOSTTY_RENDER_STATE_DATA_IMAGE_PLACEMENTS_GENERATION: c_int = 23;

// GhosttyRenderStateRowData
pub const GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY: c_int = 1;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_CELLS: c_int = 3;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION: c_int = 4;
pub const GHOSTTY_RENDER_STATE_ROW_OPT_DIRTY: c_int = 1;

// GhosttyRenderStateRowCellsData
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW: c_int = 1;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE: c_int = 2;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN: c_int = 3;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF: c_int = 4;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR: c_int = 5;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR: c_int = 6;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED: c_int = 7;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING: c_int = 8;

/// A raw cell, read with `ghostty_cell_get`.
pub type GhosttyCell = u64;
pub const GHOSTTY_CELL_DATA_CODEPOINT: c_int = 1;
pub const GHOSTTY_CELL_DATA_WIDE: c_int = 3;
pub const GHOSTTY_CELL_WIDE_NARROW: c_int = 0;
pub const GHOSTTY_CELL_WIDE_WIDE: c_int = 1;
pub const GHOSTTY_CELL_WIDE_SPACER_TAIL: c_int = 2;
pub const GHOSTTY_CELL_WIDE_SPACER_HEAD: c_int = 3;

// GhosttyRenderStateImageData
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_PROTOCOL: c_int = 1;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_ID: c_int = 2;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_GENERATION: c_int = 3;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_LAYER: c_int = 4;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_Z: c_int = 5;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_COL: c_int = 6;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_ROW: c_int = 7;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_GRID_COLS: c_int = 8;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_GRID_ROWS: c_int = 9;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_X: c_int = 10;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_Y: c_int = 11;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_WIDTH: c_int = 12;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_HEIGHT: c_int = 13;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_X: c_int = 14;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_Y: c_int = 15;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_WIDTH: c_int = 16;
pub const GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_HEIGHT: c_int = 17;

pub const GHOSTTY_IMAGE_PROTOCOL_KITTY: c_int = 0;
pub const GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND: c_int = 0;
pub const GHOSTTY_IMAGE_LAYER_BELOW_TEXT: c_int = 1;
pub const GHOSTTY_IMAGE_LAYER_ABOVE_TEXT: c_int = 2;

// GhosttyKittyGraphicsImageData
pub const GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_WIDTH: c_int = 3;
pub const GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_HEIGHT: c_int = 4;
pub const GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_FORMAT: c_int = 5;
pub const GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_PIXELS: c_int = 7;
pub const GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_GENERATION: c_int = 9;

pub const GHOSTTY_KITTY_IMAGE_FORMAT_RGB: c_int = 0;
pub const GHOSTTY_KITTY_IMAGE_FORMAT_RGBA: c_int = 1;
pub const GHOSTTY_KITTY_IMAGE_FORMAT_PNG: c_int = 2;
pub const GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA: c_int = 3;
pub const GHOSTTY_KITTY_IMAGE_FORMAT_GRAY: c_int = 4;

// GhosttyFormatterOption / GhosttyFormatterFormat
pub const GHOSTTY_FORMATTER_OPT_EMIT: c_int = 1;
pub const GHOSTTY_FORMATTER_OPT_UNWRAP: c_int = 2;
pub const GHOSTTY_FORMATTER_OPT_TRIM: c_int = 3;
pub const GHOSTTY_FORMATTER_OPT_SELECTION: c_int = 4;
pub const GHOSTTY_FORMATTER_OPT_EXTRAS: c_int = 5;
pub const GHOSTTY_FORMATTER_FORMAT_PLAIN: c_int = 0;
pub const GHOSTTY_FORMATTER_FORMAT_VT: c_int = 1;
/// Every piece of terminal state the VT format can write besides the text:
/// modes, the scrolling region, tab stops, the keyboard protocol, the cursor.
pub const GHOSTTY_FORMATTER_EXTRAS_ALL: u32 = 0xFFF;

// GhosttySysOption
pub const GHOSTTY_SYS_OPT_USERDATA: c_int = 1;
pub const GHOSTTY_SYS_OPT_DECODE_PNG: c_int = 2;

unsafe extern "C" {
    pub fn ghostty_type_json() -> *const c_char;

    pub fn ghostty_alloc(allocator: *const GhosttyAllocator, len: usize) -> *mut u8;
    pub fn ghostty_free(allocator: *const GhosttyAllocator, ptr: *mut u8, len: usize);
    pub fn ghostty_sys_set(option: c_int, value: *const c_void) -> GhosttyResult;

    pub fn ghostty_terminal_new(
        allocator: *const GhosttyAllocator,
        terminal: *mut GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_set(
        terminal: GhosttyTerminal,
        option: c_int,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_get(
        terminal: GhosttyTerminal,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    pub fn ghostty_terminal_scroll_viewport(
        terminal: GhosttyTerminal,
        behavior: GhosttyTerminalScrollViewport,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_grid_ref(
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
        out_ref: *mut GhosttyGridRef,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_point_from_grid_ref(
        terminal: GhosttyTerminal,
        r#ref: *const GhosttyGridRef,
        tag: c_int,
        out: *mut GhosttyPointCoordinate,
    ) -> GhosttyResult;

    pub fn ghostty_terminal_select_word(
        terminal: GhosttyTerminal,
        options: *const GhosttyTerminalSelectWordOptions,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_select_word_between(
        terminal: GhosttyTerminal,
        options: *const GhosttyTerminalSelectWordBetweenOptions,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_select_line(
        terminal: GhosttyTerminal,
        options: *const GhosttyTerminalSelectLineOptions,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_select_all(
        terminal: GhosttyTerminal,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_selection_ordered(
        terminal: GhosttyTerminal,
        selection: *const GhosttySelection,
        desired: c_int,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;

    pub fn ghostty_terminal_grid_ref_track(
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
        out_ref: *mut GhosttyTrackedGridRef,
    ) -> GhosttyResult;
    pub fn ghostty_tracked_grid_ref_free(r#ref: GhosttyTrackedGridRef);
    pub fn ghostty_tracked_grid_ref_has_value(r#ref: GhosttyTrackedGridRef) -> bool;
    pub fn ghostty_tracked_grid_ref_point(
        r#ref: GhosttyTrackedGridRef,
        tag: c_int,
        out_point: *mut GhosttyPointCoordinate,
    ) -> GhosttyResult;
    pub fn ghostty_tracked_grid_ref_snapshot(
        r#ref: GhosttyTrackedGridRef,
        out_ref: *mut GhosttyGridRef,
    ) -> GhosttyResult;

    pub fn ghostty_formatter_terminal_new(
        allocator: *const GhosttyAllocator,
        formatter: *mut GhosttyFormatter,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_formatter_set(
        formatter: GhosttyFormatter,
        option: c_int,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_formatter_format_alloc(
        formatter: GhosttyFormatter,
        allocator: *const GhosttyAllocator,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;
    pub fn ghostty_formatter_free(formatter: GhosttyFormatter);

    pub fn ghostty_render_state_new(
        allocator: *const GhosttyAllocator,
        state: *mut GhosttyRenderState,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_free(state: GhosttyRenderState);
    pub fn ghostty_render_state_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_get(
        state: GhosttyRenderState,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_set(
        state: GhosttyRenderState,
        option: c_int,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_clean(state: GhosttyRenderState) -> GhosttyResult;
    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iterator: *mut GhosttyRenderStateRowIterator,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_iterator_free(iterator: GhosttyRenderStateRowIterator);
    pub fn ghostty_render_state_row_iterator_next(iterator: GhosttyRenderStateRowIterator) -> bool;
    pub fn ghostty_render_state_row_get(
        iterator: GhosttyRenderStateRowIterator,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_set(
        iterator: GhosttyRenderStateRowIterator,
        option: c_int,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_new(
        allocator: *const GhosttyAllocator,
        out_cells: *mut GhosttyRenderStateRowCells,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_free(cells: GhosttyRenderStateRowCells);
    pub fn ghostty_render_state_row_cells_next(cells: GhosttyRenderStateRowCells) -> bool;
    pub fn ghostty_render_state_row_cells_get(
        cells: GhosttyRenderStateRowCells,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_image_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iterator: *mut GhosttyRenderStateImageIterator,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_image_iterator_free(iterator: GhosttyRenderStateImageIterator);
    pub fn ghostty_render_state_image_iterator_next(
        iterator: GhosttyRenderStateImageIterator,
    ) -> bool;
    pub fn ghostty_render_state_image_get(
        iterator: GhosttyRenderStateImageIterator,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;

    pub fn ghostty_cell_get(cell: GhosttyCell, data: c_int, out: *mut c_void) -> GhosttyResult;

    pub fn ghostty_kitty_graphics_image(
        graphics: GhosttyKittyGraphics,
        image_id: u32,
    ) -> GhosttyKittyGraphicsImage;
    pub fn ghostty_kitty_graphics_image_get(
        image: GhosttyKittyGraphicsImage,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::CStr,
        mem::{offset_of, size_of},
    };

    /// Checks a struct's size and the offset of each named field against what the
    /// library reports about its own types.
    macro_rules! check {
        ($types:expr, $name:ident { $($field:ident $(: $c_name:literal)?),* $(,)? }) => {{
            let name = stringify!($name);
            let entry = $types
                .get(name)
                .unwrap_or_else(|| panic!("the library does not describe {name}"));
            assert_eq!(
                entry["size"].as_u64(),
                Some(size_of::<$name>() as u64),
                "size of {name}"
            );
            $(
                let field = check!(@name $field $(, $c_name)?);
                assert_eq!(
                    entry["fields"][field]["offset"].as_u64(),
                    Some(offset_of!($name, $field) as u64),
                    "offset of {name}.{field}"
                );
            )*
        }};
        (@name $field:ident) => { stringify!($field) };
        (@name $field:ident, $c_name:literal) => { $c_name };
    }

    #[test]
    fn layouts_match_the_library() {
        // SAFETY: a static string owned by the library.
        let json = unsafe { CStr::from_ptr(ghostty_type_json()) }
            .to_str()
            .expect("UTF-8");
        let root: serde_json::Value = serde_json::from_str(json).expect("JSON");
        let types = &root["types"];
        check!(types, GhosttyString { ptr, len });
        check!(types, GhosttyColorRgb { r, g, b });
        check!(types, GhosttyStyleColor { tag, value });
        check!(
            types,
            GhosttyStyle {
                fg_color,
                bg_color,
                underline_color,
                flags,
                reserved
            }
        );
        check!(types, GhosttyCodepointBuffer { ptr, cap, len });
        check!(types, GhosttyPointCoordinate { x, y });
        check!(types, GhosttyPoint { tag, value });
        check!(types, GhosttyGridRef { node, x, y });
        check!(
            types,
            GhosttySelection {
                start,
                end,
                rectangle
            }
        );
        check!(
            types,
            GhosttyTerminalSelectWordOptions {
                size,
                r#ref: "ref",
                boundary_codepoints,
                boundary_codepoints_len
            }
        );
        check!(
            types,
            GhosttyTerminalSelectWordBetweenOptions {
                size,
                start,
                end,
                boundary_codepoints,
                boundary_codepoints_len
            }
        );
        check!(
            types,
            GhosttyTerminalSelectLineOptions {
                size,
                r#ref: "ref",
                whitespace,
                whitespace_len,
                semantic_prompt_boundary
            }
        );
        check!(types, GhosttyTerminalScrollViewport { tag, value });
        check!(types, GhosttyTerminalScrollbar { total, offset, len });
        check!(types, GhosttyTerminalModeConfig { mode, value });
        check!(
            types,
            GhosttySizeReportSize {
                rows,
                columns,
                cell_width,
                cell_height
            }
        );
        check!(types, GhosttyTerminalDesktopNotification { title, body });
        check!(types, GhosttyTerminalProgressReport { state, progress });
        check!(types, GhosttyClipboardContent { mime, data });
        check!(
            types,
            GhosttyClipboardWriteReply {
                size,
                result,
                remember
            }
        );
        check!(
            types,
            GhosttyClipboardWrite {
                location,
                contents,
                contents_len,
                name,
                granted,
                can_remember,
                ctx,
                reply
            }
        );
        check!(
            types,
            GhosttyTerminalUnknownOscSequence {
                truncated,
                content,
                terminator
            }
        );
        check!(types, GhosttyTerminalUnknownSequence { tag, value });
        check!(
            types,
            GhosttyRenderStateCursor {
                size,
                viewport_has_value,
                viewport_x,
                viewport_y,
                wide_tail,
                visible,
                blinking,
                password_input,
                visual_style
            }
        );
        check!(
            types,
            GhosttyRenderStateColors {
                size,
                background,
                foreground,
                cursor,
                cursor_has_value,
                palette
            }
        );
        check!(
            types,
            GhosttyRenderStateRowSelection {
                size,
                start_x,
                end_x
            }
        );
        check!(
            types,
            GhosttySysImage {
                width,
                height,
                data,
                data_len
            }
        );
    }

    #[test]
    fn a_terminal_takes_text() {
        let mut terminal: GhosttyTerminal = std::ptr::null_mut();
        // SAFETY: plain calls with valid arguments; the terminal is freed at the end.
        unsafe {
            assert_eq!(
                ghostty_terminal_new(std::ptr::null(), &mut terminal, 20, 4),
                GHOSTTY_SUCCESS
            );
            let text = b"hello\r\nworld";
            ghostty_terminal_vt_write(terminal, text.as_ptr(), text.len());
            let mut x: u16 = 0;
            let mut y: u16 = 0;
            ghostty_terminal_get(
                terminal,
                GHOSTTY_TERMINAL_DATA_CURSOR_X,
                (&raw mut x).cast(),
            );
            ghostty_terminal_get(
                terminal,
                GHOSTTY_TERMINAL_DATA_CURSOR_Y,
                (&raw mut y).cast(),
            );
            assert_eq!((x, y), (5, 1));
            ghostty_terminal_free(terminal);
        }
    }
}
