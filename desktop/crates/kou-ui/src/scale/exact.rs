//! Blocks of a zoomed subtree placed where they stand at 1, times the zoom.
//!
//! A subtree drawn at a zoom is laid out at the zoom: every padding, gap and
//! text height is rounded to whole device pixels on its own, and a block
//! stands where the rounded sizes above it end. Sizes that are alike (the
//! line height of every paragraph, the gap after each) round alike, so a
//! block far down a column is several pixels from its place, and as a zoom
//! gesture goes on those pixels come all at once every few frames, more of
//! them the further down: the column stalls and then jumps.
//!
//! Laid out at 1, every size is whole and nothing is rounded. So a subtree
//! is laid out once at 1 to make a [`Plan`] ([`record`], [`Recording::plan`]),
//! and drawn at a zoom by following it ([`replaying`]): each [`column`] (and
//! each grid of [`cells`]) puts its children where they stand at 1 times the
//! zoom, each edge rounded once from the subtree's top, and each
//! [`ScaledText`](super::ScaledText) wraps in the width it had at 1, so it
//! has its lines at 1 even where its box at the zoom comes out a pixel
//! narrower. A plan is made by building the subtree a second time, at 1; it
//! holds while what the subtree shows does not change, which is for the
//! owner of the subtree to know.

use super::{factor, with_factor};
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Div, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, ParentElement as _, Pixels, Styled as _, Window,
    div, size,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// Where a subtree's columns put their children and in what width its texts
/// wrap, as laid out at 1. See the [module](self).
#[derive(Default)]
pub struct Plan {
    height: f32,
    texts: Vec<Option<TextPlan>>,
    columns: Vec<Option<ColumnPlan>>,
    /// Set when a column drawn by the plan found a child that is no longer
    /// the height the plan has for it: what it shows has changed.
    stale: Rc<Cell<bool>>,
}

struct TextPlan {
    /// The text's [`hash`]: a text other than the one planned keeps to its box.
    text: u64,
    width: Pixels,
}

/// Where a column and its children stand at 1: the column's bounds from the
/// subtree's top left corner, its children's from the column's.
struct ColumnPlan {
    bounds: Bounds<f32>,
    children: Vec<Bounds<f32>>,
}

impl Plan {
    /// The subtree's height at 1.
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Whether a column drawn by this plan found that what the subtree shows
    /// has changed since it was made: it is then to be made again.
    pub fn is_stale(&self) -> bool {
        self.stale.get()
    }
}

/// A layout node an element notes when its layout is requested.
type Slot = Rc<Cell<Option<LayoutId>>>;

#[derive(Default)]
struct Slots {
    texts: Vec<(u64, Slot)>,
    columns: Vec<(Slot, Vec<Slot>)>,
}

struct Replay {
    plan: Rc<Plan>,
    texts: usize,
    columns: usize,
    /// Device pixels to one: each place is rounded to a whole device pixel.
    device: f32,
}

enum Mode {
    Recording(Rc<RefCell<Slots>>),
    Replaying(Replay),
}

thread_local! {
    static MODE: RefCell<Option<Mode>> = const { RefCell::new(None) };
    /// The top left corners of the [`anchored`] subtrees being prepainted.
    static ANCHOR: RefCell<Vec<gpui::Point<Pixels>>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f` in `mode`, and puts back what was in effect before.
fn with_mode<R>(mode: Mode, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Mode>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let before = self.0.take();
            MODE.with(|mode| *mode.borrow_mut() = before);
        }
    }
    let _restore = Restore(MODE.with(|cell| cell.borrow_mut().replace(mode)));
    f()
}

/// Whether a plan is being recorded. Text is then always a
/// [`ScaledText`](super::ScaledText), even at 1, so that it can be planned.
pub fn recording() -> bool {
    MODE.with(|mode| matches!(&*mode.borrow(), Some(Mode::Recording(_))))
}

/// What [`record`] noted, to be laid out into a [`Plan`].
pub struct Recording {
    slots: Rc<RefCell<Slots>>,
}

/// Builds what `build` returns at a factor of 1, noting its texts and
/// columns, to lay it out into a [`Plan`] with [`Recording::plan`].
pub fn record<R>(build: impl FnOnce() -> R) -> (R, Recording) {
    let slots = Rc::new(RefCell::new(Slots::default()));
    let built = with_mode(Mode::Recording(slots.clone()), || with_factor(1., build));
    (built, Recording { slots })
}

impl Recording {
    /// Lays out `element`, which [`record`] built, at 1 in `width` (unscaled,
    /// as everything at 1 is), and makes the plan of what it holds. Called
    /// while a scaled subtree is prepainted, it lays out with the rem size
    /// unscaled.
    pub fn plan(
        self,
        element: AnyElement,
        width: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Plan {
        let root: Slot = Rc::default();
        let mut element = Probe {
            slot: root.clone(),
            child: element,
        }
        .into_any_element();
        let rem = window.rem_size() / factor();
        with_factor(1., || {
            window.with_rem_size(Some(rem), |window| {
                // Built a second time, its elements have ids of their own.
                window.with_id("exact-plan", |window| {
                    element.layout_as_root(
                        size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                        window,
                        cx,
                    );
                })
            })
        });
        let mut bounds = |slot: &Slot| {
            slot.get().map(|id| {
                let bounds = window.layout_bounds(id);
                Bounds::new(
                    gpui::point(f32::from(bounds.origin.x), f32::from(bounds.origin.y)),
                    gpui::size(f32::from(bounds.size.width), f32::from(bounds.size.height)),
                )
            })
        };
        let root = bounds(&root).unwrap_or_default();
        let slots = self.slots.borrow();
        let texts = slots
            .texts
            .iter()
            .map(|(text, slot)| {
                bounds(slot).map(|laid| TextPlan {
                    text: *text,
                    width: gpui::px(laid.size.width),
                })
            })
            .collect();
        let columns = slots
            .columns
            .iter()
            .map(|(column, children)| {
                let column = bounds(column)?;
                let children = children
                    .iter()
                    .map(|child| {
                        let child = bounds(child)?;
                        Some(Bounds::new(child.origin - column.origin, child.size))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(ColumnPlan {
                    bounds: Bounds::new(column.origin - root.origin, column.size),
                    children,
                })
            })
            .collect();
        Plan {
            height: root.size.height,
            texts,
            columns,
            stale: Rc::default(),
        }
    }
}

/// Builds what `build` returns following `plan`, in a window of `device`
/// device pixels to one, at the factor in effect.
pub fn replaying<R>(plan: &Rc<Plan>, device: f32, build: impl FnOnce() -> R) -> R {
    let replay = Replay {
        plan: plan.clone(),
        texts: 0,
        columns: 0,
        device: device.max(1.),
    };
    with_mode(Mode::Replaying(replay), build)
}

/// A number that tells texts apart: FNV-1a over the text.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What a text being built takes part in: noted while recording, the width
/// it wraps in while replaying.
pub(super) enum TextRole {
    None,
    Noted(Slot),
    Wraps(Pixels),
}

/// Called by each text as it is built: notes it while recording, and while
/// replaying gives it the width it wrapped in at 1 if it is the same text.
pub(super) fn text_role(text: &str) -> TextRole {
    MODE.with(|mode| match &mut *mode.borrow_mut() {
        None => TextRole::None,
        Some(Mode::Recording(slots)) => {
            let slot = Slot::default();
            slots.borrow_mut().texts.push((hash(text), slot.clone()));
            TextRole::Noted(slot)
        }
        Some(Mode::Replaying(replay)) => {
            let at = replay.texts;
            replay.texts += 1;
            match replay.plan.texts.get(at) {
                Some(Some(planned)) if planned.text == hash(text) => TextRole::Wraps(planned.width),
                _ => TextRole::None,
            }
        }
    })
}

/// A column of `children` in `column`, a `div` styled as the column (a flex
/// column, its gap, its padding, its width). As it is everywhere outside a
/// plan; while recording, its children's places are noted; while replaying
/// a plan that has the same number of children for it, each child is put
/// at its top at 1 times the zoom and the column is its height at 1 times
/// the zoom, and at least its width (its children no longer widen it). Its
/// children then span the column's width, padding included: a column with
/// horizontal padding is not planned.
pub fn column(column: Div, children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    place(column, children, false)
}

/// Like [`column`], for a grid: each of `children` is put where it stands
/// at 1 times the zoom across as well as down, as wide and as tall as it is
/// there times the zoom, which keeps a grid's cells in their columns and
/// rows. Places are measured from the grid's border, so a grid with a
/// border has its cells that border's width off at a zoom.
pub fn cells(grid: Div, children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    place(grid, children, true)
}

fn place(parent: Div, children: impl IntoIterator<Item = AnyElement>, cells: bool) -> AnyElement {
    let children: Vec<AnyElement> = children.into_iter().collect();
    enum Then {
        Flow,
        Note(Slot, Vec<Slot>),
        Place(Rc<Plan>, usize, f32),
    }
    let then = MODE.with(|mode| match &mut *mode.borrow_mut() {
        None => Then::Flow,
        Some(Mode::Recording(slots)) => {
            let parent = Slot::default();
            let noted: Vec<Slot> = children.iter().map(|_| Slot::default()).collect();
            slots
                .borrow_mut()
                .columns
                .push((parent.clone(), noted.clone()));
            Then::Note(parent, noted)
        }
        Some(Mode::Replaying(replay)) => {
            let at = replay.columns;
            replay.columns += 1;
            match replay.plan.columns.get(at) {
                Some(Some(planned)) if planned.children.len() == children.len() => {
                    Then::Place(replay.plan.clone(), at, replay.device)
                }
                _ => Then::Flow,
            }
        }
    });
    let (plan, at, device) = match then {
        Then::Flow => return parent.children(children).into_any_element(),
        Then::Note(slot, noted) => {
            return Probe {
                slot,
                child: parent
                    .children(
                        children
                            .into_iter()
                            .zip(noted)
                            .map(|(child, slot)| Probe { slot, child }.into_any_element()),
                    )
                    .into_any_element(),
            }
            .into_any_element();
        }
        Then::Place(plan, at, device) => (plan, at, device),
    };
    let Some(Some(planned)) = plan.columns.get(at) else {
        return parent.children(children).into_any_element();
    };
    // Every edge where it stands at 1 times the zoom from the subtree's top
    // left corner, rounded to a whole device pixel once; each length between
    // two such edges.
    let zoom = factor();
    let edge = |unscaled: f32| (unscaled * zoom * device).round() / device;
    let span = |from: f32, length: f32| gpui::px(edge(from + length) - edge(from));
    let origin = planned.bounds.origin;
    let placed: Vec<AnyElement> = children
        .into_iter()
        .zip(&planned.children)
        .map(|(child, laid)| {
            let check = Check {
                place: gpui::point(
                    gpui::px(edge(origin.x + laid.origin.x)),
                    gpui::px(edge(origin.y + laid.origin.y)),
                ),
                height: laid.size.height * zoom,
                device,
                stale: plan.stale.clone(),
                child,
            };
            let top = span(origin.y, laid.origin.y);
            let wrapper = div().absolute().top(top);
            if cells {
                // A grid of one cell stretches the cell both ways, as the
                // grid it came from stretched it to its row and column.
                wrapper
                    .grid()
                    .grid_cols(1)
                    .grid_rows(1)
                    .left(span(origin.x, laid.origin.x))
                    .w(span(origin.x + laid.origin.x, laid.size.width))
                    .h(span(origin.y + laid.origin.y, laid.size.height))
                    .child(check)
                    .into_any_element()
            } else {
                wrapper.left_0().w_full().child(check).into_any_element()
            }
        })
        .collect();
    let size = planned.bounds.size;
    parent
        .relative()
        .h(span(origin.y, size.height))
        .min_w(span(origin.x, size.width))
        .children(placed)
        .into_any_element()
}

/// `child`, the subtree a [`Plan`] is followed in, drawn so that the places
/// its columns' children are moved to are counted from its top left corner:
/// each then stands where it stands at 1 times the zoom from that corner,
/// rounded once, whatever the rounding of the columns and paddings around it.
pub fn anchored(child: impl IntoElement) -> AnyElement {
    Anchor {
        child: child.into_any_element(),
    }
    .into_any_element()
}

struct Anchor {
    child: AnyElement,
}

impl IntoElement for Anchor {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Anchor {
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
        (self.child.request_layout(window, cx), ())
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
        struct Pop;
        impl Drop for Pop {
            fn drop(&mut self) {
                ANCHOR.with(|anchor| anchor.borrow_mut().pop());
            }
        }
        ANCHOR.with(|anchor| anchor.borrow_mut().push(bounds.origin));
        let _pop = Pop;
        self.child.prepaint(window, cx);
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
        self.child.paint(window, cx);
    }
}

/// Takes its child's place in the layout and notes the child's layout node.
struct Probe {
    slot: Slot,
    child: AnyElement,
}

/// Takes its child's place in the layout, and marks the plan stale when the
/// child is not the height the plan has for it, give or take rounding.
struct Check {
    /// Where the child stands at 1 times the zoom from the [`anchored`]
    /// subtree's top left corner, rounded to a whole device pixel.
    place: gpui::Point<Pixels>,
    height: f32,
    device: f32,
    stale: Rc<Cell<bool>>,
    child: AnyElement,
}

impl IntoElement for Probe {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl IntoElement for Check {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Probe {
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
        let id = self.child.request_layout(window, cx);
        self.slot.set(Some(id));
        (id, ())
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
        self.child.prepaint(window, cx);
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
        self.child.paint(window, cx);
    }
}

impl Element for Check {
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
        (self.child.request_layout(window, cx), ())
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
        // Rounded piece by piece at the zoom, a child comes out a few device
        // pixels off its height at 1 times the zoom; a line more or less is
        // what it shows having changed.
        let off = (f32::from(bounds.size.height) - self.height).abs() * self.device;
        let allowed = STALE.max(self.height * self.device * STALE_SHARE);
        if off > allowed && !self.stale.replace(true) {
            window.request_animation_frame();
        }
        // Laid out at the zoom, the child stands where its column's place
        // and the rounded sizes around the column put it, a pixel or two
        // from where it belongs. It is moved there, its place at 1 times the
        // zoom from the anchored subtree's corner, rounded once: by less than
        // a few pixels, for whatever it holds, hits included.
        let shift = ANCHOR
            .with(|anchor| anchor.borrow().last().copied())
            .map(|anchor| anchor + self.place - bounds.origin)
            .filter(|shift| {
                let most = gpui::px(SHIFT / self.device.max(1.));
                shift.x.abs() <= most && shift.y.abs() <= most
            });
        match shift {
            Some(shift) => window.with_element_offset(shift, |window| {
                self.child.prepaint(window, cx);
            }),
            None => {
                self.child.prepaint(window, cx);
            }
        }
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
        self.child.paint(window, cx);
    }
}

/// How many device pixels a planned child is moved by at most to its place:
/// a child further off than that is left where it was laid out.
const SHIFT: f32 = 6.;

/// How many device pixels, and what share of its height, a planned child
/// may be off its planned height before the plan is taken to be out of
/// date: what rounding piece by piece inside it can move it.
const STALE: f32 = 8.;
const STALE_SHARE: f32 = 0.06;
