//! A text caret that blinks along [`crate::motion::caret`]: what a view keeps
//! for it ([`Blink`]), and a caret drawn as an element of its own for a field
//! laid out from boxes ([`caret`]).
//!
//! A caret asks for frames only while it fades. While it is lit or dark it
//! asks for one redraw, when that ends; while it holds still (its field or
//! window without the keys, the system asking for less motion, a program
//! asking for a steady cursor) it asks for none.

use crate::motion::caret::{Next, blink};
use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, Pixels, Size, Style, Task, Window, fill,
};
use std::time::{Duration, Instant};

/// What a view keeps to blink a caret: where the caret was last seen, since
/// when it has been there, and the redraw asked for its next change.
pub struct Blink {
    seen: Option<u64>,
    since: Instant,
    /// It held still when last drawn: it starts lit when it blinks again.
    held: bool,
    due: Option<Instant>,
    timer: Option<Task<()>>,
}

impl Blink {
    /// A caret lit from `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            seen: None,
            since: now,
            held: false,
            due: None,
            timer: None,
        }
    }

    /// Notes where the caret is: anything that changes when it moves or is
    /// typed at (its offset, the length of the text, the selection). A change
    /// lights it at once and starts its cycle over.
    pub fn at(&mut self, place: u64, now: Instant) {
        if self.seen != Some(place) {
            self.seen = Some(place);
            self.restart(now);
        }
    }

    /// Lights it at once and starts its cycle over: a key was pressed, the
    /// keys came to it.
    pub fn restart(&mut self, now: Instant) {
        self.since = now;
        self.due = None;
        self.timer = None;
    }

    /// The caret holds still or is not drawn (its field lost the keys): no
    /// redraw is asked for, and it starts lit when it blinks again.
    pub fn hold(&mut self) {
        self.held = true;
        self.due = None;
        self.timer = None;
    }

    /// How lit the caret is at `now`, from 1 to 0, having asked the view
    /// being drawn for what its blink needs next: the next frame while it
    /// fades, one redraw when it next starts to change, nothing when it does
    /// not `blink` (then it is lit, and starts lit when it blinks again).
    pub fn lit(&mut self, blinks: bool, now: Instant, window: &mut Window, cx: &mut App) -> f32 {
        if !blinks {
            self.hold();
            return 1.;
        }
        if self.held {
            self.held = false;
            self.restart(now);
        }
        let (lit, next) = blink(now.saturating_duration_since(self.since));
        match next {
            Next::Frame => {
                self.due = None;
                self.timer = None;
                window.request_animation_frame();
            }
            Next::After(wait) => {
                let due = now + wait;
                // Asked already, for about then: drawn for something else meanwhile.
                let asked = self.timer.is_some()
                    && self.due.is_some_and(|at| {
                        at.max(due).saturating_duration_since(at.min(due)) < SLACK
                    });
                if !asked {
                    let view = window.current_view();
                    let timer = cx.background_executor().timer(wait);
                    self.due = Some(due);
                    self.timer = Some(cx.spawn(async move |cx| {
                        timer.await;
                        cx.update(|cx| cx.notify(view));
                    }));
                }
            }
        }
        lit
    }
}

// Two redraws asked for within this of each other are one.
const SLACK: Duration = Duration::from_millis(4);

/// A caret drawn as a bar of `size` in `color`, for a field laid out from
/// boxes: it takes no room in the line (the text on either side of it does
/// not move as it does), blinks while its window has the keys and lights at
/// once when `place` changes (see [`Blink::at`]). `id` keeps its blink from
/// one frame to the next; give each field its own.
pub fn caret(id: impl Into<ElementId>, place: u64, color: Hsla, size: Size<Pixels>) -> Caret {
    Caret {
        id: id.into(),
        place,
        color,
        size,
    }
}

/// See [`caret`].
pub struct Caret {
    id: ElementId,
    place: u64,
    color: Hsla,
    size: Size<Pixels>,
}

impl IntoElement for Caret {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Caret {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
        let mut style = Style::default();
        style.size.width = Pixels::ZERO.into();
        style.size.height = self.size.height.into();
        style.flex_shrink = 0.;
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let now = cx.background_executor().now();
        let blinks = window.is_window_active() && !cx.reduce_motion();
        let lit = match id {
            Some(id) => window.with_element_state::<Blink, _>(id, |blink, window| {
                let mut blink = blink.unwrap_or_else(|| Blink::new(now));
                blink.at(self.place, now);
                (blink.lit(blinks, now, window, cx), blink)
            }),
            None => 1.,
        };
        if lit > 0. {
            // Over the boundary between the characters on either side.
            let bar = Bounds::new(
                gpui::point(bounds.origin.x - self.size.width / 2., bounds.origin.y),
                self.size,
            );
            window.paint_quad(fill(bar, self.color.opacity(lit)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::caret::{DARK, FADE, LIT};
    use gpui::{
        Context, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, div,
        px, size,
    };

    /// A field with a caret, counting how often it is drawn.
    struct Field {
        drawn: usize,
    }

    impl Render for Field {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.drawn += 1;
            div()
                .size_full()
                .child(caret("caret", 7, gpui::white(), size(px(1.5), px(16.))))
        }
    }

    fn open(cx: &mut TestAppContext) -> (gpui::Entity<Field>, &mut VisualTestContext) {
        let (field, cx) = cx.add_window_view(|_, _| Field { drawn: 0 });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        (field, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    fn drawn(field: &gpui::Entity<Field>, cx: &mut VisualTestContext) -> usize {
        field.read_with(cx, |field, _| field.drawn)
    }

    #[gpui::test]
    fn a_lit_caret_asks_for_one_redraw_however_often_it_is_drawn(cx: &mut TestAppContext) {
        let (field, cx) = open(cx);
        // Drawn for other reasons many times while lit: still one timer.
        for _ in 0..50 {
            draw(cx);
        }
        let before = drawn(&field, cx);
        cx.executor().advance_clock(LIT + FADE / 4);
        cx.run_until_parked();
        let woken = drawn(&field, cx) - before;
        assert!(woken <= 1, "woken {woken} times for one change");
        // Fading, it asks for the next frame, and for no timer.
        draw(cx);
        let frames = cx.update(|window, cx| window.simulate_next_frame(cx));
        assert!(frames >= 1, "a fade is drawn frame by frame");
    }

    #[gpui::test]
    fn without_the_window_it_asks_for_nothing(cx: &mut TestAppContext) {
        let (field, cx) = open(cx);
        cx.deactivate_window();
        draw(cx);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        let before = drawn(&field, cx);
        for _ in 0..30 {
            cx.executor().advance_clock(LIT + DARK);
            cx.run_until_parked();
            let frames = cx.update(|window, cx| window.simulate_next_frame(cx));
            assert_eq!(frames, 0, "an inactive window's caret asks for no frame");
        }
        assert_eq!(drawn(&field, cx), before, "nor for any redraw");
    }

    /// A view that draws nothing but asks a blink how lit it is, at the
    /// moment it is told, as it paints.
    struct Probe {
        blink: std::rc::Rc<std::cell::RefCell<Blink>>,
        at: std::rc::Rc<std::cell::Cell<(Instant, u64, bool)>>,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (blink, at) = (self.blink.clone(), self.at.clone());
            gpui::canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    let (now, place, blinks) = at.get();
                    let mut blink = blink.borrow_mut();
                    blink.at(place, now);
                    blink.lit(blinks, now, window, cx);
                },
            )
            .size_full()
        }
    }

    #[gpui::test]
    fn a_blink_holds_one_timer_at_most_and_none_when_held(cx: &mut TestAppContext) {
        let start = cx.executor().now();
        let blink = std::rc::Rc::new(std::cell::RefCell::new(Blink::new(start)));
        let at = std::rc::Rc::new(std::cell::Cell::new((start, 0, true)));
        let (_, cx) = cx.add_window_view({
            let (blink, at) = (blink.clone(), at.clone());
            move |_, _| Probe { blink, at }
        });
        cx.update(|window, _| window.activate_window());
        for step in 0..400u64 {
            at.set((start + Duration::from_millis(step * 7), step / 100, true));
            draw(cx);
            let blink = blink.borrow();
            // One task, or none while it fades; never a list of them.
            assert!(blink.timer.is_none() || blink.due.is_some(), "{step}");
        }
        at.set((start, 0, false));
        draw(cx);
        let blink = blink.borrow();
        assert!(blink.timer.is_none() && blink.due.is_none() && blink.held);
    }
}
