//! Renders the canvas to PNG from hidden windows, for looking at what it
//! draws without a window on anyone's screen: every node kind and edge mode,
//! both themes, at 100%, 75%, 60% (the last zoom with whole nodes), 50% and
//! 25% (placeholders), a selection, an edge being drawn, a node entered, the
//! menus, the inspector, the undo bar, an empty canvas; strips of frames
//! across what moves; and, with `perf`, what a frame costs with many live
//! bodies on the board. The names on the board are fixtures.
//!
//! `cargo run -p kou-canvas --example gallery -- OUT_DIR [NAME...]`

use gpui::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, FocusHandle,
    InteractiveElement as _, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, PlatformInput, Point, Render, Styled as _, Window,
    WindowBounds, WindowHandle, WindowOptions, div, point, px, size,
};
use image::{RgbaImage, imageops};
use kou_canvas::{
    Board, Camera, CanvasView, Delivery, EdgeMode, NodeKind, NodeSpec, Offer, Pending, PortRef,
    Rect, Traffic,
};
use kou_ui::Mark;
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

const W: f32 = 1280.;
const H: f32 = 800.;

fn spec(kind: NodeKind, title: &str, tag: &str, detail: &str) -> NodeSpec {
    NodeSpec {
        kind,
        title: title.into(),
        tag: tag.into(),
        detail: detail.into(),
        size: None,
    }
}

/// Ids of the fixture board's nodes, in the order they are made.
struct Ids {
    issues: u64,
    lead: u64,
    schema: u64,
    callers: u64,
    tests: u64,
    plan: u64,
}

/// A board with every node kind and every edge mode.
fn fixture() -> (Board, Ids) {
    let mut board = Board::new();
    let issues = board.add_rect(
        spec(NodeKind::Source, "issues", "", "#212 parser drops trailing comments\n#209 schema v3 rollout\n#204 callers on the old API"),
        Rect::new(-520., 40., 320., 220.),
    );
    let lead = board.add_rect(
        spec(
            NodeKind::Agent,
            "lead",
            "CC",
            "> Split the migration into three parts\n  and review each before merging.\n\n● Read(schema/v3.sql)\n  └ 214 lines\n● Spawned schema, callers\n\n✻ Coordinating… (2m 14s)",
        ),
        Rect::new(0., 0., 560., 380.),
    );
    let schema = board.add_rect(
        spec(NodeKind::Agent, "schema", "CX", "› write the v3 migration\n\n• Edited schema/v3.sql (+42 -7)\n• Ran psql -f schema/v3.sql\n\n✔ Migration applies cleanly"),
        Rect::new(760., -220., 480., 300.),
    );
    let callers = board.add_rect(
        spec(NodeKind::Agent, "callers", "OC", "Two callers still use the old API:\n  parser/load.go:88\n  parser/emit.go:131\n\nAllow edits to parser/? (y/n)"),
        Rect::new(760., 160., 480., 300.),
    );
    let tests = board.add_rect(
        spec(NodeKind::Terminal, "tests", "", "$ cargo test -p parser\n   Compiling parser v0.1.0\n    Finished `test` profile\n     Running unittests\ntest result: ok. 42 passed; 0 failed"),
        Rect::new(1400., -220., 560., 340.),
    );
    let plan = board.add_rect(
        spec(
            NodeKind::Note,
            "plan",
            "",
            "1. schema v3\n2. move the callers\n3. tests green, then merge",
        ),
        Rect::new(1400., 220., 280., 180.),
    );
    for (id, place) in [
        (lead, "~/fixture/atlas · main"),
        (schema, "~/fixture/atlas · kou/schema"),
        (callers, "~/fixture/atlas · kou/callers"),
        (tests, "~/fixture/atlas · ▸ cargo test"),
        (issues, "github · fixture/atlas"),
    ] {
        board.node_mut(id).unwrap().place = place.into();
    }
    let wire = |board: &mut Board, from: u64, port: &str, to: u64| {
        board
            .connect(PortRef::new(from, port), PortRef::new(to, "in"))
            .unwrap()
    };
    wire(&mut board, issues, "out", lead);
    wire(&mut board, lead, "out", schema);
    let approve = wire(&mut board, lead, "out", callers);
    board.update_edge(approve, |e| e.mode = EdgeMode::Approve);
    let off = wire(&mut board, schema, "out", tests);
    board.update_edge(off, |e| {
        e.mode = EdgeMode::Off;
        e.deliver = Some(Delivery::Queue);
    });
    wire(&mut board, tests, "exit", lead);
    (
        board,
        Ids {
            issues,
            lead,
            schema,
            callers,
            tests,
            plan,
        },
    )
}

fn offers() -> Vec<Offer> {
    let offer = |id: &str, title: &str, code: &str, kind, detail: &str, ready| Offer {
        id: id.into(),
        title: title.into(),
        code: code.into(),
        kind,
        detail: detail.into(),
        ready,
    };
    vec![
        offer(
            "claude",
            "Claude Code",
            "CC",
            NodeKind::Agent,
            "2.1.322",
            true,
        ),
        offer("codex", "Codex", "CX", NodeKind::Agent, "0.160.0", true),
        offer("kou", "kou", "K", NodeKind::Agent, "native thread", true),
        offer(
            "gemini",
            "Gemini CLI",
            "GM",
            NodeKind::Agent,
            "not installed",
            false,
        ),
        offer("shell", "Shell", "", NodeKind::Terminal, "zsh", true),
    ]
}

/// A stand-in for a terminal in a node: lines as a shell shows them, drawn
/// at the canvas's zoom as a terminal following it is, so its lines keep
/// their breaks at any zoom. It counts how often it is built.
struct FixtureBody {
    focus: FocusHandle,
    lines: Vec<String>,
    zoom: Rc<Cell<f32>>,
    builds: Rc<Cell<usize>>,
}

impl Render for FixtureBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        let theme = kou_ui::theme(cx);
        let zoom = self.zoom.get();
        div()
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .px(px(10. * zoom))
            .py(px(8. * zoom))
            .bg(theme.pane)
            .font_family(theme.mono_font.clone())
            .text_size(px(13. * zoom))
            .line_height(px(18. * zoom))
            .text_color(theme.fg_dim)
            .children(
                self.lines
                    .iter()
                    .map(|line| div().whitespace_nowrap().child(line.clone())),
            )
    }
}

/// Give the fixture's agents and terminal live bodies, as the app does.
fn bodies(view: &mut CanvasView, cx: &mut Context<CanvasView>) -> Rc<Cell<usize>> {
    let builds = Rc::new(Cell::new(0));
    let zoom = view.zoom_handle();
    let nodes: Vec<_> = view
        .board()
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::Agent | NodeKind::Terminal))
        .map(|node| {
            (
                node.id,
                node.detail.lines().map(str::to_owned).collect::<Vec<_>>(),
            )
        })
        .collect();
    for (id, lines) in nodes {
        let body = cx.new(|cx| FixtureBody {
            focus: cx.focus_handle(),
            lines,
            zoom: zoom.clone(),
            builds: builds.clone(),
        });
        let focus = body.read(cx).focus.clone();
        view.set_body(id, body.into(), cx);
        view.set_body_focus(id, focus, cx);
    }
    builds
}

fn options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(W), px(H)),
        ))),
        show: false,
        focus: false,
        ..Default::default()
    }
}

/// Draw through the untyped handle: a typed one would lease the canvas the
/// frame needs to render.
fn redraw(handle: &WindowHandle<CanvasView>, cx: &mut AsyncApp) {
    let any: AnyWindowHandle = (*handle).into();
    cx.update_window(any, |_, window, cx| draw(window, cx)).ok();
}

fn draw(window: &mut Window, cx: &mut App) {
    for _ in 0..2 {
        window.refresh();
        window.draw(cx).clear(cx);
    }
}

async fn open(cx: &mut AsyncApp, board: Board, dark: bool) -> WindowHandle<CanvasView> {
    cx.update(|cx| cx.set_global(kou_ui::Theme::new(dark)));
    let handle = cx
        .update(|cx| {
            cx.open_window(options(), move |_, cx| {
                cx.new(|cx| CanvasView::new(board, cx))
            })
        })
        .expect("open a hidden window");
    redraw(&handle, cx);
    handle
}

async fn save(cx: &mut AsyncApp, handle: WindowHandle<CanvasView>, path: PathBuf) -> RgbaImage {
    // Past the selection's and the menus' fades.
    cx.background_executor()
        .timer(Duration::from_millis(320))
        .await;
    let any: AnyWindowHandle = handle.into();
    let image = cx
        .update_window(any, |_, window, cx| {
            draw(window, cx);
            window.render_to_image()
        })
        .expect("draw")
        .expect("render to an image");
    image.save(&path).expect("write the capture");
    eprintln!("saved {}", path.display());
    image
}

async fn close(cx: &mut AsyncApp, handle: WindowHandle<CanvasView>) {
    let any: AnyWindowHandle = handle.into();
    cx.update_window(any, |_, window, _| window.remove_window())
        .ok();
}

fn input(handle: &WindowHandle<CanvasView>, cx: &mut AsyncApp, event: PlatformInput) {
    let any: AnyWindowHandle = (*handle).into();
    cx.update_window(any, |_, window, cx| {
        window.dispatch_event(event, cx);
        draw(window, cx);
    })
    .ok();
}

fn down(at: Point<Pixels>, clicks: usize) -> PlatformInput {
    PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: clicks,
        first_mouse: false,
    })
}

fn moved(at: Point<Pixels>, pressed: bool) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position: at,
        pressed_button: pressed.then_some(MouseButton::Left),
        modifiers: Modifiers::default(),
    })
}

fn up(at: Point<Pixels>) -> PlatformInput {
    PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 1,
    })
}

fn at(handle: &WindowHandle<CanvasView>, cx: &mut AsyncApp, x: f32, y: f32) -> Point<Pixels> {
    handle
        .update(cx, |view, _, _| view.window_point(x, y))
        .expect("read")
}

/// The fixture as it is when work is under way: marks, traffic, what waits.
fn liven(view: &mut CanvasView, ids: &Ids, window: &mut Window, cx: &mut Context<CanvasView>) {
    view.restore_mark(ids.lead, Mark::Working, cx);
    view.restore_mark(ids.schema, Mark::Done, cx);
    view.restore_mark(ids.callers, Mark::Blocked, cx);
    view.restore_mark(ids.tests, Mark::Idle, cx);
    let edges: Vec<_> = view.board().edges().to_vec();
    for edge in &edges {
        if edge.mode == EdgeMode::Approve {
            view.set_traffic(
                edge.id,
                Traffic {
                    delivered: 2,
                    waiting: 1,
                },
                cx,
            );
        } else if edge.from_port == "exit" {
            view.set_traffic(
                edge.id,
                Traffic {
                    delivered: 3,
                    waiting: 0,
                },
                cx,
            );
        }
    }
    view.set_pending(
        ids.callers,
        Pending {
            count: 1,
            awaiting: 1,
            from: Some("lead".into()),
        },
        cx,
    );
    let _ = window;
}

async fn board_shots(cx: &mut AsyncApp, out: &Path, wants: &impl Fn(&str) -> bool) {
    for dark in [true, false] {
        let theme = if dark { "dark" } else { "light" };
        for (zoom, camera) in [
            (
                "100",
                Camera {
                    x: 20.,
                    y: 250.,
                    zoom: 1.,
                },
            ),
            (
                "75",
                Camera {
                    x: 160.,
                    y: 300.,
                    zoom: 0.75,
                },
            ),
            (
                "60",
                Camera {
                    x: 330.,
                    y: 300.,
                    zoom: 0.6,
                },
            ),
            (
                "50",
                Camera {
                    x: 290.,
                    y: 340.,
                    zoom: 0.5,
                },
            ),
            (
                "25",
                Camera {
                    x: 460.,
                    y: 370.,
                    zoom: 0.25,
                },
            ),
            (
                "12",
                Camera {
                    x: 520.,
                    y: 400.,
                    zoom: 0.12,
                },
            ),
        ] {
            let name = format!("board-{zoom}-{theme}");
            if !wants(&name) {
                continue;
            }
            let (board, ids) = fixture();
            let handle = open(cx, board, dark).await;
            handle
                .update(cx, |view, window, cx| {
                    view.set_camera(camera, cx);
                    liven(view, &ids, window, cx);
                    bodies(view, cx);
                    view.select(Some(ids.schema), cx);
                })
                .ok();
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }

        let name = format!("marquee-{theme}");
        if wants(&name) {
            let (board, ids) = fixture();
            let handle = open(cx, board, dark).await;
            handle
                .update(cx, |view, window, cx| {
                    view.set_camera(
                        Camera {
                            x: 290.,
                            y: 340.,
                            zoom: 0.5,
                        },
                        cx,
                    );
                    liven(view, &ids, window, cx);
                })
                .ok();
            redraw(&handle, cx);
            let from = at(&handle, cx, 680., -300.);
            let to = at(&handle, cx, 1300., 520.);
            input(&handle, cx, down(from, 1));
            input(
                &handle,
                cx,
                moved(point(from.x + px(20.), from.y + px(20.)), true),
            );
            input(&handle, cx, moved(to, true));
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }

        let name = format!("edge-selected-{theme}");
        if wants(&name) {
            let (board, ids) = fixture();
            let handle = open(cx, board, dark).await;
            handle
                .update(cx, |view, window, cx| {
                    view.set_camera(
                        Camera {
                            x: -100.,
                            y: 260.,
                            zoom: 1.,
                        },
                        cx,
                    );
                    liven(view, &ids, window, cx);
                    let edge = view
                        .board()
                        .edges()
                        .iter()
                        .find(|e| e.mode == EdgeMode::Approve)
                        .map(|e| e.id);
                    view.select_edge(edge, cx);
                })
                .ok();
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }

        for (name, docked) in [
            (format!("wiring-{theme}"), false),
            (format!("wiring-docked-{theme}"), true),
        ] {
            if !wants(&name) {
                continue;
            }
            let (board, ids) = fixture();
            let handle = open(cx, board, dark).await;
            let (out_port, target) = handle
                .update(cx, |view, window, cx| {
                    view.set_camera(
                        Camera {
                            x: 140.,
                            y: 420.,
                            zoom: 0.75,
                        },
                        cx,
                    );
                    liven(view, &ids, window, cx);
                    let board = view.board();
                    (
                        board
                            .node(ids.callers)
                            .unwrap()
                            .port_point(kou_canvas::Dir::Out, "out"),
                        board.node(ids.plan).unwrap().rect,
                    )
                })
                .unwrap();
            redraw(&handle, cx);
            let tests_in = handle
                .update(cx, |view, _, _| {
                    view.board()
                        .node(ids.tests)
                        .unwrap()
                        .port_point(kou_canvas::Dir::In, "in")
                })
                .unwrap();
            let from = at(&handle, cx, out_port.0, out_port.1);
            input(&handle, cx, down(from, 1));
            input(&handle, cx, moved(point(from.x + px(30.), from.y), true));
            let end = if docked {
                at(&handle, cx, tests_in.0 + 6., tests_in.1 + 4.)
            } else {
                at(&handle, cx, tests_in.0 - 40., tests_in.1 + 60.)
            };
            let _ = target;
            input(&handle, cx, moved(end, true));
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }

        let name = format!("entered-{theme}");
        if wants(&name) {
            let (board, ids) = fixture();
            let handle = open(cx, board, dark).await;
            handle
                .update(cx, |view, window, cx| {
                    view.set_camera(
                        Camera {
                            x: -540.,
                            y: 330.,
                            zoom: 0.75,
                        },
                        cx,
                    );
                    liven(view, &ids, window, cx);
                    bodies(view, cx);
                    view.set_frozen(true);
                    view.enter(ids.tests, window, cx);
                    view.freeze_at(400., window, cx);
                })
                .ok();
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }

        for kind in ["add", "node", "edge"] {
            let name = format!("menu-{kind}-{theme}");
            if !wants(&name) {
                continue;
            }
            let (board, ids) = fixture();
            let handle = open(cx, board, dark).await;
            handle
                .update(cx, |view, window, cx| {
                    view.set_camera(
                        Camera {
                            x: 20.,
                            y: 250.,
                            zoom: 1.,
                        },
                        cx,
                    );
                    view.set_offers(offers(), cx);
                    liven(view, &ids, window, cx);
                    bodies(view, cx);
                })
                .ok();
            redraw(&handle, cx);
            match kind {
                "add" => {
                    let spot = at(&handle, cx, 200., 460.);
                    input(&handle, cx, down(spot, 1));
                    input(&handle, cx, up(spot));
                    input(&handle, cx, down(spot, 2));
                    input(&handle, cx, up(spot));
                }
                "node" => {
                    let spot = at(&handle, cx, 300., 120.);
                    input(
                        &handle,
                        cx,
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Right,
                            position: spot,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                            first_mouse: false,
                        }),
                    );
                }
                _ => {
                    let middle = handle
                        .update(cx, |view, _, _| {
                            let edge = view
                                .board()
                                .edges()
                                .iter()
                                .find(|e| e.mode == EdgeMode::Approve)
                                .unwrap();
                            kou_canvas::edge_curve(view.board(), edge).unwrap().middle()
                        })
                        .unwrap();
                    let spot = at(&handle, cx, middle.0, middle.1);
                    input(
                        &handle,
                        cx,
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Right,
                            position: spot,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                            first_mouse: false,
                        }),
                    );
                }
            }
            cx.background_executor()
                .timer(Duration::from_millis(260))
                .await;
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }

        for kind in ["node", "edge"] {
            let name = format!("inspector-{kind}-{theme}");
            if !wants(&name) {
                continue;
            }
            let (board, ids) = fixture();
            let handle = open(cx, board, dark).await;
            handle
                .update(cx, |view, window, cx| {
                    view.set_camera(
                        Camera {
                            x: 260.,
                            y: 300.,
                            zoom: 0.75,
                        },
                        cx,
                    );
                    liven(view, &ids, window, cx);
                    bodies(view, cx);
                    if kind == "node" {
                        view.select(Some(ids.lead), cx);
                    } else {
                        let edge = view
                            .board()
                            .edges()
                            .iter()
                            .find(|e| e.mode == EdgeMode::Approve)
                            .map(|e| e.id);
                        view.select_edge(edge, cx);
                    }
                    let _ = window;
                })
                .ok();
            redraw(&handle, cx);
            // I opens the inspector on what is selected.
            handle
                .update(cx, |view, window, cx| {
                    window.focus(&gpui::Focusable::focus_handle(view, cx), cx);
                })
                .ok();
            let any: AnyWindowHandle = handle.into();
            cx.update_window(any, |_, window, cx| {
                window.dispatch_keystroke(gpui::Keystroke::parse("i").unwrap(), cx);
            })
            .ok();
            cx.background_executor()
                .timer(Duration::from_millis(260))
                .await;
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }

        let name = format!("undo-{theme}");
        if wants(&name) {
            let (board, ids) = fixture();
            let handle = open(cx, board, dark).await;
            handle
                .update(cx, |view, window, cx| {
                    view.set_camera(
                        Camera {
                            x: -560.,
                            y: 100.,
                            zoom: 0.75,
                        },
                        cx,
                    );
                    liven(view, &ids, window, cx);
                    bodies(view, cx);
                    view.select(Some(ids.plan), cx);
                    window.focus(&gpui::Focusable::focus_handle(view, cx), cx);
                })
                .ok();
            redraw(&handle, cx);
            let any: AnyWindowHandle = handle.into();
            cx.update_window(any, |_, window, cx| {
                window.dispatch_keystroke(gpui::Keystroke::parse("backspace").unwrap(), cx);
            })
            .ok();
            cx.background_executor()
                .timer(Duration::from_millis(260))
                .await;
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }

        let name = format!("menu-zoom-{theme}");
        if wants(&name) {
            let (board, ids) = fixture();
            let handle = open(cx, board, dark).await;
            handle
                .update(cx, |view, window, cx| {
                    view.set_camera(
                        Camera {
                            x: 20.,
                            y: 250.,
                            zoom: 1.,
                        },
                        cx,
                    );
                    liven(view, &ids, window, cx);
                    bodies(view, cx);
                })
                .ok();
            redraw(&handle, cx);
            // The percentage in the bar opens the zoom's menu.
            let spot = handle
                .update(cx, |view, _, _| {
                    let origin = view.window_point(0., 0.);
                    let camera = view.camera();
                    point(
                        origin.x - px(camera.x) + px(150.),
                        origin.y - px(camera.y) + px(H - 30.),
                    )
                })
                .unwrap();
            input(&handle, cx, down(spot, 1));
            input(&handle, cx, up(spot));
            cx.background_executor()
                .timer(Duration::from_millis(260))
                .await;
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }

        let name = format!("empty-{theme}");
        if wants(&name) {
            let handle = open(cx, Board::new(), dark).await;
            handle
                .update(cx, |view, _, cx| view.set_offers(offers(), cx))
                .ok();
            cx.background_executor()
                .timer(Duration::from_millis(700))
                .await;
            save(cx, handle, out.join(format!("{name}.png"))).await;
            close(cx, handle).await;
        }
    }
}

/// Frames of one motion at given instants, tiled four to a row.
async fn strip(
    cx: &mut AsyncApp,
    out: &Path,
    name: &str,
    times: &[f32],
    setup: impl FnOnce(&mut CanvasView, &Ids, &mut Window, &mut Context<CanvasView>),
    act: impl FnOnce(
        &mut CanvasView,
        &Ids,
        &WindowHandle<CanvasView>,
        &mut Window,
        &mut Context<CanvasView>,
    ),
) {
    let (board, ids) = fixture();
    let handle = open(cx, board, true).await;
    handle
        .update(cx, |view, window, cx| {
            liven(view, &ids, window, cx);
            setup(view, &ids, window, cx);
        })
        .ok();
    redraw(&handle, cx);
    // Let what the setup started (a level of detail crossed) finish first.
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
    redraw(&handle, cx);
    let copy = handle;
    handle
        .update(cx, |view, window, cx| {
            view.set_frozen(true);
            act(view, &ids, &copy, window, cx);
        })
        .ok();
    let mut frames = Vec::new();
    for t in times {
        handle
            .update(cx, |view, window, cx| view.freeze_at(*t, window, cx))
            .ok();
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
        let any: AnyWindowHandle = handle.into();
        let image = cx
            .update_window(any, |_, window, cx| {
                draw(window, cx);
                window.render_to_image()
            })
            .expect("draw")
            .expect("render");
        frames.push(imageops::resize(
            &image,
            image.width() / 2,
            image.height() / 2,
            imageops::FilterType::Triangle,
        ));
    }
    close(cx, handle).await;
    let (fw, fh) = (frames[0].width(), frames[0].height());
    let columns = 4u32;
    let rows = (frames.len() as u32).div_ceil(columns);
    let mut sheet = RgbaImage::new(fw * columns + (columns - 1) * 4, fh * rows + (rows - 1) * 4);
    for (i, frame) in frames.iter().enumerate() {
        let (c, r) = (i as u32 % columns, i as u32 / columns);
        imageops::overlay(
            &mut sheet,
            frame,
            (c * (fw + 4)) as i64,
            (r * (fh + 4)) as i64,
        );
    }
    let path = out.join(format!("strip-{name}.png"));
    sheet.save(&path).expect("write the strip");
    eprintln!(
        "saved {} ({})",
        path.display(),
        times
            .iter()
            .map(|t| format!("{t}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

async fn strips(cx: &mut AsyncApp, out: &Path, wants: &impl Fn(&str) -> bool) {
    let even = |end: f32| (0..8).map(|i| end * i as f32 / 7.).collect::<Vec<_>>();
    if wants("strip-glide") {
        strip(
            cx,
            out,
            "glide",
            &even(300.),
            |view, _, _, cx| {
                view.set_camera(
                    Camera {
                        x: 40.,
                        y: 300.,
                        zoom: 1.25,
                    },
                    cx,
                )
            },
            |view, _, _, _, cx| view.fit(cx),
        )
        .await;
    }
    // A child is there from the first frame, with its edge: nothing comes in.
    if wants("strip-spawn") {
        strip(
            cx,
            out,
            "spawn",
            &even(300.),
            |view, _, _, cx| {
                view.set_camera(
                    Camera {
                        x: -60.,
                        y: 280.,
                        zoom: 0.6,
                    },
                    cx,
                );
                bodies(view, cx);
            },
            |view, ids, _, window, cx| {
                view.spawn_child(
                    ids.callers,
                    spec(NodeKind::Agent, "docs", "K", "Reading the callers' docs."),
                    window,
                    cx,
                );
            },
        )
        .await;
    }
    if wants("strip-select") {
        strip(
            cx,
            out,
            "select",
            &even(210.),
            |view, _, _, cx| {
                view.set_camera(
                    Camera {
                        x: 20.,
                        y: 250.,
                        zoom: 1.,
                    },
                    cx,
                )
            },
            |view, ids, _, _, cx| view.select_nodes(&[ids.lead, ids.callers], cx),
        )
        .await;
    }
    if wants("strip-menu") {
        strip(
            cx,
            out,
            "menu",
            &even(200.),
            |view, _, _, cx| {
                view.set_camera(
                    Camera {
                        x: 20.,
                        y: 250.,
                        zoom: 1.,
                    },
                    cx,
                );
                view.set_offers(offers(), cx);
            },
            |view, _, _, window, cx| view.open_add_menu_here(window, cx),
        )
        .await;
    }
    if wants("strip-inspector") {
        strip(
            cx,
            out,
            "inspector",
            &even(200.),
            |view, ids, _, cx| {
                view.set_camera(
                    Camera {
                        x: 260.,
                        y: 300.,
                        zoom: 0.75,
                    },
                    cx,
                );
                view.select(Some(ids.lead), cx);
            },
            |view, _, _, _, cx| view.inspect(cx),
        )
        .await;
    }
    if wants("strip-flow") {
        strip(
            cx,
            out,
            "flow",
            &even(900.),
            |view, _, _, cx| {
                view.set_camera(
                    Camera {
                        x: 20.,
                        y: 250.,
                        zoom: 1.,
                    },
                    cx,
                )
            },
            |view, ids, _, _, cx| {
                let edge = view
                    .board()
                    .edges()
                    .iter()
                    .find(|e| e.from == ids.lead && e.to == ids.schema)
                    .map(|e| e.id);
                if let Some(edge) = edge {
                    view.flow(edge, cx);
                }
                let _ = ids.issues;
            },
        )
        .await;
    }
    if wants("strip-wire") {
        strip(
            cx,
            out,
            "wire",
            &even(300.),
            |view, _, _, cx| {
                view.set_camera(
                    Camera {
                        x: 140.,
                        y: 420.,
                        zoom: 0.75,
                    },
                    cx,
                )
            },
            |view, ids, _, _, cx| {
                view.connect(
                    PortRef::new(ids.callers, "out"),
                    PortRef::new(ids.tests, "in"),
                    cx,
                );
            },
        )
        .await;
    }
}

/// What a frame costs with many live bodies on the board: frames while a
/// message runs along an edge (the board animates, the bodies do not
/// change), frames of a pan (every body on screen moves), and frames of a
/// pan far out, where every node is a placeholder. Prints the time of each
/// `Window::draw` and how many bodies were built.
async fn perf(cx: &mut AsyncApp) {
    const COLUMNS: usize = 6;
    const COUNT: usize = 24;
    let mut board = Board::new();
    let lines = "$ cargo test -p parser\n   Compiling parser v0.1.0\n    Finished `test` profile [unoptimized] in 3.41s\n     Running unittests src/lib.rs\n\nrunning 42 tests\ntest result: ok. 42 passed; 0 failed\n$ ";
    let ids: Vec<u64> = (0..COUNT)
        .map(|i| {
            board.add_rect(
                spec(NodeKind::Agent, &format!("agent {i}"), "CC", lines),
                Rect::new(
                    (i % COLUMNS) as f32 * 700.,
                    (i / COLUMNS) as f32 * 480.,
                    640.,
                    416.,
                ),
            )
        })
        .collect();
    for pair in ids.windows(2) {
        board
            .connect(PortRef::new(pair[0], "out"), PortRef::new(pair[1], "in"))
            .ok();
    }
    cx.update(|cx| cx.set_global(kou_ui::Theme::new(true)));
    let (width, height) = (2560., 1440.);
    let handle = cx
        .update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(width), px(height)),
                    ))),
                    show: false,
                    focus: false,
                    ..Default::default()
                },
                move |_, cx| cx.new(|cx| CanvasView::new(board, cx)),
            )
        })
        .expect("open a hidden window");
    redraw(&handle, cx);
    let builds = handle
        .update(cx, |view, _, cx| {
            view.set_camera(
                Camera {
                    x: 20.,
                    y: 20.,
                    zoom: 0.6,
                },
                cx,
            );
            bodies(view, cx)
        })
        .unwrap();
    redraw(&handle, cx);
    let any: AnyWindowHandle = handle.into();
    let run = |cx: &mut AsyncApp,
               name: &str,
               step: &dyn Fn(&mut CanvasView, &mut Context<CanvasView>)| {
        let before = builds.get();
        let mut times = Vec::new();
        for _ in 0..60 {
            handle.update(cx, |view, _, cx| step(view, cx)).ok();
            let took = cx
                .update_window(any, |_, window, cx| {
                    let start = Instant::now();
                    window.draw(cx).clear(cx);
                    start.elapsed()
                })
                .unwrap();
            times.push(took.as_secs_f64() * 1000.);
        }
        times.sort_by(f64::total_cmp);
        let shown = handle
            .update(cx, |view, _, _| {
                let visible = view.camera().visible((width, height));
                view.board()
                    .nodes()
                    .iter()
                    .filter(|node| node.rect.overlaps(&visible, 0.))
                    .count()
            })
            .unwrap();
        eprintln!(
            "perf {name}: {shown} of {COUNT} live nodes on screen, 60 frames, median {:.2} ms, worst {:.2} ms, bodies built {}",
            times[30],
            times[59],
            builds.get() - before
        );
    };
    let edge = handle
        .update(cx, |view, _, _| view.board().edges()[0].id)
        .unwrap();
    handle.update(cx, |view, _, cx| view.flow(edge, cx)).ok();
    run(cx, "a message runs, nothing else changes", &|view, cx| {
        if view.running(cx) == 0 {
            view.flow(view.board().edges()[0].id, cx);
        }
        cx.notify();
    });
    run(cx, "panning at 60%", &|view, cx| {
        let mut camera = view.camera();
        camera.pan(-3., -1.);
        view.set_camera(camera, cx);
    });
    handle
        .update(cx, |view, _, cx| {
            view.set_camera(
                Camera {
                    x: 20.,
                    y: 20.,
                    zoom: 0.3,
                },
                cx,
            )
        })
        .ok();
    run(cx, "panning at 30% (placeholders)", &|view, cx| {
        let mut camera = view.camera();
        camera.pan(-3., -1.);
        view.set_camera(camera, cx);
    });
    close(cx, handle).await;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = args.first().map(PathBuf::from) else {
        eprintln!("usage: gallery OUT_DIR [NAME...]");
        std::process::exit(2);
    };
    std::fs::create_dir_all(&out).expect("make the output directory");
    let only: Vec<String> = args[1..].to_vec();
    gpui_platform::application()
        .with_assets(kou_ui::Assets)
        .run(move |cx: &mut App| {
            kou_ui::init(cx);
            cx.spawn(async move |cx| {
                let wants = |name: &str| {
                    only.is_empty() || only.iter().any(|o| name.starts_with(o.as_str()))
                };
                board_shots(cx, &out, &wants).await;
                strips(cx, &out, &wants).await;
                if only.iter().any(|name| name == "perf") {
                    perf(cx).await;
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
