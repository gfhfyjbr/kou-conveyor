//! Sizes that follow a zoom: a factor in effect for a subtree of elements.
//!
//! GPUI has no element that scales what is under it, and a transform would
//! leave the pointer's hits where they were. So a scaled subtree is drawn by
//! scaling its sizes themselves: every size it uses goes through [`px`], which
//! multiplies by the factor in effect, and [`scaled`] puts a factor in effect
//! while its child is laid out, prepainted and painted. Outside any scaled
//! subtree the factor is 1 and [`px`] is exactly `gpui::px`.
//!
//! A view's `render` runs before what it returns is laid out, and the views and
//! list rows under it render later, while that is laid out or prepainted. So a
//! view that draws scaled sets the factor around its own `render` with
//! [`with_factor`], wraps what it returns in [`scaled`], and every view nested
//! in it does the same with the same factor: GPUI may build a nested view again
//! on its own, without the views around it, and then only what the nested view
//! does itself is in effect.
//!
//! Text is the exception to "scale the sizes": the system font is tracked by its
//! point size, so text set at twice the size is not twice as wide and would wrap
//! elsewhere. [`ScaledText`] lays text out at its unscaled size and draws it
//! scaled, so it wraps where it wraps at 1.

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};
use std::cell::Cell;

pub mod exact;
mod text;
pub use text::{
    ScaledText, ScaledTextLayout, TextWork, count_text_work, paint_line, phase, styled_text,
    take_text_places, take_text_work, text,
};

thread_local! {
    static FACTOR: Cell<f32> = const { Cell::new(1.) };
    static MOVING: Cell<bool> = const { Cell::new(false) };
}

/// Whether the factor in effect is moving: a zoom gesture is under way.
/// Text then puts each glyph on a whole device pixel (see [`Scaled::moving`]).
pub fn moving() -> bool {
    MOVING.with(Cell::get)
}

/// The factor sizes are drawn at here: 1 outside any scaled subtree.
pub fn factor() -> f32 {
    FACTOR.with(Cell::get)
}

/// `value` logical pixels at the factor in effect: what `gpui::px` gives
/// outside a scaled subtree, `value` times the factor inside one.
pub fn px(value: f32) -> Pixels {
    gpui::px(value * factor())
}

/// Runs `f` with sizes drawn at `factor`, and puts back the factor that was in
/// effect before, also if `f` panics. A view that draws scaled builds its
/// elements inside this: what `render` computes with [`px`] is computed then.
pub fn with_factor<R>(factor: f32, f: impl FnOnce() -> R) -> R {
    let _restore = Restore(FACTOR.with(|cell| cell.replace(sane(factor))));
    f()
}

struct Restore(f32);

impl Drop for Restore {
    fn drop(&mut self) {
        FACTOR.with(|cell| cell.set(self.0));
    }
}

struct RestoreMoving(bool);

impl Drop for RestoreMoving {
    fn drop(&mut self) {
        MOVING.with(|cell| cell.set(self.0));
    }
}

/// A factor that is not a positive number draws at 1.
fn sane(factor: f32) -> f32 {
    if factor.is_finite() && factor > 0. {
        factor
    } else {
        1.
    }
}

/// How many sizes [`quantize`] has per doubling of the zoom: one every 2.9%,
/// with 1, 2, 4 and 1/2 among them.
pub const STEPS_PER_DOUBLING: f32 = 24.;

/// The zooms [`quantize`] takes; anything outside is drawn at the nearest end.
pub const ZOOM_RANGE: (f32, f32) = (1. / 16., 16.);

/// The size glyphs are rasterised at for a factor: the nearest of
/// [`STEPS_PER_DOUBLING`] steps per doubling, exactly 1 within 1.5% of 1, and
/// 1 for a factor that is not a positive number.
///
/// A scaled subtree is laid out and drawn at the zoom itself, every frame of
/// a zoom gesture: its boxes and each glyph's place move with the zoom, so
/// nothing in it moves in steps. Only the glyphs' own pictures are stepped,
/// because GPUI's glyph atlas keeps every font size it has rasterised and
/// would gain a size per frame: a glyph is drawn at the step nearest the
/// zoom, at most 1.5% larger or smaller than its place, and a sweep from 0.6
/// to 4 rasterises 67 sizes.
pub fn quantize(zoom: f32) -> f32 {
    quantize_staggered(zoom, 0.5)
}

/// How much of a step [`quantize_staggered`] spreads the zooms at which texts
/// change size over.
pub const STAGGER: f32 = 0.6;

/// The size glyphs are rasterised at for a factor, for text whose `phase` is
/// a number from 0 to 1: one of the sizes of [`quantize`], not always the
/// nearest. A text with phase 0.5 changes to the next size half way between
/// the two, as [`quantize`] does; with phase 0 a little later, with phase 1 a
/// little earlier, by up to [`STAGGER`] / 2 of a step. The glyph is then at
/// most 0.8 of a step (2.3%) larger or smaller than its place, and exactly 1
/// within 1% of 1.
///
/// The frame in which a zoom gesture reaches the next size rasterises every
/// glyph on screen anew, which can take longer than a frame has. Texts of
/// different phases change size in different frames and share that work out,
/// while every size is still one of a few, shared by all texts.
pub fn quantize_staggered(zoom: f32, phase: f32) -> f32 {
    if !zoom.is_finite() || zoom <= 0. {
        return 1.;
    }
    let zoom = zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
    let shift = (phase.clamp(0., 1.) - 0.5) * STAGGER;
    let step = (zoom.log2() * STEPS_PER_DOUBLING + shift).round();
    (step / STEPS_PER_DOUBLING).exp2()
}

/// A length divided by a factor, as near as the eye can tell: to a 64th of a
/// pixel. A size worked back from a scaled one (`13 * f / f`) is not the same
/// number at every factor, and text shaped at such a size, or wrapped in such
/// a width, would be shaped anew every frame of a zoom and could wrap anew.
pub fn steady(length: Pixels) -> Pixels {
    gpui::px((f32::from(length) * 64.).round() / 64.)
}

/// Draws `child` with every size scaled by `factor`: the factor is in effect
/// (see [`factor`]) and the window's rem size is scaled by it while `child` is
/// laid out, prepainted and painted, so sizes taken through [`px`] and sizes
/// written in rems both follow it. The child itself is built by the caller,
/// which builds it inside [`with_factor`].
///
/// The element takes its child's place in the layout: it adds no node of its
/// own, so a view nested in the child can still be built again on its own.
pub fn scaled(factor: f32, child: impl IntoElement) -> Scaled {
    Scaled {
        factor: sane(factor),
        rem: true,
        moving: false,
        child: child.into_any_element(),
    }
}

/// See [`scaled`].
pub struct Scaled {
    factor: f32,
    rem: bool,
    moving: bool,
    child: AnyElement,
}

impl Scaled {
    /// Leaves the rem size as it finds it. For a view nested in a scaled
    /// subtree: it finds the rem size already scaled where it is drawn, and
    /// GPUI gives it the same rem size when it builds the view again on its
    /// own, while the factor is only what the view puts in effect itself.
    pub fn inherit_rem(mut self) -> Self {
        self.rem = false;
        self
    }

    /// Draws the subtree as one whose factor is moving, frame by frame, or
    /// not. While it moves, each glyph of its text stands on a whole device
    /// pixel: a glyph at a fraction of a pixel is rasterised once for each
    /// of four fractions, and every size a zoom passes through would be
    /// rasterised four times over, in the frames of the gesture. Glyphs are
    /// then up to half a pixel off their places, as a terminal's are; the
    /// subtree is to be drawn again, not moving, once the zoom rests.
    pub fn moving(mut self, moving: bool) -> Self {
        self.moving = moving;
        self
    }

    /// Runs `f` with the factor in effect, the rem size scaled, and the
    /// factor moving or not.
    fn enter<R>(
        (factor, rem, moving): (f32, bool, bool),
        window: &mut Window,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        // Nested in another scaled subtree, the rem size is already the outer factor's.
        let rem = rem.then(|| window.rem_size() * (factor / self::factor()));
        let _restore = RestoreMoving(MOVING.with(|cell| cell.replace(moving)));
        with_factor(factor, || window.with_rem_size(rem, f))
    }
}

impl IntoElement for Scaled {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Scaled {
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
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = &mut self.child;
        let layout = Self::enter((self.factor, self.rem, self.moving), window, |window| {
            child.request_layout(window, cx)
        });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        Self::enter((self.factor, self.rem, self.moving), window, |window| {
            child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        Self::enter((self.factor, self.rem, self.moving), window, |window| {
            child.paint(window, cx);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Icon, Mark, Theme, button, icon, icon_button, key_hint, monogram, status_mark};
    use gpui::{
        Context, InteractiveElement as _, ParentElement as _, Render, Styled as _, TestAppContext,
        div,
    };
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn a_zoom_is_drawn_at_the_nearest_of_few_steps() {
        assert_eq!(quantize(1.), 1.);
        for exact in [0.5, 2., 4.] {
            assert_eq!(quantize(exact), exact);
        }
        // Within half a step of 1 is 1.
        assert_eq!(quantize(1.0145), 1.);
        assert_eq!(quantize(0.986), 1.);
        for nonsense in [f32::NAN, f32::INFINITY, 0., -1.] {
            assert_eq!(quantize(nonsense), 1.);
        }
        // Never more than half a step from the zoom, never smaller for a larger zoom,
        // and a sweep over the canvas's range visits 67 sizes.
        let half_step = (0.5 / STEPS_PER_DOUBLING).exp2();
        let mut zoom = 0.6f32;
        let mut seen = Vec::new();
        while zoom <= 4. {
            let step = quantize(zoom);
            assert!(
                step / zoom <= half_step * 1.0001 && zoom / step <= half_step * 1.0001,
                "{zoom} drawn at {step}"
            );
            if seen.last() != Some(&step) {
                seen.push(step);
            }
            // A gesture at 240 Hz moves the zoom by well under a percent a frame.
            zoom *= 1.002;
        }
        assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(seen.len(), 67, "{seen:?}");
    }

    #[test]
    fn texts_change_glyph_size_at_different_zooms_among_the_same_few_sizes() {
        let sizes: Vec<f32> = {
            let mut zoom = 0.6f32;
            let mut sizes = Vec::new();
            while zoom <= 4. {
                sizes.push(quantize(zoom));
                zoom *= 1.002;
            }
            sizes
        };
        // Every phase draws at one of the sizes the nearest step has, at most
        // 0.8 of a step from the zoom, and at 1 near 1.
        let most = ((0.5 + STAGGER / 2.) / STEPS_PER_DOUBLING).exp2() * 1.0001;
        let phases = [0., 0.17, 0.5, 0.83, 1.];
        let mut zoom = 0.6f32;
        while zoom <= 4. {
            for phase in phases {
                let size = quantize_staggered(zoom, phase);
                assert!(sizes.contains(&size), "{size} at {zoom}, phase {phase}");
                assert!(
                    size / zoom <= most && zoom / size <= most,
                    "{size} at {zoom}"
                );
            }
            zoom *= 1.002;
        }
        for phase in phases {
            assert_eq!(quantize_staggered(1., phase), 1.);
        }
        // A gesture reaches each next size in a different frame for each phase.
        let first_frame = |phase: f32| {
            let mut zoom = 1.04f32;
            let mut frame = 0;
            while quantize_staggered(zoom, phase) == quantize_staggered(1.04, phase) {
                zoom *= 1.002;
                frame += 1;
            }
            frame
        };
        let mut frames: Vec<usize> = phases.iter().map(|phase| first_frame(*phase)).collect();
        frames.sort();
        frames.dedup();
        assert!(frames.len() >= 4, "{frames:?}");
        // The phase of a text is decided by how it starts.
        assert_eq!(
            phase("The delay is doubled"),
            phase("The delay is doubled in two places")
        );
        assert_ne!(phase("The delay is doubled"), phase("Dropping the second"));
    }

    #[test]
    fn px_follows_the_factor_in_effect_and_only_inside_it() {
        assert_eq!(factor(), 1.);
        assert_eq!(px(13.), gpui::px(13.));
        let inner = with_factor(2., || {
            assert_eq!(px(13.), gpui::px(26.));
            with_factor(0.5, || px(13.))
        });
        assert_eq!(inner, gpui::px(6.5));
        assert_eq!(factor(), 1.);
        // A factor that is not a positive number draws at 1.
        assert_eq!(with_factor(f32::NAN, || px(13.)), gpui::px(13.));
        // Put back even when what ran inside panicked.
        let caught = std::panic::catch_unwind(|| with_factor(3., || panic!("inside")));
        assert!(caught.is_err());
        assert_eq!(factor(), 1.);
    }

    /// Controls, each with a selector, at a factor.
    struct Controls(f32);

    impl Render for Controls {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = crate::theme(cx);
            let build = || {
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .text_size(px(13.))
                    .child(
                        icon_button("close", Icon::Close, theme)
                            .debug_selector(|| "icon-button".into()),
                    )
                    .child(button("go", "Go on", true, theme).debug_selector(|| "button".into()))
                    .child(
                        div()
                            .debug_selector(|| "key".into())
                            .child(key_hint("esc", theme)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "icon".into())
                            .child(icon(Icon::Plus, theme.fg)),
                    )
                    .child(monogram("cc", theme.accent, theme).debug_selector(|| "monogram".into()))
                    .child(status_mark(Mark::Done, theme).debug_selector(|| "mark".into()))
                    .child(
                        div()
                            .w(px(120.))
                            .debug_selector(|| "text".into())
                            .child(text("one two three four five six seven eight nine ten")),
                    )
            };
            if self.0 == 1. {
                build().into_any_element()
            } else {
                scaled(self.0, with_factor(self.0, build)).into_any_element()
            }
        }
    }

    fn measure(factor: f32, cx: &mut TestAppContext) -> Vec<Bounds<Pixels>> {
        cx.update(|cx| cx.set_global(Theme::new(true)));
        let (_, cx) = cx.add_window_view(|_, _| Controls(factor));
        cx.run_until_parked();
        [
            "icon-button",
            "button",
            "key",
            "icon",
            "monogram",
            "mark",
            "text",
        ]
        .into_iter()
        .map(|selector| {
            cx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} was drawn"))
        })
        .collect()
    }

    #[gpui::test]
    fn controls_outside_a_scaled_subtree_keep_their_sizes(cx: &mut TestAppContext) {
        let drawn = measure(1., cx);
        let side = |bounds: &Bounds<Pixels>| (bounds.size.width, bounds.size.height);
        assert_eq!(side(&drawn[0]), (gpui::px(24.), gpui::px(24.)));
        assert_eq!(drawn[1].size.height, gpui::px(crate::metric::CONTROL));
        assert_eq!(side(&drawn[3]), (gpui::px(16.), gpui::px(16.)));
        assert_eq!(side(&drawn[4]), (gpui::px(16.), gpui::px(16.)));
        assert_eq!(side(&drawn[5]), (gpui::px(8.), gpui::px(8.)));
    }

    #[gpui::test]
    fn controls_in_a_scaled_subtree_scale_with_it(cx: &mut TestAppContext) {
        let one = measure(1., cx);
        let two = measure(2., cx);
        for (index, (one, two)) in one.iter().zip(&two).enumerate() {
            for (a, b) in [
                (one.size.width, two.size.width),
                (one.size.height, two.size.height),
                (one.origin.x, two.origin.x),
                (one.origin.y, two.origin.y),
            ] {
                // Text is measured in whole pixels, rounded up at each size.
                assert!(
                    (a * 2. - b).abs() <= gpui::px(2.),
                    "control {index}: {one:?} at 1, {two:?} at 2"
                );
            }
        }
    }

    /// Text in a box, wrapped or truncated, its layout kept to look at.
    struct Paragraph {
        factor: f32,
        truncate: bool,
        layout: ScaledTextLayout,
    }

    impl Render for Paragraph {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let text = ScaledText::new(
                "The task stays in the queue until a worker acknowledges it; if the worker \
                 dies, the lease runs out and the next worker takes it.",
            );
            self.layout = text.layout().clone();
            let factor = self.factor;
            let block = with_factor(factor, || {
                let block = div()
                    .w(px(160.))
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .debug_selector(|| "paragraph".into());
                if self.truncate {
                    block.truncate().child(text)
                } else {
                    block.child(text)
                }
            });
            div()
                .flex()
                .flex_col()
                .items_start()
                .child(scaled(factor, block))
        }
    }

    /// Paragraphs in a column with a gap, at a factor, as a scaled subtree
    /// draws them: laid out at the factor alone, or following a plan of the
    /// column laid out at 1, which is made on the first frame.
    struct Planned {
        factor: f32,
        planned: bool,
        plan: Rc<RefCell<Option<Rc<exact::Plan>>>>,
    }

    const LINES: [&str; 6] = [
        "The task stays in the queue until a worker acknowledges it.",
        "A lease of thirty seconds.",
        "If the worker dies, the lease runs out and the next worker takes it, \
         so handlers have to be idempotent.",
        "Attempts are counted.",
        "A task that keeps failing ends up in the dead letters.",
        "Done.",
    ];

    fn column_of(width: f32) -> gpui::AnyElement {
        div()
            .w(px(width))
            .text_size(px(13.))
            .line_height(px(20.))
            .child(exact::column(
                div().w_full().flex().flex_col().gap(px(7.)),
                LINES.iter().enumerate().map(|(at, line)| {
                    div()
                        .debug_selector(move || format!("line-{at}"))
                        .child(text(*line))
                        .into_any_element()
                }),
            ))
            .into_any_element()
    }

    impl Render for Planned {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let factor = self.factor;
            let plan = self.plan.borrow().clone();
            let column = match plan.filter(|_| self.planned) {
                Some(plan) => with_factor(factor, || {
                    exact::replaying(&plan, window.scale_factor(), || column_of(300.))
                }),
                None => with_factor(factor, || column_of(300.)),
            };
            let (slot, view) = (self.plan.clone(), cx.entity());
            let planned = self.planned;
            div()
                .child(scaled(
                    factor,
                    div().debug_selector(|| "column".into()).child(column),
                ))
                .child(gpui::canvas(
                    move |_, window, cx| {
                        if planned && slot.borrow().is_none() {
                            let (element, recording) = exact::record(|| column_of(300.));
                            *slot.borrow_mut() =
                                Some(Rc::new(recording.plan(element, gpui::px(300.), window, cx)));
                            view.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, _, _, _| {},
                ))
        }
    }

    /// Each line's top below the column's top, laid out at `factor`.
    fn tops(factor: f32, planned: bool, cx: &mut TestAppContext) -> Vec<f32> {
        cx.update(|cx| cx.set_global(Theme::new(true)));
        let (_, cx) = cx.add_window_view(|_, _| Planned {
            factor,
            planned,
            plan: Rc::default(),
        });
        // The user's display: a device pixel to a pixel, where rounding shows most.
        cx.simulate_scale_factor_change(1.);
        cx.run_until_parked();
        let column = cx.debug_bounds("column").expect("drawn");
        (0..LINES.len())
            .map(|at| {
                let line = cx.debug_bounds(Box::leak(format!("line-{at}").into_boxed_str()));
                f32::from(line.expect("a line").top() - column.top())
            })
            .collect()
    }

    #[gpui::test]
    fn a_planned_column_puts_each_child_at_its_place_at_one_times_the_zoom(
        cx: &mut TestAppContext,
    ) {
        let at_one = tops(1., false, cx);
        assert!(at_one.iter().all(|top| top.fract() == 0.), "{at_one:?}");
        let mut drifted = false;
        for factor in [0.6, 0.73, 0.88, 1.37, 2.2] {
            // Each line within half a pixel of its place at 1 times the zoom:
            // rounded once, however many lines and gaps are above it.
            let planned = tops(factor, true, cx);
            for (top, one) in planned.iter().zip(&at_one) {
                assert!(
                    (top - one * factor).abs() <= 0.5,
                    "planned at {factor}: {planned:?}, at 1: {at_one:?}"
                );
            }
            // Laid out at the zoom alone, every gap and line height is rounded
            // on its own, and the lines further down drift from their places.
            let alone = tops(factor, false, cx);
            drifted |= alone
                .iter()
                .zip(&at_one)
                .any(|(top, one)| (top - one * factor).abs() > 1.);
        }
        assert!(drifted, "the drift a plan is for");
    }

    fn paragraph(
        factor: f32,
        truncate: bool,
        cx: &mut TestAppContext,
    ) -> (ScaledTextLayout, Bounds<Pixels>) {
        let (view, cx) = cx.add_window_view(|_, _| Paragraph {
            factor,
            truncate,
            layout: ScaledTextLayout::default(),
        });
        cx.run_until_parked();
        let layout = view.read_with(cx, |view, _| view.layout.clone());
        (layout, cx.debug_bounds("paragraph").expect("drawn"))
    }

    #[gpui::test]
    fn scaled_text_keeps_its_lines_and_truncates_where_it_does_at_one(cx: &mut TestAppContext) {
        let (one, one_box) = paragraph(1., false, cx);
        assert!(one.row_count() > 2, "{} rows", one.row_count());
        for factor in [0.6, 1.5, 2.5] {
            let (scaled, drawn) = paragraph(factor, false, cx);
            assert_eq!(scaled.row_count(), one.row_count(), "rows at {factor}");
            assert!(
                (drawn.size.height - one_box.size.height * factor).abs() <= gpui::px(1.),
                "{drawn:?} at {factor}"
            );
        }
        // Truncated, it keeps to one row in its box, as wide as the box at most.
        let (one, _) = paragraph(1., true, cx);
        assert_eq!(one.row_count(), 1);
        let (two, drawn) = paragraph(2., true, cx);
        assert_eq!(two.row_count(), 1);
        let rect = two.rects(0..1);
        assert_eq!(rect.len(), 1);
        assert!(drawn.size.width <= gpui::px(321.), "{drawn:?}");
    }
}
