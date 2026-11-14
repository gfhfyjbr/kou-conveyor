//! Smooth scrolling in GPUI's scroll containers (the fork's `fast::smooth_scroll`), as the
//! app meets it: wheel events into a window with a scrolling `div`, a `list` and a
//! `uniform_list`, the executor's clock moved on one 240 Hz frame at a time, and the
//! offsets read after every frame.

use gpui::{
    Context, FollowMode, InteractiveElement as _, IntoElement, ListAlignment, ListState,
    ParentElement as _, Pixels, Point, Render, ScrollDelta, ScrollEvent, ScrollGlide, ScrollHandle,
    SmoothScroll, StatefulInteractiveElement as _, Styled as _, TestAppContext,
    UniformListScrollHandle, VisualTestContext, Window, div, list, point, px, rgb, uniform_list,
};
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

/// One frame at 240 Hz.
const FRAME: Duration = Duration::from_micros(4167);
const ROW: f32 = 30.;
const ROWS: usize = 100;
const AREA: f32 = 300.;
/// A `list` takes a line of the wheel for twenty pixels.
const LIST_LINE: f32 = 20.;

struct Areas {
    div: ScrollHandle,
    list: ListState,
    uniform: UniformListScrollHandle,
    smooth_div: bool,
    renders: Rc<Cell<usize>>,
}

impl Render for Areas {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let row = |index: usize| {
            div()
                .w_full()
                .h(px(ROW))
                .bg(rgb(0x202020 + (index as u32 % 7) * 0x111111))
        };
        div()
            .flex()
            .flex_row()
            .size_full()
            .child(
                div()
                    .id("div")
                    .w(px(AREA))
                    .h(px(AREA))
                    .overflow_y_scroll()
                    .track_scroll(&self.div)
                    .smooth_scroll(self.smooth_div)
                    .children((0..ROWS).map(row)),
            )
            .child(
                div().w(px(AREA)).h(px(AREA)).child(
                    list(self.list.clone(), move |index, _, _| {
                        row(index).into_any_element()
                    })
                    .size_full(),
                ),
            )
            .child(
                uniform_list("uniform", ROWS, move |range, _, _| {
                    range.map(row).collect::<Vec<_>>()
                })
                .track_scroll(&self.uniform)
                .w(px(AREA))
                .h(px(AREA)),
            )
    }
}

struct Window3<'a> {
    cx: &'a mut VisualTestContext,
    areas: gpui::Entity<Areas>,
    renders: Rc<Cell<usize>>,
}

fn open(cx: &mut TestAppContext, smooth_div: bool) -> Window3<'_> {
    let renders = Rc::new(Cell::new(0));
    let (areas, cx) = cx.add_window_view({
        let renders = renders.clone();
        move |_, _| Areas {
            div: ScrollHandle::new(),
            list: ListState::new(ROWS, ListAlignment::Top, px(100.)).measure_all(),
            uniform: UniformListScrollHandle::new(),
            smooth_div,
            renders,
        }
    });
    cx.run_until_parked();
    Window3 { cx, areas, renders }
}

const OVER_DIV: Point<Pixels> = point(px(AREA / 2.), px(AREA / 2.));
const OVER_LIST: Point<Pixels> = point(px(AREA * 1.5), px(AREA / 2.));
const OVER_UNIFORM: Point<Pixels> = point(px(AREA * 2.5), px(AREA / 2.));

impl Window3<'_> {
    fn wheel(&mut self, at: Point<Pixels>, delta: ScrollDelta) {
        self.cx.simulate_event(ScrollEvent {
            position: at,
            delta,
            ..Default::default()
        });
    }

    /// Wheel lines; negative scrolls towards the end.
    fn lines(&mut self, at: Point<Pixels>, lines: f32) {
        self.wheel(at, ScrollDelta::Lines(point(0., lines)));
    }

    /// Moves the clock on by a frame and runs it. Returns how many frame callbacks ran,
    /// which is zero once nothing asks for frames.
    fn frame(&mut self) -> usize {
        self.cx.executor().advance_clock(FRAME);
        self.cx.update(|window, cx| window.simulate_next_frame(cx))
    }

    /// Runs frames until none is asked for, reading `offset` after each. Returns the
    /// offsets, the first being where it was before.
    fn frames_until_still(&mut self, offset: impl Fn(&mut Self) -> f32) -> Vec<f32> {
        let mut seen = vec![offset(self)];
        for _ in 0..1000 {
            if self.frame() == 0 {
                return seen;
            }
            seen.push(offset(self));
        }
        panic!("still asking for frames after a thousand: {seen:?}");
    }

    fn div_offset(&mut self) -> f32 {
        let handle = self.areas.read_with(self.cx, |areas, _| areas.div.clone());
        f32::from(handle.offset().y)
    }

    /// The list's top in pixels from its start. A list at its end may keep its top as the
    /// end itself, which this reads as the furthest the top can be.
    fn list_top(&mut self) -> f32 {
        let (top, count) = self.areas.read_with(self.cx, |areas, _| {
            (areas.list.logical_scroll_top(), areas.list.item_count())
        });
        let furthest = count as f32 * ROW - AREA;
        (top.item_ix as f32 * ROW + f32::from(top.offset_in_item)).min(furthest)
    }

    fn uniform_offset(&mut self) -> f32 {
        let handle = self.areas.read_with(self.cx, |areas, _| {
            areas.uniform.0.borrow().base_handle.clone()
        });
        f32::from(handle.offset().y)
    }

    fn line_height(&mut self) -> f32 {
        f32::from(self.cx.update(|window, _| window.line_height()))
    }
}

fn assert_never_back(offsets: &[f32], towards_end: bool) {
    for pair in offsets.windows(2) {
        let moved = pair[1] - pair[0];
        // A div's offset goes down as it scrolls towards the end, a list's top goes up.
        let back = if towards_end { moved > 0. } else { moved < 0. };
        assert!(
            !back,
            "moved back from {} to {}: {offsets:?}",
            pair[0], pair[1]
        );
    }
}

#[gpui::test]
fn a_wheel_step_glides_over_several_frames_to_exactly_one_step(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    let line = window.line_height();
    window.lines(OVER_DIV, -3.);
    // Nothing jumps when the event comes; the frames move it.
    assert_eq!(window.div_offset(), 0.);
    let offsets = window.frames_until_still(Window3::div_offset);
    let target = -3. * line;
    assert_eq!(*offsets.last().unwrap(), target);
    assert_never_back(&offsets, true);
    let moving = offsets.windows(2).filter(|pair| pair[0] != pair[1]).count();
    assert!(moving >= 20, "{moving} frames of motion: {offsets:?}");
    // Quick at first: the first frame covers a share of the step, not all of it.
    let first = offsets[1];
    assert!(first < 0. && first > target / 4., "first frame at {first}");
    // About 150 ms at 240 Hz.
    assert!(
        (30..=40).contains(&(offsets.len() - 1)),
        "{} frames",
        offsets.len() - 1
    );
}

#[gpui::test]
fn steps_during_a_glide_add_up_and_never_go_back(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    let line = window.line_height();
    let mut offsets = vec![window.div_offset()];
    window.lines(OVER_DIV, -1.);
    for _ in 0..5 {
        window.frame();
        offsets.push(window.div_offset());
    }
    // The second step comes while the first is still on its way, as a spun wheel's do.
    window.lines(OVER_DIV, -2.);
    for _ in 0..3 {
        window.lines(OVER_DIV, -1.);
        window.frame();
        offsets.push(window.div_offset());
    }
    offsets.extend(window.frames_until_still(Window3::div_offset));
    assert_eq!(*offsets.last().unwrap(), -6. * line);
    assert_never_back(&offsets, true);
}

#[gpui::test]
fn a_step_against_the_motion_turns_it_round_at_once(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    let line = window.line_height();
    window.lines(OVER_DIV, -3.);
    for _ in 0..6 {
        window.frame();
    }
    let before = window.div_offset();
    window.lines(OVER_DIV, 2.);
    window.frame();
    let after = window.div_offset();
    assert!(after > before, "kept going from {before} to {after}");
    let offsets = window.frames_until_still(Window3::div_offset);
    assert_eq!(*offsets.last().unwrap(), -line);
    assert_never_back(&offsets, false);
}

#[gpui::test]
fn pixel_deltas_apply_in_the_frame_they_arrive(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    window.wheel(OVER_DIV, ScrollDelta::Pixels(point(px(0.), px(-17.))));
    assert_eq!(window.div_offset(), -17.);
    assert_eq!(window.frame(), 0, "a pixel delta asks for no frames");

    // A trackpad that takes over from the wheel stops its glide where it is.
    window.lines(OVER_DIV, -3.);
    window.frame();
    window.frame();
    let gliding = window.div_offset();
    window.wheel(OVER_DIV, ScrollDelta::Pixels(point(px(0.), px(-5.))));
    assert_eq!(window.div_offset(), gliding - 5.);
    window.frame();
    assert_eq!(window.div_offset(), gliding - 5.);
    assert_eq!(window.frame(), 0);
}

#[gpui::test]
fn the_edges_clamp_without_bounce(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    // At the top, a step up moves nothing and asks for nothing.
    window.lines(OVER_DIV, 3.);
    assert_eq!(window.frame(), 0);
    assert_eq!(window.div_offset(), 0.);

    let max = ROWS as f32 * ROW - AREA;
    window.lines(OVER_DIV, -1000.);
    let offsets = window.frames_until_still(Window3::div_offset);
    assert_eq!(*offsets.last().unwrap(), -max);
    assert!(offsets.iter().all(|offset| *offset >= -max), "{offsets:?}");
    assert_never_back(&offsets, true);

    // At the end, a further step moves nothing.
    window.lines(OVER_DIV, -3.);
    assert_eq!(window.frame(), 0);
    assert_eq!(window.div_offset(), -max);
}

#[gpui::test]
fn switched_off_a_step_jumps_as_before(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    window.cx.update(|_, cx| {
        cx.set_global(SmoothScroll {
            enabled: false,
            ..Default::default()
        })
    });
    let line = window.line_height();
    window.lines(OVER_DIV, -3.);
    assert_eq!(window.div_offset(), -3. * line);
    window.lines(OVER_LIST, -3.);
    assert_eq!(window.list_top(), 3. * LIST_LINE);
    assert_eq!(window.frame(), 0);
}

#[gpui::test]
fn a_container_that_opts_out_jumps(cx: &mut TestAppContext) {
    let mut window = open(cx, false);
    let line = window.line_height();
    window.lines(OVER_DIV, -3.);
    assert_eq!(window.div_offset(), -3. * line);
    // The others still glide.
    window.lines(OVER_UNIFORM, -3.);
    assert_eq!(window.uniform_offset(), 0.);
    let offsets = window.frames_until_still(Window3::uniform_offset);
    assert_eq!(*offsets.last().unwrap(), -3. * line);
}

#[gpui::test]
fn a_uniform_list_glides_like_a_div(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    let line = window.line_height();
    window.lines(OVER_UNIFORM, -2.);
    assert_eq!(window.uniform_offset(), 0.);
    let offsets = window.frames_until_still(Window3::uniform_offset);
    assert_eq!(*offsets.last().unwrap(), -2. * line);
    assert_never_back(&offsets, true);
    assert!(offsets.len() > 20);
}

#[gpui::test]
fn areas_glide_on_their_own(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    let line = window.line_height();
    window.lines(OVER_DIV, -3.);
    for _ in 0..4 {
        window.frame();
    }
    // The pointer moves to another area mid-glide: each finishes its own.
    window.lines(OVER_UNIFORM, -1.);
    let mut both = Vec::new();
    while window.frame() > 0 {
        both.push((window.div_offset(), window.uniform_offset()));
    }
    let (div, uniform) = *both.last().unwrap();
    assert_eq!(div, -3. * line);
    assert_eq!(uniform, -line);
}

#[gpui::test]
fn code_that_sets_an_offset_wins_over_a_glide(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    window.lines(OVER_DIV, -5.);
    window.frame();
    window.frame();
    let handle = window
        .areas
        .read_with(window.cx, |areas, _| areas.div.clone());
    handle.set_offset(point(px(0.), px(-10.)));
    window.frame();
    assert_eq!(window.div_offset(), -10.);
    assert_eq!(window.frame(), 0);
    assert_eq!(window.div_offset(), -10.);
}

#[gpui::test]
fn a_list_glides_and_its_steps_add_up(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    window.lines(OVER_LIST, -2.);
    assert_eq!(window.list_top(), 0.);
    let mut tops = vec![0.];
    for _ in 0..6 {
        window.frame();
        tops.push(window.list_top());
    }
    window.lines(OVER_LIST, -3.);
    tops.extend(window.frames_until_still(Window3::list_top));
    assert!(
        (tops.last().unwrap() - 5. * LIST_LINE).abs() < 0.01,
        "{tops:?}"
    );
    assert_never_back(&tops, false);
    assert!(tops.len() > 20);
}

#[gpui::test]
fn a_list_following_its_end_follows_again_when_a_glide_brings_it_back(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    let state = window
        .areas
        .read_with(window.cx, |areas, _| areas.list.clone());
    state.set_follow_mode(FollowMode::Tail);
    window.cx.update(|window, _| window.refresh());
    window.cx.run_until_parked();
    assert!(state.is_following_tail());
    let end = window.list_top();
    assert_eq!(end, ROWS as f32 * ROW - AREA);

    // A step up leaves the end on the glide's first frame.
    window.lines(OVER_LIST, 3.);
    window.frame();
    assert!(!state.is_following_tail());
    window.frames_until_still(Window3::list_top);
    assert!((window.list_top() - (end - 3. * LIST_LINE)).abs() < 0.01);

    // A step back down glides to the end while rows keep arriving; it lands on the end
    // they make, and the list follows it again.
    window.lines(OVER_LIST, -6.);
    let mut count = ROWS;
    let mut tops = vec![window.list_top()];
    for frame in 0..200 {
        if frame % 4 == 0 && frame < 24 {
            state.splice(count..count, 1);
            count += 1;
        }
        let asked = window.frame();
        tops.push(window.list_top());
        if asked == 0 && frame >= 24 {
            break;
        }
    }
    window.cx.update(|window, _| window.refresh());
    window.cx.run_until_parked();
    assert!(state.is_following_tail(), "{tops:?}");
    assert_eq!(state.is_scrolled_to_end(), Some(true));
    assert_eq!(window.list_top(), count as f32 * ROW - AREA);
    assert_never_back(&tops, false);
}

#[gpui::test]
fn nothing_is_drawn_after_a_glide_ends(cx: &mut TestAppContext) {
    let mut window = open(cx, true);
    window.lines(OVER_DIV, -3.);
    let before = window.renders.get();
    let mut frames = 0;
    while window.frame() > 0 {
        frames += 1;
    }
    let during = window.renders.get() - before;
    // Every frame of the glide draws once, and only those.
    assert!(
        during >= frames && during <= frames + 1,
        "{during} renders in {frames} frames"
    );
    let after = window.renders.get();
    for _ in 0..60 {
        assert_eq!(window.frame(), 0);
    }
    window.cx.run_until_parked();
    assert_eq!(window.renders.get(), after);
}

fn at(start: Instant, millis: f32) -> Instant {
    start + Duration::from_secs_f32(millis / 1000.)
}

#[test]
fn a_glide_eases_out_onto_its_target_without_passing_it() {
    let start = Instant::now();
    let mut glide = ScrollGlide::new(0.);
    glide.glide_to(60., start, Duration::from_millis(150));
    let mut last = 0.;
    let mut last_speed = f32::INFINITY;
    for step in 1..=40 {
        let now = at(start, step as f32 * 4.167);
        let position = glide.position(now);
        let speed = glide.velocity(now);
        assert!(
            position >= last && position <= 60.,
            "{position} after {last}"
        );
        assert!(speed <= last_speed, "sped up to {speed}");
        last = position;
        last_speed = speed;
    }
    assert_eq!(glide.position(at(start, 150.)), 60.);
    assert!(!glide.is_moving(at(start, 150.)));
    // Half way through it has covered seven eighths.
    assert!((glide.position(at(start, 75.)) - 52.5).abs() < 0.01);
}

#[test]
fn a_glide_sent_further_keeps_its_speed_and_one_sent_back_turns_at_once() {
    let start = Instant::now();
    let duration = Duration::from_millis(150);
    let mut glide = ScrollGlide::new(0.);
    glide.glide_to(100., start, duration);
    let then = at(start, 20.);
    let (position, speed) = (glide.position(then), glide.velocity(then));

    let mut further = glide;
    further.glide_to(110., then, duration);
    assert_eq!(further.position(then), position);
    assert!(
        (further.velocity(then) - speed).abs() < speed * 1e-4,
        "the speed it had"
    );
    assert!(further.velocity(at(start, 21.)) > 0.);
    assert_eq!(further.position(at(start, 200.)), 110.);

    let mut much_further = glide;
    much_further.glide_to(400., then, duration);
    assert!(
        much_further.velocity(then) > speed,
        "a long way ahead speeds it up"
    );

    let mut back = glide;
    back.glide_to(0., then, duration);
    assert!(back.velocity(then) < 0.);
    assert!(back.position(at(start, 24.)) < position);
    assert_eq!(back.position(at(start, 20. + 150.)), 0.);
}

#[test]
fn a_glide_of_no_duration_jumps() {
    let start = Instant::now();
    let mut glide = ScrollGlide::new(5.);
    glide.glide_to(50., start, Duration::ZERO);
    assert_eq!(glide.position(start), 50.);
    assert!(!glide.is_moving(start));
}
