//! The view against a real child: keys, text, the clipboard and the mouse reach the program
//! the way a terminal program expects them.

#![cfg(unix)]

use gpui::{
    AppContext as _, ClipboardItem, Context, Entity, EntityInputHandler as _, Focusable as _,
    IntoElement, KeyBinding, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, ScrollDelta, ScrollEvent, Size, Styled as _,
    TestAppContext, TouchPhase, VisualTestContext, Window, actions, div, point, px, size,
};
use kou_term::{Spawn, Terminal, TerminalStyle, TerminalView};
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

actions!(view_test, [FocusNext, NextPane]);

/// A program that prints every byte it receives with control characters (tabs included)
/// made visible, after saying it is ready: keys typed before `stty raw` took effect would be line-edited.
const SHOW_INPUT: &str = "stty raw -echo; printf ready; cat -vt";

fn open<'a>(
    cx: &'a mut TestAppContext,
    script: &str,
) -> (
    Entity<TerminalView>,
    Entity<Terminal>,
    &'a mut VisualTestContext,
) {
    let spawn = Spawn {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), script.into()],
        env: Vec::new(),
        cwd: std::env::temp_dir(),
    };
    // The child and its reader thread are outside the test scheduler's control.
    cx.executor().allow_parking();
    let terminal = cx
        .update(|cx| Terminal::open(spawn, cx))
        .expect("start the child");
    let (view, cx) = cx.add_window_view({
        let terminal = terminal.clone();
        move |window, cx| TerminalView::new(terminal, TerminalStyle::default(), window, cx)
    });
    view.update_in(cx, |view, window, cx| {
        let handle = view.focus_handle(cx);
        window.focus(&handle, cx);
    });
    cx.run_until_parked();
    (view, terminal, cx)
}

fn text(cx: &mut VisualTestContext, terminal: &Entity<Terminal>) -> String {
    terminal
        .read_with(cx, |terminal, _| terminal.text(None, false))
        .replace('\n', "")
}

fn wait_for_text(cx: &mut VisualTestContext, terminal: &Entity<Terminal>, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        let shown = text(cx, terminal);
        if shown.contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected:?}; the screen shows {shown:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
        cx.executor().advance_clock(Duration::from_millis(17));
    }
}

/// One frame at 240 Hz.
const FRAME: Duration = Duration::from_micros(4167);

/// Moves the clock on by a frame and draws it. Returns how many frame callbacks ran: none
/// once nothing moves.
fn frame(cx: &mut VisualTestContext) -> usize {
    cx.executor().advance_clock(FRAME);
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

/// Draws frames until nothing asks for another: a glide has arrived.
fn settle(cx: &mut VisualTestContext) {
    let mut still = 0;
    for _ in 0..2000 {
        cx.run_until_parked();
        still = if frame(cx) == 0 { still + 1 } else { 0 };
        if still == 2 {
            return;
        }
    }
    panic!("the view never came to rest");
}

/// A program that prints `row 000` to `row 199`, one a line, and waits.
const NUMBERED: &str =
    "i=0; while [ $i -lt 200 ]; do printf 'row %03d\\n' $i; i=$((i+1)); done; sleep 60";

/// What the numbered program's viewport row `line` shows when the view is `back` rows
/// back: its 200 rows and the empty row the cursor waits on, the last `lines` of them on
/// the screen.
fn numbered_row(lines: usize, back: usize, line: isize) -> String {
    let history = 201 - lines as isize;
    format!("row {:03}", history - back as isize + line)
}

/// The middle of a cell's left or right half, in window coordinates.
fn cell_point(
    cx: &mut VisualTestContext,
    view: &Entity<TerminalView>,
    column: usize,
    line: usize,
    right_half: bool,
) -> Point<Pixels> {
    let bounds = view
        .read_with(cx, |view, _| view.cell_bounds(column, line))
        .expect("the cell is on screen");
    let quarter = bounds.size.width / 4.;
    let x = if right_half {
        bounds.right() - quarter
    } else {
        bounds.left() + quarter
    };
    point(x, bounds.top() + bounds.size.height / 2.)
}

#[gpui::test]
fn the_grid_follows_the_window(cx: &mut TestAppContext) {
    let (view, terminal, cx) = open(cx, "stty size; read line; stty size");
    let fitted = |cx: &mut VisualTestContext, width: Pixels, height: Pixels| {
        let style = TerminalStyle::default();
        let cell = view
            .read_with(cx, |view, _| view.cell_bounds(0, 0))
            .expect("drawn")
            .size;
        (
            ((width - style.padding * 2.) / cell.width).floor() as usize,
            ((height - style.padding * 2.) / cell.height).floor() as usize,
        )
    };

    cx.simulate_resize(size(px(640.), px(400.)));
    cx.run_until_parked();
    // A second size so soon after the first is told once it has stood a moment.
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    let expected = fitted(cx, px(640.), px(400.));
    assert_eq!(
        terminal.read_with(cx, |terminal, _| terminal.size()),
        expected
    );

    cx.simulate_resize(size(px(480.), px(300.)));
    cx.run_until_parked();
    // Told at once, or held back a moment, by how close it came to the last one.
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    let expected = fitted(cx, px(480.), px(300.));
    assert_eq!(
        terminal.read_with(cx, |terminal, _| terminal.size()),
        expected
    );

    // The child reads the same size from its terminal.
    terminal.read_with(cx, |terminal, _| terminal.write(b"\r"));
    wait_for_text(cx, &terminal, &format!("{} {}", expected.1, expected.0));
}

#[gpui::test]
fn keys_reach_the_program(cx: &mut TestAppContext) {
    let (_view, terminal, cx) = open(cx, SHOW_INPUT);
    wait_for_text(cx, &terminal, "ready");

    cx.simulate_keystrokes("ctrl-c enter escape up shift-enter tab backspace alt-b f5");
    wait_for_text(cx, &terminal, "^C^M^[^[[A^[^M^I^?^[b^[[15~");
}

#[gpui::test]
fn typed_text_goes_through_text_input(cx: &mut TestAppContext) {
    let (view, terminal, cx) = open(cx, "stty raw -echo; printf ready; cat");
    wait_for_text(cx, &terminal, "ready");

    cx.simulate_input("hello");
    // A layout without Latin letters delivers its characters the same way.
    cx.simulate_input("привет");
    wait_for_text(cx, &terminal, "readyhelloпривет");

    // An input method composes first: nothing is sent until it commits.
    view.update_in(cx, |view, window, cx| {
        view.replace_and_mark_text_in_range(None, "kan", None, window, cx);
        assert_eq!(view.marked_text_range(window, cx), Some(0..3));
    });
    cx.run_until_parked();
    assert_eq!(text(cx, &terminal), "readyhelloпривет");
    view.update_in(cx, |view, window, cx| {
        view.replace_text_in_range(None, "漢", window, cx);
        assert_eq!(view.marked_text_range(window, cx), None);
    });
    wait_for_text(cx, &terminal, "readyhelloпривет漢");
}

#[gpui::test]
fn the_kitty_protocol_disambiguates_when_enabled(cx: &mut TestAppContext) {
    let (_view, terminal, cx) = open(cx, "stty raw -echo; printf '\\033[>1uready'; cat -v");
    wait_for_text(cx, &terminal, "ready");

    cx.simulate_keystrokes("shift-enter ctrl-enter alt-b escape enter");
    wait_for_text(cx, &terminal, "^[[13;2u^[[13;5u^[[98;3u^[[27u^M");
}

#[gpui::test]
fn copy_and_paste_use_the_platform_shortcuts(cx: &mut TestAppContext) {
    let (_view, terminal, cx) = open(cx, SHOW_INPUT);
    wait_for_text(cx, &terminal, "ready");
    let (copy, paste) = if cfg!(target_os = "macos") {
        ("cmd-c", "cmd-v")
    } else {
        ("ctrl-shift-c", "ctrl-shift-v")
    };

    cx.write_to_clipboard(ClipboardItem::new_string("from the clipboard".into()));
    cx.simulate_keystrokes(paste);
    wait_for_text(cx, &terminal, "readyfrom the clipboard");

    // Copy with nothing selected sends nothing; Ctrl+C is still the interrupt.
    cx.simulate_keystrokes(copy);
    cx.simulate_keystrokes("x");
    wait_for_text(cx, &terminal, "readyfrom the clipboardx");
    cx.simulate_keystrokes("ctrl-c");
    wait_for_text(cx, &terminal, "readyfrom the clipboardx^C");
}

#[gpui::test]
fn dragging_selects_and_copy_takes_the_selection(cx: &mut TestAppContext) {
    let (view, terminal, cx) = open(
        cx,
        "printf 'alpha beta gamma\\r\\nsecond line\\r\\n'; sleep 60",
    );
    wait_for_text(cx, &terminal, "second line");
    let none = Modifiers::default();

    // From the left half of "b" to the right half of "a" in "beta".
    let from = cell_point(cx, &view, 6, 0, false);
    let to = cell_point(cx, &view, 9, 0, true);
    cx.simulate_mouse_down(from, MouseButton::Left, none);
    cx.simulate_mouse_move(to, MouseButton::Left, none);
    cx.simulate_mouse_up(to, MouseButton::Left, none);
    assert!(view.update(cx, |view, cx| view.copy(cx)));
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("beta")
    );

    // Across two lines.
    let to = cell_point(cx, &view, 5, 1, true);
    cx.simulate_mouse_down(from, MouseButton::Left, none);
    cx.simulate_mouse_move(to, MouseButton::Left, none);
    cx.simulate_mouse_up(to, MouseButton::Left, none);
    let shortcut = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-shift-c"
    };
    cx.simulate_keystrokes(shortcut);
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("beta gamma\nsecond")
    );

    // A double click takes the word, a triple click the line.
    let click = |cx: &mut VisualTestContext, position: Point<Pixels>, count: usize| {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: none,
            click_count: count,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: none,
            click_count: count,
        });
    };
    let on_gamma = cell_point(cx, &view, 12, 0, false);
    click(cx, on_gamma, 2);
    assert!(view.update(cx, |view, cx| view.copy(cx)));
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("gamma")
    );
    click(cx, on_gamma, 3);
    assert!(view.update(cx, |view, cx| view.copy(cx)));
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("alpha beta gamma\n")
    );

    // A plain click clears the selection.
    click(cx, on_gamma, 1);
    assert!(!view.update(cx, |view, cx| view.copy(cx)));
}

#[gpui::test]
fn the_mouse_is_reported_when_the_program_tracks_it(cx: &mut TestAppContext) {
    let script = "stty raw -echo; printf '\\033[?1002h\\033[?1006hready'; cat -v";
    let (view, terminal, cx) = open(cx, script);
    wait_for_text(cx, &terminal, "ready");
    let none = Modifiers::default();

    let pressed = cell_point(cx, &view, 2, 1, false);
    let dragged = cell_point(cx, &view, 4, 1, false);
    cx.simulate_mouse_down(pressed, MouseButton::Left, none);
    cx.simulate_mouse_move(dragged, MouseButton::Left, none);
    cx.simulate_mouse_up(dragged, MouseButton::Left, none);
    wait_for_text(cx, &terminal, "^[[<0;3;2M^[[<32;5;2M^[[<0;5;2m");

    cx.simulate_event(ScrollEvent {
        position: pressed,
        delta: ScrollDelta::Lines(point(0., -1.)),
        ..Default::default()
    });
    wait_for_text(cx, &terminal, "^[[<65;3;2M");

    // The wheel reaches the program in whole steps at once, as many as its deltas add up
    // to, trackpad pixels included; nothing glides and the history stays put.
    let wheel_down = "^[[<65;3;2M";
    let height = view
        .read_with(cx, |view, _| view.cell_bounds(0, 0))
        .expect("drawn")
        .size
        .height;
    cx.simulate_event(ScrollEvent {
        position: pressed,
        delta: ScrollDelta::Lines(point(0., -3.)),
        ..Default::default()
    });
    assert_eq!(frame(cx), 0);
    wait_for_text(cx, &terminal, &wheel_down.repeat(4));
    for pixels in [2.5, 0.5] {
        cx.simulate_event(ScrollEvent {
            position: pressed,
            delta: ScrollDelta::Pixels(point(px(0.), -height * pixels)),
            ..Default::default()
        });
    }
    wait_for_text(cx, &terminal, &wheel_down.repeat(7));
    assert!(!text(cx, &terminal).contains(&wheel_down.repeat(8)));
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 0);

    // Nothing was selected along the way.
    assert!(!view.update(cx, |view, cx| view.copy(cx)));
}

/// The rows' drawn place in the history, in pixels back from the live screen: whole rows
/// back and how much lower than at rest the first row is drawn.
fn drawn_back(cx: &mut VisualTestContext, view: &Entity<TerminalView>, rest_top: Pixels) -> f32 {
    let (back, top, height) = view.read_with(cx, |view, cx| {
        let cell = view.cell_bounds(0, 0).expect("drawn");
        (view.scrollback_offset(cx), cell.top(), cell.size.height)
    });
    back as f32 * f32::from(height) + f32::from(top - rest_top)
}

#[gpui::test]
fn a_wheel_step_glides_through_the_history_and_rests_on_a_row(cx: &mut TestAppContext) {
    let (view, terminal, cx) = open(cx, NUMBERED);
    wait_for_text(cx, &terminal, "row 199");
    let rest = view
        .read_with(cx, |view, _| view.cell_bounds(0, 0))
        .expect("drawn");
    cx.simulate_event(ScrollEvent {
        position: rest.center(),
        delta: ScrollDelta::Lines(point(0., 3.)),
        ..Default::default()
    });
    cx.run_until_parked();
    let mut drawn = vec![drawn_back(cx, &view, rest.top())];
    while frame(cx) > 0 {
        drawn.push(drawn_back(cx, &view, rest.top()));
    }
    // Over many frames, never back, onto exactly three rows with the first row at rest.
    let moving = drawn.windows(2).filter(|pair| pair[1] != pair[0]).count();
    assert!(moving > 10, "{drawn:?}");
    assert!(drawn.windows(2).all(|pair| pair[1] >= pair[0]), "{drawn:?}");
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 3);
    assert_eq!(
        view.read_with(cx, |view, _| view.cell_bounds(0, 0)),
        Some(rest)
    );
    assert_eq!(*drawn.last().unwrap(), 3. * f32::from(rest.size.height));
}

#[gpui::test]
fn a_trackpad_moves_the_rows_by_its_pixels_and_they_settle_on_a_row(cx: &mut TestAppContext) {
    let (view, terminal, cx) = open(cx, NUMBERED);
    wait_for_text(cx, &terminal, "row 199");
    let rest = view
        .read_with(cx, |view, _| view.cell_bounds(0, 0))
        .expect("drawn");
    let height = rest.size.height;
    let trackpad = |cx: &mut VisualTestContext, rows: f32, phase: TouchPhase| {
        cx.simulate_event(ScrollEvent {
            position: rest.center(),
            delta: ScrollDelta::Pixels(point(px(0.), height * rows)),
            touch_phase: phase,
            ..Default::default()
        });
        cx.run_until_parked();
    };

    // The next frame shows the rows exactly that many pixels lower, between two rows.
    trackpad(cx, 0.5, TouchPhase::Started);
    assert_eq!(drawn_back(cx, &view, rest.top()), f32::from(height * 0.5));
    trackpad(cx, 1.25, TouchPhase::Moved);
    assert_eq!(drawn_back(cx, &view, rest.top()), f32::from(height * 1.75));
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 1);

    // While the fingers are down it stays between the rows.
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(drawn_back(cx, &view, rest.top()), f32::from(height * 1.75));

    // Lifted and still, it settles on the nearer row.
    trackpad(cx, 0., TouchPhase::Ended);
    cx.executor().advance_clock(Duration::from_millis(100));
    settle(cx);
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 2);
    assert_eq!(
        view.read_with(cx, |view, _| view.cell_bounds(0, 0)),
        Some(rest)
    );
}

#[gpui::test]
fn a_click_mid_glide_lands_on_the_row_under_the_pointer(cx: &mut TestAppContext) {
    let (view, terminal, cx) = open(cx, NUMBERED);
    wait_for_text(cx, &terminal, "row 199");
    let lines = terminal.read_with(cx, |terminal, _| terminal.size().1);
    let rest = view
        .read_with(cx, |view, _| view.cell_bounds(0, 0))
        .expect("drawn");
    cx.simulate_event(ScrollEvent {
        position: rest.center(),
        delta: ScrollDelta::Lines(point(0., 4.)),
        ..Default::default()
    });
    cx.run_until_parked();
    // A few frames in, the rows are between two of the history.
    let mut shift = px(0.);
    for _ in 0..40 {
        frame(cx);
        shift = view
            .read_with(cx, |view, _| view.cell_bounds(0, 0))
            .expect("drawn")
            .top()
            - rest.top();
        if shift > px(3.) && shift < rest.size.height - px(3.) {
            break;
        }
    }
    assert!(shift > px(3.), "the rows never stood between two");
    let back = view.read_with(cx, |view, cx| view.scrollback_offset(cx));

    let triple_click = |cx: &mut VisualTestContext, position: Point<Pixels>| {
        for (count, up) in [(3, false), (3, true)] {
            if up {
                cx.simulate_event(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: count,
                });
            } else {
                cx.simulate_event(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: count,
                    first_mouse: false,
                });
            }
        }
        assert!(view.update(cx, |view, cx| view.copy(cx)));
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
    };
    // A row of the viewport, drawn lower than at rest.
    let on_row = cell_point(cx, &view, 0, 2, false);
    assert_eq!(
        triple_click(cx, on_row),
        format!("{}\n", numbered_row(lines, back, 2))
    );
    // The row above the viewport, in the gap at the top.
    let in_gap = point(rest.left() + px(2.), rest.top() + shift / 2.);
    assert_eq!(
        triple_click(cx, in_gap),
        format!("{}\n", numbered_row(lines, back, -1))
    );
}

#[gpui::test]
fn output_at_the_bottom_keeps_the_bottom_and_a_glide_there_arrives(cx: &mut TestAppContext) {
    let script = "i=0; while [ $i -lt 100 ]; do echo line $i; i=$((i+1)); done; read x; \
                  i=0; while [ $i -lt 100 ]; do echo more $i; i=$((i+1)); done; read x; \
                  i=0; while [ $i -lt 50 ]; do echo last $i; i=$((i+1)); done; sleep 60";
    let (view, terminal, cx) = open(cx, script);
    wait_for_text(cx, &terminal, "line 99");
    let rest = view
        .read_with(cx, |view, _| view.cell_bounds(0, 0))
        .expect("drawn");
    let at_bottom = |cx: &mut VisualTestContext| {
        view.read_with(cx, |view, cx| {
            (view.scrollback_offset(cx), view.cell_bounds(0, 0))
        })
    };

    // At the bottom, new output keeps the view there, at once.
    terminal.read_with(cx, |terminal, _| terminal.write(b"\r"));
    wait_for_text(cx, &terminal, "more 99");
    assert_eq!(at_bottom(cx), (0, Some(rest)));

    // Scrolled back, then sent back down: output arriving on the way does not stop it
    // short of the bottom.
    let wheel = |cx: &mut VisualTestContext, lines: f32| {
        cx.simulate_event(ScrollEvent {
            position: rest.center(),
            delta: ScrollDelta::Lines(point(0., lines)),
            ..Default::default()
        });
        cx.run_until_parked();
    };
    wheel(cx, 5.);
    settle(cx);
    assert_eq!(at_bottom(cx), (5, Some(rest)));
    wheel(cx, -5.);
    frame(cx);
    frame(cx);
    terminal.read_with(cx, |terminal, _| terminal.write(b"\r"));
    wait_for_text(cx, &terminal, "last 49");
    settle(cx);
    assert_eq!(at_bottom(cx), (0, Some(rest)));
}

#[gpui::test]
fn the_wheel_scrolls_history_or_moves_the_cursor(cx: &mut TestAppContext) {
    // A long output on the primary screen, then the alternate screen on request.
    let script = "i=0; while [ $i -lt 300 ]; do echo line $i; i=$((i+1)); done; \
                  stty raw -echo; printf ready; dd bs=1 count=1 >/dev/null 2>&1; \
                  printf '\\033[?1049hfull'; cat -v";
    let (view, terminal, cx) = open(cx, script);
    wait_for_text(cx, &terminal, "ready");
    let over = cell_point(cx, &view, 3, 3, false);
    let wheel = |cx: &mut VisualTestContext, lines: f32| {
        cx.simulate_event(ScrollEvent {
            position: over,
            delta: ScrollDelta::Lines(point(0., lines)),
            ..Default::default()
        });
        // The history glides there.
        settle(cx);
    };

    wheel(cx, 5.);
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 5);
    wheel(cx, -2.);
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 3);
    cx.simulate_keystrokes("shift-pageup");
    assert!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)) > 3);
    cx.simulate_keystrokes("shift-end");
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 0);

    // Typing returns to the live screen.
    wheel(cx, 4.);
    cx.simulate_keystrokes("enter");
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 0);

    // On the alternate screen there is no history: the wheel becomes arrow keys.
    wait_for_text(cx, &terminal, "full");
    wheel(cx, 2.);
    wait_for_text(cx, &terminal, "full^[[A^[[A");
    wheel(cx, -1.);
    wait_for_text(cx, &terminal, "full^[[A^[[A^[[B");
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 0);
}

#[gpui::test]
fn application_bindings_do_not_take_terminal_keys(cx: &mut TestAppContext) {
    let (_view, terminal, cx) = open(cx, SHOW_INPUT);
    wait_for_text(cx, &terminal, "ready");

    let focus_moves = Rc::new(Cell::new(0));
    let pane_switches = Rc::new(Cell::new(0));
    cx.update(|_, cx| {
        // The kind of global bindings an application has for its own controls.
        cx.bind_keys([
            KeyBinding::new("tab", FocusNext, None),
            KeyBinding::new("escape", FocusNext, None),
            KeyBinding::new("ctrl-tab", NextPane, None),
        ]);
        let counter = focus_moves.clone();
        cx.on_action(move |_: &FocusNext, _| counter.set(counter.get() + 1));
        let counter = pane_switches.clone();
        cx.on_action(move |_: &NextPane, _| counter.set(counter.get() + 1));
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("tab escape a");
    wait_for_text(cx, &terminal, "ready^I^[a");
    assert_eq!(focus_moves.get(), 0);

    // A chord the terminal does not claim goes to the application.
    cx.simulate_keystrokes("ctrl-tab b");
    wait_for_text(cx, &terminal, "ready^I^[ab");
    assert_eq!(pane_switches.get(), 1);
}

#[gpui::test]
fn focus_is_reported_when_the_program_asks(cx: &mut TestAppContext) {
    let script = "stty raw -echo; printf '\\033[?1004hready'; cat -v";
    let (view, terminal, cx) = open(cx, script);
    wait_for_text(cx, &terminal, "ready");

    // Focus means the focused pane of the active window: activating the window is a
    // focus-in for the pane that already has the keyboard.
    cx.update(|window, _| window.activate_window());
    wait_for_text(cx, &terminal, "ready^[[I");
    cx.deactivate_window();
    wait_for_text(cx, &terminal, "ready^[[I^[[O");
    cx.update(|window, _| window.activate_window());
    wait_for_text(cx, &terminal, "ready^[[I^[[O^[[I");

    // Moving the keyboard elsewhere in the window and back.
    cx.update(|window, cx| window.blur(cx));
    wait_for_text(cx, &terminal, "ready^[[I^[[O^[[I^[[O");
    view.update_in(cx, |view, window, cx| {
        let handle = view.focus_handle(cx);
        window.focus(&handle, cx);
    });
    wait_for_text(cx, &terminal, "ready^[[I^[[O^[[I^[[O^[[I");
}

/// A canvas in miniature: shows a terminal view in a node whose size on screen is its
/// base size times the zoom, the way the application's canvas lays out its nodes.
struct Canvas {
    node: Entity<TerminalView>,
    zoom: Rc<Cell<f32>>,
    base: Size<Pixels>,
}

impl Render for Canvas {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let zoom = self.zoom.get();
        div().size_full().child(
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .w(self.base.width * zoom)
                .h(self.base.height * zoom)
                .child(self.node.clone()),
        )
    }
}

/// The canvas, its terminal view, the terminal, the zoom handle and the window.
type Zoomed<'a> = (
    Entity<Canvas>,
    Entity<TerminalView>,
    Entity<Terminal>,
    Rc<Cell<f32>>,
    &'a mut VisualTestContext,
);

fn open_zoomed<'a>(cx: &'a mut TestAppContext, script: &str, base: Size<Pixels>) -> Zoomed<'a> {
    let spawn = Spawn {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), script.into()],
        env: Vec::new(),
        cwd: std::env::temp_dir(),
    };
    cx.executor().allow_parking();
    let terminal = cx
        .update(|cx| Terminal::open(spawn, cx))
        .expect("start the child");
    let zoom = Rc::new(Cell::new(1.0f32));
    let (canvas, cx) = cx.add_window_view({
        let terminal = terminal.clone();
        let zoom = zoom.clone();
        move |window, cx| {
            let node =
                cx.new(|cx| TerminalView::new(terminal, TerminalStyle::default(), window, cx));
            Canvas { node, zoom, base }
        }
    });
    cx.simulate_resize(size(px(1000.), px(800.)));
    let node = canvas.read_with(cx, |canvas, _| canvas.node.clone());
    node.update_in(cx, |view, window, cx| {
        view.follow_zoom(Some(zoom.clone()), cx);
        let handle = view.focus_handle(cx);
        window.focus(&handle, cx);
    });
    cx.run_until_parked();
    (canvas, node, terminal, zoom, cx)
}

fn set_zoom(cx: &mut VisualTestContext, canvas: &Entity<Canvas>, zoom: &Rc<Cell<f32>>, value: f32) {
    zoom.set(value);
    canvas.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

#[gpui::test]
fn zooming_keeps_the_grid_and_sends_no_resize(cx: &mut TestAppContext) {
    // The program prints its size whenever it gets SIGWINCH, which only a real change of
    // the window size sends.
    let script = "trap 'stty size' WINCH; printf ready; while :; do sleep 0.05; done";
    let base = size(px(420.), px(260.));
    let (canvas, view, terminal, zoom, cx) = open_zoomed(cx, script, base);
    wait_for_text(cx, &terminal, "ready");
    let cell = view
        .read_with(cx, |view, _| view.cell_bounds(0, 0))
        .expect("drawn")
        .size;
    // A node exactly 40 by 12 cells wide inside its padding: the size at which the least
    // rounding in a zoomed layout would lose a column or a row.
    let padding = TerminalStyle::default().padding;
    canvas.update(cx, |canvas, cx| {
        canvas.base = size(
            padding * 2. + cell.width * 40.,
            padding * 2. + cell.height * 12.,
        );
        cx.notify();
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    let grid = terminal.read_with(cx, |terminal, _| terminal.size());
    assert_eq!(grid, (40, 12));
    // The program was just told of that size, and says so: waited for, however
    // long a busy machine takes, so that it is not taken for a later one.
    wait_for_text(cx, &terminal, "12 40");
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    terminal.read_with(cx, |terminal, _| terminal.write(b""));
    let before = text(cx, &terminal);
    assert_eq!(before, "ready12 40");

    // A gesture's worth of steps across the whole range, out and back.
    let mut steps: Vec<f32> = (0..=60).map(|step| 1.0 - step as f32 * 0.015).collect();
    steps.extend((0..=120).map(|step| 0.1 + step as f32 * 0.0325));
    steps.extend([0.5, 0.333, 0.25, 0.1, 0.73, 1.0, 1.5, 2.0, 3.3, 4.0, 1.0]);
    for value in steps {
        set_zoom(cx, &canvas, &zoom, value);
        assert_eq!(
            terminal.read_with(cx, |terminal, _| terminal.size()),
            grid,
            "the grid changed at zoom {value}"
        );
        // Everything is drawn scaled: a cell is the unzoomed cell times the zoom, to
        // within the device pixel its edges are rounded to.
        let zoomed = view
            .read_with(cx, |view, _| view.cell_bounds(1, 1))
            .expect("drawn")
            .size;
        assert!(
            (zoomed.width - cell.width * value).abs() <= px(1.01)
                && (zoomed.height - cell.height * value).abs() <= px(1.01),
            "cell {zoomed:?} at zoom {value}, {cell:?} unzoomed"
        );
    }

    // Long enough for a resize that did happen to have been reported.
    std::thread::sleep(Duration::from_millis(300));
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(text(cx, &terminal), before);

    // Without a handle the view sizes the grid from its own bounds again.
    view.update(cx, |view, cx| view.follow_zoom(None, cx));
    set_zoom(cx, &canvas, &zoom, 0.5);
    let unfollowed = terminal.read_with(cx, |terminal, _| terminal.size());
    assert!(
        unfollowed.0 < grid.0,
        "{unfollowed:?} at half size, {grid:?} before"
    );
}

#[gpui::test]
fn a_click_reaches_the_right_cell_at_any_zoom(cx: &mut TestAppContext) {
    let script = "stty raw -echo; printf '\\033[?1000h\\033[?1006hready'; cat -v";
    let base = size(px(420.), px(260.));
    let (canvas, view, terminal, zoom, cx) = open_zoomed(cx, script, base);
    wait_for_text(cx, &terminal, "ready");
    let cell = view
        .read_with(cx, |view, _| view.cell_bounds(0, 0))
        .expect("drawn")
        .size;
    let padding = TerminalStyle::default().padding;
    let none = Modifiers::default();
    let mut expected = String::from("ready");
    for (value, column, line) in [
        (0.5f32, 5usize, 2usize),
        (2.0, 3, 1),
        (1.0, 7, 4),
        (0.27, 11, 6),
    ] {
        set_zoom(cx, &canvas, &zoom, value);
        // Worked out from the unzoomed cell, not from what the view reports, so the test
        // checks the mapping rather than agreeing with itself.
        let at = point(
            (padding + cell.width * (column as f32 + 0.5)) * value,
            (padding + cell.height * (line as f32 + 0.5)) * value,
        );
        cx.simulate_mouse_down(at, MouseButton::Left, none);
        cx.simulate_mouse_up(at, MouseButton::Left, none);
        expected.push_str(&format!(
            "^[[<0;{c};{l}M^[[<0;{c};{l}m",
            c = column + 1,
            l = line + 1
        ));
        wait_for_text(cx, &terminal, &expected);
    }
}

#[gpui::test]
fn a_dragged_edge_tells_the_program_a_few_sizes_not_every_one(cx: &mut TestAppContext) {
    // The program counts the sizes it is told.
    let script = "n=0; trap 'n=$((n+1)); printf \"<%s>\" $n' WINCH; printf ready; while :; do sleep 0.02; done";
    let (_view, terminal, cx) = open(cx, script);
    wait_for_text(cx, &terminal, "ready");
    // Let the size it started with stand.
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();

    // Sixty frames of a drag, four milliseconds apart: sixty widths.
    for step in 0..60 {
        cx.simulate_resize(size(px(900. - step as f32 * 6.), px(400.)));
        cx.executor().advance_clock(Duration::from_millis(4));
        cx.run_until_parked();
    }
    // The drag ends; the last width is the one the program is left with.
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    let grid = terminal.read_with(cx, |terminal, _| terminal.size());
    std::thread::sleep(Duration::from_millis(400));
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    let shown = terminal.read_with(cx, |terminal, _| terminal.text(None, false));
    let told = shown.matches('<').count();
    assert!(
        (1..=6).contains(&told),
        "told {told} sizes during a quarter of a second of dragging: {shown:?}"
    );
    // And what it has now is the size the view came to rest at.
    assert!(grid.0 < 80, "the grid is {grid:?}");
}

#[gpui::test]
fn the_rows_move_by_screen_pixels_at_any_zoom(cx: &mut TestAppContext) {
    let (canvas, view, terminal, zoom, cx) = open_zoomed(cx, NUMBERED, size(px(500.), px(300.)));
    wait_for_text(cx, &terminal, "row 199");
    set_zoom(cx, &canvas, &zoom, 2.);
    let rest = view
        .read_with(cx, |view, _| view.cell_bounds(0, 0))
        .expect("drawn");
    // A trackpad's pixels move the zoomed rows by as many pixels on screen.
    cx.simulate_event(ScrollEvent {
        position: rest.center(),
        delta: ScrollDelta::Pixels(point(px(0.), rest.size.height * 0.4)),
        touch_phase: TouchPhase::Started,
        ..Default::default()
    });
    cx.run_until_parked();
    let shifted = view
        .read_with(cx, |view, _| view.cell_bounds(0, 0))
        .expect("drawn");
    let moved = shifted.top() - rest.top();
    assert!(
        (moved - rest.size.height * 0.4).abs() <= px(0.5),
        "moved {moved:?} of a {:?} row",
        rest.size.height
    );
    assert_eq!(view.read_with(cx, |view, cx| view.scrollback_offset(cx)), 0);
}

/// A blink for the tests: lit for 300 ms, a 100 ms fade out, dark for 100 ms,
/// a 100 ms fade in.
fn test_blink(elapsed: Duration) -> (f32, Option<Duration>) {
    let at = elapsed.as_millis() % 600;
    let left = |until: u128| Some(Duration::from_millis((until - at) as u64));
    match at {
        0..300 => (1., left(300)),
        300..400 => (1. - (at - 300) as f32 / 100., None),
        400..500 => (0., left(500)),
        _ => ((at - 500) as f32 / 100., None),
    }
}

/// How many frames are asked for over `cycles` of the test blink.
fn frames_over(cx: &mut VisualTestContext, cycles: u32) -> usize {
    let steps = (Duration::from_millis(600) * cycles).as_micros() / FRAME.as_micros();
    let mut asked = 0;
    for _ in 0..steps {
        cx.run_until_parked();
        asked += frame(cx);
    }
    asked
}

#[gpui::test]
fn a_blinking_cursor_asks_for_frames_only_while_it_fades_and_has_the_keys(cx: &mut TestAppContext) {
    // DECSCUSR 1: a blinking block.
    let (view, terminal, cx) = open(cx, "printf 'ready\\033[1 q'; sleep 60");
    cx.update(|window, _| window.activate_window());
    view.update(cx, |view, cx| view.set_cursor_blink(Some(test_blink), cx));
    wait_for_text(cx, &terminal, "ready");
    settle(cx);
    assert!(
        frames_over(cx, 2) > 0,
        "a fading cursor is drawn frame by frame"
    );
    // Without the keys it holds still and asks for nothing.
    view.update_in(cx, |_, window, cx| window.blur(cx));
    cx.run_until_parked();
    frame(cx);
    frame(cx);
    assert_eq!(
        frames_over(cx, 4),
        0,
        "an unfocused cursor asks for no frame"
    );
}

#[gpui::test]
fn a_steady_cursor_asks_for_nothing(cx: &mut TestAppContext) {
    // DECSCUSR 2: a steady block.
    let (view, terminal, cx) = open(cx, "printf 'ready\\033[2 q'; sleep 60");
    cx.update(|window, _| window.activate_window());
    view.update(cx, |view, cx| view.set_cursor_blink(Some(test_blink), cx));
    wait_for_text(cx, &terminal, "ready");
    settle(cx);
    assert_eq!(frames_over(cx, 3), 0, "a steady cursor asks for no frame");
}

#[gpui::test]
fn a_view_without_a_blink_keeps_a_blinking_cursor_lit(cx: &mut TestAppContext) {
    let (_, terminal, cx) = open(cx, "printf 'ready\\033[1 q'; sleep 60");
    cx.update(|window, _| window.activate_window());
    wait_for_text(cx, &terminal, "ready");
    settle(cx);
    assert_eq!(frames_over(cx, 3), 0, "no blink set, no frame asked for");
}

#[gpui::test]
fn the_default_cursor_blinks(cx: &mut TestAppContext) {
    let (view, terminal, cx) = open(cx, "printf ready; sleep 60");
    cx.update(|window, _| window.activate_window());
    view.update(cx, |view, cx| view.set_cursor_blink(Some(test_blink), cx));
    wait_for_text(cx, &terminal, "ready");
    settle(cx);
    assert!(
        frames_over(cx, 2) > 0,
        "a program that sets no style gets a blinking cursor"
    );
}
