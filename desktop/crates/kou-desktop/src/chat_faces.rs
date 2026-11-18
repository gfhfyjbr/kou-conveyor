//! The parts of a chat that the app's plugins draw, through the hook a chat
//! lends the app for them ([`kou_thread_view::ChatFace`]). The prompt area's
//! are views of the `composer` plugin (`plugins/composer`), beside the
//! `composer` view that draws the field's frame through a hook of its own
//! ([`Shell::field_face`]): the line over the field while its agent works
//! (`composer-activity`), the queue's tray (`composer-queue`) and the
//! figures under the field (`composer-usage`).
//!
//! A view is given `{ theme, data }`, `data` being what the chat hands over
//! with `slots` added, the names of the pieces the chat still draws itself
//! (`ui.slot(name)` places one). What a drawing asks with `api.act(name,
//! args)` goes back to the chat that drew it.
//!
//! A part whose view no plugin has, whose script fails before it ever drew
//! it, or that is asked for while something else has the plugins, is drawn
//! by the chat itself, as it always was.
//!
//! A row of the transcript is drawn at every frame it is on screen, nearly
//! always as it was. A part the chat hands over with a digest of what its
//! data is made from ([`ChatPart::memo`]) is drawn, while that digest, the
//! names of its slots and the theme are what they were and the host still
//! has the tree drawn from them, from that tree: its data is not made, and
//! no input is written out and hashed for the host to look up.

use crate::{
    plugin_view::{Drawing, On, element, theme_json},
    shell::Shell,
};
use gpui::Context;
use kou_plugins::Node;
use kou_thread_view::{ChatFace, ChatPart};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    hash::{Hash as _, Hasher as _},
    rc::Rc,
};

/// A part as its view last drew it: from what (the chat's digest of its
/// data with the names of its slots, and the theme), and the tree built.
struct Kept {
    made_from: u64,
    theme: (kou_ui::Palette, bool),
    tree: Rc<Node>,
    /// The input it was drawn from: in tests, a part drawn from its digest
    /// is checked to be drawn from the same.
    #[cfg(test)]
    data: Value,
}

/// How many parts are kept at most, of all a chat has drawn: past it, all
/// are forgotten and drawn again from their data.
const KEPT: usize = 1024;

#[cfg(test)]
thread_local! {
    /// How many times the data of a part was made, in tests.
    static MADE: Cell<u64> = const { Cell::new(0) };
}

impl Shell {
    /// What draws the parts of a chat that the app's plugins draw.
    pub(crate) fn chat_face(&mut self, cx: &Context<Self>) -> ChatFace {
        self.start_plugins();
        let (plugins, shell) = (self.plugins.clone(), cx.weak_entity());
        let kept: Rc<RefCell<HashMap<String, Kept>>> = Rc::default();
        // The theme as scripts are given it, made once for each theme.
        let themed: RefCell<Option<((kou_ui::Palette, bool), Value)>> = RefCell::default();
        Rc::new(move |part: ChatPart, cx| {
            let ChatPart {
                view,
                key,
                memo,
                data,
                slots,
                act,
            } = part;
            let theme = kou_ui::theme(cx);
            let stamp = (theme.palette, theme.dark);
            let made_from = memo.map(|memo| {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                memo.hash(&mut hasher);
                for (name, _) in &slots {
                    name.hash(&mut hasher);
                }
                hasher.finish()
            });
            let tree = {
                // (Never while something else has the plugins: the chat
                // draws the part itself then.)
                let mut hosted = plugins.try_borrow_mut().ok()?;
                let host = hosted.host.as_mut().filter(|host| host.has(view))?;
                // Drawn from what it was last drawn from, it is the tree the
                // host still holds for it, if it does: no script ran for it
                // since, it was not reloaded, and nothing was kept by it.
                // (The host lets go of a tree it drew anew or forgot: then
                // the one kept here is the only hold left on it.)
                let same = made_from.and_then(|made_from| {
                    let kept = kept.borrow();
                    let last = kept.get(key.as_ref())?;
                    (last.made_from == made_from
                        && last.theme == stamp
                        && Rc::strong_count(&last.tree) > 1)
                        .then(|| last.tree.clone())
                });
                let with_slots = |mut data: Value| {
                    if let Some(given) = data.as_object_mut() {
                        let names: Vec<&str> = slots.iter().map(|(name, _)| *name).collect();
                        given.insert("slots".into(), json!(names));
                    }
                    data
                };
                match same {
                    Some(tree) => {
                        #[cfg(test)]
                        assert_eq!(
                            kept.borrow().get(key.as_ref()).map(|last| &last.data),
                            Some(&with_slots(data())),
                            "{key} was drawn from its digest, and its data is not what it was"
                        );
                        tree
                    }
                    None => {
                        let data = with_slots(data());
                        #[cfg(test)]
                        MADE.with(|made| made.set(made.get() + 1));
                        let theme_given = {
                            let mut themed = themed.borrow_mut();
                            match &*themed {
                                Some((made, given)) if *made == stamp => given.clone(),
                                _ => themed.insert((stamp, theme_json(theme))).1.clone(),
                            }
                        };
                        let input = json!({ "theme": theme_given, "data": data });
                        // (A script that fails leaves the part to the chat.)
                        let Ok(tree) = host.render_strict(view, &key, &input) else {
                            kept.borrow_mut().remove(key.as_ref());
                            return None;
                        };
                        if let Some(made_from) = made_from {
                            let mut kept = kept.borrow_mut();
                            if kept.len() >= KEPT {
                                kept.clear();
                            }
                            kept.insert(
                                key.to_string(),
                                Kept {
                                    made_from,
                                    theme: stamp,
                                    tree: tree.clone(),
                                    #[cfg(test)]
                                    data: input["data"].clone(),
                                },
                            );
                        }
                        tree
                    }
                }
            };
            let (plugins, shell, instance) = (plugins.clone(), shell.clone(), key.to_string());
            let kept = kept.clone();
            let on: On = Rc::new(move |handler, event: Value, window, cx| {
                // What is being typed in a field a plugin drew is kept first,
                // as for any other press on what a plugin drew.
                shell
                    .update(cx, |this, cx| this.commit_plugin_edit(window, cx))
                    .ok();
                let asked = match plugins.try_borrow_mut() {
                    Ok(mut hosted) => match hosted.host.as_mut() {
                        Some(host) => host.dispatch_as(view, &instance, handler, &event),
                        None => return,
                    },
                    Err(_) => return,
                };
                // What the handler kept is the part's to be drawn from: it is
                // drawn by its script again.
                kept.borrow_mut().remove(&instance);
                act(asked, window, cx);
            });
            let drawing = Drawing {
                page: key,
                theme,
                made: Cell::new(0),
                on,
                aids: None,
                root: None,
                typing: None,
                slots: RefCell::new(slots),
                dropped: None,
            };
            Some(element(&tree, &drawing))
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        plugin_view::theme_json,
        shell::{Shell, board_for},
        state::{PaneKind, Seat, Store},
    };
    use gpui::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext};
    use kou_plugins::{Host, Source};
    use kou_thread::{
        BackendEnd, Block, Capabilities, Choice, Command, ThreadState, Update, Usage,
    };
    use kou_thread_view::ThreadView;
    use serde_json::{Value, json};

    /// A window with Claude Code's pane as a chat, its agent at the other
    /// end of the channel returned.
    fn open(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext, BackendEnd) {
        open_in(cx, "/fixture/atlas".into())
    }

    /// The same, the pane working in `cwd`.
    fn open_in(
        cx: &mut TestAppContext,
        cwd: std::path::PathBuf,
    ) -> (Entity<Shell>, &mut VisualTestContext, BackendEnd) {
        cx.update(|cx| {
            kou_ui::init(cx);
            cx.set_global(kou_ui::Theme::new(true));
            crate::shell::bind_keys(cx);
            // Tests have no frames for animations to run on: what moves
            // takes its end at once (the queue's tray grows in, say).
            cx.set_reduce_motion(true);
        });
        let mut store = Store::default();
        let space = store.add_space("/fixture/atlas".into());
        let session = store.add_session(&space).unwrap();
        store.selected = Some(session.clone());
        let (board, nodes) = board_for(&[("Claude Code", "CC", None)]);
        let pane = store.add_pane(
            &session,
            "claude",
            "Claude Code",
            cwd,
            PaneKind::Thread,
            None,
            nodes[0],
            Seat::Tab(board),
        );
        store.focus(&pane);
        let (shell, cx) = cx.add_window_view(|_, cx| {
            let mut shell = Shell::new(store, None, cx);
            // (Nothing of the app's own folder is touched.)
            shell.home =
                std::env::temp_dir().join(format!("kou-chat-faces-{}", std::process::id()));
            shell
        });
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(800.)));
        let (backend, end) = kou_thread::channel();
        shell.update_in(cx, |shell, window, cx| {
            let saved = shell.store.pane(&pane).unwrap().clone();
            shell.install_thread(&saved, backend, "Claude Code".into(), window, cx);
            shell.focus_pane(&pane, cx);
        });
        draw(cx);
        (shell, cx, end)
    }

    // Two frames: the first measures, the second lays out in what it found.
    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..2 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        cx.run_until_parked();
    }

    // What leaves the queue goes over a moment: the clock is moved past it.
    fn leave(cx: &mut VisualTestContext) {
        draw(cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(400));
        draw(cx);
    }

    fn choice(id: &str, name: &str) -> Choice {
        Choice {
            id: id.into(),
            name: name.into(),
            description: None,
        }
    }

    /// The agent at work, with models, modes and figures to show.
    fn at_work(end: &BackendEnd, cx: &mut VisualTestContext) {
        for update in [
            Update::Ready {
                session: "s-1".into(),
                capabilities: Capabilities::default(),
                modes: vec![
                    choice("ask", "Ask before edits"),
                    choice("auto", "Auto-edit"),
                ],
                mode: Some("ask".into()),
                models: vec![choice("opus", "Opus"), choice("sonnet", "Sonnet")],
                model: Some("opus".into()),
            },
            Update::UserChunk(Block::Text("why is it slow".into())),
            Update::TurnStarted,
            Update::Usage(Usage {
                context_used: Some(48_210),
                context_limit: Some(200_000),
                input_tokens: Some(183_400),
                output_tokens: Some(9_812),
                cost_usd: Some(1.37),
            }),
        ] {
            end.updates.try_send(update).unwrap();
        }
        cx.run_until_parked();
        draw(cx);
    }

    fn heard(end: &BackendEnd) -> Vec<Command> {
        std::iter::from_fn(|| end.commands.try_recv().ok()).collect()
    }

    /// The parts of the prompt area, where they are drawn now.
    fn places(cx: &mut VisualTestContext) -> Vec<(&'static str, Option<Bounds<Pixels>>)> {
        [
            "frame",
            "attach",
            "model-picker",
            "mode-picker",
            "stop",
            "send",
            "force",
            // (The button that queues, or else the queue's tray.)
            "queue",
            "activity",
            "activity-meter",
            "usage",
            "queue-count",
            "queue-pause",
            "queue-resume",
            "queue-clear",
        ]
        .into_iter()
        .map(|part| (part, cx.debug_bounds(part)))
        .collect()
    }

    #[gpui::test]
    fn the_prompt_area_is_drawn_by_the_composer_plugin_where_the_chat_drew_it_and_works_as_it_did(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        at_work(&end, cx);
        // The app's own `composer` plugin draws the field's frame, its
        // controls, the line over it and the figures under it.
        assert!(shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            [
                "composer",
                "composer-activity",
                "composer-queue",
                "composer-usage",
            ]
            .iter()
            .all(|view| host.has(view))
                && !hosted.fields.is_empty()
                && host.problems().is_empty()
        }));
        for part in [
            "frame",
            "attach",
            "model-picker",
            "mode-picker",
            "stop",
            "activity",
            "usage",
        ] {
            assert!(cx.debug_bounds(part).is_some(), "{part}");
        }
        // The mode's control opens its list over it, and a choice there is
        // the agent's mode.
        let mode = cx.debug_bounds("mode-picker").unwrap();
        cx.simulate_click(mode.center(), gpui::Modifiers::none());
        draw(cx);
        let second = cx.debug_bounds("choice-1").expect("the modes' list");
        assert!(
            second.origin.y < mode.origin.y,
            "the list stands over the control"
        );
        cx.simulate_click(second.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(heard(&end), [Command::SetMode("auto".into())]);
        let busy = places(cx);
        // A prompt written while the agent works waits in the queue, whose
        // tray is the plugin's and whose tools work.
        cx.simulate_input("then run the tests");
        draw(cx);
        assert!(cx.debug_bounds("force").is_some() && cx.debug_bounds("queue-count").is_none());
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(cx.debug_bounds("queue-count").is_some());
        let pause = cx.debug_bounds("queue-pause").expect("the queue's pause");
        cx.simulate_click(pause.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("queue-resume").is_some(), "paused");
        let queued = places(cx);
        let clear = cx.debug_bounds("queue-clear").unwrap();
        cx.simulate_click(clear.center(), gpui::Modifiers::none());
        leave(cx);
        assert!(cx.debug_bounds("queue-count").is_none(), "cleared");
        // The button that stops the turn does.
        let stop = cx.debug_bounds("stop").unwrap();
        cx.simulate_click(stop.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(heard(&end), [Command::Cancel]);
        // With no plugins to draw them, the chat draws every part itself,
        // in the same places.
        shell.update(cx, |shell, _| shell.plugins.borrow_mut().host = None);
        // (The queue is still paused: clearing it left it so.)
        cx.simulate_input("then run the tests");
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(cx.debug_bounds("queue-resume").is_some());
        assert_eq!(places(cx), queued);
        let clear = cx.debug_bounds("queue-clear").unwrap();
        cx.simulate_click(clear.center(), gpui::Modifiers::none());
        leave(cx);
        assert_eq!(places(cx), busy);
        // The modes' list opens where it did over the plugin's control.
        let mode = cx.debug_bounds("mode-picker").unwrap();
        cx.simulate_click(mode.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(cx.debug_bounds("choice-1"), Some(second));
        cx.simulate_keystrokes("escape");
        draw(cx);
        // And the turn ends the same way: the line over the field goes.
        end.updates
            .try_send(Update::TurnEnded(kou_thread::Stop::Finished))
            .unwrap();
        cx.run_until_parked();
        draw(cx);
        assert!(cx.debug_bounds("activity").is_none());
        assert_eq!(
            shell.read_with(cx, |shell, cx| shell
                .threads
                .values()
                .next()
                .map(|talking| { talking.view().read(cx).thread().state().clone() })),
            Some(ThreadState::Idle)
        );
        std::mem::forget(end);
    }

    /// A user's `composer` whose figures under the field are a word of its
    /// own, and whose other parts are left to the chat.
    const MINE: &[kou_plugins::File] = &[
        (
            "composer",
            "plugin.json",
            r#"{"name": "composer", "desktop": {"script": "desktop/main.js"}}"#,
        ),
        (
            "composer",
            "desktop/main.js",
            r#"kou.view({ id: "composer-usage", render: (ctx) => ui.text(
                 ctx.data.figures.map((figure) => figure.key + " " + figure.value).join(", "),
                 { id: "my-usage" }) });"#,
        ),
    ];

    #[gpui::test]
    fn a_part_is_drawn_by_the_plugin_that_has_its_view_and_by_the_chat_where_none_has(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        shell.update(cx, |shell, _| {
            shell.plugins.borrow_mut().host =
                Some(Host::new(vec![Source::Embedded(MINE)]).unwrap());
        });
        at_work(&end, cx);
        // The figures are the plugin's; the rest, which it has no view of,
        // is the chat's own.
        let said = shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            assert!(host.has("composer-usage") && !host.has("composer-activity"));
            host.problems().to_vec()
        });
        assert_eq!(said, []);
        assert!(cx.debug_bounds("my-usage").is_some());
        assert!(cx.debug_bounds("usage").is_none());
        assert!(cx.debug_bounds("activity").is_some() && cx.debug_bounds("frame").is_some());
        // A script that fails before it ever drew the part leaves it to the
        // chat.
        const BROKEN: &[kou_plugins::File] = &[
            (
                "composer",
                "plugin.json",
                r#"{"name": "composer", "desktop": {"script": "desktop/main.js"}}"#,
            ),
            (
                "composer",
                "desktop/main.js",
                r#"kou.view({ id: "composer-usage", render: (ctx) => ctx.data.nothing.here });"#,
            ),
        ];
        shell.update(cx, |shell, _| {
            shell.plugins.borrow_mut().host =
                Some(Host::new(vec![Source::Embedded(BROKEN)]).unwrap());
        });
        end.updates
            .try_send(Update::Usage(Usage {
                cost_usd: Some(2.5),
                ..Usage::default()
            }))
            .unwrap();
        cx.run_until_parked();
        draw(cx);
        assert!(cx.debug_bounds("my-usage").is_none());
        assert!(cx.debug_bounds("usage").is_some());
        std::mem::forget(end);
    }

    /// A transcript with every kind of entry, its last run of work open:
    /// a prompt, notices, an answer, thinking, tool calls done, failed,
    /// running and waiting on the user, and a plan.
    fn transcript(end: &BackendEnd, cx: &mut VisualTestContext) {
        use kou_thread::{
            Level, Location, PermissionKind, PermissionOption, PermissionRequest, PlanStatus,
            PlanStep, ToolCall, ToolContent, ToolKind, ToolStatus,
        };
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
            path: "/fixture/atlas/retry/backoff.go".into(),
            line: Some(12),
        }];
        let mut edit = tool(
            "t2",
            "Edit retry/backoff.go",
            ToolKind::Edit,
            ToolStatus::Done,
        );
        edit.content = vec![ToolContent::Diff {
            path: "/fixture/atlas/retry/backoff.go".into(),
            old: Some("a\n".into()),
            new: "b\n".into(),
        }];
        let mut failed = tool("t3", "Run go vet", ToolKind::Execute, ToolStatus::Failed);
        failed.content = vec![ToolContent::Terminal {
            id: "term-1".into(),
            output: "unreachable code\n".into(),
            exit: Some(1),
        }];
        let mut running = tool("t4", "Run go test", ToolKind::Execute, ToolStatus::Running);
        running.content = vec![ToolContent::Terminal {
            id: "term-2".into(),
            output: "=== RUN   TestBackoff\n".into(),
            exit: None,
        }];
        let asks = tool(
            "t5",
            "Edit retry/queue.go",
            ToolKind::Edit,
            ToolStatus::Pending,
        );
        let option = |id: &str, label: &str, kind| PermissionOption {
            id: id.into(),
            label: label.into(),
            kind,
        };
        let step = |text: &str, status| PlanStep {
            text: text.into(),
            status,
        };
        for update in [
            Update::Ready {
                session: "s-1".into(),
                capabilities: Capabilities::default(),
                modes: Vec::new(),
                mode: None,
                models: Vec::new(),
                model: None,
            },
            Update::UserChunk(Block::Text("why is the retry slow".into())),
            Update::TurnStarted,
            Update::Notice(Level::Info, "Context compacted".into()),
            Update::Notice(Level::Warning, "The model is busy".into()),
            Update::AssistantChunk("The delay is doubled.".into()),
            Update::Notice(Level::Error, "The adapter dropped a message".into()),
            Update::ThoughtChunk("**Where it grows**\n\nThe shift and the factor.".into()),
            Update::Tool(read),
            Update::Plan(vec![
                step("Find the delay", PlanStatus::Done),
                step("Drop the factor", PlanStatus::Running),
                step("Run the tests", PlanStatus::Pending),
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
                    option("reject", "Reject", PermissionKind::RejectOnce),
                ],
            }),
        ] {
            end.updates.try_send(update).unwrap();
        }
        cx.run_until_parked();
    }

    /// Where the transcript's entries stand: each kind's own mark, and the
    /// ones under them, which stand lower if anything over them is taller.
    fn rows(cx: &mut VisualTestContext) -> Vec<(&'static str, Option<Bounds<Pixels>>)> {
        [
            "reply-3",
            "copy-answer-3",
            "thought-5",
            "tool-6",
            "tool-8",
            "tool-9",
            "tool-10",
            "tool-11",
            "answer-once",
            "answer-reject",
            "frame",
        ]
        .into_iter()
        .map(|row| (row, cx.debug_bounds(row)))
        .collect()
    }

    #[gpui::test]
    fn the_transcript_s_entries_are_drawn_by_the_chat_plugin_as_tall_as_the_chat_draws_them(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(1600.)));
        transcript(&end, cx);
        // The first frame that shows them: drawn by the app's own `chat`
        // plugin, the tool calls known by their own ids.
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let first = rows(cx);
        assert!(first.iter().all(|(_, at)| at.is_some()), "{first:?}");
        let view = shell.read_with(cx, |shell, _| {
            shell
                .threads
                .values()
                .next()
                .unwrap()
                .view()
                .entity_id()
                .as_u64()
        });
        shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            assert_eq!(host.problems(), []);
            for (part, key) in [
                ("chat-tool", format!("chat-tool:{view}:t1")),
                ("chat-tool", format!("chat-tool:{view}:t5")),
                ("chat-plan", format!("chat-plan:{view}:7")),
                ("chat-notice", format!("chat-notice:{view}:1")),
                ("chat-thought", format!("chat-thought:{view}:5")),
                ("chat-answer-head", format!("chat-answer-head:{view}:3")),
                ("chat-user-head", format!("chat-user-head:{view}:0")),
                ("chat-user-card", format!("chat-user-card:{view}:0")),
            ] {
                assert!(host.drawn(part, &key).is_some(), "{key}");
            }
        });
        // Laid out again, nothing moves: a row is as tall as it was measured.
        draw(cx);
        assert_eq!(rows(cx), first);
        // A card opened and closed by its head comes back as it was.
        let head = cx.debug_bounds("tool-8").unwrap();
        cx.simulate_click(head.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("tool-9").unwrap().origin.y > first[5].1.unwrap().origin.y);
        let head = cx.debug_bounds("tool-8").unwrap();
        cx.simulate_click(head.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(rows(cx), first);
        // The chat draws them where the plugin did.
        shell.update(cx, |shell, cx| {
            shell.plugins.borrow_mut().host = None;
            if let Some(talking) = shell.threads.values().next() {
                talking
                    .view()
                    .update(cx, |view, cx| view.set_chat_face(None, cx));
            }
        });
        draw(cx);
        assert_eq!(rows(cx), first);
        // What a call waits for is answered from the plugin's buttons too.
        std::mem::forget(end);
    }

    #[gpui::test]
    fn a_run_of_work_folds_into_a_line_the_plugin_draws_where_the_chat_does(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(1600.)));
        transcript(&end, cx);
        end.updates
            .try_send(Update::TurnEnded(kou_thread::Stop::Finished))
            .unwrap();
        cx.run_until_parked();
        draw(cx);
        let line = cx.debug_bounds("worked-5").expect("the run's line");
        assert!(cx.debug_bounds("tool-6").is_none(), "folded");
        // A click opens the run under it, and another closes it.
        cx.simulate_click(line.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("tool-6").is_some(), "opened");
        cx.simulate_click(line.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("tool-6").is_none(), "closed");
        // The chat draws it where the plugin did.
        shell.update(cx, |shell, _| shell.plugins.borrow_mut().host = None);
        if let Some(view) = shell.read_with(cx, |shell, _| {
            shell
                .threads
                .values()
                .next()
                .map(|talking| talking.view().clone())
        }) {
            view.update(cx, |view, cx| view.set_chat_face(None, cx));
        }
        draw(cx);
        assert_eq!(cx.debug_bounds("worked-5"), Some(line));
        std::mem::forget(end);
    }

    #[gpui::test]
    fn a_row_drawn_again_as_it_was_makes_no_data_for_its_plugin_and_looks_the_same(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(1600.)));
        transcript(&end, cx);
        draw(cx);
        let made = || super::MADE.with(std::cell::Cell::get);
        let first = rows(cx);
        // Drawn again and again with nothing changed, no part's data is
        // made: each is drawn from the tree it was drawn from last. (In
        // tests, a part drawn so is checked to have the data it had.)
        let before = made();
        for _ in 0..3 {
            draw(cx);
        }
        assert_eq!(made(), before);
        assert_eq!(rows(cx), first);
        // A card opened by its head is drawn from its new data, and so is
        // the frame of what it gave (a change to a file): they alone.
        let head = cx.debug_bounds("tool-8").unwrap();
        cx.simulate_click(head.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(made(), before + 2);
        let closed = rows(cx);
        // Another theme draws every part anew, as does a plugin read again.
        cx.update(|_, cx| cx.set_global(kou_ui::Theme::new(false)));
        draw(cx);
        let themed = made();
        assert!(themed >= before + 10, "{before} {themed}");
        shell.update(cx, |shell, _| {
            shell.plugins.borrow_mut().host = Some(
                kou_plugins::Host::new(vec![kou_plugins::Source::Embedded(
                    crate::plugins::BUILT_IN,
                )])
                .unwrap(),
            );
        });
        draw(cx);
        let reloaded = made();
        assert!(reloaded >= themed + 10, "{themed} {reloaded}");
        // And then, nothing changing, none again.
        draw(cx);
        assert_eq!(made(), reloaded);
        assert_eq!(rows(cx), closed);
        std::mem::forget(end);
    }

    /// Where the parts over the field and after the last entry stand: the
    /// first line of the list of commands, the help card, the second mode
    /// of the modes' list, and the field's frame.
    fn over_the_field(cx: &mut VisualTestContext) -> Vec<(&'static str, Option<Bounds<Pixels>>)> {
        let mut found = Vec::new();
        // `/`: the list of commands.
        cx.simulate_input("/");
        draw(cx);
        draw(cx);
        found.push(("slash-command-0", cx.debug_bounds("slash-command-0")));
        found.push(("slash-command-1", cx.debug_bounds("slash-command-1")));
        found.push(("commands-card", cx.debug_bounds("commands-card")));
        // Closed, and what was typed taken back.
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.simulate_keystrokes("backspace");
        // `/help`: the card of keys and commands.
        cx.simulate_input("/help");
        cx.simulate_keystrokes("escape enter");
        draw(cx);
        draw(cx);
        found.push(("help", cx.debug_bounds("help")));
        found.push(("help-card", cx.debug_bounds("help-card")));
        cx.simulate_keystrokes("escape");
        draw(cx);
        // `/model`: the model picker.
        cx.simulate_input("/model");
        cx.simulate_keystrokes("escape enter");
        draw(cx);
        draw(cx);
        found.push(("model-choices", cx.debug_bounds("model-choices")));
        found.push(("model-line-0", cx.debug_bounds("model-line-0")));
        found.push(("models-card", cx.debug_bounds("models-card")));
        // (Escape goes back to the providers before it closes the picker;
        // pressed once more, it would stop the turn.)
        for _ in 0..3 {
            if cx.debug_bounds("model-choices").is_none() {
                break;
            }
            cx.simulate_keystrokes("escape");
            draw(cx);
        }
        // The modes' list, from its control.
        let mode = cx.debug_bounds("mode-picker").unwrap();
        cx.simulate_click(mode.center(), gpui::Modifiers::none());
        draw(cx);
        draw(cx);
        found.push(("choice-1", cx.debug_bounds("choice-1")));
        found.push(("choices-card", cx.debug_bounds("choices-card")));
        cx.simulate_keystrokes("escape");
        draw(cx);
        found.push(("tail", cx.debug_bounds("tail")));
        found.push(("frame", cx.debug_bounds("frame")));
        found
    }

    #[gpui::test]
    fn the_lists_and_the_card_over_the_field_and_the_line_after_the_entries_are_drawn_by_the_chat_plugin_where_the_chat_draws_them(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        at_work(&end, cx);
        // Waiting on the user: the line after the last entry says so.
        end.updates
            .try_send(Update::Permission(kou_thread::PermissionRequest {
                questions: Vec::new(),
                id: 3,
                tool: kou_thread::ToolCall {
                    id: "t1".into(),
                    title: "Edit retry/queue.go".into(),
                    kind: kou_thread::ToolKind::Edit,
                    status: kou_thread::ToolStatus::Pending,
                    ..kou_thread::ToolCall::default()
                },
                options: vec![kou_thread::PermissionOption {
                    id: "once".into(),
                    label: "Allow once".into(),
                    kind: kou_thread::PermissionKind::AllowOnce,
                }],
            }))
            .unwrap();
        cx.run_until_parked();
        draw(cx);
        let drawn = over_the_field(cx);
        // Each of them the plugin's (the cards are known by ids of its own).
        for (part, at) in &drawn {
            assert!(at.is_some(), "{part}: {drawn:?}");
        }
        assert_eq!(
            shell.read_with(cx, |shell, _| shell
                .plugins
                .borrow()
                .host
                .as_ref()
                .unwrap()
                .problems()
                .to_vec()),
            []
        );
        // And the chat draws them where the plugin did.
        shell.update(cx, |shell, cx| {
            shell.plugins.borrow_mut().host = None;
            if let Some(talking) = shell.threads.values().next() {
                talking
                    .view()
                    .update(cx, |view, cx| view.set_chat_face(None, cx));
            }
        });
        draw(cx);
        let native = over_the_field(cx);
        for ((part, plugin), (_, chat)) in drawn.iter().zip(&native) {
            if part.ends_with("-card") || *part == "tail" {
                assert!(chat.is_none(), "{part} is the plugin's own");
            } else {
                assert_eq!(plugin, chat, "{part}");
            }
        }
        std::mem::forget(end);
    }

    /// Where the model picker's lines stand as it is used: its providers,
    /// one of them opened, a search over all of them, and one that finds
    /// nothing but a model ID to use.
    fn picker_lines(cx: &mut VisualTestContext) -> Vec<(String, Option<Bounds<Pixels>>)> {
        let mut found = Vec::new();
        let mut note = |stage: &str, cx: &mut VisualTestContext, parts: &[&'static str]| {
            for part in parts {
                found.push((format!("{stage} {part}"), cx.debug_bounds(part)));
            }
        };
        cx.simulate_input("/model");
        cx.simulate_keystrokes("escape enter");
        draw(cx);
        let lines = ["model-line-0", "model-line-1", "model-choices"];
        note("providers", cx, &lines);
        note("providers", cx, &["provider-anthropic", "provider-openai"]);
        cx.simulate_keystrokes("down right");
        draw(cx);
        // (One model, under the row back.)
        note(
            "opened",
            cx,
            &["model-back", "model-line-0", "model-choices"],
        );
        // The row at the top goes back to the providers.
        let back = cx.debug_bounds("model-back").unwrap();
        cx.simulate_click(back.center(), gpui::Modifiers::none());
        draw(cx);
        note("back", cx, &["provider-openai"]);
        cx.simulate_input("o");
        draw(cx);
        note("search", cx, &lines);
        cx.simulate_input("zz");
        draw(cx);
        // (No model, and the ID typed to use.)
        note("nothing", cx, &["model-line-0", "model-choices"]);
        for _ in 0..4 {
            if cx.debug_bounds("model-choices").is_none() {
                break;
            }
            cx.simulate_keystrokes("escape");
            draw(cx);
        }
        found
    }

    #[gpui::test]
    fn the_model_picker_s_lines_are_drawn_by_the_chat_plugin_where_the_chat_draws_them(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        end.updates
            .try_send(Update::Ready {
                session: "s-1".into(),
                capabilities: Capabilities::default(),
                modes: Vec::new(),
                mode: None,
                models: vec![
                    choice("anthropic/opus", "Opus"),
                    choice("anthropic/sonnet", "Sonnet"),
                    choice("openai/gpt-5", "GPT-5"),
                ],
                model: Some("anthropic/opus".into()),
            })
            .unwrap();
        cx.run_until_parked();
        draw(cx);
        let runs = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.plugins.borrow().host.as_ref().unwrap().runs()
            })
        };
        let before = runs(cx);
        let drawn = picker_lines(cx);
        assert!(drawn.iter().all(|(_, at)| at.is_some()), "{drawn:?}");
        // Every line was the script's to draw: more ran than there are cards.
        assert!(runs(cx) >= before + 12, "{before} {}", runs(cx));
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.plugins.borrow().host.as_ref().unwrap().problems(), []);
        });
        native(&shell, cx);
        draw(cx);
        assert_eq!(picker_lines(cx), drawn);
        std::mem::forget(end);
    }

    #[gpui::test]
    fn what_clear_left_is_drawn_by_the_chat_plugin_and_shows_the_entries_again(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        transcript(&end, cx);
        end.updates
            .try_send(Update::TurnEnded(kou_thread::Stop::Finished))
            .unwrap();
        cx.run_until_parked();
        draw(cx);
        cx.simulate_input("/clear");
        cx.simulate_keystrokes("escape enter");
        draw(cx);
        let unclear = cx.debug_bounds("unclear").expect("what /clear left");
        assert!(cx.debug_bounds("reply-3").is_none());
        // The chat draws it where the plugin did.
        let view = shell.read_with(cx, |shell, _| {
            shell
                .threads
                .values()
                .next()
                .map(|talking| talking.view().clone())
                .unwrap()
        });
        view.update(cx, |view, cx| view.set_chat_face(None, cx));
        draw(cx);
        assert_eq!(cx.debug_bounds("unclear"), Some(unclear));
        // The plugin's way back works as the chat's does.
        let face = shell.update(cx, |shell, cx| shell.chat_face(cx));
        view.update(cx, |view, cx| view.set_chat_face(Some(face), cx));
        draw(cx);
        cx.simulate_click(unclear.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("unclear").is_none());
        assert!(cx.debug_bounds("reply-3").is_some());
        std::mem::forget(end);
    }

    #[gpui::test]
    fn a_surface_s_card_is_drawn_by_the_chat_plugin_around_the_surface_where_the_chat_draws_it(
        cx: &mut TestAppContext,
    ) {
        // A surface kou's agent made, as the `shell-surface` scene has it.
        let root =
            std::env::temp_dir().join(format!("kou-chat-faces-surface-{}", std::process::id()));
        let version = root.join(".harness/ui/s-1/bench");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::create_dir_all(root.join("bench")).unwrap();
        std::fs::write(
            version.join("v1.json"),
            include_str!("../fixtures/surface-bench.json"),
        )
        .unwrap();
        std::fs::write(
            root.join("bench/runs.jsonl"),
            include_str!("../fixtures/surface-runs.jsonl"),
        )
        .unwrap();
        let (shell, cx, end) = open_in(cx, root.clone());
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(1600.)));
        for update in [
            Update::Ready {
                session: "s-1".into(),
                capabilities: Capabilities::default(),
                modes: Vec::new(),
                mode: None,
                models: Vec::new(),
                model: None,
            },
            Update::UserChunk(Block::Text("Where do the benchmarks stand?".into())),
            Update::TurnStarted,
            Update::Tool(kou_thread::ToolCall {
                id: "ui-1".into(),
                title: "UI".into(),
                status: kou_thread::ToolStatus::Done,
                content: vec![kou_thread::ToolContent::Text(
                    include_str!("../fixtures/surface-bench.txt").into(),
                )],
                ..kou_thread::ToolCall::default()
            }),
            Update::TurnEnded(kou_thread::Stop::Finished),
        ] {
            end.updates.try_send(update).unwrap();
        }
        cx.run_until_parked();
        draw(cx);
        let parts = |cx: &mut VisualTestContext| {
            ["tool-1", "surface-status-1", "frame"].map(|part| cx.debug_bounds(part))
        };
        let drawn = parts(cx);
        assert!(drawn.iter().all(Option::is_some), "{drawn:?}");
        let view = shell.read_with(cx, |shell, _| {
            shell.threads.values().next().unwrap().view().clone()
        });
        let key = format!("chat-app-card:{}:ui-1", view.entity_id().as_u64());
        assert!(shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            host.drawn("chat-app-card", &key).is_some() && host.problems().is_empty()
        }));
        // Closed and opened again by its head, it is as it was.
        let head = drawn[0].unwrap();
        cx.simulate_click(head.center(), gpui::Modifiers::none());
        draw(cx);
        cx.simulate_click(head.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(parts(cx), drawn);
        // The chat draws it where the plugin did.
        view.update(cx, |view, cx| view.set_chat_face(None, cx));
        draw(cx);
        assert_eq!(parts(cx), drawn);
        let _ = std::fs::remove_dir_all(root);
        std::mem::forget(end);
    }

    /// The chat's view.
    fn view_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ThreadView> {
        shell.read_with(cx, |shell, _| {
            shell.threads.values().next().unwrap().view().clone()
        })
    }

    /// Gives the chat back its own drawing of everything.
    fn native(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
        let view = view_of(shell, cx);
        shell.update(cx, |shell, _| shell.plugins.borrow_mut().host = None);
        view.update(cx, |view, cx| view.set_chat_face(None, cx));
    }

    /// A second turn that runs three subagents (one done, one failed, one
    /// at work with a call of its own done and one running), and three
    /// agents started in panes (at work, done, waiting on the user), the
    /// clock held still.
    fn agents(shell: &Entity<Shell>, end: &BackendEnd, cx: &mut VisualTestContext) {
        use kou_thread::{AgentNote, AgentWork, ToolCall, ToolContent, ToolKind, ToolStatus};
        use kou_thread_view::{Nested, NestedState};
        let subagent = |id: &str, title: &str, kind: &str, status| ToolCall {
            id: id.into(),
            title: title.into(),
            status,
            agent: Some(kind.into()),
            ..ToolCall::default()
        };
        let mut exploring = subagent("a1", "Find the delay", "Explore", ToolStatus::Running);
        exploring.raw_input = Some(json!({ "prompt": "Find where the delay is computed." }));
        exploring.work = AgentWork {
            steps: 2,
            said: vec![AgentNote {
                after: 1,
                text: "Found `backoff()`.".into(),
            }],
            ..AgentWork::default()
        };
        let mut reviewed = subagent(
            "a2",
            "Review the tests",
            "general-purpose",
            ToolStatus::Done,
        );
        reviewed.content = vec![ToolContent::Text("One asserts the old curve.".into())];
        reviewed.work = AgentWork {
            steps: 5,
            tokens: Some(18_400),
            ..AgentWork::default()
        };
        let step = |id: &str, title: &str, status| ToolCall {
            id: id.into(),
            title: title.into(),
            kind: ToolKind::Read,
            status,
            parent: Some("a1".into()),
            ..ToolCall::default()
        };
        for update in [
            Update::Ready {
                session: "s-1".into(),
                capabilities: Capabilities::default(),
                modes: Vec::new(),
                mode: None,
                models: Vec::new(),
                model: None,
            },
            Update::UserChunk(Block::Text("why is it slow".into())),
            Update::TurnStarted,
            Update::AssistantChunk("The delay is doubled.".into()),
            Update::TurnEnded(kou_thread::Stop::Finished),
            Update::UserChunk(Block::Text("fix it".into())),
            Update::TurnStarted,
            Update::Tool(reviewed),
            Update::Tool(subagent("a3", "Plan it", "Plan", ToolStatus::Failed)),
            Update::Tool(exploring),
            Update::Tool(step("a1-read", "Read retry/backoff.go", ToolStatus::Done)),
            Update::Tool(step("a1-grep", "Read retry/queue.go", ToolStatus::Running)),
        ] {
            end.updates.try_send(update).unwrap();
        }
        cx.run_until_parked();
        let wall =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_763_000_000);
        let agent = |id: &str, harness: &str, code: &str, state, detail: &str| Nested {
            id: id.into(),
            title: format!("{id}: a worker of ours"),
            harness: harness.into(),
            code: code.into(),
            state,
            detail: detail.into(),
            since: wall - std::time::Duration::from_secs(95),
            after: Some(3),
        };
        view_of(shell, cx).update(cx, |view, cx| {
            view.pin_clock(Some(wall), cx);
            view.set_nested(
                vec![
                    agent(
                        "n1",
                        "codex",
                        "CX",
                        NestedState::Running,
                        "Rewriting queue.go",
                    ),
                    agent("n2", "opencode", "OC", NestedState::Finished, ""),
                    agent("n3", "kou", "K", NestedState::Asking, ""),
                ],
                cx,
            );
        });
        cx.run_until_parked();
    }

    /// Where the agents' cards and what is in them stand.
    fn agent_cards(cx: &mut VisualTestContext) -> Vec<(&'static str, Option<Bounds<Pixels>>)> {
        [
            "tool-3",
            "agent-state-3",
            "agent-latest-3",
            "tool-4",
            "agent-state-4",
            "tool-5",
            "agent-latest-5",
            "nested-n1",
            "nested-n2",
            "nested-n3",
            "frame",
        ]
        .into_iter()
        .map(|part| (part, cx.debug_bounds(part)))
        .collect()
    }

    #[gpui::test]
    fn the_cards_of_agents_started_by_calls_and_in_panes_are_drawn_by_the_chat_plugin_where_the_chat_draws_them(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(1600.)));
        agents(&shell, &end, cx);
        draw(cx);
        let drawn = agent_cards(cx);
        assert!(drawn.iter().all(|(_, at)| at.is_some()), "{drawn:?}");
        let view = view_of(&shell, cx).entity_id().as_u64();
        shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            assert_eq!(host.problems(), []);
            for (part, key) in [
                ("chat-agent", format!("chat-agent:{view}:a1")),
                ("chat-agent", format!("chat-agent:{view}:a2")),
                ("chat-agent", format!("chat-agent:{view}:a3")),
                ("chat-nested", format!("chat-nested:{view}:n1")),
                ("chat-nested", format!("chat-nested:{view}:n3")),
            ] {
                assert!(host.drawn(part, &key).is_some(), "{key}");
            }
        });
        // A card opened by its head shows what the agent did, and closed
        // again it is as it was.
        let head = cx.debug_bounds("tool-5").unwrap();
        cx.simulate_click(head.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("agent-latest-5").is_none(), "opened");
        cx.simulate_click(head.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(agent_cards(cx), drawn);
        // The chat draws them where the plugin did.
        native(&shell, cx);
        draw(cx);
        assert_eq!(agent_cards(cx), drawn);
        std::mem::forget(end);
    }

    /// Claude Code with levels of effort apart from its modes, and a call
    /// of its that asks two questions (`id`): one that takes an option,
    /// each described, and one that takes several and words, with a way to
    /// refuse.
    fn asking(end: &BackendEnd, id: u64, cx: &mut VisualTestContext) {
        use kou_thread::{
            PermissionKind, PermissionOption, PermissionRequest, Question, QuestionOption,
            ToolCall, ToolStatus,
        };
        let option = |label: &str, description: Option<&str>| QuestionOption {
            label: label.into(),
            description: description.map(str::to_owned),
        };
        let mut updates = Vec::new();
        if id == 1 {
            updates.extend([
                Update::Ready {
                    session: "s-1".into(),
                    capabilities: Capabilities::default(),
                    modes: vec![choice("default", "Default"), choice("plan", "Plan mode")],
                    mode: Some("default".into()),
                    models: vec![choice("opus", "Opus")],
                    model: Some("opus".into()),
                },
                Update::Efforts {
                    levels: vec![
                        choice("low", "Low"),
                        choice("medium", "Medium"),
                        choice("high", "High"),
                    ],
                    current: Some("high".into()),
                },
                Update::UserChunk(Block::Text("make it survive a restart".into())),
                Update::TurnStarted,
            ]);
        }
        updates.push(Update::Permission(PermissionRequest {
            id,
            tool: ToolCall {
                id: format!("q{id}"),
                title: "Ask 2 questions".into(),
                status: ToolStatus::Pending,
                ..ToolCall::default()
            },
            options: vec![PermissionOption {
                id: "skip".into(),
                label: "Skip".into(),
                kind: PermissionKind::RejectOnce,
            }],
            questions: vec![
                Question {
                    header: Some("Store".into()),
                    text: "Where should the jobs be kept?".into(),
                    options: vec![
                        option("SQLite", Some("A file beside the binary")),
                        option("Postgres", Some("Needs a server")),
                    ],
                    multiple: false,
                    free: false,
                },
                Question {
                    header: None,
                    text: "Which checks should run?".into(),
                    options: vec![option("Lint", None), option("Tests", None)],
                    multiple: true,
                    free: true,
                },
            ],
        }));
        for update in updates {
            end.updates.try_send(update).unwrap();
        }
        cx.run_until_parked();
        draw(cx);
    }

    /// Where the question's parts and the field's controls stand.
    fn asked_parts(cx: &mut VisualTestContext) -> Vec<(&'static str, Option<Bounds<Pixels>>)> {
        [
            "ask-option-0",
            "ask-option-1",
            "answer-skip",
            "ask-back",
            "ask-confirm",
            "model-picker",
            "effort-picker",
            "mode-picker",
            "frame",
        ]
        .into_iter()
        .map(|part| (part, cx.debug_bounds(part)))
        .collect()
    }

    fn click(part: &'static str, cx: &mut VisualTestContext) {
        let at = cx
            .debug_bounds(part)
            .unwrap_or_else(|| panic!("{part} is drawn"));
        cx.simulate_click(at.center(), gpui::Modifiers::none());
        draw(cx);
    }

    #[gpui::test]
    fn the_composer_plugin_s_permissions_control_sends_the_selected_access(
        cx: &mut TestAppContext,
    ) {
        let (_shell, cx, end) = open(cx);
        end.updates
            .try_send(Update::Access(kou_thread::Access {
                choices: vec![choice("auto", "Auto"), choice("read-only", "Read Only")],
                current: Some("read-only".into()),
                ..Default::default()
            }))
            .unwrap();
        cx.run_until_parked();
        draw(cx);
        click("access-picker", cx);
        click("choice-0", cx);
        assert_eq!(heard(&end), [Command::SetAccess("auto".into())]);
    }

    #[gpui::test]
    fn a_call_s_questions_and_the_level_of_effort_are_drawn_by_the_plugins_where_the_chat_draws_them_and_work_as_they_do(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(1600.)));
        asking(&end, 1, cx);
        let view = view_of(&shell, cx).entity_id().as_u64();
        shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            assert_eq!(host.problems(), []);
            let key = format!("chat-tool:{view}:q1");
            assert!(host.drawn("chat-tool", &key).is_some(), "{key}");
        });
        let first = asked_parts(cx);
        // The first question: no Back, no Confirm.
        for (part, at) in &first {
            assert_eq!(
                at.is_none(),
                ["ask-back", "ask-confirm"].contains(part),
                "{part}"
            );
        }
        // What the plugin drew answers as the chat's own does: an option of
        // the first question, then two of the second and Confirm.
        click("ask-option-1", cx);
        assert!(cx.debug_bounds("ask-back").is_some(), "the second question");
        click("ask-option-0", cx);
        click("ask-option-1", cx);
        click("ask-confirm", cx);
        assert_eq!(
            heard(&end),
            [Command::Answer {
                request: 1,
                answers: vec![vec!["Postgres".into()], vec!["Lint".into(), "Tests".into()]],
            }]
        );
        assert!(cx.debug_bounds("ask-option-0").is_none(), "the card closed");
        // The level of effort, from the plugin's control and list.
        click("effort-picker", cx);
        click("choice-0", cx);
        assert_eq!(heard(&end), [Command::SetEffort("low".into())]);
        // The chat draws them where the plugins did: the same questions
        // asked again, at either question.
        asking(&end, 2, cx);
        let first = asked_parts(cx);
        click("ask-option-1", cx);
        let second = asked_parts(cx);
        click("ask-back", cx);
        native(&shell, cx);
        draw(cx);
        assert_eq!(asked_parts(cx), first);
        click("ask-option-1", cx);
        assert_eq!(asked_parts(cx), second);
        std::mem::forget(end);
    }

    /// The card beside pulled out over the text, the agent that started
    /// this one in it, and how full the context is.
    fn beside(shell: &Entity<Shell>, end: &BackendEnd, cx: &mut VisualTestContext) {
        use kou_thread_view::{Beside, Nested, NestedState};
        end.updates
            .try_send(Update::Usage(Usage {
                context_used: Some(61_300),
                context_limit: Some(200_000),
                ..Usage::default()
            }))
            .unwrap();
        cx.run_until_parked();
        let wall =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_763_000_000);
        view_of(shell, cx).update(cx, |view, cx| {
            view.set_beside(
                Some(Beside {
                    project: "atlas".into(),
                    path: "/fixture/atlas".into(),
                    branch: Some("fix/retry-delay".into()),
                    changes: Some((42, 0)),
                    parent: Some(Nested {
                        id: "lead".into(),
                        title: "Plan the release".into(),
                        harness: "claude".into(),
                        code: "CC".into(),
                        state: NestedState::Running,
                        detail: String::new(),
                        since: wall - std::time::Duration::from_secs(30),
                        after: Some(0),
                    }),
                    earlier: 2,
                }),
                cx,
            );
            view.pull_beside(true, cx);
        });
        cx.run_until_parked();
    }

    /// Where the card beside, its rows and the ticks stand.
    fn beside_parts(cx: &mut VisualTestContext) -> Vec<(&'static str, Option<Bounds<Pixels>>)> {
        [
            "beside",
            "beside-folder",
            "beside-hide",
            "lineage-lead",
            "lineage-n1",
            "lineage-n3",
            "lineage-call:a1",
            "beside-context",
            "ticks",
            "tick-0",
            "tick-2",
            "frame",
        ]
        .into_iter()
        .map(|part| (part, cx.debug_bounds(part)))
        .collect()
    }

    #[gpui::test]
    fn the_card_beside_and_the_ticks_are_drawn_by_the_chat_plugin_where_the_chat_draws_them_and_work_as_they_did(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(1600.)));
        agents(&shell, &end, cx);
        beside(&shell, &end, cx);
        draw(cx);
        let drawn = beside_parts(cx);
        assert!(drawn.iter().all(|(_, at)| at.is_some()), "{drawn:?}");
        let view = view_of(&shell, cx).entity_id().as_u64();
        shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            assert_eq!(host.problems(), []);
            for (part, key) in [
                ("chat-beside", format!("chat-beside:{view}:card")),
                ("chat-ticks", format!("chat-ticks:{view}:ticks")),
            ] {
                assert!(host.drawn(part, &key).is_some(), "{key}");
            }
        });
        // Put away, the card leaves a handle at the edge, which pulls it
        // out again.
        let hide = cx.debug_bounds("beside-hide").unwrap();
        cx.simulate_click(hide.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("beside").is_none(), "put away");
        let pull = cx.debug_bounds("beside-pull").expect("the handle");
        cx.simulate_click(pull.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(beside_parts(cx), drawn);
        // A row of an agent the agent started shows its card, open.
        let row = cx.debug_bounds("lineage-call:a1").unwrap();
        cx.simulate_click(row.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(
            cx.debug_bounds("agent-latest-5").is_none(),
            "its card is open"
        );
        let opened = beside_parts(cx);
        // The chat draws them where the plugin did, the handle too.
        native(&shell, cx);
        draw(cx);
        assert_eq!(beside_parts(cx), opened);
        let hide = cx.debug_bounds("beside-hide").unwrap();
        cx.simulate_click(hide.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(cx.debug_bounds("beside-pull"), Some(pull));
        std::mem::forget(end);
    }

    #[gpui::test]
    fn an_answer_being_written_runs_no_script_as_its_words_come(cx: &mut TestAppContext) {
        let (shell, cx, end) = open(cx);
        let runs = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.plugins.borrow().host.as_ref().unwrap().runs()
            })
        };
        for update in [
            Update::Ready {
                session: "s-1".into(),
                capabilities: Capabilities::default(),
                modes: Vec::new(),
                mode: None,
                models: Vec::new(),
                model: None,
            },
            Update::UserChunk(Block::Text("tell me about backoff".into())),
            Update::TurnStarted,
            Update::AssistantChunk("Backoff".into()),
        ] {
            end.updates.try_send(update).unwrap();
        }
        cx.run_until_parked();
        draw(cx);
        assert!(cx.debug_bounds("reply-1").is_some());
        let before = runs(cx);
        for word in 0..24 {
            end.updates
                .try_send(Update::AssistantChunk(format!(" word{word}")))
                .unwrap();
            cx.run_until_parked();
            draw(cx);
        }
        assert_eq!(runs(cx), before, "a script ran as the words came");
        // Said in full, its line changes once: it can be copied now.
        end.updates
            .try_send(Update::TurnEnded(kou_thread::Stop::Finished))
            .unwrap();
        cx.run_until_parked();
        draw(cx);
        assert!(cx.debug_bounds("copy-answer-1").is_some());
        assert!(runs(cx) > before);
        std::mem::forget(end);
    }

    fn field_input(controls: Option<Value>, slots: &[&str]) -> Value {
        let mut data = json!({
            "focused": true, "busy": false, "closed": false, "narrow": false,
            "standalone": false, "slots": slots,
        });
        if let Some(controls) = controls {
            data["controls"] = controls;
        }
        json!({ "theme": theme_json(&kou_ui::Theme::new(true)), "data": data })
    }

    #[gpui::test]
    fn clicking_a_missing_file_reports_it_and_a_theme_link_opens_its_preview(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.md");
        let mut document = kou_ui::themes::Preset::Charcoal.document();
        document.name = "Shared from chat".into();
        let link = document.link().unwrap();
        at_work(&end, cx);
        end.updates
            .try_send(Update::AssistantChunk(format!(
                "[Missing file](<{}>) and [Shared theme]({link})",
                missing.display()
            )))
            .unwrap();
        cx.run_until_parked();
        draw(cx);
        let file = cx.debug_bounds("link-0").unwrap();
        cx.simulate_click(file.center(), gpui::Modifiers::none());
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert!(
                shell
                    .notice
                    .as_ref()
                    .is_some_and(|text| text.contains("File not found:"))
            )
        });
        let theme = cx.debug_bounds("link-1").unwrap();
        cx.simulate_click(theme.center(), gpui::Modifiers::none());
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert!(shell.settings_open);
            assert_eq!(shell.appearance.draft.as_ref().unwrap().document, document);
            assert!(
                shell.store.themes.is_empty(),
                "importing previews before saving"
            );
        });
    }

    #[gpui::test]
    fn studio_uses_js_for_messages_and_composer_and_keeps_controls_working(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, end) = open(cx);
        let view = view_of(&shell, cx);
        view.update(cx, |view, cx| {
            view.set_look(kou_thread_view::Look::Studio, cx)
        });
        at_work(&end, cx);
        let key = format!("studio-user:{}:0", view.entity_id().as_u64());
        shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            assert!(
                host.drawn("studio-user", &key).is_some(),
                "the message was rendered in JS"
            );
            assert!(host.problems().is_empty(), "{:?}", host.problems());
        });
        let copy = cx.debug_bounds("copy-prompt-0").unwrap();
        cx.simulate_click(copy.center(), gpui::Modifiers::none());
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("why is it slow")
            )
        });
        let mode = cx.debug_bounds("mode-picker").unwrap();
        cx.simulate_click(mode.center(), gpui::Modifiers::none());
        draw(cx);
        let choice = cx.debug_bounds("choice-1").unwrap();
        assert!(
            choice.bottom() <= mode.top(),
            "the menu opens above its control"
        );
        cx.simulate_click(choice.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(heard(&end), [Command::SetMode("auto".into())]);
        cx.simulate_input("then run tests");
        draw(cx);
        assert!(cx.debug_bounds("force").is_some());
        let queue = cx.debug_bounds("queue").unwrap();
        cx.simulate_click(queue.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("queue-count").is_some());
        let stop = cx.debug_bounds("waiting-stop").unwrap();
        cx.simulate_click(stop.center(), gpui::Modifiers::none());
        assert_eq!(heard(&end), [Command::Cancel]);
    }

    #[test]
    fn the_prompt_frame_has_no_border_in_any_agent_or_focus_state() {
        let mut host = Host::new(vec![Source::Embedded(crate::plugins::BUILT_IN)]).unwrap();
        for busy in [false, true] {
            for focused in [false, true] {
                let mut input = field_input(None, &["editor"]);
                input["data"]["busy"] = busy.into();
                input["data"]["focused"] = focused.into();
                let tree = host.render_as("composer", "border-check", &input).unwrap();
                let frame = tree.find("frame").unwrap();
                assert!(frame.props.get("border").is_none());
                assert!(frame.props.get("borderColor").is_none());
                assert!(frame.props.get("spring").is_none());
            }
        }
    }

    #[test]
    fn the_composer_draws_the_field_s_controls_when_told_how_they_stand_and_places_the_app_s_otherwise()
     {
        let mut host = Host::new(vec![Source::Embedded(crate::plugins::BUILT_IN)]).unwrap();
        let slots = ["editor", "attach", "model", "mode", "controls"];
        // An app that does not say how its controls stand (an older one,
        // reading this checkout's plugins) has its own placed, as before.
        let tree = host
            .render_as("composer", "f1", &field_input(None, &slots))
            .unwrap();
        let placed: Vec<&str> = tree
            .walk()
            .into_iter()
            .filter(|(_, node)| node.kind == "slot")
            .filter_map(|(_, node)| node.text("name"))
            .collect();
        assert_eq!(placed, slots);
        // Told, the script draws them: the paperclip, the model's control
        // with its provider's dot, the effort's bars, the button that sends.
        let controls = json!({
            "buttons": "send", "ready": true, "attach": true,
            "model": { "name": "Opus", "open": false, "dot": "#d97757" },
            "mode": { "name": "high", "open": false, "effort": { "level": 3, "bars": 4 } },
        });
        let tree = host
            .render_as("composer", "f2", &field_input(Some(controls), &slots))
            .unwrap();
        assert!(
            tree.walk()
                .iter()
                .all(|(_, node)| node.kind != "slot" || node.text("name") == Some("editor")),
            "only what is typed is the app's"
        );
        // (Each control says what is chosen, not what it chooses among.)
        assert_eq!(tree.said(), "Opus HIGH");
        let model = tree.find("model-picker").unwrap();
        assert_eq!(model.children[0].text("bg"), Some("#d97757"));
        let bars = &tree.find("mode-picker").unwrap().children[0].children;
        let lit: Vec<Option<&str>> = bars.iter().map(|bar| bar.text("bg")).collect();
        assert_eq!(
            lit,
            [
                Some("accent"),
                Some("accent"),
                Some("accent"),
                Some("lineStrong")
            ]
        );
        // Each asks the field for what it does.
        for (id, asked) in [
            ("attach", "field.attach"),
            ("model-picker", "field.model"),
            ("mode-picker", "field.mode"),
            ("send", "field.send"),
        ] {
            let handler = tree.find(id).unwrap().handler("onClick").unwrap();
            assert_eq!(
                host.dispatch_as("composer", "f2", handler, &json!({ "count": 1 })),
                [(asked.to_owned(), Value::Null)],
                "{id}"
            );
        }
        // Nothing typed, the button that sends answers to nothing; at work,
        // it stops, or forces and queues once something is typed.
        let at = |buttons: &str, ready: bool| json!({ "buttons": buttons, "ready": ready, "attach": true, "model": null, "mode": null });
        let idle = host
            .render_as(
                "composer",
                "f3",
                &field_input(Some(at("send", false)), &slots),
            )
            .unwrap();
        assert!(idle.find("send").unwrap().handler("onClick").is_none());
        let stop = host
            .render_as(
                "composer",
                "f3",
                &field_input(Some(at("stop", false)), &slots),
            )
            .unwrap();
        assert!(stop.find("stop").is_some() && stop.find("send").is_none());
        let force = host
            .render_as(
                "composer",
                "f3",
                &field_input(Some(at("force", false)), &slots),
            )
            .unwrap();
        let handler = force.find("force").unwrap().children[0]
            .handler("onClick")
            .unwrap();
        assert_eq!(
            host.dispatch_as("composer", "f3", handler, &json!({ "count": 1 }))[0].0,
            "field.force"
        );
        assert!(force.find("queue").is_some());
        assert_eq!(host.problems(), []);
    }
}
