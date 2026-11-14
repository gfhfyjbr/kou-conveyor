//! What a view draws, copied out of libghostty-vt's render state under the emulator's
//! lock so painting needs no lock: the cells of the viewport, the cursor, the
//! selection, the colours and the pictures with their pixels.
//!
//! The render state tracks which rows changed since the last copy; only those are read
//! again, so a frame drawn for another reason (the cursor's flight, a zoom step) costs
//! no more than the lock and a few calls.
//!
//! It also copies the row just above the viewport when there is one: a view drawn part of
//! a row lower than the viewport, while it scrolls by pixels, shows that row in the gap at
//! its top.

use crate::style;
use crate::vt::Vt;
use ghostty_vt_sys as sys;
use gpui::{Hsla, RenderImage};
use image::{Frame, RgbaImage};
use std::{
    collections::HashMap,
    ffi::{c_int, c_void},
    ptr,
    sync::Arc,
    time::{Duration, Instant},
};

/// The largest side a picture is kept at for drawing. A larger one is scaled down once
/// when it arrives: no screen shows it bigger, and the GPU atlas has a size limit.
const MAX_TEXTURE_SIDE: u32 = 4096;

/// How long a picture's pixels stay cached after it was last on screen, so scrolling a
/// picture out of view and back does not copy and upload it again.
const IMAGE_GRACE: Duration = Duration::from_secs(2);

/// Kitty's Unicode placeholder: a cell that shows part of a virtual placement. It is
/// drawn as the picture, never as a glyph.
pub(crate) const PLACEHOLDER: char = '\u{10EEEE}';

/// One cell, resolved.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CellCopy {
    /// The first codepoint; `' '` for an empty cell.
    pub character: char,
    /// Combining marks and the rest of a grapheme cluster.
    pub extra: Option<Box<[char]>>,
    pub foreground: Hsla,
    /// `None` is the default background.
    pub background: Option<Hsla>,
    /// `None` is the foreground.
    pub underline_color: Option<Hsla>,
    /// libghostty-vt's style flags, underline kind included.
    pub flags: u16,
    /// Narrow, wide, or one of the two spacer kinds.
    pub wide: c_int,
}

impl Default for CellCopy {
    fn default() -> Self {
        Self {
            character: ' ',
            extra: None,
            foreground: Hsla::default(),
            background: None,
            underline_color: None,
            flags: 0,
            wide: sys::GHOSTTY_CELL_WIDE_NARROW,
        }
    }
}

impl CellCopy {
    pub(crate) fn is_spacer(&self) -> bool {
        matches!(
            self.wide,
            sys::GHOSTTY_CELL_WIDE_SPACER_TAIL | sys::GHOSTTY_CELL_WIDE_SPACER_HEAD
        )
    }

    pub(crate) fn is_wide(&self) -> bool {
        self.wide == sys::GHOSTTY_CELL_WIDE_WIDE
    }

    pub(crate) fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    /// The underline kind, one of the `GHOSTTY_SGR_UNDERLINE_*` values.
    pub(crate) fn underline(&self) -> u16 {
        (self.flags & sys::GHOSTTY_STYLE_UNDERLINE_MASK) >> sys::GHOSTTY_STYLE_UNDERLINE_SHIFT
    }
}

/// How the cursor is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorShape {
    Block,
    Beam,
    Underline,
    HollowBlock,
}

/// The cursor, when it is in the viewport and visible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CursorCopy {
    /// The row of the copy it is on (see [`ScreenCopy::above`]).
    pub line: usize,
    pub column: usize,
    pub shape: CursorShape,
    /// The program asked for a blinking cursor, or left the terminal's default, which blinks.
    pub blinking: bool,
}

/// Where a picture is drawn, in cells and in the terminal's pixels (device pixels of a
/// cell at the size the program was told).
#[derive(Clone)]
pub(crate) struct PictureCopy {
    /// Below the cell backgrounds, between them and the text, or above the text.
    pub layer: c_int,
    pub z: i32,
    /// The top-left cell, in the viewport; negative when the picture starts above it.
    pub column: i32,
    pub line: i32,
    /// How far into that cell the picture starts.
    pub offset_x: u32,
    pub offset_y: u32,
    /// The size it is drawn at.
    pub width: u32,
    pub height: u32,
    /// The part of the picture shown, in its own pixels.
    pub source: [u32; 4],
    /// The size of `image` relative to the picture's own pixels, when it was scaled down.
    pub texture_scale: f32,
    pub image: Arc<RenderImage>,
}

/// A copy of the viewport, and of the row above it when there is one.
#[derive(Clone, Default)]
pub(crate) struct ScreenCopy {
    pub columns: usize,
    /// The rows copied: the viewport's, after the [`ScreenCopy::above`] rows before it.
    pub lines: usize,
    /// How many of the rows come before the viewport's first: one when there is history
    /// above it, otherwise none.
    pub above: usize,
    pub cells: Vec<CellCopy>,
    /// The selected columns of each line, inclusive.
    pub selection: Vec<Option<(usize, usize)>>,
    pub cursor: Option<CursorCopy>,
    pub background: Hsla,
    pub foreground: Hsla,
    pub cursor_color: Hsla,
    pub pictures: Vec<PictureCopy>,
}

impl ScreenCopy {
    pub(crate) fn row(&self, line: usize) -> &[CellCopy] {
        &self.cells[line * self.columns..(line + 1) * self.columns]
    }

    pub(crate) fn cell(&self, line: usize, column: usize) -> Option<&CellCopy> {
        (line < self.lines && column < self.columns)
            .then(|| &self.cells[line * self.columns + column])
    }
}

struct CachedImage {
    generation: u64,
    image: Arc<RenderImage>,
    texture_scale: f32,
    seen: Instant,
}

/// A view's handle on libghostty-vt's render state, and the copy it keeps of it.
pub(crate) struct RenderState {
    state: sys::GhosttyRenderState,
    rows: sys::GhosttyRenderStateRowIterator,
    cells: sys::GhosttyRenderStateRowCells,
    images: sys::GhosttyRenderStateImageIterator,
    codepoints: Vec<u32>,
    copy: ScreenCopy,
    placements_generation: Option<u64>,
    /// Pictures' pixels as textures, by screen and image id.
    textures: HashMap<(bool, u32), CachedImage>,
    /// Textures no longer used, for the view to take out of the GPU atlas.
    retired: Vec<Arc<RenderImage>>,
}

impl Drop for RenderState {
    fn drop(&mut self) {
        // SAFETY: each handle came from its `_new` function and is freed once.
        unsafe {
            sys::ghostty_render_state_image_iterator_free(self.images);
            sys::ghostty_render_state_row_cells_free(self.cells);
            sys::ghostty_render_state_row_iterator_free(self.rows);
            sys::ghostty_render_state_free(self.state);
        }
    }
}

fn check(result: sys::GhosttyResult, what: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        result == sys::GHOSTTY_SUCCESS,
        "libghostty-vt could not create {what} ({result})"
    );
    Ok(())
}

impl RenderState {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let mut state = ptr::null_mut();
        let mut rows = ptr::null_mut();
        let mut cells = ptr::null_mut();
        let mut images = ptr::null_mut();
        // SAFETY: valid out pointers; null allocators select the default. Handles that
        // were created are freed by `Drop` even when a later one fails, because `Drop`
        // only runs on a complete value, so they are freed here on failure.
        unsafe {
            let created = check(
                sys::ghostty_render_state_new(ptr::null(), &mut state),
                "a render state",
            )
            .and_then(|()| {
                // One row above the viewport is copied too: a view scrolled
                // by part of a row shows it in the gap at the top.
                let overscan = sys::GhosttyRenderStateOverscan { above: 1, below: 0 };
                check(
                    sys::ghostty_render_state_set(
                        state,
                        sys::GHOSTTY_RENDER_STATE_OPT_OVERSCAN,
                        (&raw const overscan).cast(),
                    ),
                    "a render state with a row above the viewport",
                )
            })
            .and_then(|()| {
                check(
                    sys::ghostty_render_state_row_iterator_new(ptr::null(), &mut rows),
                    "a row iterator",
                )
            })
            .and_then(|()| {
                check(
                    sys::ghostty_render_state_row_cells_new(ptr::null(), &mut cells),
                    "a cell iterator",
                )
            })
            .and_then(|()| {
                check(
                    sys::ghostty_render_state_image_iterator_new(ptr::null(), &mut images),
                    "an image iterator",
                )
            });
            if let Err(error) = created {
                if !images.is_null() {
                    sys::ghostty_render_state_image_iterator_free(images);
                }
                if !cells.is_null() {
                    sys::ghostty_render_state_row_cells_free(cells);
                }
                if !rows.is_null() {
                    sys::ghostty_render_state_row_iterator_free(rows);
                }
                if !state.is_null() {
                    sys::ghostty_render_state_free(state);
                }
                return Err(error);
            }
        }
        Ok(Self {
            state,
            rows,
            cells,
            images,
            codepoints: vec![0; 16],
            copy: ScreenCopy::default(),
            placements_generation: None,
            textures: HashMap::new(),
            retired: Vec::new(),
        })
    }

    /// The copy as of the last [`RenderState::update`].
    pub(crate) fn copy(&self) -> &ScreenCopy {
        &self.copy
    }

    /// Textures that left the screen for good; the caller removes them from the GPU
    /// atlas.
    pub(crate) fn take_retired(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.retired)
    }

    /// Every texture this state ever handed out, for the view to take out of the GPU
    /// atlas when it goes away.
    pub(crate) fn release_all(&mut self) -> Vec<Arc<RenderImage>> {
        let mut all = std::mem::take(&mut self.retired);
        all.extend(self.textures.drain().map(|(_, cached)| cached.image));
        self.copy.pictures.clear();
        all
    }

    /// Brings the copy up to date with the emulator. Call with the emulator locked.
    pub(crate) fn update(&mut self, vt: &Vt) {
        // SAFETY: valid handles; the terminal is locked by the caller, which is what the
        // render state reads from.
        unsafe {
            if sys::ghostty_render_state_update(self.state, vt.raw()) != sys::GHOSTTY_SUCCESS {
                return;
            }
        }
        let columns = usize::from(self.get::<u16>(sys::GHOSTTY_RENDER_STATE_DATA_COLS));
        let overscan =
            self.get::<sys::GhosttyRenderStateOverscan>(sys::GHOSTTY_RENDER_STATE_DATA_OVERSCAN);
        let above = usize::from(overscan.above);
        let viewport = usize::from(self.get::<u16>(sys::GHOSTTY_RENDER_STATE_DATA_ROWS));
        let lines = viewport + above + usize::from(overscan.below);
        let dirty = self.get::<c_int>(sys::GHOSTTY_RENDER_STATE_DATA_DIRTY);
        let resized =
            columns != self.copy.columns || lines != self.copy.lines || above != self.copy.above;
        let everything = resized || dirty == sys::GHOSTTY_RENDER_STATE_DIRTY_FULL;
        if resized {
            self.copy.columns = columns;
            self.copy.lines = lines;
            self.copy.above = above;
            self.copy.cells = vec![CellCopy::default(); columns * lines];
            self.copy.selection = vec![None; lines];
        }

        let mut colors = sys::GhosttyRenderStateColors {
            size: size_of::<sys::GhosttyRenderStateColors>(),
            background: Default::default(),
            foreground: Default::default(),
            cursor: Default::default(),
            cursor_has_value: false,
            palette: [Default::default(); 256],
        };
        // SAFETY: the documented sized output type.
        unsafe {
            sys::ghostty_render_state_get(
                self.state,
                sys::GHOSTTY_RENDER_STATE_DATA_COLORS,
                (&raw mut colors).cast(),
            );
        }
        let color = |rgb: sys::GhosttyColorRgb| style::from_bytes(rgb.r, rgb.g, rgb.b);
        let background = color(colors.background);
        let foreground = color(colors.foreground);
        // Colours changed (a theme, or a program's OSC 10/11): every cell may resolve
        // differently.
        let recolored = background != self.copy.background || foreground != self.copy.foreground;
        self.copy.background = background;
        self.copy.foreground = foreground;
        self.copy.cursor_color = if colors.cursor_has_value {
            color(colors.cursor)
        } else {
            foreground
        };

        self.read_rows(&colors, everything || recolored);
        self.read_cursor();
        self.read_pictures(vt);
        // SAFETY: a valid handle.
        unsafe { sys::ghostty_render_state_clean(self.state) };
    }

    fn get<T: Default>(&self, data: c_int) -> T {
        let mut value = T::default();
        // SAFETY: each caller names the output type its key documents.
        unsafe { sys::ghostty_render_state_get(self.state, data, (&raw mut value).cast()) };
        value
    }

    fn read_rows(&mut self, colors: &sys::GhosttyRenderStateColors, everything: bool) {
        let columns = self.copy.columns;
        // SAFETY: the iterators are valid and filled from the state just updated.
        unsafe {
            if sys::ghostty_render_state_get(
                self.state,
                sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&raw mut self.rows).cast(),
            ) != sys::GHOSTTY_SUCCESS
            {
                return;
            }
            let mut line = 0;
            while line < self.copy.lines && sys::ghostty_render_state_row_iterator_next(self.rows) {
                let mut selection = sys::GhosttyRenderStateRowSelection {
                    size: size_of::<sys::GhosttyRenderStateRowSelection>(),
                    ..Default::default()
                };
                let selected = sys::ghostty_render_state_row_get(
                    self.rows,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                    (&raw mut selection).cast(),
                ) == sys::GHOSTTY_SUCCESS;
                self.copy.selection[line] = selected
                    .then(|| (usize::from(selection.start_x), usize::from(selection.end_x)));

                let mut dirty = false;
                sys::ghostty_render_state_row_get(
                    self.rows,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                    (&raw mut dirty).cast(),
                );
                if (everything || dirty)
                    && sys::ghostty_render_state_row_get(
                        self.rows,
                        sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                        (&raw mut self.cells).cast(),
                    ) == sys::GHOSTTY_SUCCESS
                {
                    let row = &mut self.copy.cells[line * columns..(line + 1) * columns];
                    let mut column = 0;
                    while column < columns && sys::ghostty_render_state_row_cells_next(self.cells) {
                        row[column] = read_cell(self.cells, colors, &mut self.codepoints);
                        column += 1;
                    }
                    for cell in &mut row[column..] {
                        *cell = CellCopy {
                            foreground: style::from_bytes(
                                colors.foreground.r,
                                colors.foreground.g,
                                colors.foreground.b,
                            ),
                            ..CellCopy::default()
                        };
                    }
                }
                line += 1;
            }
        }
    }

    fn read_cursor(&mut self) {
        let mut cursor = sys::GhosttyRenderStateCursor {
            size: size_of::<sys::GhosttyRenderStateCursor>(),
            ..Default::default()
        };
        // SAFETY: the documented sized output type.
        unsafe {
            sys::ghostty_render_state_get(
                self.state,
                sys::GHOSTTY_RENDER_STATE_DATA_CURSOR,
                (&raw mut cursor).cast(),
            );
        }
        let shape = match cursor.visual_style {
            sys::GHOSTTY_CURSOR_STYLE_BAR => CursorShape::Beam,
            sys::GHOSTTY_CURSOR_STYLE_UNDERLINE => CursorShape::Underline,
            sys::GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW => CursorShape::HollowBlock,
            _ => CursorShape::Block,
        };
        self.copy.cursor = (cursor.visible && cursor.viewport_has_value).then(|| CursorCopy {
            line: usize::from(cursor.viewport_y) + self.copy.above,
            // On the right half of a wide character, the cursor covers the whole of it.
            column: usize::from(cursor.viewport_x).saturating_sub(usize::from(cursor.wide_tail)),
            shape,
            blinking: cursor.blinking,
        });
    }

    fn read_pictures(&mut self, vt: &Vt) {
        let generation =
            self.get::<u64>(sys::GHOSTTY_RENDER_STATE_DATA_IMAGE_PLACEMENTS_GENERATION);
        let now = Instant::now();
        if self.placements_generation != Some(generation) {
            self.placements_generation = Some(generation);
            self.copy.pictures.clear();
            let alt = vt.alt_screen();
            let mut graphics: sys::GhosttyKittyGraphics = ptr::null_mut();
            // SAFETY: valid handles; the storage pointer is used before the terminal
            // changes, under the caller's lock.
            unsafe {
                sys::ghostty_terminal_get(
                    vt.raw(),
                    sys::GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS,
                    (&raw mut graphics).cast(),
                );
                if sys::ghostty_render_state_get(
                    self.state,
                    sys::GHOSTTY_RENDER_STATE_DATA_IMAGE_ITERATOR,
                    (&raw mut self.images).cast(),
                ) == sys::GHOSTTY_SUCCESS
                {
                    while sys::ghostty_render_state_image_iterator_next(self.images) {
                        if let Some(picture) = self.read_picture(graphics, alt, now) {
                            self.copy.pictures.push(picture);
                        }
                    }
                }
            }
            // Within a layer, lower z first; equal z keeps the library's order.
            self.copy
                .pictures
                .sort_by_key(|picture| (picture.layer, picture.z));
        }

        for picture in &self.copy.pictures {
            if let Some(cached) = self
                .textures
                .values_mut()
                .find(|cached| Arc::ptr_eq(&cached.image, &picture.image))
            {
                cached.seen = now;
            }
        }
        let retired = &mut self.retired;
        self.textures.retain(|_, cached| {
            let keep = now.duration_since(cached.seen) < IMAGE_GRACE;
            if !keep {
                retired.push(cached.image.clone());
            }
            keep
        });
    }

    /// The current placement of the image iterator, with its pixels.
    ///
    /// # Safety
    ///
    /// The iterator must be on a placement and `graphics` must be the storage of the
    /// locked terminal it was read from.
    unsafe fn read_picture(
        &mut self,
        graphics: sys::GhosttyKittyGraphics,
        alt: bool,
        now: Instant,
    ) -> Option<PictureCopy> {
        let get = |data: c_int, out: *mut c_void| {
            // SAFETY: guaranteed by the caller; `out` matches the key's documented type.
            unsafe {
                sys::ghostty_render_state_image_get(self.images, data, out) == sys::GHOSTTY_SUCCESS
            }
        };
        let mut protocol: c_int = -1;
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_PROTOCOL,
            (&raw mut protocol).cast(),
        );
        if protocol != sys::GHOSTTY_IMAGE_PROTOCOL_KITTY || graphics.is_null() {
            return None;
        }
        let mut id: u32 = 0;
        let mut generation: u64 = 0;
        let mut layer: c_int = sys::GHOSTTY_IMAGE_LAYER_ABOVE_TEXT;
        let mut z: i32 = 0;
        let (mut column, mut line) = (0i32, 0i32);
        let (mut offset_x, mut offset_y) = (0u32, 0u32);
        let (mut width, mut height) = (0u32, 0u32);
        let mut source = [0u32; 4];
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_ID,
            (&raw mut id).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_GENERATION,
            (&raw mut generation).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_LAYER,
            (&raw mut layer).cast(),
        );
        get(sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_Z, (&raw mut z).cast());
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_COL,
            (&raw mut column).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_ROW,
            (&raw mut line).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_X,
            (&raw mut offset_x).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_Y,
            (&raw mut offset_y).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_WIDTH,
            (&raw mut width).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_HEIGHT,
            (&raw mut height).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_X,
            (&raw mut source[0]).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_Y,
            (&raw mut source[1]).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_WIDTH,
            (&raw mut source[2]).cast(),
        );
        get(
            sys::GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_HEIGHT,
            (&raw mut source[3]).cast(),
        );
        if width == 0 || height == 0 || source[2] == 0 || source[3] == 0 {
            return None;
        }

        let key = (alt, id);
        let cached = match self.textures.get_mut(&key) {
            Some(cached) if cached.generation == generation => {
                cached.seen = now;
                (cached.image.clone(), cached.texture_scale)
            }
            _ => {
                // SAFETY: guaranteed by the caller.
                let (image, texture_scale) = unsafe { texture(graphics, id) }?;
                if let Some(old) = self.textures.insert(
                    key,
                    CachedImage {
                        generation,
                        image: image.clone(),
                        texture_scale,
                        seen: now,
                    },
                ) {
                    self.retired.push(old.image);
                }
                (image, texture_scale)
            }
        };
        Some(PictureCopy {
            layer,
            z,
            column,
            line,
            offset_x,
            offset_y,
            width,
            height,
            source,
            texture_scale: cached.1,
            image: cached.0,
        })
    }
}

/// Reads the iterator's current cell.
///
/// # Safety
///
/// `cells` must be positioned on a cell of a freshly updated render state.
unsafe fn read_cell(
    cells: sys::GhosttyRenderStateRowCells,
    colors: &sys::GhosttyRenderStateColors,
    codepoints: &mut Vec<u32>,
) -> CellCopy {
    let rgb = |rgb: sys::GhosttyColorRgb| style::from_bytes(rgb.r, rgb.g, rgb.b);
    let get = |data: c_int, out: *mut c_void| {
        // SAFETY: guaranteed by the caller; each `out` matches its key's type.
        unsafe { sys::ghostty_render_state_row_cells_get(cells, data, out) == sys::GHOSTTY_SUCCESS }
    };

    let mut raw: sys::GhosttyCell = 0;
    get(
        sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
        (&raw mut raw).cast(),
    );
    let mut wide: c_int = sys::GHOSTTY_CELL_WIDE_NARROW;
    // SAFETY: a raw cell value and the key's documented output type.
    unsafe { sys::ghostty_cell_get(raw, sys::GHOSTTY_CELL_DATA_WIDE, (&raw mut wide).cast()) };

    let mut length: u32 = 0;
    get(
        sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
        (&raw mut length).cast(),
    );
    let (character, extra) = if length == 0 {
        (' ', None)
    } else {
        if codepoints.len() < length as usize {
            codepoints.resize(length as usize, 0);
        }
        let mut buffer = sys::GhosttyCodepointBuffer {
            ptr: codepoints.as_mut_ptr(),
            cap: codepoints.len(),
            len: 0,
        };
        get(
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
            (&raw mut buffer).cast(),
        );
        let count = buffer.len.min(buffer.cap);
        let mut chars = codepoints[..count]
            .iter()
            .filter_map(|&value| char::from_u32(value));
        let first = chars.next().unwrap_or(' ');
        let rest: Box<[char]> = chars.collect();
        (first, (!rest.is_empty()).then_some(rest))
    };

    let mut styled = false;
    get(
        sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
        (&raw mut styled).cast(),
    );
    let default_foreground = rgb(colors.foreground);
    let mut cell = CellCopy {
        character,
        extra,
        foreground: default_foreground,
        background: None,
        underline_color: None,
        flags: 0,
        wide,
    };
    if styled {
        let mut style = sys::GhosttyStyle {
            fg_color: sys::GhosttyStyleColor {
                tag: 0,
                value: sys::GhosttyStyleColorValue { palette: 0 },
            },
            bg_color: sys::GhosttyStyleColor {
                tag: 0,
                value: sys::GhosttyStyleColorValue { palette: 0 },
            },
            underline_color: sys::GhosttyStyleColor {
                tag: 0,
                value: sys::GhosttyStyleColorValue { palette: 0 },
            },
            flags: 0,
            reserved: 0,
        };
        get(
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
            (&raw mut style).cast(),
        );
        cell.flags = style.flags;
        cell.foreground = resolve(style.fg_color, colors).unwrap_or(default_foreground);
        cell.underline_color = resolve(style.underline_color, colors);
    }
    // The background has three possible sources (the style, and two kinds of blank cell
    // that carry only a colour); the library resolves them all.
    let mut background = sys::GhosttyColorRgb::default();
    if get(
        sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
        (&raw mut background).cast(),
    ) {
        cell.background = Some(rgb(background));
    }
    cell
}

fn resolve(color: sys::GhosttyStyleColor, colors: &sys::GhosttyRenderStateColors) -> Option<Hsla> {
    // SAFETY: the tag says which union member is set.
    let rgb = unsafe {
        match color.tag {
            sys::GHOSTTY_STYLE_COLOR_PALETTE => colors.palette[usize::from(color.value.palette)],
            sys::GHOSTTY_STYLE_COLOR_RGB => color.value.rgb,
            _ => return None,
        }
    };
    Some(style::from_bytes(rgb.r, rgb.g, rgb.b))
}

/// A picture's pixels as a texture: BGRA, scaled down when larger than
/// [`MAX_TEXTURE_SIDE`]. Returns the texture and its scale relative to the picture.
///
/// # Safety
///
/// `graphics` must be the storage of the locked terminal.
unsafe fn texture(graphics: sys::GhosttyKittyGraphics, id: u32) -> Option<(Arc<RenderImage>, f32)> {
    // SAFETY: guaranteed by the caller; the pixels are copied before the lock is released.
    let (width, height, format, pixels) = unsafe {
        let image = sys::ghostty_kitty_graphics_image(graphics, id);
        if image.is_null() {
            return None;
        }
        let mut width: u32 = 0;
        let mut height: u32 = 0;
        let mut format: c_int = -1;
        let mut pixels = sys::GhosttyString {
            ptr: ptr::null(),
            len: 0,
        };
        sys::ghostty_kitty_graphics_image_get(
            image,
            sys::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_WIDTH,
            (&raw mut width).cast(),
        );
        sys::ghostty_kitty_graphics_image_get(
            image,
            sys::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_HEIGHT,
            (&raw mut height).cast(),
        );
        sys::ghostty_kitty_graphics_image_get(
            image,
            sys::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_FORMAT,
            (&raw mut format).cast(),
        );
        if sys::ghostty_kitty_graphics_image_get(
            image,
            sys::GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_PIXELS,
            (&raw mut pixels).cast(),
        ) != sys::GHOSTTY_SUCCESS
            || pixels.ptr.is_null()
        {
            return None;
        }
        (
            width,
            height,
            format,
            std::slice::from_raw_parts(pixels.ptr, pixels.len),
        )
    };
    let bgra = to_bgra(width, height, format, pixels)?;
    let mut buffer = RgbaImage::from_raw(width, height, bgra)?;
    let mut scale = 1.;
    let largest = width.max(height);
    if largest > MAX_TEXTURE_SIDE {
        scale = MAX_TEXTURE_SIDE as f32 / largest as f32;
        let scaled_width = ((width as f32 * scale).round() as u32).max(1);
        let scaled_height = ((height as f32 * scale).round() as u32).max(1);
        buffer = image::imageops::resize(
            &buffer,
            scaled_width,
            scaled_height,
            image::imageops::FilterType::Triangle,
        );
    }
    Some((Arc::new(RenderImage::new(vec![Frame::new(buffer)])), scale))
}

/// Pixels in any of the library's stored formats as BGRA, the order GPUI's textures use.
pub(crate) fn to_bgra(width: u32, height: u32, format: c_int, pixels: &[u8]) -> Option<Vec<u8>> {
    let count = width as usize * height as usize;
    let mut out = Vec::with_capacity(count * 4);
    let channels = match format {
        sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA => 4,
        sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGB => 3,
        sys::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA => 2,
        sys::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY => 1,
        _ => return None,
    };
    if pixels.len() < count * channels {
        return None;
    }
    for pixel in pixels[..count * channels].chunks_exact(channels) {
        let (r, g, b, a) = match channels {
            4 => (pixel[0], pixel[1], pixel[2], pixel[3]),
            3 => (pixel[0], pixel[1], pixel[2], 255),
            2 => (pixel[0], pixel[0], pixel[0], pixel[1]),
            _ => (pixel[0], pixel[0], pixel[0], 255),
        };
        out.extend_from_slice(&[b, g, r, a]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stored_format_becomes_bgra() {
        assert_eq!(
            to_bgra(1, 1, sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGBA, &[1, 2, 3, 4]),
            Some(vec![3, 2, 1, 4])
        );
        assert_eq!(
            to_bgra(1, 1, sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGB, &[1, 2, 3]),
            Some(vec![3, 2, 1, 255])
        );
        assert_eq!(
            to_bgra(1, 1, sys::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA, &[9, 7]),
            Some(vec![9, 9, 9, 7])
        );
        assert_eq!(
            to_bgra(2, 1, sys::GHOSTTY_KITTY_IMAGE_FORMAT_GRAY, &[5, 6]),
            Some(vec![5, 5, 5, 255, 6, 6, 6, 255])
        );
        // Too few bytes for the size, or a format that is never stored.
        assert_eq!(
            to_bgra(2, 2, sys::GHOSTTY_KITTY_IMAGE_FORMAT_RGB, &[0; 5]),
            None
        );
        assert_eq!(
            to_bgra(1, 1, sys::GHOSTTY_KITTY_IMAGE_FORMAT_PNG, &[0; 4]),
            None
        );
    }

    #[test]
    fn the_row_above_the_viewport_is_copied_before_it() {
        let mut harness = crate::vt::tests::Harness::new(10, 4);
        let lines: Vec<String> = (0..20).map(|line| format!("L{line}")).collect();
        harness.write(lines.join("\r\n"));
        let text = |copy: &ScreenCopy, row: usize| {
            let row: String = copy.row(row).iter().map(|cell| cell.character).collect();
            row.trim_end().to_string()
        };
        let mut render = RenderState::new().expect("a render state");

        // On the live screen the row above is the history's last, and the cursor's row
        // counts it.
        render.update(&harness.vt);
        let copy = render.copy();
        assert_eq!((copy.above, copy.lines), (1, 5));
        assert_eq!(text(copy, 0), "L15");
        assert_eq!(text(copy, 1), "L16");
        assert_eq!(copy.cursor.map(|cursor| cursor.line), Some(4));

        // Scrolled back, the rows move with the viewport.
        harness.vt.scroll(3);
        render.update(&harness.vt);
        let copy = render.copy();
        assert_eq!(text(copy, 0), "L12");
        assert_eq!(text(copy, 1), "L13");
        assert_eq!(text(copy, 4), "L16");
        assert_eq!(copy.cursor, None);

        // At the top of the history there is nothing above.
        harness.vt.scroll_to_top();
        render.update(&harness.vt);
        let copy = render.copy();
        assert_eq!((copy.above, copy.lines), (0, 4));
        assert_eq!(text(copy, 0), "L0");
    }
}
