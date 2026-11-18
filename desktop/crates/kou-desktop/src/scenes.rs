//! Scripted states of the interface, for looking at a frame without clicking
//! through to it: `kou-desktop --scene canvas-spawn --time 420 --capture out.png`.
//! The names in them are fixtures; nothing here reaches the real app.

use crate::{
    shell::{HarnessInfo, Picker, Shell, board_for},
    state::{Layout, PaneKind, Store},
};
use gpui::{AnyView, App, AppContext as _, Context, Entity, Window};
use kou_canvas::{Board, CanvasView, NodeKind, NodeSpec};
use kou_ui::Mark;
use std::time::{Duration, Instant};

/// The scenes `--scene` accepts.
pub const NAMES: &[&str] = &[
    "canvas-empty",
    "canvas-first",
    "canvas-placed",
    "canvas-spawn",
    "canvas-spawn-down",
    "canvas-done",
    "canvas-remove",
    "canvas-board",
    "shell-empty",
    "shell-single",
    "shell-split",
    "shell-canvas",
    "shell-picker",
    "shell-new-menu",
    "usage",
    "usage-plain",
    "shell-surface",
    "shell-surface-native",
    "shell-settings-accounts",
    "shell-settings-account-form",
    "shell-settings-account-form-native",
    "shell-settings-claude",
    "shell-settings-claude-native",
    "shell-settings",
    "shell-settings-accent",
    "shell-settings-editor",
    "shell-settings-white",
    "shell-settings-charcoal",
    "shell-settings-refused",
    "shell-sessions",
    "shell-tab-editor",
    "shell-rail-rename",
    "shell-new-tab",
    "shell-new-tab-list",
    "shell-new-tab-models",
    "shell-harness-thread",
    "shell-harness-beside",
    "shell-chat-working",
    "shell-chat-force",
    "shell-chat-effort",
    "shell-chat-studio",
    "shell-chat-transcript",
    "shell-chat-transcript-native",
    "shell-chat-cyrillic",
    "shell-chat-cyrillic-native",
    "shell-chat-working-native",
    "shell-chat-force-native",
    "shell-chat-effort-native",
    "shell-chat-scroll",
    "shell-chat-scroll-native",
    "shell-chat-stream",
    "shell-chat-stream-native",
    "shell-chat-slash",
    "shell-chat-slash-native",
    "shell-chat-help",
    "shell-chat-help-native",
    "shell-chat-modes",
    "shell-chat-modes-native",
    "shell-chat-model",
    "shell-chat-model-native",
    "shell-chat-model-providers",
    "shell-chat-model-providers-native",
    "shell-chat-model-find",
    "shell-chat-model-find-native",
    "shell-chat-agents",
    "shell-chat-agents-native",
    "shell-chat-efforts",
    "shell-chat-efforts-native",
    "shell-chat-efforts-open",
    "shell-chat-efforts-open-native",
    "shell-chat-question",
    "shell-chat-question-native",
    "shell-chat-question-several",
    "shell-chat-question-several-native",
    "shell-chat-question-second",
    "shell-chat-question-second-native",
    "shell-chat-question-words",
    "shell-chat-question-words-native",
    "shell-split-choosing",
    "shell-new-tab-behind",
    "shell-canvas-thread-60",
    "shell-canvas-thread-100",
    "shell-canvas-thread-150",
    "shell-canvas-thread-60-native",
    "shell-canvas-thread-100-native",
    "shell-canvas-thread-150-native",
    "shell-canvas-transcript-60",
    "shell-canvas-transcript-150",
    "shell-canvas-transcript-60-native",
    "shell-canvas-transcript-150-native",
    "motion-canvas-thread-zoom",
    "motion-canvas-wheel-zoom",
    "shell-tab-canvases",
    "shell-canvas-menu",
    "shell-canvas-menu-native",
    "shell-canvas-undo",
    "shell-canvas-undo-native",
    "shell-canvas-empty",
    "shell-canvas-empty-native",
    "shell-canvas-inspect-node",
    "shell-canvas-inspect-node-native",
    "shell-canvas-inspect-edge",
    "shell-canvas-inspect-edge-native",
    "shell-canvas-inspect-many",
    "shell-canvas-inspect-many-native",
    "shell-canvas-nodes-60",
    "shell-canvas-nodes-60-native",
    "shell-canvas-nodes-100",
    "shell-canvas-nodes-100-native",
    "shell-canvas-nodes-150",
    "shell-canvas-nodes-150-native",
    "shell-canvas-nodes-30",
    "shell-canvas-nodes-30-native",
    "shell-canvas-labels-60",
    "shell-canvas-labels-60-native",
    "shell-canvas-labels-100",
    "shell-canvas-labels-100-native",
    "shell-canvas-labels-150",
    "shell-canvas-labels-150-native",
    "shell-no-session",
    "motion-rail",
    "motion-tabs",
    "motion-close",
    "motion-split",
    "motion-select",
    "motion-layout",
    "motion-pane",
];

fn agent(title: &str, tag: &str, detail: &str) -> NodeSpec {
    NodeSpec {
        kind: NodeKind::Agent,
        title: title.into(),
        tag: tag.into(),
        detail: detail.into(),
        size: None,
    }
}

fn lead() -> NodeSpec {
    agent(
        "lead",
        "CC",
        "~/fixture/project\n\nSplit the migration into three parts\nand review each before merging.",
    )
}

fn worker(name: &str, tag: &str) -> NodeSpec {
    agent(
        name,
        tag,
        "~/fixture/project\n\nReading the schema and the two\ncallers that still use the old one.",
    )
}

/// What a scene does once its first frame has been drawn: the time to stop
/// its transitions at, and the window.
pub type Act = Box<dyn FnOnce(f32, &mut Window, &mut App)>;
/// Lists what a scene has in flight.
pub type Describe = Box<dyn Fn(&App) -> Vec<String>>;

/// A scene's first state: what is on screen before its action.
pub struct Scene {
    /// The view to put in the window.
    pub view: AnyView,
    /// What is in flight, for the capture log.
    pub describe: Describe,
    /// What the scene does once the first frame has been drawn, and where to
    /// stop its transitions.
    pub act: Act,
}

fn canvas(
    board: Board,
    cx: &mut App,
    act: impl FnOnce(&mut CanvasView, &mut Window, &mut Context<CanvasView>) + 'static,
) -> Scene {
    let view: Entity<CanvasView> = cx.new(|cx| {
        let mut view = CanvasView::new(board, cx);
        view.set_frozen(true);
        view
    });
    let handle = view.clone();
    let described = view.clone();
    Scene {
        describe: Box::new(move |cx| described.read(cx).describe_transitions(cx)),
        view: view.into(),
        act: Box::new(move |time, window, cx| {
            handle.update(cx, |view, cx| {
                act(view, window, cx);
                view.freeze_at(time, window, cx);
            });
        }),
    }
}

fn family() -> (Board, u64, u64) {
    let mut board = Board::new();
    let root = board.add_at(lead(), 0., 0.);
    let first = board.add_child(root, worker("schema", "CX")).unwrap();
    board.add_child(root, worker("callers", "OC"));
    (board, root, first)
}

/// Build a scene by name.
pub fn build(name: &str, cx: &mut App) -> Option<Scene> {
    Some(match name {
        "canvas-empty" => canvas(Board::new(), cx, |_, _, _| {}),
        "canvas-first" => canvas(Board::new(), cx, |view, window, cx| {
            view.create_at(0., 0., lead(), window, cx);
        }),
        "canvas-placed" => {
            let (board, ..) = family();
            canvas(board, cx, |view, window, cx| {
                view.create_at(-120., 420., worker("notes", "PI"), window, cx);
            })
        }
        "canvas-spawn" => {
            let (board, _, first) = family();
            canvas(board, cx, move |view, window, cx| {
                view.set_mark(first, Mark::Working, window, cx);
                view.spawn_child(first, worker("tests", "CC"), window, cx);
            })
        }
        "canvas-spawn-down" => {
            let mut board = Board::new();
            let root = board.add_at(lead(), 0., 0.);
            let below = board.add_at(worker("below", "CX"), 0., 420.);
            board.remove(below);
            canvas(board, cx, move |view, window, cx| {
                let id = view.create_at(0., 420., worker("below", "CX"), window, cx);
                view.remove(id, window, cx);
                let _ = root;
            })
        }
        "canvas-done" => {
            let (board, _, first) = family();
            canvas(board, cx, move |view, window, cx| {
                view.set_mark(first, Mark::Working, window, cx);
                view.set_mark(first, Mark::Done, window, cx);
            })
        }
        "canvas-remove" => {
            let (board, _, first) = family();
            canvas(board, cx, move |view, window, cx| {
                view.remove(first, window, cx)
            })
        }
        "canvas-board" => {
            let (mut board, root, first) = family();
            let tests = board.add_child(first, worker("tests", "CC")).unwrap();
            board.add_child(first, worker("docs", "PI"));
            board.add_at(
                NodeSpec {
                    kind: NodeKind::Note,
                    title: "plan".into(),
                    detail: "1. schema\n2. callers\n3. tests".into(),
                    ..NodeSpec::default()
                },
                -420.,
                40.,
            );
            canvas(board, cx, move |view, window, cx| {
                view.set_mark(root, Mark::Working, window, cx);
                view.set_mark(tests, Mark::Blocked, window, cx);
                view.select(Some(first), cx);
            })
        }
        "shell-empty" => shell(Store::default(), cx, |_, _, _| {}),
        "shell-no-session" => shell(fixture().0, cx, |shell, _, cx| {
            shell.store.selected = None;
            cx.notify();
        }),
        "shell-single" => shell(fixture().0, cx, |_, _, _| {}),
        // The lead beside the two it started, one over the other.
        "shell-split" => shell(split_fixture().0, cx, |_, _, _| {}),
        // The same tab as its canvas: the three, and who started whom.
        "shell-canvas" => shell(split_fixture().0, cx, |shell, _, cx| {
            shell.set_layout(Layout::Canvas, cx)
        }),
        // The canvas's chrome as the `canvas` plugin draws it, and (the
        // `-native` twins) as the canvas does: a menu open, the bar after a
        // deletion, an empty board. Taken at `--time 2000`, when they are in.
        "shell-canvas-menu" | "shell-canvas-menu-native" => {
            on_canvas(name.ends_with("-native"), cx, |canvas, window, cx| {
                canvas.open_add_menu_here(window, cx)
            })
        }
        "shell-canvas-undo" | "shell-canvas-undo-native" => {
            on_canvas(name.ends_with("-native"), cx, |canvas, window, cx| {
                if let Some(edge) = canvas.board().edges().first().map(|edge| edge.id) {
                    canvas.delete_edge(edge, window, cx);
                }
            })
        }
        "shell-canvas-empty" | "shell-canvas-empty-native" => {
            on_canvas(name.ends_with("-native"), cx, |canvas, window, cx| {
                let nodes: Vec<_> = canvas.board().nodes().iter().map(|node| node.id).collect();
                for node in nodes {
                    canvas.remove(node, window, cx);
                }
            })
        }
        // The inspector on a node, on an edge an agent wired, on several.
        "shell-canvas-inspect-node" | "shell-canvas-inspect-node-native" => {
            on_canvas(name.ends_with("-native"), cx, |canvas, _, cx| {
                let first = canvas.board().nodes().first().map(|node| node.id);
                canvas.select(first, cx);
                canvas.inspect(cx);
            })
        }
        "shell-canvas-inspect-edge" | "shell-canvas-inspect-edge-native" => {
            on_canvas(name.ends_with("-native"), cx, |canvas, _, cx| {
                let first = canvas
                    .board()
                    .edges()
                    .first()
                    .map(|edge| (edge.id, edge.from));
                if let Some((edge, by)) = first {
                    canvas.update_edge(
                        edge,
                        |edge| {
                            edge.origin = kou_canvas::EdgeOrigin::Wired;
                            edge.by = Some(by);
                        },
                        cx,
                    );
                    canvas.select_edge(Some(edge), cx);
                    canvas.inspect(cx);
                }
            })
        }
        "shell-canvas-inspect-many" | "shell-canvas-inspect-many-native" => {
            on_canvas(name.ends_with("-native"), cx, |canvas, _, cx| {
                let all: Vec<_> = canvas.board().nodes().iter().map(|node| node.id).collect();
                canvas.select_nodes(&all, cx);
                canvas.inspect(cx);
            })
        }
        // The labels of edges at three zooms: one an agent wired, along
        // which three went; one that asks first, with two waiting, selected.
        name if name.starts_with("shell-canvas-labels-") => {
            let zoom = if name.contains("-60") {
                0.6
            } else if name.contains("-150") {
                1.5
            } else {
                1.
            };
            let native = name.ends_with("-native");
            shell(split_fixture().0, cx, move |shell, _, cx| {
                shell.set_layout(Layout::Canvas, cx);
                let area = shell.content_area.get().size;
                let viewport = (f32::from(area.width), f32::from(area.height));
                let tab = shell
                    .store
                    .current()
                    .and_then(|session| session.front())
                    .map(|tab| tab.id.clone());
                let Some(canvas) = tab.and_then(|tab| shell.canvas(&tab, cx)) else {
                    return;
                };
                canvas.update(cx, |canvas, cx| {
                    if native {
                        canvas.set_face(None, cx);
                    }
                    let edges: Vec<_> = canvas
                        .board()
                        .edges()
                        .iter()
                        .map(|edge| (edge.id, edge.from))
                        .collect();
                    if let Some((edge, by)) = edges.first().copied() {
                        canvas.update_edge(
                            edge,
                            |edge| {
                                edge.origin = kou_canvas::EdgeOrigin::Wired;
                                edge.by = Some(by);
                            },
                            cx,
                        );
                        let traffic = kou_canvas::Traffic {
                            delivered: 3,
                            waiting: 0,
                        };
                        canvas.set_traffic(edge, traffic, cx);
                    }
                    if let Some((edge, _)) = edges.get(1).copied() {
                        canvas.update_edge(
                            edge,
                            |edge| edge.mode = kou_canvas::EdgeMode::Approve,
                            cx,
                        );
                        let traffic = kou_canvas::Traffic {
                            delivered: 0,
                            waiting: 2,
                        };
                        canvas.set_traffic(edge, traffic, cx);
                        canvas.select_edge(Some(edge), cx);
                    }
                    let first = canvas.board().nodes().first().map(|node| node.rect);
                    if let Some(rect) = first {
                        let mut camera = kou_canvas::Camera::centre_on(&rect, viewport, zoom);
                        // (The labels are right of the first node.)
                        camera.x -= rect.w * zoom * 0.6;
                        canvas.set_camera(camera, cx);
                    }
                });
            })
        }
        // The nodes' own chrome (their headers and footers) at three zooms:
        // the first selected, the last a note.
        name if name.starts_with("shell-canvas-nodes-") => {
            // (At 30% they are placeholders, which are the canvas's own,
            // with the tabs of what waits over them.)
            let zoom = if name.contains("-60") {
                0.6
            } else if name.contains("-150") {
                1.5
            } else if name.contains("-30") {
                0.3
            } else {
                1.
            };
            let native = name.ends_with("-native");
            shell(split_fixture().0, cx, move |shell, _, cx| {
                shell.set_layout(Layout::Canvas, cx);
                let area = shell.content_area.get().size;
                let viewport = (f32::from(area.width), f32::from(area.height));
                let tab = shell
                    .store
                    .current()
                    .and_then(|session| session.front())
                    .map(|tab| tab.id.clone());
                let Some(canvas) = tab.and_then(|tab| shell.canvas(&tab, cx)) else {
                    return;
                };
                canvas.update(cx, |canvas, cx| {
                    if native {
                        canvas.set_face(None, cx);
                    }
                    let nodes: Vec<_> = canvas.board().nodes().iter().map(|node| node.id).collect();
                    if let Some(last) = nodes.last() {
                        canvas.update_node(
                            *last,
                            |node| {
                                node.kind = NodeKind::Note;
                                node.detail = "Ask for the schema first.".into();
                            },
                            cx,
                        );
                    }
                    // What waits for two of them: to approve, and only waiting.
                    if let Some(first) = nodes.first() {
                        let pending = kou_canvas::Pending {
                            count: 2,
                            awaiting: 1,
                            from: Some("Codex".into()),
                        };
                        canvas.set_pending(*first, pending, cx);
                    }
                    if let Some(second) = nodes.get(1) {
                        let pending = kou_canvas::Pending {
                            count: 3,
                            awaiting: 0,
                            from: None,
                        };
                        canvas.set_pending(*second, pending, cx);
                        // What it says, a line at a time.
                        canvas.update_node(
                            *second,
                            |node| {
                                node.detail = "Reading the schema and the two callers\nthat still use the old one.".into();
                            },
                            cx,
                        );
                    }
                    if let Some(first) = nodes.first()
                        && let Some(rect) = canvas.board().node(*first).map(|node| node.rect)
                    {
                        canvas.select(Some(*first), cx);
                        canvas.set_camera(kou_canvas::Camera::centre_on(&rect, viewport, zoom), cx);
                    }
                });
            })
        }
        "shell-picker" => {
            let (store, session, lead) = fixture();
            shell(store, cx, move |shell, _, cx| {
                shell.picker = Some(Picker {
                    session: session.clone(),
                    parent: Some(lead.clone()),
                    tab: None,
                    front: true,
                    beside: None,
                });
                cx.notify();
            })
        }
        // What moves, for strips: each does one thing after the first frame.
        "motion-rail" => shell(fixture().0, cx, |shell, _, cx| shell.toggle_rail(cx)),
        "motion-tabs" => {
            let (store, _, lead) = fixture();
            shell(store, cx, move |shell, window, cx| {
                shell.close_pane(&lead, window, cx)
            })
        }
        // A pane closed in a split: what was beside it takes its room at once.
        "motion-close" => {
            let (store, closing) = split_fixture();
            shell(store, cx, move |shell, window, cx| {
                shell.close_pane(&closing, window, cx)
            })
        }
        // A split made: the new terminal comes in once.
        "motion-split" => shell(fixture().0, cx, |shell, window, cx| {
            shell.split(crate::state::Axis::Row, window, cx);
        }),
        "motion-select" => {
            let (store, ..) = fixture();
            let other = store.sessions[0].id.clone();
            shell(store, cx, move |shell, _, cx| {
                shell.select_session(&other, cx)
            })
        }
        "motion-layout" => shell(split_fixture().0, cx, |shell, _, cx| {
            shell.set_layout(Layout::Canvas, cx)
        }),
        "motion-pane" => {
            let (store, session, lead) = fixture();
            let other = store
                .panes_of(&session)
                .map(|pane| pane.id.clone())
                .find(|pane| *pane != lead);
            shell(store, cx, move |shell, _, cx| {
                if let Some(other) = other {
                    shell.focus_pane(&other, cx);
                }
            })
        }
        // A tab that was given a name and an icon, with its editor open.
        "shell-tab-editor" => {
            let (mut store, session, _) = fixture();
            if let Some(saved) = store.session_mut(&session) {
                saved.tabs[1].name = Some("api server".into());
                saved.tabs[1].icon = Some("server".into());
                saved.tabs[2].icon = Some("flask".into());
            }
            shell(store, cx, move |shell, window, cx| {
                shell.edit_tab(&session, 1, window, cx);
            })
        }
        // A conversation in a node of a tab's canvas, at three zooms: it is
        // drawn at the canvas's zoom, text and all.
        // The transcript with every kind of entry, in its node on the canvas
        // at 60% and 150%; `-native` without the plugins.
        "shell-canvas-transcript-60"
        | "shell-canvas-transcript-150"
        | "shell-canvas-transcript-60-native"
        | "shell-canvas-transcript-150-native" => {
            let zoom = if name.contains("-60") { 0.6 } else { 1.5 };
            let native = name.ends_with("-native");
            shell(split_fixture().0, cx, move |shell, window, cx| {
                cx.set_reduce_motion(true);
                transcript_on_canvas(shell, zoom, native, window, cx);
            })
        }
        // The same as the chat draws them without the plugins.
        "shell-canvas-thread-60-native"
        | "shell-canvas-thread-100-native"
        | "shell-canvas-thread-150-native" => {
            let zoom = match name {
                "shell-canvas-thread-60-native" => 0.6,
                "shell-canvas-thread-150-native" => 1.5,
                _ => 1.,
            };
            shell(split_fixture().0, cx, move |shell, window, cx| {
                thread_on_canvas(shell, zoom, window, cx);
                native_chats(shell, cx);
            })
        }
        "shell-canvas-thread-60" | "shell-canvas-thread-100" | "shell-canvas-thread-150" => {
            let zoom = match name {
                "shell-canvas-thread-60" => 0.6,
                "shell-canvas-thread-150" => 1.5,
                _ => 1.,
            };
            shell(split_fixture().0, cx, move |shell, window, cx| {
                thread_on_canvas(shell, zoom, window, cx);
            })
        }
        // The same node zoomed as a pinch zooms it, a little more every
        // frame from 60%: frame by frame the conversation is as large as its
        // node.
        "motion-canvas-thread-zoom" => shell(split_fixture().0, cx, |shell, window, cx| {
            let Some((canvas, node)) = thread_on_canvas(shell, 0.6, window, cx) else {
                return;
            };
            let area = shell.content_area.get().size;
            let viewport = (f32::from(area.width), f32::from(area.height));
            cx.spawn(async move |_, cx| {
                let mut zoom = 0.6f32;
                while zoom < 1.6 {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    zoom *= 1.012;
                    canvas.update(cx, |canvas, cx| {
                        if let Some(rect) = canvas.board().node(node).map(|node| node.rect) {
                            canvas.set_camera(
                                kou_canvas::Camera::centre_on(&rect, viewport, zoom),
                                cx,
                            );
                        }
                    });
                }
            })
            .detach();
        }),
        // The same conversation zoomed as the user zooms it: steps of a
        // wheel with Cmd held over the node, each of which the canvas glides
        // through, frame by frame, from inside its own drawing.
        "motion-canvas-wheel-zoom" => shell(split_fixture().0, cx, |shell, window, cx| {
            use gpui::InputEvent as _;
            if thread_on_canvas(shell, 0.6, window, cx).is_none() {
                return;
            }
            let area = shell.content_area.get();
            let (handle, over) = (window.window_handle(), area.center());
            cx.spawn(async move |_, cx| {
                for _ in 0..14 {
                    cx.background_executor()
                        .timer(Duration::from_millis(90))
                        .await;
                    let step = gpui::ScrollEvent {
                        position: over,
                        delta: gpui::ScrollDelta::Lines(gpui::point(0., 1.)),
                        modifiers: gpui::Modifiers {
                            platform: true,
                            ..gpui::Modifiers::none()
                        },
                        touch_phase: gpui::TouchPhase::Moved,
                        source: Default::default(),
                    };
                    let sent = cx.update_window(handle, |_, window, cx| {
                        window.dispatch_event(step.to_platform_input(), cx);
                    });
                    if sent.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }),
        // A split asked for: its new half shows what can be started there.
        "shell-split-choosing" => shell(split_fixture().0, cx, |shell, window, cx| {
            shell.split(crate::state::Axis::Column, window, cx);
        }),
        // A tab an agent is yet to be chosen in: in front, and left behind
        // another tab, where it waits in the strip.
        "shell-new-tab" => shell(fixture().0, cx, |shell, _, cx| shell.pick(None, cx)),
        // A chat with a harness, in its tab: as kou's own chat looks.
        "shell-harness-thread" => shell(fixture().0, cx, |shell, window, cx| {
            let Some(pane) = shell
                .store
                .panes
                .iter()
                .find(|pane| pane.harness == "claude")
                .cloned()
            else {
                return;
            };
            if let Some(saved) = shell.store.pane_mut(&pane.id) {
                saved.kind = crate::state::PaneKind::Thread;
            }
            let (backend, end) = kou_thread::channel();
            shell.install_thread(&pane, backend, "Claude Code".into(), window, cx);
            for update in conversation() {
                let _ = end.updates.try_send(update);
            }
            // The scene's agent never hangs up.
            std::mem::forget(end);
            shell.focus_pane(&pane.id, cx);
        }),
        // A surface kou's agent shows with its `UI` tool, in its chat: the
        // tool's own output for a benchmark's file, which the scene writes
        // to a workspace of its own.
        // (`-native`: drawn without the plugins, as the chat draws it.)
        "shell-surface" | "shell-surface-native" => {
            let native = name.ends_with("-native");
            shell(fixture().0, cx, move |shell, window, cx| {
                use kou_thread::{
                    Block, Capabilities, Stop, ToolCall, ToolContent, ToolStatus, Update,
                };
                let Some(mut pane) = shell
                    .store
                    .panes
                    .iter()
                    .find(|pane| pane.harness == "claude")
                    .cloned()
                else {
                    return;
                };
                let root =
                    std::env::temp_dir().join(format!("kou-scene-surface-{}", std::process::id()));
                let version = root.join(".harness/ui/s-1/bench");
                let _ = std::fs::create_dir_all(&version);
                let _ = std::fs::create_dir_all(root.join("bench"));
                let _ = std::fs::write(
                    version.join("v1.json"),
                    include_str!("../fixtures/surface-bench.json"),
                );
                let _ = std::fs::write(
                    root.join("bench/runs.jsonl"),
                    include_str!("../fixtures/surface-runs.jsonl"),
                );
                if let Some(saved) = shell.store.pane_mut(&pane.id) {
                    saved.kind = crate::state::PaneKind::Thread;
                    saved.title = "kou".into();
                }
                pane.cwd = root;
                let (backend, end) = kou_thread::channel();
                shell.install_thread(&pane, backend, "kou".into(), window, cx);
                for update in [
                    Update::Ready {
                        session: "s-1".into(),
                        capabilities: Capabilities::default(),
                        modes: Vec::new(),
                        mode: None,
                        models: Vec::new(),
                        model: None,
                    },
                    Update::UserChunk(Block::Text(
                        "Show me where the sessionstore benchmarks stand.".into(),
                    )),
                    Update::TurnStarted,
                    Update::Tool(ToolCall {
                        id: "ui-1".into(),
                        title: "UI".into(),
                        status: ToolStatus::Done,
                        content: vec![ToolContent::Text(
                            include_str!("../fixtures/surface-bench.txt").into(),
                        )],
                        ..ToolCall::default()
                    }),
                    Update::AssistantChunk(
                        "It follows `bench/runs.jsonl`: new runs show as they are written.".into(),
                    ),
                    Update::TurnEnded(Stop::Finished),
                ] {
                    let _ = end.updates.try_send(update);
                }
                std::mem::forget(end);
                shell.focus_pane(&pane.id, cx);
                if native {
                    native_chats(shell, cx);
                }
            })
        }
        // A chat's prompt area with all it shows: the model's and the
        // mode's controls and the figures under the field; at work, the
        // line over it and the queue, with nothing typed (stop) or with a
        // draft (force and queue); at rest, the effort's bars and a draft
        // ready to send. Taken a while after the scene starts
        // (`--strip 1 --every 3000`): what it types is typed once the
        // conversation is in.
        // A transcript with every kind of entry: a prompt, thinking, tool
        // calls done, failed, running and waiting on the user, a plan,
        // notices and an answer. With less motion: nothing in it is on its
        // way, so two captures of it are the same.
        "shell-chat-studio" => shell(fixture().0, cx, |shell, window, cx| {
            cx.set_reduce_motion(true);
            chat_transcript(shell, window, cx);
            for talking in shell.threads.values() {
                talking.view().update(cx, |view, cx| {
                    view.set_look(kou_thread_view::Look::Studio, cx)
                });
            }
        }),
        "shell-chat-transcript" => shell(fixture().0, cx, |shell, window, cx| {
            cx.set_reduce_motion(true);
            chat_transcript(shell, window, cx);
        }),
        // The same chats as the chat draws them without the plugins (the
        // `-native` twins): what the plugins' drawings are compared with.
        "shell-chat-transcript-native" => shell(fixture().0, cx, |shell, window, cx| {
            cx.set_reduce_motion(true);
            chat_transcript(shell, window, cx);
            native_chats(shell, cx);
        }),
        // A conversation held in Russian, every kind of text in it: what
        // the faces chosen for the interface make of Cyrillic.
        "shell-chat-cyrillic" | "shell-chat-cyrillic-native" => {
            let native = name.ends_with("-native");
            shell(fixture().0, cx, move |shell, window, cx| {
                cx.set_reduce_motion(true);
                chat_cyrillic(shell, window, cx);
                if native {
                    native_chats(shell, cx);
                }
            })
        }
        // The same chat with `/` typed: the list of commands over the field.
        // Or `/help` sent: the card of keys and commands over the field; or
        // `/mode`: the modes' list over its control; or `/model`: the model
        // picker, on one provider's models, or (`-providers`) on two
        // providers, or (`-find`) with a search typed in it.
        "shell-chat-slash"
        | "shell-chat-slash-native"
        | "shell-chat-help"
        | "shell-chat-help-native"
        | "shell-chat-modes"
        | "shell-chat-modes-native"
        | "shell-chat-model"
        | "shell-chat-model-native"
        | "shell-chat-model-providers"
        | "shell-chat-model-providers-native"
        | "shell-chat-model-find"
        | "shell-chat-model-find-native" => {
            let native = name.ends_with("-native");
            let name = name.trim_end_matches("-native").to_owned();
            shell(fixture().0, cx, move |shell, window, cx| {
                chat_at_work(&name, shell, window, cx);
                if native {
                    native_chats(shell, cx);
                }
            })
        }
        "shell-chat-working-native" | "shell-chat-force-native" | "shell-chat-effort-native" => {
            let name = name.trim_end_matches("-native").to_owned();
            shell(fixture().0, cx, move |shell, window, cx| {
                chat_at_work(&name, shell, window, cx);
                native_chats(shell, cx);
            })
        }
        // A chat whose agent started agents of its own (subagents, by its
        // calls: one at work, one done, one failed) and agents in panes
        // (one at work, one done, one waiting on the user), with the card
        // beside pulled out over the text and the ticks of its two prompts;
        // `-native` without the plugins.
        "shell-chat-agents" | "shell-chat-agents-native" => {
            let native = name.ends_with("-native");
            shell(fixture().0, cx, move |shell, window, cx| {
                cx.set_reduce_motion(true);
                chat_agents(shell, window, cx);
                if native {
                    native_chats(shell, cx);
                }
            })
        }
        // Claude Code's chat with levels of effort on offer apart from its
        // modes: the effort's bars between the model's and the mode's
        // controls, at rest with a draft typed; `-open` with the levels'
        // list open (`/effort` sent). `-native` without the plugins. Taken
        // a while after the scene starts (`--strip 2 --every 1200`).
        "shell-chat-efforts"
        | "shell-chat-efforts-native"
        | "shell-chat-efforts-open"
        | "shell-chat-efforts-open-native" => {
            let native = name.ends_with("-native");
            let name = name.trim_end_matches("-native").to_owned();
            shell(fixture().0, cx, move |shell, window, cx| {
                chat_at_work(&name, shell, window, cx);
                if native {
                    native_chats(shell, cx);
                }
            })
        }
        // A call of Claude Code's that asks the user: one question, its
        // options described, and a refusal; `-several`, a question that
        // takes several options with two chosen (by their digits); `-second`,
        // the second of three with the first answered (by its digit);
        // `-words`, one answered only in words. `-native` without the
        // plugins. (`--strip 2 --every 1200` for the digits.)
        "shell-chat-question"
        | "shell-chat-question-native"
        | "shell-chat-question-several"
        | "shell-chat-question-several-native"
        | "shell-chat-question-second"
        | "shell-chat-question-second-native"
        | "shell-chat-question-words"
        | "shell-chat-question-words-native" => {
            let native = name.ends_with("-native");
            let name = name.trim_end_matches("-native").to_owned();
            shell(fixture().0, cx, move |shell, window, cx| {
                cx.set_reduce_motion(true);
                chat_question(&name, shell, window, cx);
                if native {
                    native_chats(shell, cx);
                }
            })
        }
        // A long conversation scrolled up by a wheel over it, or with an
        // answer being written under it, frame after frame; with `-native`,
        // drawn without the plugins. For timing frames: with
        // `KOU_ZOOM_TRACE=1` and `--strip 40 --every 25`, each frame prints
        // what it cost.
        "shell-chat-scroll"
        | "shell-chat-scroll-native"
        | "shell-chat-stream"
        | "shell-chat-stream-native" => {
            let native = name.ends_with("-native");
            let stream = name.starts_with("shell-chat-stream");
            shell(fixture().0, cx, move |shell, window, cx| {
                chat_long(native, stream, shell, window, cx);
            })
        }
        "shell-chat-working" | "shell-chat-force" | "shell-chat-effort" => {
            let name = name.to_owned();
            shell(fixture().0, cx, move |shell, window, cx| {
                chat_at_work(&name, shell, window, cx);
            })
        }
        // The same chat with the card beside it pulled out by its handle,
        // where the window leaves it no room: over the text.
        "shell-harness-beside" => shell(fixture().0, cx, |shell, window, cx| {
            let Some(pane) = shell
                .store
                .panes
                .iter()
                .find(|pane| pane.harness == "claude")
                .cloned()
            else {
                return;
            };
            if let Some(saved) = shell.store.pane_mut(&pane.id) {
                saved.kind = crate::state::PaneKind::Thread;
            }
            let (backend, end) = kou_thread::channel();
            let view = shell.install_thread(&pane, backend, "Claude Code".into(), window, cx);
            for update in conversation() {
                let _ = end.updates.try_send(update);
            }
            std::mem::forget(end);
            shell.focus_pane(&pane.id, cx);
            view.update(cx, |view, cx| view.pull_beside(true, cx));
        }),
        // The same page once the harness has said which models it offers.
        "shell-new-tab-models" => shell(fixture().0, cx, |shell, _, cx| {
            let model = |id: &str, name: &str| kou_thread::Choice {
                id: id.into(),
                name: name.into(),
                description: None,
            };
            shell.offered.insert(
                "claude".into(),
                crate::shell::Offered {
                    models: vec![
                        model("opus", "Opus"),
                        model("sonnet", "Sonnet"),
                        model("haiku", "Haiku"),
                    ],
                    providers: Vec::new(),
                    current: Some("opus".into()),
                    cwd: "/fixture/atlas".into(),
                    at: Some(std::time::Instant::now()),
                    ..Default::default()
                },
            );
            shell.pick(None, cx);
        }),
        // The same page with the field's list open.
        "shell-new-tab-list" => shell(fixture().0, cx, |shell, _, cx| {
            shell.pick(None, cx);
            shell.toggle_ask_menu(cx);
        }),
        "shell-new-tab-behind" => {
            let (store, _, lead) = fixture();
            shell(store, cx, move |shell, _, cx| {
                shell.pick(None, cx);
                shell.focus_pane(&lead, cx);
            })
        }
        // A session being renamed where its name stands in the rail.
        "shell-rail-rename" => {
            let (store, session, _) = fixture();
            shell(store, cx, move |shell, window, cx| {
                shell.edit_name(crate::shell::Named::Session(session.clone()), window, cx);
            })
        }
        // Three tabs, the one in front shown as its canvas: a tab's canvas
        // has that tab's panes and no others.
        "shell-tab-canvases" => {
            let (mut store, session, _) = fixture();
            if let Some(saved) = store.session_mut(&session) {
                saved.tabs[0].name = Some("review".into());
                saved.tabs[0].icon = Some("eye".into());
            }
            shell(store, cx, |shell, _, cx| {
                shell.set_layout(Layout::Canvas, cx)
            })
        }
        "shell-sessions" => {
            let (store, _, lead) = fixture();
            shell(store, cx, move |shell, _, cx| {
                let now = std::time::SystemTime::now();
                let ago = |minutes: u64| now - std::time::Duration::from_secs(minutes * 60);
                let session =
                    |id: &str, title: &str, minutes, messages, running| crate::shell::Past {
                        id: id.into(),
                        title: title.into(),
                        updated: Some(ago(minutes)),
                        running,
                        prompts: Some(messages),
                    };
                shell.resuming = Some(crate::shell::Resuming {
                    pane: lead.clone(),
                    sessions: Some(vec![
                        session("a", "Split the migration into three parts", 2, 14, true),
                        session(
                            "b",
                            "Why does the parser drop trailing commas",
                            95,
                            6,
                            false,
                        ),
                        session(
                            "c",
                            "Review the retry policy in the relay client",
                            1500,
                            1,
                            false,
                        ),
                        session("d", "", 4300, 0, false),
                    ]),
                });
                cx.notify();
            })
        }
        "shell-settings-editor" | "shell-settings-white" | "shell-settings-charcoal" => {
            let name = name.to_owned();
            shell(fixture().0, cx, move |shell, window, cx| {
                use kou_ui::themes::{Preset, Selection};
                let base = if name.ends_with("white") {
                    Preset::White
                } else {
                    Preset::Charcoal
                };
                shell.choose_theme(Selection::Preset(base), window, cx);
                shell.settings_open = true;
                if name.ends_with("editor") {
                    shell.appearance.draft = Some(crate::appearance::Draft {
                        document: base.document(),
                        id: None,
                    });
                }
            })
        }
        "shell-settings" => shell(fixture().0, cx, |shell, _, cx| {
            shell.settings_open = true;
            cx.notify();
        }),
        // The usage page: two accounts of one provider put together, a
        // provider with one, one that does not say; and the chats' figures.
        "usage" => usage(true, cx),
        // The same as a machine with one account of each has it: one
        // answers, one's sign-in was refused, one says nothing here.
        "usage-plain" => usage(false, cx),
        // The settings with a second account of a harness.
        "shell-settings-accounts" => shell(fixture().0, cx, |shell, _, cx| {
            shell.appearance.page = crate::appearance::Page::Accounts;
            shell
                .store
                .add_profile("claude", "/Users/me/.claude-work".into());
            shell.settings_open = true;
            cx.notify();
        }),
        // The same with another account being added: its name typed, its
        // proxy refused. (`-native`: as the app draws it itself.)
        "shell-settings-account-form" | "shell-settings-account-form-native" => {
            let native = name.ends_with("-native");
            shell(fixture().0, cx, move |shell, window, cx| {
                use crate::usage::{Account, Asked};
                if native {
                    // (No scripts at all: everything is the app's own.)
                    shell.plugins = std::rc::Rc::default();
                }
                let work = shell
                    .store
                    .add_profile("claude", "/Users/me/.claude-work".into());
                if let Some(profile) = shell.store.profiles.last_mut() {
                    profile.proxy = Some("http://gate.example.com:3128".into());
                }
                // What is known of the two that are there.
                let left = |used: f32| Asked {
                    answer: Some(Ok(kou_usage::Limits {
                        plan: None,
                        windows: vec![kou_usage::Window {
                            kind: kou_usage::Kind::Weekly,
                            used,
                            resets: None,
                        }],
                    })),
                    at: None,
                    asking: false,
                };
                shell.usage.insert(
                    Account {
                        harness: "claude".into(),
                        profile: Some(work),
                    },
                    left(36.),
                );
                shell.settings_open = true;
                shell.open_account_form("claude", None, window, cx);
                if let Some(form) = shell.account_form.as_mut() {
                    form.name = kou_canvas::field::Field::new("personal", false);
                    form.proxy = kou_canvas::field::Field::new("socks5://10.0.0.7:1080", false);
                    form.on_proxy = true;
                    form.problem = Some(
                        "A SOCKS proxy is not one the agents take: give an HTTP or HTTPS proxy."
                            .into(),
                    );
                }
                cx.notify();
            })
        }
        // The settings on a machine with Claude Code, its chat held with
        // Claude Code itself. (`-native`: as the app draws it itself.)
        "shell-settings-claude" | "shell-settings-claude-native" => {
            let native = name.ends_with("-native");
            shell(fixture().0, cx, move |shell, _, cx| {
                if native {
                    shell.plugins = std::rc::Rc::default();
                }
                shell.detected.push(kou_agents::Detected {
                    spec: kou_agents::spec("claude").expect("Claude Code is a known harness"),
                    path: Some("/usr/local/bin/claude".into()),
                    version: Some("2.1.325".into()),
                    readiness: kou_agents::Readiness::Ready,
                    message: None,
                    native: false,
                    acp: None,
                    hooks: true,
                    chat: kou_agents::Chat::None,
                });
                shell.store.claude_direct = true;
                shell.appearance.page = crate::appearance::Page::General;
                shell.settings_open = true;
                cx.notify();
            })
        }
        // The list under the tab strip's plus: what a new tab can be.
        "shell-new-menu" => shell(fixture().0, cx, |shell, _, cx| {
            shell.toggle_new_menu(cx);
        }),
        // The settings after another accent was chosen there: what follows it.
        "shell-settings-accent" => shell(fixture().0, cx, |shell, _, cx| {
            shell.settings_open = true;
            shell.set_accent(Some(kou_ui::ACCENTS[5].1), cx);
        }),
        // The settings when the system does not let the app post.
        "shell-settings-refused" => shell(fixture().0, cx, |shell, _, cx| {
            shell.settings_open = true;
            shell.appearance.page = crate::appearance::Page::General;
            shell.notice_permission = crate::notify::Permission::Refused;
            cx.notify();
        }),
        _ => return None,
    })
}

// The usage page of a shell whose accounts have answered: `several` with a
// second account of Claude Code, and every provider saying something.
fn usage(several: bool, cx: &mut App) -> Scene {
    use crate::usage::{Account, Asked};
    use kou_usage::{Kind, Limits, Reason, Window as Limit};
    use std::time::SystemTime;
    // A clock of the scene's own: what is left of a window never changes.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_960_000);
    let limits = move |plan: &str, windows: &[(Kind, f32, u64)]| {
        Ok(Limits {
            plan: Some(plan.to_owned()),
            windows: windows
                .iter()
                .map(|(kind, used, minutes)| Limit {
                    kind: kind.clone(),
                    used: *used,
                    resets: Some(now + Duration::from_secs(minutes * 60)),
                })
                .collect(),
        })
    };
    let fable = || Kind::Model("Fable".into());
    shell(fixture().0, cx, move |shell, window, cx| {
        shell.harnesses.insert(
            2,
            HarnessInfo {
                id: "grok".into(),
                name: "Grok Build".into(),
                code: "GK".into(),
                ready: true,
                version: Some("1.0.77".into()),
                message: None,
                native: true,
                adapter: None,
            },
        );
        let answered = |shell: &mut Shell, harness: &str, profile: Option<String>, answer| {
            shell.usage.insert(
                Account {
                    harness: harness.to_owned(),
                    profile,
                },
                Asked {
                    answer: Some(answer),
                    at: Some(Instant::now() - Duration::from_secs(140)),
                    asking: false,
                },
            );
        };
        if several {
            answered(
                shell,
                "claude",
                None,
                limits(
                    "Max",
                    &[
                        (Kind::Session, 38., 134),
                        (Kind::Weekly, 27., 7140),
                        (fable(), 16., 7140),
                    ],
                ),
            );
            let work = shell
                .store
                .add_profile("claude", "/Users/me/.claude-work".into());
            answered(
                shell,
                "claude",
                Some(work),
                limits(
                    "Max",
                    &[
                        (Kind::Session, 4., 251),
                        (Kind::Weekly, 7., 9240),
                        (fable(), 13., 9240),
                    ],
                ),
            );
            answered(
                shell,
                "codex",
                None,
                limits(
                    "Plus",
                    &[(Kind::Session, 31., 212), (Kind::Weekly, 78., 2890)],
                ),
            );
        } else {
            answered(
                shell,
                "claude",
                None,
                limits("Max", &[(Kind::Weekly, 68., 6260), (fable(), 93., 6260)]),
            );
            answered(
                shell,
                "codex",
                None,
                Err(Reason::SignIn(
                    "Codex's sign-in was refused by its service: sign in again with `codex login`"
                        .into(),
                )),
            );
        }
        answered(
            shell,
            "grok",
            None,
            Err(Reason::NotHere(
                "`grok usage` can't show your plan's usage here yet. Run /usage inside grok instead."
                    .into(),
            )),
        );
        // Two chats that have spent something, one under the second account.
        let spent = [
            (
                "claude",
                "Fix the retry backoff",
                96_400,
                200_000,
                183_400,
                9_800,
                1.37,
                Mark::Working,
            ),
            (
                "codex",
                "Review the migration",
                31_000,
                272_000,
                48_240,
                3_120,
                0.42,
                Mark::Idle,
            ),
        ];
        for (harness, title, context, limit, input, output, cost, mark) in spent {
            let Some(pane) = shell
                .store
                .panes
                .iter()
                .find(|pane| pane.harness == harness)
                .cloned()
            else {
                continue;
            };
            let account = (harness == "claude" && several)
                .then(|| shell_profile(&shell.store))
                .flatten();
            if let Some(saved) = shell.store.pane_mut(&pane.id) {
                saved.kind = PaneKind::Thread;
                saved.title = title.to_owned();
                saved.profile = account;
            }
            let saved = shell.store.pane(&pane.id).cloned().unwrap_or(pane);
            let (backend, end) = kou_thread::channel();
            shell.install_thread(&saved, backend, title.into(), window, cx);
            let _ = end
                .updates
                .try_send(kou_thread::Update::Usage(kou_thread::Usage {
                    context_used: Some(context),
                    context_limit: Some(limit),
                    input_tokens: Some(input),
                    output_tokens: Some(output),
                    cost_usd: Some(cost),
                }));
            std::mem::forget(end);
            if let Some(live) = shell.live.get_mut(&saved.id) {
                live.mark = mark;
                live.seen = true;
            }
        }
        // The page as it stands once its bars have filled.
        shell.usage_open = true;
        shell.usage_now = Some(now);
        cx.notify();
    })
}

// The first account added to the fixture's store, if any.
fn shell_profile(store: &Store) -> Option<String> {
    store.profiles.first().map(|profile| profile.id.clone())
}

fn harnesses() -> Vec<HarnessInfo> {
    let harness =
        |id: &str, name: &str, code: &str, version: Option<&str>, message: Option<&str>| {
            HarnessInfo {
                id: id.into(),
                name: name.into(),
                code: code.into(),
                ready: version.is_some() && message.is_none(),
                version: version.map(str::to_owned),
                message: message.map(str::to_owned),
                native: matches!(id, "kou" | "opencode"),
                adapter: None,
            }
        };
    vec![
        harness("claude", "Claude Code", "CC", Some("2.1.322"), None),
        harness("codex", "Codex", "CX", Some("0.160.0"), None),
        harness("opencode", "OpenCode", "OC", Some("2.2.5"), None),
        harness("kou", "kou", "K", Some("fixture"), None),
        harness("shell", "Shell", "SH", Some("zsh"), None),
        harness("gemini", "Gemini CLI", "GM", None, None),
        harness(
            "kiro",
            "Kiro",
            "KR",
            None,
            Some("its command is a link to an app that is gone"),
        ),
    ]
}

// The chat of the `shell-chat-*` scenes: Claude Code's pane as a chat with
// models, modes (efforts for `shell-chat-effort`) and figures, at work but
// for `shell-chat-effort`; then, once the conversation is in, what the
// scene types, a line at a time (a line that ends in Enter is sent, which
// at work queues it).
fn chat_at_work(name: &str, shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    use kou_thread::{
        Block, Capabilities, Choice, Provider, Stop, ToolCall, ToolKind, ToolStatus, Update, Usage,
    };
    let Some(pane) = shell
        .store
        .panes
        .iter()
        .find(|pane| pane.harness == "claude")
        .cloned()
    else {
        return;
    };
    if let Some(saved) = shell.store.pane_mut(&pane.id) {
        saved.kind = PaneKind::Thread;
    }
    let (backend, end) = kou_thread::channel();
    shell.install_thread(&pane, backend, "Claude Code".into(), window, cx);
    let effort = name == "shell-chat-effort";
    // (Levels of effort apart from the modes, which are Claude Code's own.)
    let efforts = name.starts_with("shell-chat-efforts");
    // (The model picker's scenes with a second provider.)
    let two = name.starts_with("shell-chat-model-");
    let choice = |id: &str, name: &str| Choice {
        id: id.into(),
        name: name.into(),
        description: None,
    };
    let modes = if effort {
        ["low", "medium", "high", "xhigh"]
            .map(|level| choice(level, &format!("Effort: {level}")))
            .to_vec()
    } else if efforts {
        claude_modes()
    } else {
        vec![
            choice("ask", "Ask before edits"),
            choice("auto", "Auto-edit"),
        ]
    };
    let mut updates = vec![
        Update::Ready {
            session: "0198f2c4".into(),
            capabilities: Capabilities::default(),
            modes,
            mode: Some(
                if effort {
                    "high"
                } else if efforts {
                    "default"
                } else {
                    "ask"
                }
                .into(),
            ),
            models: if two {
                vec![
                    choice("claude-opus-5-5", "Claude Opus 5.5"),
                    choice("claude-sonnet-5", "Claude Sonnet 5"),
                    choice("gpt-5.2", "GPT-5.2"),
                    Choice {
                        description: Some("Pictures from a prompt; makes media".into()),
                        ..choice("gpt-image-2", "GPT Image 2")
                    },
                ]
            } else {
                vec![
                    choice("claude-opus-5-5", "Claude Opus 5.5"),
                    choice("claude-sonnet-5", "Claude Sonnet 5"),
                ]
            },
            model: Some("claude-opus-5-5".into()),
        },
        Update::Providers(
            [
                Provider {
                    id: "anthropic".into(),
                    name: "Anthropic".into(),
                    models: vec!["claude-opus-5-5".into(), "claude-sonnet-5".into()],
                },
                Provider {
                    id: "openai".into(),
                    name: "OpenAI".into(),
                    models: vec!["gpt-5.2".into(), "gpt-image-2".into()],
                },
            ][..if two { 2 } else { 1 }]
                .to_vec(),
        ),
        Update::UserChunk(Block::Text(
            "Why does the retry queue back off twice as long as the config says?".into(),
        )),
        Update::TurnStarted,
        Update::Tool(ToolCall {
            id: "t1".into(),
            title: "Read retry/backoff.go".into(),
            kind: ToolKind::Read,
            status: ToolStatus::Done,
            ..ToolCall::default()
        }),
        Update::Usage(Usage {
            context_used: Some(48_210),
            context_limit: Some(200_000),
            input_tokens: Some(183_400),
            output_tokens: Some(9_812),
            cost_usd: Some(1.37),
        }),
    ];
    if efforts {
        updates.insert(
            1,
            Update::Efforts {
                levels: [
                    ("low", "Low"),
                    ("medium", "Medium"),
                    ("high", "High"),
                    ("xhigh", "Extra high"),
                    ("max", "Max"),
                ]
                .map(|(id, name)| choice(id, name))
                .to_vec(),
                current: Some("high".into()),
            },
        );
    }
    if effort || efforts {
        updates.push(Update::AssistantChunk(
            "The delay is doubled in two places: `backoff()` shifts by the attempt number, and the caller multiplies by `factor` again."
                .into(),
        ));
        updates.push(Update::TurnEnded(Stop::Finished));
    }
    for update in updates {
        let _ = end.updates.try_send(update);
    }
    // The scene's agent never hangs up.
    std::mem::forget(end);
    shell.focus_pane(&pane.id, cx);
    let typed: &'static [&'static str] = match name {
        "shell-chat-working" => &[
            "then run the migration tests\n",
            "and say which of them failed\n",
        ],
        "shell-chat-force" => &[
            "then run the migration tests\n",
            "stop after the first failure",
        ],
        "shell-chat-slash" => &["/"],
        "shell-chat-help" => &["/help\n"],
        // (Escape closes the list of commands, where `/model` comes first.)
        "shell-chat-modes" => &["/mode\u{1b}\n"],
        "shell-chat-model" | "shell-chat-model-providers" => &["/model\u{1b}\n"],
        "shell-chat-model-find" => &["/model\u{1b}\n", "image"],
        "shell-chat-efforts-open" => &["/effort\u{1b}\n"],
        _ => &["fix the flaky retry test"],
    };
    let handle = window.window_handle();
    cx.spawn(async move |_, cx| {
        // A line at a time: what Enter does is done after the keys that
        // come with it in one go.
        for line in typed {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let os = cx.platform_info().os;
                for ch in line.chars() {
                    let key = match ch {
                        ' ' => "space".to_owned(),
                        '\n' => "enter".to_owned(),
                        '\u{1b}' => "escape".to_owned(),
                        ch => ch.to_string(),
                    };
                    if let Ok(keystroke) = gpui::Keystroke::parse_with_os(&key, os) {
                        window.dispatch_keystroke(keystroke, cx);
                    }
                }
            });
        }
    })
    .detach();
}

// Claude Code's own four modes, as it names them.
fn claude_modes() -> Vec<kou_thread::Choice> {
    [
        ("default", "Default"),
        ("acceptEdits", "Accept edits"),
        ("plan", "Plan mode"),
        ("bypassPermissions", "Bypass permissions"),
    ]
    .map(|(id, name)| kou_thread::Choice {
        id: id.into(),
        name: name.into(),
        description: None,
    })
    .to_vec()
}

// The chat of the `shell-chat-question*` scenes: Claude Code's pane as a
// chat whose turn stops on a call that asks the user (which questions, by
// the scene's name); then, once the conversation is in, the digits the
// scene presses over the empty field.
fn chat_question(name: &str, shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    use kou_thread::{
        Block, Capabilities, Choice, PermissionKind, PermissionOption, PermissionRequest, Question,
        QuestionOption, ToolCall, ToolKind, ToolStatus, Update,
    };
    let Some(pane) = shell
        .store
        .panes
        .iter()
        .find(|pane| pane.harness == "claude")
        .cloned()
    else {
        return;
    };
    if let Some(saved) = shell.store.pane_mut(&pane.id) {
        saved.kind = PaneKind::Thread;
    }
    let (backend, end) = kou_thread::channel();
    shell.install_thread(&pane, backend, "Claude Code".into(), window, cx);
    let option = |label: &str, description: &str| QuestionOption {
        label: label.into(),
        description: (!description.is_empty()).then(|| description.into()),
    };
    let store = Question {
        header: Some("Store".into()),
        text: "Where should the retry queue keep its jobs between restarts?".into(),
        options: vec![
            option("SQLite", "A file beside the binary; nothing else to run"),
            option("Postgres", "The database the service already talks to"),
            option("In memory", "Lost on a restart; enough for the tests"),
        ],
        multiple: false,
        free: false,
    };
    let checks = Question {
        header: Some("Checks".into()),
        text: "Which checks should run before the change is pushed?".into(),
        options: vec![
            option("Lint", "golangci-lint over the module"),
            option("Unit tests", "go test ./..."),
            option("Race detector", "Slower; catches the queue's data races"),
            option("Benchmarks", "Backoff timings against main"),
        ],
        multiple: true,
        free: true,
    };
    let branch = Question {
        header: Some("Branch".into()),
        text: "What should the branch for this fix be called?".into(),
        options: Vec::new(),
        multiple: false,
        free: true,
    };
    let (questions, refusals, typed): (Vec<Question>, &[(&str, &str)], &'static str) = match name {
        "shell-chat-question-several" => (vec![checks], &[], "1 3"),
        "shell-chat-question-second" => (vec![store, checks, branch], &[("skip", "Skip")], "1"),
        "shell-chat-question-words" => (vec![branch], &[], ""),
        _ => (vec![store], &[("skip", "Skip")], ""),
    };
    let asked = questions.len();
    let choice = |id: &str, name: &str| Choice {
        id: id.into(),
        name: name.into(),
        description: None,
    };
    let updates = vec![
        Update::Ready {
            session: "0198f2c4".into(),
            capabilities: Capabilities::default(),
            modes: claude_modes(),
            mode: Some("default".into()),
            models: vec![choice("claude-opus-5-5", "Claude Opus 5.5")],
            model: Some("claude-opus-5-5".into()),
        },
        Update::UserChunk(Block::Text(
            "Make the retry queue survive a restart.".into(),
        )),
        Update::TurnStarted,
        Update::Tool(ToolCall {
            id: "t1".into(),
            title: "Read retry/queue.go".into(),
            kind: ToolKind::Read,
            status: ToolStatus::Done,
            ..ToolCall::default()
        }),
        Update::AssistantChunk(
            "The queue lives in memory today. A few things to settle before I change it.".into(),
        ),
        Update::Permission(PermissionRequest {
            id: 7,
            tool: ToolCall {
                id: "t2".into(),
                title: if asked == 1 {
                    "Ask 1 question".into()
                } else {
                    format!("Ask {asked} questions")
                },
                kind: ToolKind::Other,
                status: ToolStatus::Pending,
                raw_input: Some(serde_json::json!({ "questions": asked })),
                ..ToolCall::default()
            },
            options: refusals
                .iter()
                .map(|(id, label)| PermissionOption {
                    id: (*id).into(),
                    label: (*label).into(),
                    kind: PermissionKind::RejectOnce,
                })
                .collect(),
            questions,
        }),
    ];
    for update in updates {
        let _ = end.updates.try_send(update);
    }
    // The scene's agent never hangs up.
    std::mem::forget(end);
    shell.focus_pane(&pane.id, cx);
    if typed.is_empty() {
        return;
    }
    let handle = window.window_handle();
    cx.spawn(async move |_, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
        let _ = cx.update_window(handle, |_, window, cx| {
            let os = cx.platform_info().os;
            for key in typed.split(' ') {
                if let Ok(keystroke) = gpui::Keystroke::parse_with_os(key, os) {
                    window.dispatch_keystroke(keystroke, cx);
                }
            }
        });
    })
    .detach();
}

// The chat of `shell-chat-transcript`: Claude Code's pane as a chat, its
// turn under way, the work it does after its last words (work that is
// followed by something folds into one line).
fn chat_transcript(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    use kou_thread::{
        Block, Capabilities, Level, Location, PermissionKind, PermissionOption, PermissionRequest,
        PlanStatus, PlanStep, ToolCall, ToolContent, ToolKind, ToolStatus, Update,
    };
    let Some(pane) = shell
        .store
        .panes
        .iter()
        .find(|pane| pane.harness == "claude")
        .cloned()
    else {
        return;
    };
    if let Some(saved) = shell.store.pane_mut(&pane.id) {
        saved.kind = PaneKind::Thread;
    }
    let (backend, end) = kou_thread::channel();
    shell.install_thread(&pane, backend, "Claude Code".into(), window, cx);
    let file = |name: &str| pane.cwd.join(name);
    let tool = |id: &str, title: &str, kind, status| ToolCall {
        id: id.into(),
        title: title.into(),
        kind,
        status,
        ..ToolCall::default()
    };
    let mut read = tool(
        "t1",
        "Read retry/backoff.go",
        ToolKind::Read,
        ToolStatus::Done,
    );
    read.locations = vec![Location {
        path: file("retry/backoff.go"),
        line: Some(12),
    }];
    let mut edit = tool(
        "t2",
        "Edit retry/backoff.go",
        ToolKind::Edit,
        ToolStatus::Done,
    );
    edit.content = vec![ToolContent::Diff {
        path: file("retry/backoff.go"),
        old: Some("return base << attempt * factor\n".into()),
        new: "return base << attempt\n".into(),
    }];
    let mut failed = tool(
        "t3",
        "Run go vet ./retry",
        ToolKind::Execute,
        ToolStatus::Failed,
    );
    failed.content = vec![ToolContent::Terminal {
        id: "term-1".into(),
        output: "retry/queue.go:41: unreachable code\n".into(),
        exit: Some(1),
    }];
    let mut running = tool(
        "t4",
        "Run go test ./retry/...",
        ToolKind::Execute,
        ToolStatus::Running,
    );
    running.content = vec![ToolContent::Terminal {
        id: "term-2".into(),
        output: "=== RUN   TestBackoffGrows\n--- PASS: TestBackoffGrows (0.00s)\n=== RUN   TestRequeue\n".into(),
        exit: None,
    }];
    let mut asks = tool(
        "t5",
        "Edit retry/queue.go",
        ToolKind::Edit,
        ToolStatus::Pending,
    );
    asks.content = vec![ToolContent::Diff {
        path: file("retry/queue.go"),
        old: Some("delay := backoff(n) * factor\n".into()),
        new: "delay := backoff(n)\n".into(),
    }];
    let option = |id: &str, label: &str, kind| PermissionOption {
        id: id.into(),
        label: label.into(),
        kind,
    };
    let step = |text: &str, status| PlanStep {
        text: text.into(),
        status,
    };
    let updates = vec![
        Update::Ready {
            session: "0198f2c4".into(),
            capabilities: Capabilities::default(),
            modes: Vec::new(),
            mode: None,
            models: Vec::new(),
            model: None,
        },
        Update::UserChunk(Block::Text(
            "Why does the retry queue back off twice as long as the config says?".into(),
        )),
        Update::TurnStarted,
        Update::Notice(Level::Info, "Context compacted".into()),
        Update::Notice(Level::Warning, "The model is busy: retrying in 4 s".into()),
        Update::AssistantChunk(
            "The delay is doubled in two places; the shift alone is enough.".into(),
        ),
        Update::Notice(Level::Error, "The adapter dropped a message".into()),
        Update::ThoughtChunk(
            "**Where the delay grows**\n\nThe shift and the factor both scale it.".into(),
        ),
        Update::Tool(read),
        Update::Plan(vec![
            step("Find where the delay is computed", PlanStatus::Done),
            step("Drop the second multiplication", PlanStatus::Running),
            step("Run the queue's tests", PlanStatus::Pending),
        ]),
        Update::Tool(edit),
        Update::Tool(failed),
        Update::Tool(running),
        Update::Permission(PermissionRequest {
            questions: Vec::new(),
            id: 7,
            tool: asks,
            options: vec![
                option("once", "Allow once", PermissionKind::AllowOnce),
                option("always", "Always allow edits", PermissionKind::AllowAlways),
                option("reject", "Reject", PermissionKind::RejectOnce),
            ],
        }),
    ];
    for update in updates {
        let _ = end.updates.try_send(update);
    }
    // The scene's agent never hangs up.
    std::mem::forget(end);
    shell.focus_pane(&pane.id, cx);
}

// The chat of `shell-chat-cyrillic`: a turn in Russian, with prose, a
// thought, a call, a plan, a notice, and words typed into the field.
fn chat_cyrillic(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    use kou_thread::{
        Block, Capabilities, Level, PlanStatus, PlanStep, Stop, ToolCall, ToolContent, ToolKind,
        ToolStatus, Update,
    };
    let Some(pane) = shell
        .store
        .panes
        .iter()
        .find(|pane| pane.harness == "claude")
        .cloned()
    else {
        return;
    };
    if let Some(saved) = shell.store.pane_mut(&pane.id) {
        saved.kind = PaneKind::Thread;
    }
    let (backend, end) = kou_thread::channel();
    shell.install_thread(&pane, backend, "Claude Code".into(), window, cx);
    let step = |text: &str, status| PlanStep {
        text: text.into(),
        status,
    };
    let updates = vec![
        Update::Ready {
            session: "0198f2c4".into(),
            capabilities: Capabilities::default(),
            modes: Vec::new(),
            mode: None,
            models: Vec::new(),
            model: None,
        },
        Update::UserChunk(Block::Text(
            "Почему очередь повторов ждёт вдвое дольше, чем сказано в конфиге? Проверь и \
             исправь, пожалуйста."
                .into(),
        )),
        Update::TurnStarted,
        Update::ThoughtChunk(
            "**Где растёт задержка**\n\nСдвиг и множитель оба её увеличивают.".into(),
        ),
        Update::AssistantChunk(
            "Задержка удваивается в **двух местах**: сдвиг `base << attempt` и ещё множитель \
             `factor`. Хватит *одного* сдвига.\n\n- Убираю второе умножение в `backoff.go`\n- \
             Запускаю тесты очереди\n\n## Что изменилось\n\nЩедрый объём: «ёжик», Жёлтый, \
             Цапля, Эхо, Юла, Ящик — 0123456789."
                .into(),
        ),
        Update::Tool(ToolCall {
            id: "t1".into(),
            title: "Run go test ./retry/... # проверка очереди".into(),
            kind: ToolKind::Execute,
            status: ToolStatus::Done,
            content: vec![ToolContent::Terminal {
                id: "term-1".into(),
                output: "ok  \tretry\t0.41s  — все тесты прошли\n".into(),
                exit: Some(0),
            }],
            ..ToolCall::default()
        }),
        Update::Plan(vec![
            step("Найти, где считается задержка", PlanStatus::Done),
            step("Убрать второе умножение", PlanStatus::Running),
            step("Прогнать тесты очереди", PlanStatus::Pending),
        ]),
        Update::Notice(Level::Warning, "Модель занята: повтор через 4 с".into()),
        Update::AssistantChunk("Готово: теперь задержка растёт так, как задано.".into()),
        Update::TurnEnded(Stop::Finished),
    ];
    for update in updates {
        let _ = end.updates.try_send(update);
    }
    // The scene's agent never hangs up.
    std::mem::forget(end);
    shell.focus_pane(&pane.id, cx);
    let handle = window.window_handle();
    cx.spawn(async move |_, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
        let _ = cx.update_window(handle, |_, window, cx| {
            let os = cx.platform_info().os;
            for ch in "проверь ещё раз очередь".chars() {
                let key = match ch {
                    ' ' => "space".to_owned(),
                    ch => ch.to_string(),
                };
                if let Ok(keystroke) = gpui::Keystroke::parse_with_os(&key, os) {
                    window.dispatch_keystroke(keystroke, cx);
                }
            }
        });
    })
    .detach();
}

// The chat of `shell-chat-agents`: Claude Code's pane as a chat whose
// second turn runs three subagents, beside the agents it started in panes,
// its clock held still so that their times are the same in every capture.
fn chat_agents(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
    use kou_thread::{
        AgentNote, AgentWork, Block, Capabilities, Stop, ToolCall, ToolContent, ToolKind,
        ToolStatus, Update,
    };
    use kou_thread_view::{Beside, Nested, NestedState};
    let Some(pane) = shell
        .store
        .panes
        .iter()
        .find(|pane| pane.harness == "claude")
        .cloned()
    else {
        return;
    };
    if let Some(saved) = shell.store.pane_mut(&pane.id) {
        saved.kind = PaneKind::Thread;
    }
    let (backend, end) = kou_thread::channel();
    let view = shell.install_thread(&pane, backend, "Claude Code".into(), window, cx);
    let subagent = |id: &str, title: &str, kind: &str, status| ToolCall {
        id: id.into(),
        title: title.into(),
        kind: ToolKind::Other,
        status,
        agent: Some(kind.into()),
        ..ToolCall::default()
    };
    let mut exploring = subagent(
        "a1",
        "Find every place the delay is computed",
        "Explore",
        ToolStatus::Running,
    );
    exploring.raw_input = Some(serde_json::json!({
        "prompt": "Find every place the retry delay is computed.\nList the callers of each."
    }));
    exploring.work = AgentWork {
        steps: 2,
        said: vec![AgentNote {
            after: 1,
            text: "Found `backoff()` in retry/backoff.go.".into(),
        }],
        ..AgentWork::default()
    };
    let step = |id: &str, title: &str, kind, status| ToolCall {
        id: id.into(),
        title: title.into(),
        kind,
        status,
        parent: Some("a1".into()),
        ..ToolCall::default()
    };
    let mut reviewed = subagent(
        "a2",
        "Review the queue's tests",
        "general-purpose",
        ToolStatus::Done,
    );
    reviewed.content = vec![ToolContent::Text(
        "The tests cover the doubling; one of them asserts the old curve.".into(),
    )];
    reviewed.work = AgentWork {
        steps: 5,
        tokens: Some(18_400),
        ..AgentWork::default()
    };
    let failed = subagent("a3", "Plan the migration", "Plan", ToolStatus::Failed);
    for update in [
        Update::Ready {
            session: "0198f2c4".into(),
            capabilities: Capabilities::default(),
            modes: Vec::new(),
            mode: None,
            models: Vec::new(),
            model: None,
        },
        Update::UserChunk(Block::Text(
            "Why does the retry queue back off twice as long as the config says?".into(),
        )),
        Update::TurnStarted,
        Update::AssistantChunk("The delay is doubled in two places.".into()),
        Update::TurnEnded(Stop::Finished),
        Update::UserChunk(Block::Text(
            "Fix it, and check the tests and the plan.".into(),
        )),
        Update::TurnStarted,
        Update::Tool(reviewed),
        Update::Tool(failed),
        Update::Tool(exploring),
        Update::Tool(step(
            "a1-read",
            "Read retry/backoff.go",
            ToolKind::Read,
            ToolStatus::Done,
        )),
        Update::Tool(step(
            "a1-search",
            "Search backoff(",
            ToolKind::Search,
            ToolStatus::Running,
        )),
        Update::Usage(kou_thread::Usage {
            context_used: Some(61_300),
            context_limit: Some(200_000),
            ..kou_thread::Usage::default()
        }),
    ] {
        let _ = end.updates.try_send(update);
    }
    // The scene's agent never hangs up.
    std::mem::forget(end);
    let wall = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_763_000_000);
    let agent =
        |id: &str, title: &str, harness: &str, code: &str, state, detail: &str, after| Nested {
            id: id.into(),
            title: title.into(),
            harness: harness.into(),
            code: code.into(),
            state,
            detail: detail.into(),
            since: wall - Duration::from_secs(95),
            after: Some(after),
        };
    view.update(cx, |view, cx| {
        view.pin_clock(Some(wall), cx);
        view.set_nested(
            vec![
                agent(
                    "n1",
                    "Migrate the queue to the new delay",
                    "codex",
                    "CX",
                    NestedState::Running,
                    "Rewriting retry/queue.go",
                    3,
                ),
                agent(
                    "n2",
                    "Write the changelog",
                    "opencode",
                    "OC",
                    NestedState::Finished,
                    "Added an entry under Fixed",
                    3,
                ),
                agent(
                    "n3",
                    "Bench the queue",
                    "kou",
                    "K",
                    NestedState::Asking,
                    "",
                    5,
                ),
            ],
            cx,
        );
        view.set_beside(
            Some(Beside {
                project: "atlas".into(),
                path: "/fixture/atlas".into(),
                branch: Some("fix/retry-delay".into()),
                changes: Some((42, 7)),
                parent: Some(agent(
                    "lead",
                    "Plan the release",
                    "claude",
                    "CC",
                    NestedState::Running,
                    "",
                    0,
                )),
                earlier: 2,
            }),
            cx,
        );
        view.pull_beside(true, cx);
    });
    shell.focus_pane(&pane.id, cx);
}

// The split fixture's tab as its canvas, with `shell-chat-transcript`'s chat
// in Claude Code's node and the camera on that node at `zoom`; without the
// plugins' faces when `native`.
fn transcript_on_canvas(
    shell: &mut Shell,
    zoom: f32,
    native: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    shell.set_layout(Layout::Canvas, cx);
    chat_transcript(shell, window, cx);
    if native {
        native_chats(shell, cx);
    }
    let Some(pane) = shell
        .store
        .panes
        .iter()
        .find(|pane| pane.harness == "claude")
        .cloned()
    else {
        return;
    };
    let area = shell.content_area.get().size;
    let viewport = (f32::from(area.width), f32::from(area.height));
    let Some(tab) = shell.store.tab_of(&pane.id).map(|tab| tab.id.clone()) else {
        return;
    };
    if let Some(canvas) = shell.canvas(&tab, cx) {
        canvas.update(cx, |canvas, cx| {
            if let Some(rect) = canvas.board().node(pane.node).map(|node| node.rect) {
                canvas.set_camera(kou_canvas::Camera::centre_on(&rect, viewport, zoom), cx);
            }
        });
    }
}

// Takes the plugins' faces off every chat of the shell: each draws all of
// itself, its prompt field too, as it does when no plugin has its views.
fn native_chats(shell: &mut Shell, cx: &mut Context<Shell>) {
    for talking in shell.threads.values() {
        talking.view().update(cx, |view, cx| {
            view.set_chat_face(None, cx);
            view.set_field_face(None, cx);
        });
    }
}

// The chat of `shell-chat-scroll` and `shell-chat-stream`: Claude Code's
// pane with forty turns that are over (a prompt, thinking, three calls and
// an answer each) and one under way, with a run of calls after its first
// words; drawn without the plugins when `native`. Then, every 8 ms for a
// few seconds, the transcript is scrolled up by a wheel over it, or (for
// `stream`) an answer is written under the calls a word at a time.
fn chat_long(
    native: bool,
    stream: bool,
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    use gpui::InputEvent as _;
    use kou_thread::{
        Block, Capabilities, Level, Location, PlanStatus, PlanStep, Stop, ToolCall, ToolContent,
        ToolKind, ToolStatus, Update,
    };
    let Some(pane) = shell
        .store
        .panes
        .iter()
        .find(|pane| pane.harness == "claude")
        .cloned()
    else {
        return;
    };
    if let Some(saved) = shell.store.pane_mut(&pane.id) {
        saved.kind = PaneKind::Thread;
    }
    let (backend, end) = kou_thread::channel();
    shell.install_thread(&pane, backend, "Claude Code".into(), window, cx);
    if native {
        native_chats(shell, cx);
    }
    let file = |name: &str| pane.cwd.join(name);
    // The calls of a run: a file read, an edit, a command.
    let calls = |id: String, at: usize| {
        let read = ToolCall {
            id: format!("{id}-read"),
            title: format!("Read retry/backoff_{at}.go"),
            kind: ToolKind::Read,
            status: ToolStatus::Done,
            locations: vec![Location {
                path: file(&format!("retry/backoff_{at}.go")),
                line: Some(12 + at as u32),
            }],
            ..ToolCall::default()
        };
        let edit = ToolCall {
            id: format!("{id}-edit"),
            title: format!("Edit retry/queue_{at}.go"),
            kind: ToolKind::Edit,
            status: ToolStatus::Done,
            content: vec![ToolContent::Diff {
                path: file(&format!("retry/queue_{at}.go")),
                old: Some("delay := backoff(n) * factor\n".into()),
                new: "delay := backoff(n)\n".into(),
            }],
            ..ToolCall::default()
        };
        let run = ToolCall {
            id: format!("{id}-run"),
            title: format!("Run go test ./retry/... -run TestBackoff{at}"),
            kind: ToolKind::Execute,
            status: ToolStatus::Done,
            content: vec![ToolContent::Terminal {
                id: format!("{id}-term"),
                output: "=== RUN   TestBackoffGrows\n--- PASS: TestBackoffGrows (0.00s)\nok  \tretry\t0.012s\n".into(),
                exit: Some(0),
            }],
            ..ToolCall::default()
        };
        [Update::Tool(read), Update::Tool(edit), Update::Tool(run)]
    };
    let mut updates = vec![Update::Ready {
        session: "0198f2c4".into(),
        capabilities: Capabilities::default(),
        modes: Vec::new(),
        mode: None,
        models: Vec::new(),
        model: None,
    }];
    for turn in 0..40 {
        updates.push(Update::UserChunk(Block::Text(format!(
            "Turn {turn}: why does the retry queue back off twice as long as the config says?"
        ))));
        updates.push(Update::TurnStarted);
        updates.push(Update::ThoughtChunk(
            "**Where the delay grows**\n\nThe shift and the factor both scale it.".into(),
        ));
        updates.extend(calls(format!("t{turn}"), turn));
        updates.push(Update::AssistantChunk(format!(
            "The delay is doubled in two places in turn {turn}:\n\n- `backoff()` shifts by the attempt number\n- the caller multiplies by `factor` again\n\n```go\nfunc backoff(attempt int) time.Duration {{\n    return base << attempt\n}}\n```\n\nDropping the second multiplication gives the configured curve."
        )));
        updates.push(Update::TurnEnded(Stop::Finished));
    }
    updates.push(Update::UserChunk(Block::Text(
        "Now fix every caller and run the whole suite.".into(),
    )));
    updates.push(Update::TurnStarted);
    updates.push(Update::AssistantChunk(
        "Going through the callers one by one.".into(),
    ));
    for call in 0..6 {
        updates.extend(calls(format!("last{call}"), 100 + call));
    }
    updates.push(Update::Plan(vec![
        PlanStep {
            text: "Find every caller".into(),
            status: PlanStatus::Done,
        },
        PlanStep {
            text: "Drop the second multiplication".into(),
            status: PlanStatus::Running,
        },
        PlanStep {
            text: "Run the whole suite".into(),
            status: PlanStatus::Pending,
        },
    ]));
    updates.push(Update::Notice(Level::Info, "Context compacted".into()));
    for update in updates {
        let _ = end.updates.try_send(update);
    }
    shell.focus_pane(&pane.id, cx);
    let (handle, over) = (window.window_handle(), shell.content_area.get().center());
    cx.spawn(async move |_, cx| {
        for step in 0..600 {
            cx.background_executor()
                .timer(Duration::from_millis(8))
                .await;
            if stream {
                let word = if step % 40 == 39 {
                    "\n\nAnd".to_owned()
                } else {
                    format!(" word{step}")
                };
                if end.updates.try_send(Update::AssistantChunk(word)).is_err() {
                    break;
                }
                continue;
            }
            let wheel = gpui::ScrollEvent {
                position: over,
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(12.))),
                modifiers: gpui::Modifiers::none(),
                touch_phase: gpui::TouchPhase::Moved,
                source: Default::default(),
            };
            let sent = cx.update_window(handle, |_, window, cx| {
                window.dispatch_event(wheel.to_platform_input(), cx);
            });
            if sent.is_err() {
                break;
            }
        }
        // The scene's agent never hangs up.
        std::mem::forget(end);
    })
    .detach();
}

// Shows the fixture's tab as its canvas with a scripted conversation in the
// kou node, the camera on that node at `zoom`. Returns the canvas and the node.
fn thread_on_canvas(
    shell: &mut Shell,
    zoom: f32,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Option<(Entity<CanvasView>, u64)> {
    shell.set_layout(Layout::Canvas, cx);
    let pane = shell
        .store
        .panes
        .iter()
        .find(|pane| pane.harness == "kou")
        .cloned()?;
    let (backend, end) = kou_thread::channel();
    shell.install_thread(&pane, backend, "kou".into(), window, cx);
    for update in conversation() {
        let _ = end.updates.try_send(update);
    }
    // The scene's agent never hangs up.
    std::mem::forget(end);
    let area = shell.content_area.get().size;
    let viewport = (f32::from(area.width), f32::from(area.height));
    let tab = shell.store.tab_of(&pane.id).map(|tab| tab.id.clone())?;
    let canvas = shell.canvas(&tab, cx)?;
    canvas.update(cx, |canvas, cx| {
        if let Some(rect) = canvas.board().node(pane.node).map(|node| node.rect) {
            canvas.set_camera(kou_canvas::Camera::centre_on(&rect, viewport, zoom), cx);
        }
    });
    Some((canvas, pane.node))
}

// A short exchange with prose, a list, code and a tool call in it.
fn conversation() -> Vec<kou_thread::Update> {
    use kou_thread::{Block, Capabilities, Stop, ToolCall, ToolKind, ToolStatus, Update};
    vec![
        Update::Ready {
            session: "0198f2c4".into(),
            capabilities: Capabilities::default(),
            modes: Vec::new(),
            mode: None,
            models: Vec::new(),
            model: None,
        },
        Update::UserChunk(Block::Text(
            "Why does the retry queue back off twice as long as the config says?".into(),
        )),
        Update::TurnStarted,
        Update::Tool(ToolCall {
            id: "t1".into(),
            title: "Read retry/backoff.go".into(),
            kind: ToolKind::Read,
            status: ToolStatus::Done,
            ..ToolCall::default()
        }),
        Update::AssistantChunk(
            "The delay is doubled in two places:\n\n- `backoff()` shifts by the attempt number\n- the caller multiplies by `factor` again\n\n```go\nfunc backoff(attempt int) time.Duration {\n    return base << attempt\n}\n```\n\nDropping the second multiplication gives the configured curve."
                .into(),
        ),
        Update::TurnEnded(Stop::Finished),
    ]
}

// The fixture with its three agents in one tab: the lead on the left, the two
// it started one over the other on the right. Returns the one at the top right.
fn split_fixture() -> (Store, String) {
    let (store, _, _, beside) = fixture_with(true);
    (store, beside)
}

// Two spaces, a few sessions, and one session with three agents in different
// states, each in a tab of its own.
fn fixture() -> (Store, String, String) {
    let (store, session, lead, _) = fixture_with(false);
    (store, session, lead)
}

// The store, its session with the agents, the lead, and the first it started.
fn fixture_with(split: bool) -> (Store, String, String, String) {
    use crate::state::{Axis, Seat};
    let mut store = Store::default();
    let atlas = store.add_space("/fixture/atlas".into());
    let parser = store.add_session(&atlas).unwrap();
    store.session_mut(&parser).unwrap().name = "parser".into();
    let migrations = store.add_session(&atlas).unwrap();
    store.session_mut(&migrations).unwrap().name = "migrations".into();
    let old = store.add_session(&atlas).unwrap();
    store.session_mut(&old).unwrap().name = "spike".into();
    store.session_mut(&old).unwrap().archived = true;
    let relay = store.add_space("/fixture/relay".into());
    let review = store.add_session(&relay).unwrap();
    store.session_mut(&review).unwrap().name = "review".into();

    // In one tab the three share a board, wired by who started whom; apart,
    // each tab's board has its one node.
    let family = [
        ("claude", "Claude Code", "CC", PaneKind::Terminal),
        ("codex", "Codex", "CX", PaneKind::Terminal),
        ("kou", "kou", "K", PaneKind::Thread),
    ];
    let cwd = std::path::PathBuf::from("/fixture/atlas");
    let mut panes: Vec<String> = Vec::new();
    let (shared, nodes) = board_for(&[
        ("Claude Code", "CC", None),
        ("Codex", "CX", Some(0)),
        ("kou", "K", Some(0)),
    ]);
    let mut shared = Some(shared);
    for (index, (harness, title, code, kind)) in family.into_iter().enumerate() {
        let (node, seat) = match (split, index) {
            (true, 0) => (nodes[0], Seat::Tab(shared.take().unwrap_or_default())),
            (true, 1) => (
                nodes[1],
                Seat::Beside {
                    at: panes[0].clone(),
                    axis: Axis::Row,
                },
            ),
            (true, _) => (
                nodes[index],
                Seat::Beside {
                    at: panes[1].clone(),
                    axis: Axis::Column,
                },
            ),
            (false, _) => {
                let (board, nodes) = board_for(&[(title, code, None)]);
                (nodes[0], Seat::Tab(board))
            }
        };
        let parent = panes.first().cloned();
        panes.push(store.add_pane(
            &migrations,
            harness,
            title,
            cwd.clone(),
            kind,
            parent,
            node,
            seat,
        ));
    }
    let (board, nodes) = board_for(&[("OpenCode", "OC", None)]);
    store.add_pane(
        &review,
        "opencode",
        "OpenCode",
        "/fixture/relay".into(),
        PaneKind::Terminal,
        None,
        nodes[0],
        Seat::Tab(board),
    );
    store.focus(&panes[0]);
    store.selected = Some(migrations.clone());
    (store, migrations, panes[0].clone(), panes[1].clone())
}

// The split fixture's tab as its canvas, and `act` done to the canvas; with
// the canvas's own chrome rather than its plugin's when `native`.
fn on_canvas(
    native: bool,
    cx: &mut App,
    act: impl FnOnce(&mut CanvasView, &mut Window, &mut Context<CanvasView>) + 'static,
) -> Scene {
    shell(split_fixture().0, cx, move |shell, window, cx| {
        shell.set_layout(Layout::Canvas, cx);
        let tab = shell
            .store
            .current()
            .and_then(|session| session.front())
            .map(|tab| tab.id.clone());
        if let Some(canvas) = tab.and_then(|tab| shell.canvas(&tab, cx)) {
            canvas.update(cx, |canvas, cx| {
                if native {
                    canvas.set_face(None, cx);
                }
                act(canvas, window, cx);
            });
        }
    })
}

fn shell(
    store: Store,
    cx: &mut App,
    act: impl FnOnce(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
) -> Scene {
    let view: Entity<Shell> = cx.new(|cx| {
        let mut shell = Shell::new(store, None, cx);
        shell.harnesses = harnesses();
        // States a rail has to tell apart, with ages that do not depend on the clock.
        let marks = [
            (Mark::Working, 40, true),
            (Mark::Blocked, 200, true),
            (Mark::Idle, 4000, false),
            (Mark::Idle, 9000, true),
        ];
        let panes: Vec<String> = shell
            .store
            .panes
            .iter()
            .map(|pane| pane.id.clone())
            .collect();
        for (pane, (mark, seconds, seen)) in panes.iter().zip(marks) {
            if let Some(live) = shell.live.get_mut(pane) {
                live.mark = mark;
                live.seen = seen;
                live.changed = Instant::now() - Duration::from_secs(seconds);
            }
        }
        shell
    });
    let handle = view.clone();
    Scene {
        describe: Box::new(|_| Vec::new()),
        view: view.into(),
        act: Box::new(move |time, window, cx| {
            handle.update(cx, |shell, cx| {
                shell.scene_time = time;
                act(shell, window, cx);
                // A negative time leaves the canvas running, for a strip of
                // what it does in real time.
                let front = shell
                    .store
                    .current()
                    .and_then(|session| session.front())
                    .map(|tab| tab.id.clone());
                if time >= 0.
                    && let Some(tab) = front
                    && let Some(canvas) = shell.canvas(&tab, cx)
                {
                    canvas.update(cx, |canvas, cx| canvas.freeze_at(time, window, cx));
                }
                cx.notify();
            });
        }),
    }
}
