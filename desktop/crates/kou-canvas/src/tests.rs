//! The canvas driven as a user drives it: pointer and keys into a test
//! window, and what the board and the events say afterwards.

use crate::{
    BoardEvent, Camera, CanvasEvent, CanvasView, Edge, EdgeMode, Lod, NodeKind, NodeSpec, Offer,
    PortRef, Rect, Wire, catalog::AddRequest, model::Dir, view::Gesture,
};
use gpui::{
    AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyBinding, KeyUpEvent, Keystroke, Modifiers, MouseButton, ParentElement as _, Pixels, Point,
    Render, ScrollDelta, ScrollEvent, Styled as _, TestAppContext, VisualTestContext, Window,
    actions, canvas, div, point, px,
};
use kou_ui::Mark;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

type Events = Rc<RefCell<Vec<CanvasEvent>>>;
type BoardEvents = Rc<RefCell<Vec<BoardEvent>>>;

fn spec(title: &str, tag: &str) -> NodeSpec {
    NodeSpec {
        title: title.into(),
        tag: tag.into(),
        ..NodeSpec::default()
    }
}

fn open(
    board: crate::Board,
    cx: &mut TestAppContext,
) -> (
    Entity<CanvasView>,
    &mut VisualTestContext,
    Events,
    BoardEvents,
) {
    cx.update(|cx| cx.set_global(kou_ui::Theme::new(true)));
    let (view, cx) = cx.add_window_view(|_, cx| CanvasView::new(board, cx));
    let (events, board_events) = subscribe(&view, cx);
    frame(cx);
    (view, cx, events, board_events)
}

fn subscribe(view: &Entity<CanvasView>, cx: &mut VisualTestContext) -> (Events, BoardEvents) {
    let events = Events::default();
    let board_events = BoardEvents::default();
    cx.update(|_, cx| {
        let events = events.clone();
        cx.subscribe(view, move |_, event: &CanvasEvent, _| {
            events.borrow_mut().push(event.clone());
        })
        .detach();
        let board_events = board_events.clone();
        cx.subscribe(view, move |_, event: &BoardEvent, _| {
            board_events.borrow_mut().push(event.clone());
        })
        .detach();
    });
    cx.run_until_parked();
    (events, board_events)
}

/// Draw twice: the first frame measures the canvas, the second lays out in it.
fn frame(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }
    cx.run_until_parked();
}

/// Hold the camera where a test wants it, and draw.
fn at_camera(view: &Entity<CanvasView>, camera: Camera, cx: &mut VisualTestContext) {
    view.update(cx, |view, cx| view.set_camera(camera, cx));
    frame(cx);
}

/// Where a world point is in the window.
fn screen(view: &Entity<CanvasView>, x: f32, y: f32, cx: &mut VisualTestContext) -> Point<Pixels> {
    view.read_with(cx, |view, _| {
        let (sx, sy) = view.camera.to_screen(x, y);
        view.area.get().origin + point(px(sx), px(sy))
    })
}

fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>, modifiers: Modifiers) {
    cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
    // Two steps, as a hand moves: the first passes the drag's threshold.
    let middle = from + (to - from) / 2.;
    cx.simulate_mouse_move(middle, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(to, MouseButton::Left, modifiers);
    cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
}

fn drag_world(
    view: &Entity<CanvasView>,
    cx: &mut VisualTestContext,
    from: (f32, f32),
    to: (f32, f32),
    modifiers: Modifiers,
) {
    let from = screen(view, from.0, from.1, cx);
    let to = screen(view, to.0, to.1, cx);
    drag(cx, from, to, modifiers);
}

/// Three agents in a row, a hundred units apart, at a camera of 1 with the
/// world origin at (40, 40): what most pointer tests work on.
fn row_of_three(
    cx: &mut TestAppContext,
) -> (
    Entity<CanvasView>,
    &mut VisualTestContext,
    Events,
    BoardEvents,
    [u64; 3],
) {
    let mut board = crate::Board::new();
    let a = board.add_rect(spec("a", "CC"), Rect::new(0., 0., 240., 160.));
    let b = board.add_rect(spec("b", "CX"), Rect::new(400., 0., 240., 160.));
    let c = board.add_rect(spec("c", "OC"), Rect::new(400., 320., 240., 160.));
    let (view, cx, events, board_events) = open(board, cx);
    at_camera(
        &view,
        Camera {
            x: 40.,
            y: 40.,
            zoom: 1.,
        },
        cx,
    );
    (view, cx, events, board_events, [a, b, c])
}

#[gpui::test]
fn the_keyboard_walks_the_board_opens_and_asks(cx: &mut TestAppContext) {
    let mut board = crate::Board::new();
    let root = board.add_at(spec("lead", "CC"), 0., 0.);
    let child = board.add_child(root, spec("tests", "CX")).unwrap();
    let (view, cx, events, _) = open(board, cx);
    view.update_in(cx, |view, window, cx| window.focus(&view.focus, cx));

    cx.simulate_keystrokes("right");
    assert_eq!(view.read_with(cx, |view, _| view.selected()), Some(root));
    cx.simulate_keystrokes("right");
    assert_eq!(view.read_with(cx, |view, _| view.selected()), Some(child));
    cx.simulate_keystrokes("left enter backspace");
    // N opens what can be added; with no offers from the app, starting an
    // agent asks the app.
    cx.simulate_keystrokes("n");
    frame(cx);
    assert!(view.read_with(cx, |view, _| view.menu.is_some()));
    cx.simulate_keystrokes("enter");
    frame(cx);
    cx.simulate_keystrokes("escape");
    assert_eq!(view.read_with(cx, |view, _| view.selected()), None);
    let events = events.borrow();
    assert!(
        events.contains(&CanvasEvent::Opened(root)),
        "no body to enter: it opens as a pane"
    );
    assert!(events.contains(&CanvasEvent::RemoveRequested(root)));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CanvasEvent::CreateRequested { .. })),
        "{events:?}"
    );
    // Nothing was removed: that is the owner's decision.
    assert!(view.read_with(cx, |view, _| view.board().node(root).is_some()));
}

#[gpui::test]
fn a_drag_by_the_header_moves_a_node_and_the_middle_button_moves_the_camera(
    cx: &mut TestAppContext,
) {
    let mut board = crate::Board::new();
    let root = board.add_at(spec("lead", "CC"), 0., 0.);
    let (view, cx, events, _) = open(board, cx);
    let (camera, rect) = view.read_with(cx, |view, _| {
        (view.camera(), view.board().node(root).unwrap().rect)
    });
    let grab = screen(&view, rect.x + 120., rect.y + 12., cx);
    drag(
        cx,
        grab,
        grab + point(px(64.), px(32.)),
        Modifiers::default(),
    );
    let moved = view.read_with(cx, |view, _| view.board().node(root).unwrap().rect);
    assert!((moved.x - rect.x - 64. / camera.zoom).abs() <= crate::SNAP / 2. + 0.01);
    assert!((moved.y - rect.y - 32. / camera.zoom).abs() <= crate::SNAP / 2. + 0.01);
    assert_eq!(moved.x % crate::SNAP, 0., "on the grid");
    assert_eq!(view.read_with(cx, |view, _| view.selected()), Some(root));

    let empty = screen(&view, moved.right() + 200., moved.bottom() + 200., cx);
    cx.simulate_mouse_down(empty, MouseButton::Middle, Modifiers::default());
    cx.simulate_mouse_move(
        empty + point(px(-30.), px(10.)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    cx.simulate_mouse_up(
        empty + point(px(-30.), px(10.)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    let after = view.read_with(cx, |view, _| view.camera());
    assert!((after.x - camera.x + 30.).abs() < 0.5 && (after.y - camera.y - 10.).abs() < 0.5);
    assert!(
        events
            .borrow()
            .iter()
            .filter(|event| **event == CanvasEvent::Changed)
            .count()
            >= 2
    );
}

#[gpui::test]
fn a_drag_from_a_port_to_another_node_wires_them(cx: &mut TestAppContext) {
    let (view, cx, _, board_events, [a, b, _]) = row_of_three(cx);
    let (out, inp) = view.read_with(cx, |view, _| {
        let board = view.board();
        (
            board.node(a).unwrap().port_point(Dir::Out, "out"),
            board.node(b).unwrap().port_point(Dir::In, "in"),
        )
    });
    let from = screen(&view, out.0, out.1, cx);
    // Let go a little off the port: the wire docks in it.
    let to = screen(&view, inp.0 + 9., inp.1 - 6., cx);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        from + point(px(80.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert!(view.read_with(cx, |view, _| matches!(view.gesture, Some(Gesture::Wire(_)))));
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    let docked = view.read_with(cx, |view, _| match &view.gesture {
        Some(Gesture::Wire(wiring)) => wiring.target.clone(),
        _ => None,
    });
    assert_eq!(docked, Some(PortRef::new(b, "in")));
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());

    let edges: Vec<Edge> = view.read_with(cx, |view, _| view.board().edges().to_vec());
    assert_eq!(edges.len(), 1);
    assert_eq!(
        (edges[0].from, edges[0].to, edges[0].mode),
        (a, b, EdgeMode::Auto)
    );
    assert!(
        board_events
            .borrow()
            .contains(&BoardEvent::EdgeAdded(edges[0].clone()))
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.selection().1),
        Some(edges[0].id)
    );

    // Escape lets go of a wire being drawn: nothing is wired.
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |view, _| view.gesture.is_none()));
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    assert_eq!(view.read_with(cx, |view, _| view.board().edges().len()), 1);

    // The same wire again is refused and says so. (With the edge selected, its
    // end handle sits on the port and would move the edge instead.)
    cx.simulate_keystrokes("escape");
    frame(cx);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    assert_eq!(view.read_with(cx, |view, _| view.board().edges().len()), 1);
    assert!(view.read_with(cx, |view, _| view.toast.is_some()));
}

#[gpui::test]
fn a_click_on_a_curve_selects_its_edge_delete_removes_it_and_undo_brings_it_back(
    cx: &mut TestAppContext,
) {
    let (view, cx, _, board_events, [a, b, _]) = row_of_three(cx);
    let edge = view.update(cx, |view, cx| {
        view.connect(PortRef::new(a, "out"), PortRef::new(b, "in"), cx)
            .unwrap()
    });
    frame(cx);
    let middle = view.read_with(cx, |view, _| {
        let e = view.board().edge(edge).unwrap();
        crate::edge_curve(view.board(), e).unwrap().middle()
    });
    // A few pixels off the line still picks it.
    let at = screen(&view, middle.0, middle.1 + 4., cx);
    cx.simulate_click(at, Modifiers::default());
    assert_eq!(view.read_with(cx, |view, _| view.selection().1), Some(edge));
    // Far off it, it does not.
    view.read_with(cx, |view, _| {
        let local = view.camera.to_screen(middle.0, middle.1 + 40.);
        assert_eq!(view.edge_at(local), None);
    });

    cx.simulate_keystrokes("backspace");
    assert!(view.read_with(cx, |view, _| view.board().edges().is_empty()));
    assert!(
        board_events
            .borrow()
            .iter()
            .any(|e| matches!(e, BoardEvent::EdgeRemoved(edge_) if edge_.id == edge))
    );
    cx.simulate_keystrokes("cmd-z");
    assert!(view.read_with(cx, |view, _| view.board().edge(edge).is_some()));
    assert!(
        board_events
            .borrow()
            .iter()
            .any(|e| matches!(e, BoardEvent::EdgeAdded(edge_) if edge_.id == edge))
    );
    cx.simulate_keystrokes("cmd-shift-z");
    assert!(
        view.read_with(cx, |view, _| view.board().edge(edge).is_none()),
        "redo takes it again"
    );
}

#[gpui::test]
fn the_selected_edges_end_moves_to_another_node_and_m_cycles_its_mode(cx: &mut TestAppContext) {
    let (view, cx, _, board_events, [a, b, c]) = row_of_three(cx);
    let edge = view.update(cx, |view, cx| {
        let edge = view
            .connect(PortRef::new(a, "out"), PortRef::new(b, "in"), cx)
            .unwrap();
        view.select_edge(Some(edge), cx);
        edge
    });
    frame(cx);
    view.update_in(cx, |view, window, cx| window.focus(&view.focus, cx));
    let (end, target) = view.read_with(cx, |view, _| {
        let board = view.board();
        (
            board.node(b).unwrap().port_point(Dir::In, "in"),
            board.node(c).unwrap().port_point(Dir::In, "in"),
        )
    });
    drag_world(
        &view,
        cx,
        (end.0, end.1),
        (target.0, target.1),
        Modifiers::default(),
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.board().edge(edge).unwrap().to),
        c
    );
    assert!(board_events.borrow().iter().any(|e| matches!(
        e,
        BoardEvent::EdgeChanged { before, after } if before.to == b && after.to == c
    )));

    cx.simulate_keystrokes("m");
    assert_eq!(
        view.read_with(cx, |view, _| view.board().edge(edge).unwrap().mode),
        EdgeMode::Approve
    );
    cx.simulate_keystrokes("m");
    assert_eq!(
        view.read_with(cx, |view, _| view.board().edge(edge).unwrap().mode),
        EdgeMode::Off
    );
}

#[gpui::test]
fn a_marquee_selects_what_it_covers_and_the_selection_moves_together(cx: &mut TestAppContext) {
    let (view, cx, _, board_events, [a, b, c]) = row_of_three(cx);
    // From above the left of a, to below the middle of b: a and b, not c.
    drag_world(&view, cx, (-20., -20.), (500., 100.), Modifiers::default());
    let selected = view.read_with(cx, |view, _| view.selection().0.to_vec());
    assert_eq!(selected, vec![a, b]);
    assert!(
        board_events
            .borrow()
            .iter()
            .any(|e| matches!(e, BoardEvent::SelectionChanged { nodes, .. } if nodes.len() == 2))
    );
    // Shift adds what a second marquee covers.
    drag_world(&view, cx, (380., 300.), (420., 340.), Modifiers::shift());
    assert_eq!(view.read_with(cx, |view, _| view.selection().0.len()), 3);
    // A click on the board clears it.
    let empty = screen(&view, 900., 900., cx);
    cx.simulate_click(empty, Modifiers::default());
    assert!(view.read_with(cx, |view, _| view.selection().0.is_empty()));

    // Select a and b again; dragging a's header moves both by the same.
    view.update(cx, |view, cx| view.select_nodes(&[a, b], cx));
    frame(cx);
    let before: Vec<Rect> = view.read_with(cx, |view, _| {
        [a, b, c]
            .map(|id| view.board().node(id).unwrap().rect)
            .to_vec()
    });
    drag_world(&view, cx, (120., 12.), (168., 92.), Modifiers::default());
    let after: Vec<Rect> = view.read_with(cx, |view, _| {
        [a, b, c]
            .map(|id| view.board().node(id).unwrap().rect)
            .to_vec()
    });
    assert_eq!(
        (after[0].x - before[0].x, after[0].y - before[0].y),
        (48., 80.)
    );
    assert_eq!(
        (after[1].x - before[1].x, after[1].y - before[1].y),
        (48., 80.)
    );
    assert_eq!(after[2], before[2], "what is not selected stays");
    // Undo puts both back at once.
    view.update_in(cx, |view, window, cx| window.focus(&view.focus, cx));
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(
        view.read_with(cx, |view, _| view.board().node(b).unwrap().rect),
        before[1]
    );
}

#[gpui::test]
fn the_corner_resizes_a_node_within_its_limits(cx: &mut TestAppContext) {
    let (view, cx, _, _, [a, _, _]) = row_of_three(cx);
    view.update(cx, |view, cx| view.select(Some(a), cx));
    frame(cx);
    let corner = screen(&view, 240. - 6., 160. - 6., cx);
    drag(
        cx,
        corner,
        corner + point(px(100.), px(60.)),
        Modifiers::default(),
    );
    let rect = view.read_with(cx, |view, _| view.board().node(a).unwrap().rect);
    assert_eq!((rect.w, rect.h), (344., 224.));
    frame(cx);
    let corner = screen(&view, rect.w - 6., rect.h - 6., cx);
    drag(
        cx,
        corner,
        corner - point(px(900.), px(900.)),
        Modifiers::default(),
    );
    let rect = view.read_with(cx, |view, _| view.board().node(a).unwrap().rect);
    assert_eq!((rect.w, rect.h), (crate::MIN_W, crate::MIN_H));
}

struct Body {
    focus: FocusHandle,
    typed: Rc<RefCell<String>>,
}

impl Render for Body {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let typed = self.typed.clone();
        div()
            .track_focus(&self.focus)
            .size_full()
            .on_key_down(move |event, _, cx| {
                typed.borrow_mut().push_str(&event.keystroke.key);
                cx.stop_propagation();
            })
            .child("body")
    }
}

#[gpui::test]
fn entering_a_node_gives_its_body_the_keys_and_cmd_escape_takes_them_back(cx: &mut TestAppContext) {
    let (view, cx, _, board_events, [a, _, _]) = row_of_three(cx);
    let typed = Rc::new(RefCell::new(String::new()));
    let body = cx.update(|_, cx| {
        let typed = typed.clone();
        cx.new(|cx| Body {
            focus: cx.focus_handle(),
            typed,
        })
    });
    let focus = body.read_with(cx, |body, _| body.focus.clone());
    view.update(cx, |view, cx| {
        view.set_body(a, body.clone().into(), cx);
        view.set_body_focus(a, focus.clone(), cx);
        view.select(Some(a), cx);
    });
    view.update_in(cx, |view, window, cx| window.focus(&view.focus, cx));
    frame(cx);
    cx.simulate_keystrokes("enter");
    frame(cx);
    assert_eq!(view.read_with(cx, |view, _| view.entered()), Some(a));
    assert!(cx.update(|window, _| focus.is_focused(window)));
    // Keys go to the body, not the board: arrows do not walk.
    cx.simulate_keystrokes("x right");
    assert_eq!(*typed.borrow(), "xright");
    assert_eq!(view.read_with(cx, |view, _| view.selected()), Some(a));

    cx.simulate_keystrokes("cmd-escape");
    frame(cx);
    assert_eq!(view.read_with(cx, |view, _| view.entered()), None);
    assert!(cx.update(|window, cx| view.read(cx).focus.is_focused(window)));
    let entered: Vec<_> = board_events
        .borrow()
        .iter()
        .filter_map(|e| match e {
            BoardEvent::Entered(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(entered, vec![Some(a), None]);
    // The board has the keys again.
    cx.simulate_keystrokes("right");
    assert_ne!(view.read_with(cx, |view, _| view.selected()), Some(a));
}

actions!(canvas_test, [FocusNext]);

struct Host {
    canvas: Entity<CanvasView>,
    after: FocusHandle,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
            .child(div().flex_1().child(self.canvas.clone()))
            .child(
                div()
                    .id("after")
                    .h(px(20.))
                    .track_focus(&self.after)
                    .tab_index(0)
                    .child("after"),
            )
    }
}

#[gpui::test]
fn tab_leaves_the_canvas_and_a_control_answers_enter(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(kou_ui::Theme::new(true));
        cx.bind_keys([KeyBinding::new("tab", FocusNext, None)]);
    });
    let mut board = crate::Board::new();
    board.add_at(spec("lead", "CC"), 0., 0.);
    let (host, cx) = cx.add_window_view(|_, cx| {
        let canvas = cx.new(|cx| CanvasView::new(board, cx));
        Host {
            canvas,
            after: cx.focus_handle().tab_index(0).tab_stop(true),
        }
    });
    let (canvas, after) = host.read_with(cx, |host, _| (host.canvas.clone(), host.after.clone()));
    frame(cx);
    canvas.update_in(cx, |view, window, cx| window.focus(&view.focus, cx));
    // The first Tab goes to the canvas's own bar: its Add answers Enter, on release.
    cx.simulate_keystrokes("tab");
    // A control answers Enter once it is drawn focused.
    frame(cx);
    // A control clicks when Enter is released, not pressed.
    cx.simulate_keystrokes("enter");
    assert!(canvas.read_with(cx, |view, _| view.menu.is_none()));
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").unwrap(),
    });
    frame(cx);
    assert!(
        canvas.read_with(cx, |view, _| view.menu.is_some()),
        "Enter on Add opens the add menu"
    );
    cx.simulate_keystrokes("escape");
    frame(cx);
    // Tab goes on and out: no trap.
    let mut left = false;
    for _ in 0..12 {
        cx.simulate_keystrokes("tab");
        frame(cx);
        if cx.update(|window, _| after.is_focused(window)) {
            left = true;
            break;
        }
    }
    assert!(left, "Tab reached the control after the canvas");
    assert_eq!(
        canvas.read_with(cx, |view, _| view.selected()),
        None,
        "Tab never walked the nodes"
    );
}

#[gpui::test]
fn a_wire_let_go_on_the_board_asks_what_to_add_there_and_the_answer_is_wired(
    cx: &mut TestAppContext,
) {
    let (view, cx, _, board_events, [a, _, _]) = row_of_three(cx);
    view.update(cx, |view, cx| {
        view.set_offers(
            vec![
                Offer {
                    id: "claude".into(),
                    title: "Claude Code".into(),
                    code: "CC".into(),
                    kind: NodeKind::Agent,
                    detail: "2.1".into(),
                    ready: true,
                },
                Offer {
                    id: "gemini".into(),
                    title: "Gemini CLI".into(),
                    code: "GM".into(),
                    kind: NodeKind::Agent,
                    detail: "not installed".into(),
                    ready: false,
                },
            ],
            cx,
        )
    });
    frame(cx);
    let out = view.read_with(cx, |view, _| {
        view.board().node(a).unwrap().port_point(Dir::Out, "out")
    });
    drag_world(
        &view,
        cx,
        (out.0, out.1),
        (out.0 + 120., out.1 + 200.),
        Modifiers::default(),
    );
    frame(cx);
    let items: Vec<String> = view.read_with(cx, |view, _| {
        view.menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|item| item.label.to_string())
            .collect()
    });
    assert_eq!(
        items,
        ["Claude Code", "Gemini CLI"],
        "only what takes input, and no note"
    );
    // Down skips what cannot start; Enter picks what is highlighted.
    cx.simulate_keystrokes("down enter");
    let request = board_events.borrow().iter().find_map(|e| match e {
        BoardEvent::AddRequested(request) => Some(request.clone()),
        _ => None,
    });
    let request: AddRequest = request.expect("the app is asked");
    assert_eq!(request.offer, "claude");
    assert_eq!(request.wire, Some(Wire::From(PortRef::new(a, "out"))));
    assert!((request.x - (out.0 + 120.)).abs() < 1. && (request.y - (out.1 + 200.)).abs() < 1.);

    let made = view.update_in(cx, |view, window, cx| {
        view.fulfil(&request, spec("tests", "CC"), window, cx)
    });
    let edge = view.read_with(cx, |view, _| {
        view.board().edges().iter().find(|e| e.to == made).cloned()
    });
    assert_eq!(edge.map(|e| e.from), Some(a));
    let node = view.read_with(cx, |view, _| {
        view.board().node(made).unwrap().port_point(Dir::In, "in")
    });
    assert!((node.0 - request.x).abs() <= crate::SNAP && (node.1 - request.y).abs() <= crate::SNAP);
}

#[gpui::test]
fn a_node_is_renamed_in_place_and_a_note_is_written_deleted_and_undone(cx: &mut TestAppContext) {
    let (view, cx, _, board_events, [a, _, _]) = row_of_three(cx);
    view.update(cx, |view, cx| view.select(Some(a), cx));
    view.update_in(cx, |view, window, cx| window.focus(&view.focus, cx));
    frame(cx);
    cx.simulate_keystrokes("f2");
    frame(cx);
    cx.simulate_input("lead");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        view.read_with(cx, |view, _| view.board().node(a).unwrap().title.clone()),
        "lead"
    );
    assert!(board_events.borrow().contains(&BoardEvent::Renamed {
        node: a,
        title: "lead".into()
    }));

    // A: the add menu; with no offers it has "Start an agent here" and "Note".
    cx.simulate_keystrokes("escape a");
    frame(cx);
    cx.simulate_keystrokes("down enter");
    frame(cx);
    cx.simulate_input("plan");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("tests");
    cx.simulate_keystrokes("cmd-enter");
    let note = view.read_with(cx, |view, _| {
        view.board()
            .nodes()
            .iter()
            .find(|node| node.kind == NodeKind::Note)
            .map(|node| (node.id, node.detail.clone()))
    });
    let (note, text) = note.expect("a note was added");
    assert_eq!(text, "plan\ntests");
    cx.simulate_keystrokes("backspace");
    assert!(
        view.read_with(cx, |view, _| view.board().node(note).is_none()),
        "a note is the canvas's to delete"
    );
    assert!(
        board_events
            .borrow()
            .iter()
            .any(|e| matches!(e, BoardEvent::NodeRemoved(node) if node.id == note))
    );
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(
        view.read_with(cx, |view, _| view
            .board()
            .node(note)
            .map(|n| n.detail.clone())),
        Some(text)
    );
}

#[gpui::test]
fn a_message_runs_along_its_edge_and_waiting_ones_show_on_it(cx: &mut TestAppContext) {
    let (view, cx, _, _, [a, b, _]) = row_of_three(cx);
    let edge = view.update(cx, |view, cx| {
        view.connect(PortRef::new(a, "out"), PortRef::new(b, "in"), cx)
            .unwrap()
    });
    // From an event handler, outside a frame: the app's case.
    view.update(cx, |view, cx| {
        view.flow(edge, cx);
        view.set_traffic(
            edge,
            crate::Traffic {
                delivered: 3,
                waiting: 1,
            },
            cx,
        );
        view.set_pending(
            b,
            crate::Pending {
                count: 1,
                awaiting: 1,
                from: Some("a".into()),
            },
            cx,
        );
    });
    frame(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.flows.len(), 1);
        let scene = view.scene(kou_ui::theme(cx));
        assert_eq!(scene.dots.len(), 1, "the message is drawn on its way");
        let line = scene
            .lines
            .iter()
            .find(|line| line.color == kou_ui::theme(cx).blocked);
        assert!(
            line.is_some(),
            "an edge with a message to approve is drawn in the colour of waiting"
        );
    });
    cx.executor().advance_clock(Duration::from_secs(2));
    view.update(cx, |view, cx| {
        view.set_traffic(edge, crate::Traffic::default(), cx)
    });
    frame(cx);
}

#[gpui::test]
fn a_node_is_there_as_soon_as_it_is_added_and_the_camera_goes_to_one_out_of_sight(
    cx: &mut TestAppContext,
) {
    let (view, cx, events, _) = open(crate::Board::new(), cx);
    let first = view.update_in(cx, |view, window, cx| {
        let id = view.create_at(0., 0., spec("lead", "CC"), window, cx);
        assert_eq!(view.selected(), Some(id));
        assert!(view.live.contains_key(&id));
        assert!(view.glide.is_none(), "it is where the eye already is");
        let moving = view.describe_transitions(cx);
        assert!(
            moving.iter().all(|line| line.contains("selection")),
            "nothing comes in, only its selection fades in: {moving:?}"
        );
        id
    });
    assert_eq!(*events.borrow(), [CanvasEvent::Changed]);

    // Out of sight, it is on the board at once and the camera glides to it.
    let second = view.update_in(cx, |view, window, cx| {
        let second = view.create_at(4000., 0., spec("notes", "PI"), window, cx);
        assert!(view.board().node(second).is_some());
        assert!(view.glide.is_some(), "the camera is on its way");
        let moving = view.describe_transitions(cx);
        assert!(
            moving.iter().any(|line| line.starts_with("camera")),
            "{moving:?}"
        );
        second
    });
    assert_ne!(first, second);
    frame(cx);
    view.read_with(cx, |view, _| {
        assert!(
            view.live.contains_key(&second),
            "drawn from the first frame"
        );
    });
}

#[gpui::test]
fn a_spawned_child_and_its_edge_are_there_at_once(cx: &mut TestAppContext) {
    let mut board = crate::Board::new();
    let root = board.add_at(spec("lead", "CC"), 0., 0.);
    let (view, cx, events, _) = open(board, cx);
    let child = view
        .update_in(cx, |view, window, cx| {
            view.spawn_child(root, spec("tests", "CX"), window, cx)
        })
        .unwrap();
    view.read_with(cx, |view, _| {
        assert!(view.live.contains_key(&child));
        let edges = view.board().edges();
        assert_eq!(edges.len(), 1);
        assert_eq!((edges[0].from, edges[0].to), (root, child));
    });
    frame(cx);
    view.read_with(cx, |view, cx| {
        // The edge is drawn whole, with its arrow, on the first frame.
        let scene = view.scene(kou_ui::theme(cx));
        assert_eq!(scene.lines.len(), 1);
        assert!(scene.lines[0].arrow);
    });
    assert!(events.borrow().contains(&CanvasEvent::Changed));
    let nothing = view.update_in(cx, |view, window, cx| {
        view.spawn_child(999, spec("ghost", ""), window, cx)
    });
    assert_eq!(nothing, None);
}

#[gpui::test]
fn a_removed_node_and_its_edges_are_gone_at_once_and_a_mark_moves_nothing(cx: &mut TestAppContext) {
    let mut board = crate::Board::new();
    let root = board.add_at(spec("lead", "CC"), 0., 0.);
    let child = board.add_child(root, spec("tests", "CX")).unwrap();
    let (view, cx, events, _) = open(board, cx);
    view.update_in(cx, |view, window, cx| {
        view.set_mark(child, Mark::Working, window, cx);
        view.set_mark(child, Mark::Done, window, cx);
        assert_eq!(view.live[&child].mark, Mark::Done);
        assert_eq!(view.running(cx), 0, "finishing work is not animated");

        view.select(Some(root), cx);
        view.remove(root, window, cx);
        assert!(view.board().node(root).is_none());
        assert!(!view.live.contains_key(&root));
        assert_eq!(view.selected(), None);
        assert_eq!(view.board().edges().len(), 0);
        assert!(view.edge_fx.is_empty());
        assert_eq!(view.running(cx), 0, "nothing lingers where it was");
        view.remove(root, window, cx);
    });
    frame(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.scene(kou_ui::theme(cx)).lines.is_empty());
    });
    let changed = events
        .borrow()
        .iter()
        .filter(|event| **event == CanvasEvent::Changed)
        .count();
    assert_eq!(changed, 1, "removing it twice is removing it once");
}

#[gpui::test]
fn a_messages_dot_runs_along_its_edge_and_is_gone_when_it_arrives(cx: &mut TestAppContext) {
    let (view, cx, _, _, [a, b, _]) = row_of_three(cx);
    view.update(cx, |view, cx| {
        view.clock.hold();
        let edge = view
            .connect(PortRef::new(a, "out"), PortRef::new(b, "in"), cx)
            .unwrap();
        view.flow(edge, cx);
    });
    frame(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.flows.len(), 1);
        assert_eq!(view.running(cx), 1);
    });
    view.update(cx, |view, cx| {
        view.clock.hold_at(Duration::from_secs(2));
        cx.notify();
    });
    frame(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.flows.is_empty());
        assert_eq!(view.running(cx), 0, "nothing answers its landing");
    });
}

/// A stand-in for a node's live view: it counts how often it is built,
/// records the zoom it read and where it was laid out, and counts the wheel
/// and the presses it got.
struct Probe {
    focus: FocusHandle,
    zoom: Option<Rc<Cell<f32>>>,
    builds: Rc<Cell<usize>>,
    read: Rc<RefCell<Vec<f32>>>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    wheel: Rc<Cell<usize>>,
    presses: Rc<Cell<usize>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        if let Some(zoom) = &self.zoom {
            self.read.borrow_mut().push(zoom.get());
        }
        let (bounds, wheel, presses) = (
            self.bounds.clone(),
            self.wheel.clone(),
            self.presses.clone(),
        );
        div()
            .id("probe")
            .track_focus(&self.focus)
            .size_full()
            .on_scroll(move |_, _, cx| {
                wheel.set(wheel.get() + 1);
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                presses.set(presses.get() + 1)
            })
            .child(
                canvas(move |at, _, _| bounds.set(at), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
    }
}

/// Give each node a probe for its body, following the canvas's zoom.
fn probe(view: &Entity<CanvasView>, ids: &[u64], cx: &mut VisualTestContext) -> Vec<Entity<Probe>> {
    let zoom = view.read_with(cx, |view, _| view.zoom_handle());
    ids.iter()
        .map(|id| {
            let probe = cx.update(|_, cx| {
                cx.new(|cx| Probe {
                    focus: cx.focus_handle(),
                    zoom: Some(zoom.clone()),
                    builds: Rc::default(),
                    read: Rc::default(),
                    bounds: Rc::default(),
                    wheel: Rc::default(),
                    presses: Rc::default(),
                })
            });
            let focus = probe.read_with(cx, |probe, _| probe.focus.clone());
            view.update(cx, |view, cx| {
                view.set_body(*id, probe.clone().into(), cx);
                view.set_body_focus(*id, focus, cx);
            });
            probe
        })
        .collect()
}

fn builds(probes: &[Entity<Probe>], cx: &mut VisualTestContext) -> Vec<usize> {
    probes
        .iter()
        .map(|probe| probe.read_with(cx, |probe, _| probe.builds.get()))
        .collect()
}

/// Draw one frame as the window does, keeping what did not change.
fn draw_kept(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn a_body_is_live_at_every_whole_zoom_and_far_out_every_node_is_a_placeholder(
    cx: &mut TestAppContext,
) {
    let (view, cx, _, _, [a, _, _]) = row_of_three(cx);
    let probes = probe(&view, &[a], cx);
    for zoom in [2., 1., 0.75, Lod::LIVE] {
        at_camera(
            &view,
            Camera {
                x: 40.,
                y: 40.,
                zoom,
            },
            cx,
        );
        let before = builds(&probes, cx)[0];
        frame(cx);
        assert!(builds(&probes, cx)[0] > before, "live at {zoom}");
        let bounds = probes[0].read_with(cx, |probe, _| probe.bounds.get());
        let rect = view.read_with(cx, |view, _| view.board().node(a).unwrap().rect);
        // Laid out across the node as it is on screen, under its header, so
        // that its size over the zoom is the same at every zoom.
        let (width, height): (f32, f32) = (bounds.size.width.into(), bounds.size.height.into());
        // Within the pixel GPUI snaps a layout to.
        assert!((width - rect.w * zoom).abs() <= 1., "{width} at {zoom}");
        assert!(
            (height - (rect.h - crate::card::HEAD) * zoom).abs() <= 1.,
            "{height} at {zoom}"
        );
        let read = probes[0].read_with(cx, |probe, _| *probe.read.borrow().last().unwrap());
        assert_eq!(read, zoom, "the zoom of the frame it is drawn in");
        assert_eq!(view.read_with(cx, |view, _| view.lod()), Lod::Whole);
    }
    // Past the threshold, every node is a placeholder: no body is drawn.
    at_camera(
        &view,
        Camera {
            x: 40.,
            y: 40.,
            zoom: 0.5,
        },
        cx,
    );
    assert_eq!(view.read_with(cx, |view, _| view.lod()), Lod::Placeholder);
    let before = builds(&probes, cx)[0];
    frame(cx);
    frame(cx);
    assert_eq!(builds(&probes, cx)[0], before, "not drawn as a placeholder");
    // A press on a placeholder takes the node, as its header would.
    let grab = screen(&view, 120., 80., cx);
    drag(
        cx,
        grab,
        grab + point(px(40.), px(0.)),
        Modifiers::default(),
    );
    assert_eq!(view.read_with(cx, |view, _| view.selected()), Some(a));
    assert!(view.read_with(cx, |view, _| view.board().node(a).unwrap().rect.x) > 0.);
}

#[gpui::test]
fn the_zoom_handle_holds_the_zoom_of_each_frame_of_a_glide(cx: &mut TestAppContext) {
    let (view, cx, _, _, [a, _, _]) = row_of_three(cx);
    let probes = probe(&view, &[a], cx);
    view.update_in(cx, |view, window, cx| {
        view.set_frozen(true);
        view.zoom_step(-1, cx);
        view.freeze_at(140., window, cx);
    });
    frame(cx);
    let (camera, handle) = view.read_with(cx, |view, _| (view.camera(), view.zoom_handle().get()));
    assert!(
        camera.zoom < 1. && camera.zoom > 0.75,
        "half way: {}",
        camera.zoom
    );
    assert_eq!(handle, camera.zoom);
    let read = probes[0].read_with(cx, |probe, _| *probe.read.borrow().last().unwrap());
    assert_eq!(
        read, camera.zoom,
        "the body read this frame's zoom, not the last one's"
    );
}

#[gpui::test]
fn a_body_is_not_drawn_again_while_the_board_animates_around_it(cx: &mut TestAppContext) {
    let (view, cx, _, _, [a, b, c]) = row_of_three(cx);
    let probes = probe(&view, &[a, b, c], cx);
    let edge = view.update(cx, |view, cx| {
        view.connect(PortRef::new(a, "out"), PortRef::new(b, "in"), cx)
            .unwrap()
    });
    frame(cx);
    draw_kept(cx);
    let settled = builds(&probes, cx);
    // A message runs along an edge and a node's selection fades in: the board
    // draws frame after frame, the bodies in it stay as they were drawn.
    view.update(cx, |view, cx| {
        view.flow(edge, cx);
        view.select(Some(c), cx);
    });
    for _ in 0..6 {
        view.update(cx, |_, cx| cx.notify());
        draw_kept(cx);
    }
    assert_eq!(
        builds(&probes, cx),
        settled,
        "no body was built for the board's frames"
    );
    // One body changes: only it is built again.
    probes[1].update(cx, |_, cx| cx.notify());
    draw_kept(cx);
    let after: Vec<usize> = builds(&probes, cx);
    assert_eq!(after, vec![settled[0], settled[1] + 1, settled[2]]);
    // The camera moves: every body on screen is drawn where it now is.
    view.update(cx, |view, cx| {
        view.set_camera(
            Camera {
                x: 10.,
                y: 40.,
                zoom: 1.,
            },
            cx,
        )
    });
    draw_kept(cx);
    let moved = builds(&probes, cx);
    assert!(
        moved.iter().zip(&after).all(|(now, was)| now > was),
        "{moved:?}"
    );
}

#[gpui::test]
fn the_wheel_over_a_body_moves_the_board_unless_the_body_has_the_keys(cx: &mut TestAppContext) {
    let (view, cx, _, _, [a, _, _]) = row_of_three(cx);
    let probes = probe(&view, &[a], cx);
    frame(cx);
    let over = screen(&view, 120., 100., cx);
    let wheel = |cx: &mut VisualTestContext, modifiers: Modifiers| {
        cx.simulate_event(ScrollEvent {
            position: over,
            delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
            modifiers,
            ..ScrollEvent::default()
        });
    };
    let before = view.read_with(cx, |view, _| view.camera());
    wheel(cx, Modifiers::default());
    let after = view.read_with(cx, |view, _| view.camera());
    assert!((after.y - before.y + 30.).abs() < 0.5, "the board moved");
    assert_eq!(probes[0].read_with(cx, |probe, _| probe.wheel.get()), 0);

    // A press in the body reaches it, and gives it the keys.
    frame(cx);
    let inside = screen(&view, 120., 100., cx);
    cx.simulate_click(inside, Modifiers::default());
    assert_eq!(probes[0].read_with(cx, |probe, _| probe.presses.get()), 1);
    assert_eq!(view.read_with(cx, |view, _| view.entered()), Some(a));
    let focus = probes[0].read_with(cx, |probe, _| probe.focus.clone());
    assert!(cx.update(|window, _| focus.is_focused(window)));
    frame(cx);

    // Entered, the wheel is the body's; with Cmd it still zooms the board.
    let over = screen(&view, 120., 100., cx);
    let camera = view.read_with(cx, |view, _| view.camera());
    cx.simulate_event(ScrollEvent {
        position: over,
        delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
        ..ScrollEvent::default()
    });
    assert_eq!(view.read_with(cx, |view, _| view.camera()), camera);
    assert_eq!(probes[0].read_with(cx, |probe, _| probe.wheel.get()), 1);
    cx.simulate_event(ScrollEvent {
        position: over,
        delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
        modifiers: Modifiers::command(),
        ..ScrollEvent::default()
    });
    assert_ne!(
        view.read_with(cx, |view, _| view.camera().zoom),
        camera.zoom
    );
    assert_eq!(probes[0].read_with(cx, |probe, _| probe.wheel.get()), 1);
}

#[gpui::test]
fn a_wheels_steps_glide_the_board_and_a_trackpads_pixels_move_it_at_once(cx: &mut TestAppContext) {
    let (view, cx, _, _, _) = row_of_three(cx);
    frame(cx);
    let over = screen(&view, 600., 500., cx);
    let step = |cx: &mut VisualTestContext, lines: f32, modifiers: Modifiers| {
        cx.simulate_event(ScrollEvent {
            position: over,
            delta: ScrollDelta::Lines(point(0., lines)),
            modifiers,
            ..ScrollEvent::default()
        });
    };
    let at = |cx: &mut VisualTestContext, milliseconds: u64| {
        view.update(cx, |view, _| {
            view.clock.hold_at(Duration::from_millis(milliseconds))
        });
        frame(cx);
        view.read_with(cx, |view, _| view.camera())
    };
    let before = at(cx, 0);

    // Three lines down: nothing yet in the frame of the step, part of the
    // way while it glides, and exactly there at the end.
    step(cx, -3., Modifiers::default());
    assert_eq!(at(cx, 0), before);
    let part = at(cx, 40);
    assert!(part.y < before.y && part.y > before.y - 48., "{part:?}");
    assert_eq!(part.zoom, before.zoom);
    // A further step on the way adds up, and never turns the board back.
    step(cx, -1., Modifiers::default());
    let mut last = part.y;
    for milliseconds in (50..=190).step_by(10) {
        let now = at(cx, milliseconds).y;
        assert!(
            now <= last,
            "the board never moves back: {now} after {last}"
        );
        last = now;
    }
    let rested = at(cx, 400);
    assert!((rested.y - (before.y - 64.)).abs() < 0.01, "{rested:?}");
    assert_eq!(rested.x, before.x);
    assert!(view.read_with(cx, |view, _| view.wheel.is_none()));

    // With Shift the wheel moves the board sideways.
    step(cx, -2., Modifiers::shift());
    let sideways = at(cx, 800);
    assert!((sideways.x - (rested.x - 32.)).abs() < 0.01, "{sideways:?}");
    assert_eq!(sideways.y, rested.y);

    // With Cmd it zooms: evenly, around what is under the pointer, and to
    // exactly the zoom the steps come to.
    let pointer = view.read_with(cx, |view, _| view.local(over));
    let (wx, wy) = sideways.to_world(pointer.0, pointer.1);
    let still = |camera: Camera| {
        let now = camera.to_screen(wx, wy);
        (now.0 - pointer.0).abs() < 0.05 && (now.1 - pointer.1).abs() < 0.05
    };
    step(cx, -2., Modifiers::command());
    let part = at(cx, 840);
    let wanted = sideways.zoom * (-32. * 0.006f32).exp();
    assert!(part.zoom < sideways.zoom && part.zoom > wanted, "{part:?}");
    assert!(still(part), "{part:?}");
    let zoomed = at(cx, 1200);
    assert!((zoomed.zoom - wanted).abs() < 1e-4, "{zoomed:?}");
    assert!(still(zoomed), "{zoomed:?}");

    // Steps past the furthest zoom stop at it, and one back moves at once.
    for _ in 0..40 {
        step(cx, -4., Modifiers::command());
    }
    assert_eq!(at(cx, 1600).zoom, Camera::MIN_ZOOM);
    step(cx, 1., Modifiers::command());
    assert!(at(cx, 1640).zoom > Camera::MIN_ZOOM);
    let back = at(cx, 2000);

    // A trackpad's pixels are applied as they come, and end a glide.
    step(cx, -3., Modifiers::default());
    at(cx, 2020);
    let mid = view.read_with(cx, |view, _| view.camera());
    cx.simulate_event(ScrollEvent {
        position: over,
        delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
        ..ScrollEvent::default()
    });
    let moved = view.read_with(cx, |view, _| view.camera());
    assert!((moved.y - (mid.y - 10.)).abs() < 0.01);
    assert!(view.read_with(cx, |view, _| view.wheel.is_none()));
    assert_eq!(at(cx, 2400), moved);
    assert!(moved.y > back.y - 48., "the rest of the glide is dropped");

    // Switched off, a step is there at once.
    cx.update(|_, cx| {
        cx.set_global(gpui::SmoothScroll {
            enabled: false,
            ..Default::default()
        })
    });
    step(cx, -1., Modifiers::default());
    let jumped = view.read_with(cx, |view, _| view.camera());
    assert!((jumped.y - (moved.y - 16.)).abs() < 0.01);
}

#[gpui::test]
fn zooming_out_to_placeholders_takes_the_keys_back_and_entering_one_brings_it_close(
    cx: &mut TestAppContext,
) {
    let (view, cx, _, _, [a, _, _]) = row_of_three(cx);
    let probes = probe(&view, &[a], cx);
    let focus = probes[0].read_with(cx, |probe, _| probe.focus.clone());
    frame(cx);
    view.update_in(cx, |view, window, cx| view.enter(a, window, cx));
    frame(cx);
    assert!(cx.update(|window, _| focus.is_focused(window)));
    at_camera(
        &view,
        Camera {
            x: 40.,
            y: 40.,
            zoom: 0.3,
        },
        cx,
    );
    assert_eq!(view.read_with(cx, |view, _| view.entered()), None);
    assert!(cx.update(|window, cx| view.read(cx).focus.is_focused(window)));

    // Entered from afar, the node is brought close and takes the keys there.
    view.update_in(cx, |view, window, cx| {
        view.clock.hold();
        view.enter(a, window, cx);
    });
    assert!(view.read_with(cx, |view, _| view.glide.is_some()));
    frame(cx);
    assert!(
        !cx.update(|window, _| focus.is_focused(window)),
        "not while it is far"
    );
    view.update(cx, |view, cx| {
        view.clock
            .hold_at(crate::view::GLIDE + Duration::from_millis(20));
        cx.notify();
    });
    for _ in 0..2 {
        frame(cx);
    }
    view.read_with(cx, |view, _| {
        assert!(view.camera().zoom >= Lod::LIVE);
        assert_eq!(view.entered(), Some(a));
    });
    assert!(cx.update(|window, _| focus.is_focused(window)));
}
