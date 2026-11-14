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
    "shell-settings-accounts",
    "shell-settings",
    "shell-settings-accent",
    "shell-settings-refused",
    "shell-sessions",
    "shell-tab-editor",
    "shell-rail-rename",
    "shell-new-tab",
    "shell-new-tab-list",
    "shell-new-tab-models",
    "shell-harness-thread",
    "shell-harness-beside",
    "shell-split-choosing",
    "shell-new-tab-behind",
    "shell-canvas-thread-60",
    "shell-canvas-thread-100",
    "shell-canvas-thread-150",
    "motion-canvas-thread-zoom",
    "shell-tab-canvases",
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
            shell
                .store
                .add_profile("claude", "/Users/me/.claude-work".into());
            shell.settings_open = true;
            cx.notify();
        }),
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
