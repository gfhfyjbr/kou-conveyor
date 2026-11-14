//! Text laid out at its unscaled size and drawn at the factor in effect.
//!
//! The system font is tracked and drawn by its point size: set at twice the
//! size it comes out about 9% narrower than twice as wide, and at 0.6 of the
//! size about 7% wider. Text set at the scaled size would therefore wrap
//! elsewhere than it does at 1, and a conversation would reflow as it is
//! zoomed. This element shapes and wraps the text at the size it has at a
//! factor of 1, in the width it has at 1, and draws each glyph where that
//! layout puts it times the factor, at the scaled size. Its lines are the
//! lines it has at 1, whatever the factor.

use super::{
    exact::{self, TextRole},
    factor, moving, quantize_staggered, sane, steady,
};
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, DecorationRun, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, LineLayout, Pixels, ShapedGlyph, ShapedRun,
    SharedString, Size, Style, TextAlign, TextOverflow, TextRun, TruncateFrom, WhiteSpace, Window,
    WrappedLine, point, size,
};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

thread_local! {
    /// The text work done since it was last taken, while it is counted.
    static WORK: Cell<Option<TextWork>> = const { Cell::new(None) };
    /// Where text was placed since that was last taken, while work is counted.
    static PLACES: RefCell<Vec<Bounds<Pixels>>> = const { RefCell::new(Vec::new()) };
}

/// How many times text in a scaled subtree was laid out (shaped, wrapped and
/// truncated at its unscaled size) and how long that took. Counted after
/// [`count_text_work`] turns counting on, to see what a frame of a zoom costs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextWork {
    pub layouts: u32,
    pub time: Duration,
}

/// Counts [`TextWork`] on this thread from now on, and notes where each text
/// is placed (see [`take_text_places`]), or stops.
pub fn count_text_work(on: bool) {
    WORK.with(|work| work.set(on.then(TextWork::default)));
    PLACES.with(|places| places.borrow_mut().clear());
}

/// The text work counted since the last call, starting again from nothing;
/// `None` while it is not counted.
pub fn take_text_work() -> Option<TextWork> {
    WORK.with(|work| {
        let taken = work.get();
        if taken.is_some() {
            work.set(Some(TextWork::default()));
        }
        taken
    })
}

/// The bounds of every text in a scaled subtree prepainted since the last
/// call, in the order they were prepainted, while work is counted: where a
/// zoom put each paragraph, to compare with where it belongs.
pub fn take_text_places() -> Vec<Bounds<Pixels>> {
    PLACES.with(|places| std::mem::take(&mut *places.borrow_mut()))
}

/// Adds a layout that began at `started` to the work counted.
fn count_layout(started: Instant) {
    WORK.with(|work| {
        if let Some(mut counted) = work.get() {
            counted.layouts += 1;
            counted.time += started.elapsed();
            work.set(Some(counted));
        }
    });
}

/// How far, in device pixels, a text that fits its box at 1 may stick out of
/// it at a zoom and still not wrap: what rounding a few paddings and widths to
/// whole device pixels can take from the box.
const WRAP_SLACK: f32 = 2.;

/// `text` in the text style in effect: GPUI's own text element at a factor of
/// 1, so that nothing changes outside a scaled subtree, and [`ScaledText`] at
/// any other factor, or while a plan is recorded (see [`exact`]).
pub fn text(text: impl Into<SharedString>) -> AnyElement {
    let text = text.into();
    if factor() == 1. && !exact::recording() {
        text.into_any_element()
    } else {
        ScaledText::new(text).into_any_element()
    }
}

/// `text` in runs of fonts and colours, as [`gpui::StyledText`] takes them:
/// GPUI's own at a factor of 1, [`ScaledText`] at any other.
pub fn styled_text(text: impl Into<SharedString>, runs: Vec<TextRun>) -> AnyElement {
    let text = text.into();
    if factor() == 1. && !exact::recording() {
        gpui::StyledText::new(text)
            .with_runs(runs)
            .into_any_element()
    } else {
        ScaledText::new(text).with_runs(runs).into_any_element()
    }
}

/// Text that wraps at a factor where it wraps at 1. See the [module](self).
///
/// It takes the text style in effect where it is drawn (font, size, line
/// height, colour, white space, alignment) as GPUI's text does, with the size
/// and line height scaled as everything in a scaled subtree is. It wraps
/// (`white_space` normal) or does not; it does not truncate.
pub struct ScaledText {
    text: SharedString,
    runs: Option<Vec<TextRun>>,
    layout: ScaledTextLayout,
    /// What it takes part in of a plan being recorded or followed.
    role: TextRole,
}

impl ScaledText {
    /// The text in the style in effect.
    pub fn new(text: impl Into<SharedString>) -> Self {
        let text = text.into();
        let role = exact::text_role(&text);
        Self {
            text,
            runs: None,
            layout: ScaledTextLayout::default(),
            role,
        }
    }

    /// The text in these runs instead of one run of the style in effect.
    pub fn with_runs(mut self, runs: Vec<TextRun>) -> Self {
        self.runs = Some(runs);
        self
    }

    /// Where the text came to be drawn, to find the rectangles of a part of
    /// it once it is prepainted.
    pub fn layout(&self) -> &ScaledTextLayout {
        &self.layout
    }
}

/// A [`ScaledText`]'s layout, shared with whoever needs to know where its
/// text was drawn.
#[derive(Clone, Default)]
pub struct ScaledTextLayout(Rc<RefCell<Option<Measured>>>);

struct Measured {
    text: SharedString,
    /// The runs of `text`.
    given_runs: Vec<TextRun>,
    /// Each line of what is shown, shaped and wrapped at the unscaled size.
    lines: Vec<WrappedLine>,
    /// The runs of what is shown: `given_runs`, or those of the text truncated.
    runs: Vec<TextRun>,
    /// The width, as drawn, the lines were laid out in.
    laid_in: Option<Option<Pixels>>,
    /// The unscaled width it wraps in whatever its box, from a plan.
    planned: Option<Pixels>,
    factor: f32,
    /// The unscaled font size.
    font_size: Pixels,
    /// When the glyphs change to the next size in a zoom: see [`phase`].
    phase: f32,
    /// The line pitch as drawn: the unscaled pitch times the factor, not
    /// snapped to device pixels, so that many lines add up to their height at
    /// 1 times the factor. Each row's glyphs still land on whole pixels.
    line_height: Pixels,
    align: TextAlign,
    wraps: bool,
    overflow: Option<TextOverflow>,
    font: gpui::Font,
    bounds: Option<Bounds<Pixels>>,
}

/// One row of a wrapped line: the glyphs on it and where it starts, unscaled.
struct Row {
    line: usize,
    /// Byte range in the line's text.
    range: Range<usize>,
    /// Where the row starts and ends on the unwrapped line.
    start_x: Pixels,
    end_x: Pixels,
}

/// The rows a wrapped line is drawn in, as line `index` of a text.
fn line_rows(index: usize, line: &WrappedLine, rows: &mut Vec<Row>) {
    let layout = &line.unwrapped_layout;
    let mut start = (0, Pixels::ZERO);
    for boundary in line.wrap_boundaries() {
        let glyph = &layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
        rows.push(Row {
            line: index,
            range: start.0..glyph.index,
            start_x: start.1,
            end_x: glyph.position.x,
        });
        start = (glyph.index, glyph.position.x);
    }
    rows.push(Row {
        line: index,
        range: start.0..layout.len,
        start_x: start.1,
        end_x: layout.width,
    });
}

/// Where between two glyph sizes `text` changes from one to the next (see
/// [`quantize_staggered`]), as a number from 0 to 1 that its first sixteen
/// bytes decide: the same while the text grows at its end, as an answer does
/// while it streams in, and different from one paragraph to the next.
pub fn phase(text: &str) -> f32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes().take(16) {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    (hash >> 8) as f32 / (1u32 << 24) as f32
}

/// A row's glyphs as a line of their own, scaled, their pictures at the size
/// [`quantize_staggered`] gives for `phase`. With `whole`, the left edge of
/// the row as drawn and the device pixels to one, each glyph is moved to the
/// nearest whole device pixel (see [`super::Scaled::moving`]).
fn row_layout(
    line: &WrappedLine,
    row: &Row,
    factor: f32,
    phase: f32,
    whole: Option<(Pixels, f32)>,
) -> LineLayout {
    let place = |x: Pixels| match whole {
        Some((left, device)) => gpui::px(((f32::from(left + x)) * device).round() / device) - left,
        None => x,
    };
    let layout = &line.unwrapped_layout;
    let runs = layout
        .runs
        .iter()
        .filter_map(|run| {
            let glyphs: Vec<ShapedGlyph> = run
                .glyphs
                .iter()
                .filter(|glyph| row.range.contains(&glyph.index))
                .map(|glyph| ShapedGlyph {
                    id: glyph.id,
                    position: point(
                        place((glyph.position.x - row.start_x) * factor),
                        glyph.position.y * factor,
                    ),
                    index: glyph.index - row.range.start,
                    is_emoji: glyph.is_emoji,
                })
                .collect();
            (!glyphs.is_empty()).then_some(ShapedRun {
                font_id: run.font_id,
                glyphs,
            })
        })
        .collect();
    LineLayout {
        // Each glyph stands where the factor puts it; its picture is one of
        // a few sizes (see `quantize_staggered`), not one more size a frame.
        font_size: layout.font_size * quantize_staggered(factor, phase),
        width: (row.end_x - row.start_x) * factor,
        ascent: layout.ascent * factor,
        descent: layout.descent * factor,
        runs,
        len: row.range.len(),
    }
}

/// The decoration runs of the bytes `range` of a text in `runs`.
fn decorations(runs: &[TextRun], range: Range<usize>) -> Vec<DecorationRun> {
    let mut decorations = Vec::new();
    let mut at = 0;
    for run in runs {
        let run_range = at..at + run.len;
        at += run.len;
        let from = run_range.start.max(range.start);
        let to = run_range.end.min(range.end);
        if from < to {
            decorations.push(DecorationRun {
                len: (to - from) as u32,
                color: run.color,
                background_color: run.background_color,
                underline: run.underline,
                strikethrough: run.strikethrough,
            });
        }
    }
    decorations
}

/// Paints a row laid out by [`row_layout`] with its top left at `origin`.
fn paint_row(
    layout: &LineLayout,
    origin: gpui::Point<Pixels>,
    line_height: Pixels,
    decorations: &[DecorationRun],
    window: &mut Window,
    cx: &mut App,
) {
    // A glyph that cannot be drawn leaves a gap, as it does in GPUI's own text.
    let _ = layout.paint_background(
        origin,
        line_height,
        TextAlign::Left,
        None,
        decorations,
        window,
        cx,
    );
    let _ = layout.paint(
        origin,
        line_height,
        TextAlign::Left,
        None,
        decorations,
        window,
        cx,
    );
}

/// Paints `line`, one of the lines `shape_text` gave for a text in `runs`
/// shaped at its unscaled size, scaled by `factor`: its rows one under the
/// other from `origin`, `line_height` apart (the pitch as drawn), each glyph
/// where the unscaled layout puts it times the factor. `line_start` is where
/// the line starts in the text. For text that keeps its own layout, as a
/// field does; [`ScaledText`] draws its lines this way.
#[allow(clippy::too_many_arguments)]
pub fn paint_line(
    line: &WrappedLine,
    runs: &[TextRun],
    line_start: usize,
    origin: gpui::Point<Pixels>,
    line_height: Pixels,
    factor: f32,
    window: &mut Window,
    cx: &mut App,
) {
    let mut rows = Vec::new();
    line_rows(0, line, &mut rows);
    for (at, row) in rows.iter().enumerate() {
        // The field's text changes anywhere as it is typed: at the nearest size.
        let layout = row_layout(line, row, factor, 0.5, None);
        let decorations = decorations(
            runs,
            line_start + row.range.start..line_start + row.range.end,
        );
        let top = origin.y + line_height * at as f32;
        paint_row(
            &layout,
            point(origin.x, top),
            line_height,
            &decorations,
            window,
            cx,
        );
    }
}

impl Measured {
    /// Lays the text out in `width` as drawn, or unconstrained: wrapped and
    /// truncated at the unscaled size in the unscaled width, as it is at 1.
    fn lay_out(&mut self, width: Option<Pixels>, window: &mut Window, cx: &mut App) {
        if self.laid_in == Some(width) {
            return;
        }
        self.laid_in = Some(width);
        let started = WORK.with(|work| work.get().is_some()).then(Instant::now);
        let planned = width.and(self.planned);
        let width = planned.or(width.map(|width| steady(width / self.factor)));
        let (font_size, text) = (self.font_size, self.text.clone());
        let wrap = width.filter(|_| self.wraps);
        let truncated = match (&self.overflow, width) {
            (Some(overflow), Some(width)) => {
                let fits = window
                    .text_system()
                    .shape_text(text.clone(), font_size, &self.given_runs, None, None)
                    .is_ok_and(|lines| lines.iter().all(|line| line.width() <= width));
                (!fits).then(|| {
                    let (affix, from) = match overflow {
                        TextOverflow::Truncate(affix) => (affix.clone(), TruncateFrom::End),
                        TextOverflow::TruncateStart(affix) => (affix.clone(), TruncateFrom::Start),
                        TextOverflow::TruncateMiddle(affix) => {
                            (affix.clone(), TruncateFrom::Middle)
                        }
                    };
                    let mut wrapper = cx.text_system().line_wrapper(self.font.clone(), font_size);
                    let (text, runs) =
                        wrapper.truncate_line(text.clone(), width, &affix, &self.given_runs, from);
                    (text, runs.into_owned())
                })
            }
            _ => None,
        };
        let (text, runs) = truncated.unwrap_or_else(|| (text, self.given_runs.clone()));
        // A text that is one line at 1 stays one line: its box, drawn at a
        // zoom, is rounded to whole device pixels piece by piece, and can come
        // out a little narrower than the text, which would wrap it at some
        // zooms and not at others.
        // Planned, it wraps where it does at 1, which is what that is for.
        let slack = gpui::px(WRAP_SLACK / (self.factor * window.scale_factor().max(1.)));
        let wrap = wrap.filter(|width| {
            planned.is_some()
                || window
                    .text_system()
                    .shape_text(text.clone(), font_size, &runs, None, None)
                    .map_or(true, |lines| {
                        lines.iter().any(|line| line.width() > *width + slack)
                    })
        });
        self.lines = window
            .text_system()
            .shape_text(text, font_size, &runs, wrap, None)
            .map(|lines| lines.into_iter().collect())
            .unwrap_or_default();
        self.runs = runs;
        if let Some(started) = started {
            count_layout(started);
        }
    }

    /// The size of the text as drawn.
    fn size(&self) -> Size<Pixels> {
        let mut width = Pixels::ZERO;
        let mut rows = 0;
        for line in &self.lines {
            width = width.max(line.width());
            rows += line.wrap_boundaries().len() + 1;
        }
        // Rounded up to a whole unscaled pixel, as GPUI's text is at 1, and
        // then scaled: the text takes the room it takes at 1, times the factor.
        size(
            width.ceil() * self.factor,
            self.line_height * rows.max(1) as f32,
        )
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            line_rows(index, line, &mut rows);
        }
        rows
    }

    /// Where row `row` is drawn: its left edge after alignment, and its top.
    fn row_origin(&self, row: &Row, at: usize, bounds: Bounds<Pixels>) -> gpui::Point<Pixels> {
        let width = (row.end_x - row.start_x) * self.factor;
        let x = match self.align {
            TextAlign::Left => bounds.origin.x,
            TextAlign::Center => bounds.origin.x + (bounds.size.width - width) / 2.,
            TextAlign::Right => bounds.origin.x + bounds.size.width - width,
        };
        point(x, bounds.origin.y + self.line_height * at as f32)
    }

    /// Where each line starts in the whole text.
    fn line_starts(&self) -> Vec<usize> {
        let mut starts = Vec::with_capacity(self.lines.len());
        let mut at = 0;
        for line in &self.lines {
            starts.push(at);
            at += line.len() + 1;
        }
        starts
    }
}

impl ScaledTextLayout {
    /// The rectangles the bytes `range` of the text cover as drawn, one per
    /// row they are on, each as wide as their glyphs there. Empty until the
    /// text is prepainted.
    pub fn rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let measured = self.0.borrow();
        let Some(measured) = measured.as_ref() else {
            return Vec::new();
        };
        let Some(bounds) = measured.bounds else {
            return Vec::new();
        };
        let starts = measured.line_starts();
        let mut rects = Vec::new();
        for (at, row) in measured.rows().iter().enumerate() {
            let start = starts[row.line];
            let from = range.start.max(start + row.range.start);
            let to = range.end.min(start + row.range.end);
            if from >= to {
                continue;
            }
            let layout = &measured.lines[row.line].unwrapped_layout;
            let x_of = |index: usize| -> Pixels {
                if index >= start + layout.len {
                    return layout.width;
                }
                layout.x_for_index(index - start)
            };
            let origin = measured.row_origin(row, at, bounds);
            let left = (x_of(from) - row.start_x) * measured.factor;
            // A row that ends in its wrap boundary ends there, not after the space.
            let right = (x_of(to).min(row.end_x) - row.start_x) * measured.factor;
            rects.push(Bounds::from_corners(
                point(origin.x + left, origin.y),
                point(origin.x + right, origin.y + measured.line_height),
            ));
        }
        rects
    }

    /// How many rows the text took, as last laid out.
    pub fn row_count(&self) -> usize {
        self.0
            .borrow()
            .as_ref()
            .map_or(0, |measured| measured.rows().len())
    }
}

impl IntoElement for ScaledText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ScaledText {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        _: &mut App,
    ) -> (LayoutId, ()) {
        let style = window.text_style();
        let factor = sane(factor());
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = style
            .line_height
            .to_pixels(font_size.into(), window.rem_size());
        let runs = self
            .runs
            .take()
            .unwrap_or_else(|| vec![style.to_run(self.text.len())]);
        *self.layout.0.borrow_mut() = Some(Measured {
            text: self.text.clone(),
            given_runs: runs.clone(),
            lines: Vec::new(),
            runs,
            laid_in: None,
            planned: match self.role {
                TextRole::Wraps(width) => Some(width),
                _ => None,
            },
            factor,
            font_size: steady(font_size / factor),
            phase: phase(&self.text),
            line_height,
            align: style.text_align,
            wraps: style.white_space == WhiteSpace::Normal,
            overflow: style.text_overflow.clone(),
            font: style.font(),
            bounds: None,
        });
        let layout = self.layout.clone();
        let id = window.request_measured_layout(
            Style::default(),
            move |known, available, window, cx| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
                });
                let mut measured = layout.0.borrow_mut();
                let Some(measured) = measured.as_mut() else {
                    return Size::default();
                };
                measured.lay_out(width, window, cx);
                measured.size()
            },
        );
        if let TextRole::Noted(slot) = &self.role {
            slot.set(Some(id));
        }
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut measured = self.layout.0.borrow_mut();
        let Some(measured) = measured.as_mut() else {
            return;
        };
        // Its box is rounded to whole pixels after it was measured, to a
        // different fraction at every factor. Laid out in the box's width,
        // the text would be wrapped in a width that moves by most of an
        // unscaled pixel from one frame of a zoom to the next, and a word at
        // the end of a line would come and go. So what was measured is kept
        // while the box is the width it was measured in, or has the room it
        // asked for, give or take that rounding; only a box that is really
        // another width lays the text out again.
        let rounding = gpui::px(1.);
        let keep = match measured.laid_in {
            None => false,
            Some(None) => measured.size().width <= bounds.size.width + rounding,
            Some(Some(width)) => (width - bounds.size.width).abs() <= rounding,
        };
        if !keep {
            measured.lay_out(Some(bounds.size.width), window, cx);
        }
        measured.bounds = Some(bounds);
        if WORK.with(|work| work.get().is_some()) {
            PLACES.with(|places| places.borrow_mut().push(bounds));
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let measured = self.layout.0.borrow();
        let Some(measured) = measured.as_ref() else {
            return;
        };
        let starts = measured.line_starts();
        // Only the rows that can be seen: a glyph painted out of sight is
        // still rasterised, at every size a zoom passes through.
        let visible = window.content_mask().bounds;
        let device = moving().then(|| window.scale_factor());
        for (at, row) in measured.rows().iter().enumerate() {
            let origin = measured.row_origin(row, at, bounds);
            if origin.y > bounds.bottom() + measured.line_height || origin.y > visible.bottom() {
                break;
            }
            if origin.y + measured.line_height < visible.top() {
                continue;
            }
            let start = starts[row.line];
            let layout = row_layout(
                &measured.lines[row.line],
                row,
                measured.factor,
                measured.phase,
                device.map(|device| (origin.x, device)),
            );
            let decorations = decorations(
                &measured.runs,
                start + row.range.start..start + row.range.end,
            );
            paint_row(
                &layout,
                origin,
                measured.line_height,
                &decorations,
                window,
                cx,
            );
        }
    }
}
