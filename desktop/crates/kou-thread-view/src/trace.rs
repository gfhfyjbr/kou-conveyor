//! What each frame of the view costs and where it puts things, for finding out
//! what makes a zoom, a scroll or a stream uneven. Off unless the environment
//! variable `KOU_ZOOM_TRACE` is set when the first view is made; then every
//! frame a view draws anew prints one line to stderr, starting `zoom-trace`
//! (a view that follows no zoom is traced too, at a zoom of 1):
//!
//! - `frame`, `t` (ms since the view's first frame) and `gap` (ms since its
//!   last frame began): frames drawn back to back are as far apart as one
//!   frame takes;
//! - `zoom` and `factor`, and `new=1` when the factor changed this frame;
//! - `held`, `room` and `top`: the view's unzoomed width its content is laid
//!   out at, the room the transcript's last row keeps for the prompt area
//!   (both unscaled), and the transcript's first row shown and how far into
//!   it, with `tail` while it follows its end;
//! - `build`, `layout`, `prepaint` and `paint`: ms spent building the view's
//!   elements, requesting their layout, prepainting (where the transcript's
//!   rows are built and laid out) and painting;
//! - `rows`: transcript rows built; `texts`: texts laid out, and the ms that
//!   took (shaping included, which the text system's cache should answer);
//! - `faces`: parts of the chat the app drew (see [`crate::face`]), and the
//!   ms that took, from their data being made to the drawing handed back;
//! - `dock` and `field`: the factor the prompt area and the field were built
//!   at this frame, `-` when they were drawn from an earlier frame;
//! - `device`: the window's device pixels to one;
//! - `view`, `content` and `frame`: the view's bounds, the bounds its scaled
//!   content was laid out in, and the field's frame, as `x,y,w,h`.
//!
//! With `KOU_ZOOM_TRACE=rows` the line also lists each transcript row drawn
//! as `index:top:height`, its top relative to the content's.

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};
use std::{
    cell::RefCell,
    fmt::Write as _,
    rc::Rc,
    sync::OnceLock,
    time::{Duration, Instant},
};

/// Whether frames are traced, and with their rows.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Off,
    Frames,
    Rows,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("KOU_ZOOM_TRACE") {
        Ok(value) if value == "rows" => Mode::Rows,
        Ok(value) if !value.is_empty() && value != "0" => Mode::Frames,
        _ => Mode::Off,
    })
}

/// A view's trace, shared with the views nested in it: `None` when off.
pub(crate) type Trace = Option<Rc<RefCell<Record>>>;

/// A trace for a new view, if tracing is on.
pub(crate) fn new() -> Trace {
    if mode() == Mode::Off {
        return None;
    }
    kou_ui::scale::count_text_work(true);
    Some(Rc::new(RefCell::new(Record::default())))
}

/// The frame being drawn, and what is needed to compare it with the last.
#[derive(Default)]
pub(crate) struct Record {
    frames: u64,
    first: Option<Instant>,
    last_start: Option<Instant>,
    frame: Frame,
}

#[derive(Default)]
struct Frame {
    start: Option<Instant>,
    gap: Option<Duration>,
    zoom: f32,
    factor: f32,
    rescaled: bool,
    /// What the view held this frame: its unzoomed width, the room the last
    /// row keeps for the prompt area, and where the transcript is scrolled.
    state: String,
    build: Duration,
    layout: Duration,
    prepaint: Duration,
    paint: Duration,
    rows: u32,
    faces: u32,
    face_time: Duration,
    dock: Option<f32>,
    field: Option<f32>,
    view: Option<Bounds<Pixels>>,
    device: f32,
    content: Option<Bounds<Pixels>>,
    field_frame: Option<Bounds<Pixels>>,
    drawn_rows: Vec<(usize, Bounds<Pixels>)>,
}

/// What a traced element stands for.
#[derive(Clone, Copy)]
pub(crate) enum Part {
    /// Everything the view draws: its frame ends when this is painted.
    Content,
    /// A row of the transcript.
    Row(usize),
    /// The field's frame.
    FieldFrame,
}

/// The view begins a frame at `zoom`, drawn at `factor`; `rescaled` when the
/// factor is new. `state` describes what else the view holds that moves things.
pub(crate) fn begin(
    trace: &Trace,
    zoom: f32,
    factor: f32,
    rescaled: bool,
    state: impl FnOnce() -> String,
) -> Option<Instant> {
    let trace = trace.as_ref()?;
    let now = Instant::now();
    let mut record = trace.borrow_mut();
    record.first.get_or_insert(now);
    let gap = record.last_start.map(|last| now - last);
    record.last_start = Some(now);
    record.frame = Frame {
        start: Some(now),
        gap,
        zoom,
        factor,
        rescaled,
        state: state(),
        ..Frame::default()
    };
    // Text laid out or placed outside a traced frame is not this frame's.
    kou_ui::scale::take_text_work();
    kou_ui::scale::take_text_places();
    Some(now)
}

/// The view's elements were built, since `started`.
pub(crate) fn built(trace: &Trace, started: Option<Instant>) {
    if let (Some(trace), Some(started)) = (trace, started) {
        trace.borrow_mut().frame.build += started.elapsed();
    }
}

/// A transcript row was built.
pub(crate) fn row_built(trace: &Trace) {
    if let Some(trace) = trace {
        trace.borrow_mut().frame.rows += 1;
    }
}

/// When a part the app draws began to be drawn, if tracing is on.
pub(crate) fn face_started(trace: &Trace) -> Option<Instant> {
    trace.as_ref().map(|_| Instant::now())
}

/// A part the app draws was drawn, since `started`.
pub(crate) fn face_drawn(trace: &Trace, started: Option<Instant>) {
    if let (Some(trace), Some(started)) = (trace, started) {
        let frame = &mut trace.borrow_mut().frame;
        frame.faces += 1;
        frame.face_time += started.elapsed();
    }
}

/// The prompt area was built at `factor`.
pub(crate) fn dock_built(trace: &Trace, factor: f32) {
    if let Some(trace) = trace {
        trace.borrow_mut().frame.dock = Some(factor);
    }
}

/// The field was built at `factor`.
pub(crate) fn field_built(trace: &Trace, factor: f32) {
    if let Some(trace) = trace {
        trace.borrow_mut().frame.field = Some(factor);
    }
}

/// The view was laid out in `bounds`, in a window of `device` pixels to one.
pub(crate) fn view_bounds(trace: &Trace, bounds: Bounds<Pixels>, device: f32) {
    if let Some(trace) = trace {
        let frame = &mut trace.borrow_mut().frame;
        frame.view = Some(bounds);
        frame.device = device;
    }
}

/// `child`, traced as `part` when tracing is on.
pub(crate) fn traced(trace: &Trace, part: Part, child: impl IntoElement) -> AnyElement {
    let wanted = !matches!(part, Part::Row(_)) || mode() == Mode::Rows;
    match trace {
        Some(trace) if wanted => Traced {
            trace: trace.clone(),
            part,
            child: child.into_any_element(),
        }
        .into_any_element(),
        _ => child.into_any_element(),
    }
}

/// Times its child's phases or notes where it was laid out, and takes the
/// child's place in the layout.
struct Traced {
    trace: Rc<RefCell<Record>>,
    part: Part,
    child: AnyElement,
}

impl IntoElement for Traced {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Traced {
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
        let started = Instant::now();
        let layout = self.child.request_layout(window, cx);
        if let Part::Content = self.part {
            self.trace.borrow_mut().frame.layout += started.elapsed();
        }
        (layout, ())
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
        let started = Instant::now();
        self.child.prepaint(window, cx);
        let mut record = self.trace.borrow_mut();
        let frame = &mut record.frame;
        match self.part {
            Part::Content => {
                frame.prepaint += started.elapsed();
                frame.content = Some(bounds);
            }
            Part::Row(index) => frame.drawn_rows.push((index, bounds)),
            Part::FieldFrame => frame.field_frame = Some(bounds),
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
        let started = Instant::now();
        self.child.paint(window, cx);
        if let Part::Content = self.part {
            let mut record = self.trace.borrow_mut();
            record.frame.paint += started.elapsed();
            record.frames += 1;
            eprintln!("{}", record.line());
        }
    }
}

impl Record {
    /// The frame as one line.
    fn line(&self) -> String {
        let frame = &self.frame;
        let ms = |duration: Duration| duration.as_secs_f64() * 1000.;
        let since = match (self.first, frame.start) {
            (Some(first), Some(start)) => ms(start - first),
            _ => 0.,
        };
        let text = kou_ui::scale::take_text_work().unwrap_or_default();
        let factor =
            |value: Option<f32>| value.map_or("-".to_owned(), |value| format!("{value:.5}"));
        let rect = |bounds: Option<Bounds<Pixels>>| {
            bounds.map_or("-".to_owned(), |bounds| {
                format!(
                    "{:.3},{:.3},{:.3},{:.3}",
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height)
                )
            })
        };
        let mut line = format!(
            "zoom-trace frame={} t={since:.3} gap={} zoom={:.5} factor={:.5} new={} {} \
             build={:.3} layout={:.3} prepaint={:.3} paint={:.3} rows={} texts={}/{:.3} \
             faces={}/{:.3} dock={} field={} device={} view={} content={} frame={}",
            self.frames,
            frame
                .gap
                .map_or("-".to_owned(), |gap| format!("{:.3}", ms(gap))),
            frame.zoom,
            frame.factor,
            u8::from(frame.rescaled),
            frame.state,
            ms(frame.build),
            ms(frame.layout),
            ms(frame.prepaint),
            ms(frame.paint),
            frame.rows,
            text.layouts,
            ms(text.time),
            frame.faces,
            ms(frame.face_time),
            factor(frame.dock),
            factor(frame.field),
            frame.device,
            rect(frame.view),
            rect(frame.content),
            rect(frame.field_frame),
        );
        if mode() == Mode::Rows {
            let top = frame
                .content
                .map_or(Pixels::ZERO, |content| content.origin.y);
            line.push_str(" rows=");
            for (at, (index, bounds)) in frame.drawn_rows.iter().enumerate() {
                if at > 0 {
                    line.push(';');
                }
                let _ = write!(
                    line,
                    "{index}:{:.3}:{:.3}",
                    f32::from(bounds.origin.y - top),
                    f32::from(bounds.size.height)
                );
            }
        }
        line
    }
}
