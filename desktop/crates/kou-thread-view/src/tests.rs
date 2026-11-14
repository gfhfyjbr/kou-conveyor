//! The pane against a scripted backend: what the user does reaches the backend
//! as commands, and what the backend says reaches the screen.

use crate::{
    entries::Parsed,
    markdown::Block as Markdown,
    view::{Header, ThreadView, ThreadViewEvent},
};
use gpui::{
    Bounds, Entity, Focusable as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Pixels,
    ScrollDelta, ScrollEvent, TestAppContext, TouchPhase, VisualTestContext, point, px, size,
};
use kou_thread::{
    BackendEnd, Block, Capabilities, Choice, Command, Entry, Level, Offers, PermissionKind,
    PermissionOption, PermissionRequest, Request, Stop, ThreadState, ToolCall, ToolContent,
    ToolKind, ToolPatch, ToolStatus, Update, Usage,
};
use std::{cell::RefCell, rc::Rc};

struct Pane {
    view: Entity<ThreadView>,
    end: BackendEnd,
    events: Rc<RefCell<Vec<ThreadViewEvent>>>,
}

impl Pane {
    /// A pane in a 900 by 700 window with the composer focused, fed by `end`.
    fn open(cx: &mut TestAppContext) -> (Self, &mut VisualTestContext) {
        Self::open_in(cx, std::path::Path::new("/work/project"))
    }

    /// A pane whose agent works in `cwd`.
    fn open_in<'a>(
        cx: &'a mut TestAppContext,
        cwd: &std::path::Path,
    ) -> (Self, &'a mut VisualTestContext) {
        cx.update(|cx| {
            cx.set_global(kou_ui::Theme::new(true));
            // Tests have no frames for animations to run on: everything takes
            // its end state at once, as it does for a user who asks for less motion.
            cx.set_reduce_motion(true);
        });
        let (backend, end) = kou_thread::channel();
        let cwd = cwd.to_owned();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let header = Header {
                agent: "kou".into(),
                code: "K".into(),
                cwd,
            };
            let view = ThreadView::new(backend, header, window, cx);
            view.focus_handle(cx).focus(window, cx);
            view
        });
        cx.simulate_resize(size(px(900.), px(700.)));
        // Focus only moves in the window the user is in.
        cx.update(|window, _| window.activate_window());
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &ThreadViewEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
            .detach();
        });
        cx.run_until_parked();
        (Self { view, end, events }, cx)
    }

    /// The backend says these things; the pane takes them in and redraws.
    fn say(&self, updates: impl IntoIterator<Item = Update>, cx: &mut VisualTestContext) {
        for update in updates {
            self.end.updates.try_send(update).expect("the pane listens");
        }
        cx.run_until_parked();
    }

    /// The commands the backend received since the last look.
    fn heard(&self) -> Vec<Command> {
        std::iter::from_fn(|| self.end.commands.try_recv().ok()).collect()
    }

    fn entries(&self, cx: &mut VisualTestContext) -> Vec<Entry> {
        self.view
            .read_with(cx, |view, _| view.thread().entries().to_vec())
    }

    fn state(&self, cx: &mut VisualTestContext) -> ThreadState {
        self.view
            .read_with(cx, |view, _| view.thread().state().clone())
    }

    fn draft(&self, cx: &mut VisualTestContext) -> String {
        self.view.read_with(cx, |view, cx| view.composer_text(cx))
    }

    fn queued(&self, cx: &mut VisualTestContext) -> Vec<(String, bool)> {
        self.view
            .read_with(cx, |view, cx| view.dock.read(cx).queued_texts())
    }

    fn requests(&self) -> Vec<Request> {
        std::iter::from_fn(|| self.end.requests.try_recv().ok()).collect()
    }

    fn click(&self, selector: &'static str, cx: &mut VisualTestContext) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is on screen"));
        cx.simulate_click(bounds.center(), Modifiers::none());
        cx.run_until_parked();
    }
}

fn ready() -> Update {
    Update::Ready {
        session: "s1".into(),
        capabilities: Capabilities::default(),
        modes: Vec::new(),
        mode: None,
        models: Vec::new(),
        model: None,
    }
}

fn choice(id: &str, name: &str) -> Choice {
    Choice {
        id: id.into(),
        name: name.into(),
        description: None,
    }
}

fn text(text: &str) -> Vec<Block> {
    vec![Block::Text(text.into())]
}

fn tool(id: &str, status: ToolStatus) -> ToolCall {
    ToolCall {
        id: id.into(),
        title: format!("Run {id}"),
        kind: ToolKind::Execute,
        status,
        content: vec![ToolContent::Text(format!("output of {id}"))],
        ..ToolCall::default()
    }
}

fn ask(id: u64, tool_id: &str) -> Update {
    let option = |id: &str, label: &str, kind| PermissionOption {
        id: id.into(),
        label: label.into(),
        kind,
    };
    Update::Permission(PermissionRequest {
        id,
        tool: tool(tool_id, ToolStatus::Pending),
        options: vec![
            option("reject", "Reject", PermissionKind::RejectOnce),
            option("once", "Allow once", PermissionKind::AllowOnce),
            option("always", "Always allow", PermissionKind::AllowAlways),
        ],
    })
}

#[gpui::test]
fn sending_a_prompt_emits_the_command_and_echoes_the_user_entry(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready()], cx);
    assert_eq!(pane.state(cx), ThreadState::Idle);

    cx.simulate_input("почини");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("тест");
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.heard(), [Command::Prompt(text("почини\nтест"))]);
    assert_eq!(pane.entries(cx), [Entry::User(text("почини\nтест"))]);
    assert_eq!(pane.draft(cx), "");

    // The send button does the same as Enter.
    cx.simulate_input("ещё");
    pane.click("send", cx);
    assert_eq!(pane.heard(), [Command::Prompt(text("ещё"))]);
    assert_eq!(pane.entries(cx).len(), 2);

    // A prompt from another agent takes the same road and is echoed the same way.
    pane.view
        .update(cx, |view, cx| view.send("from the orchestrator", cx));
    assert_eq!(
        pane.heard(),
        [Command::Prompt(text("from the orchestrator"))]
    );
    assert_eq!(
        pane.entries(cx).last(),
        Some(&Entry::User(text("from the orchestrator")))
    );

    // Blank text is not a prompt.
    cx.simulate_keystrokes("space enter");
    pane.view.update(cx, |view, cx| view.send("  \n", cx));
    assert_eq!(pane.heard(), []);
}

#[gpui::test]
fn a_prompt_written_while_a_turn_runs_waits_in_the_queue(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready(), Update::TurnStarted], cx);
    cx.simulate_input("next");
    cx.simulate_keystrokes("enter");
    // Not sent, not echoed, not lost: it waits in the queue, in sight.
    assert_eq!(pane.heard(), []);
    assert!(pane.entries(cx).is_empty());
    assert_eq!(pane.draft(cx), "");
    assert_eq!(pane.queued(cx), [("next".to_owned(), false)]);
    assert!(cx.debug_bounds("queued-1").is_some());
    // While the turn runs and nothing is written, the button stops it.
    assert!(cx.debug_bounds("send").is_none());
    pane.click("stop", cx);
    assert_eq!(pane.heard(), [Command::Cancel]);

    // A stopped turn holds the queue until the user resumes it.
    pane.say([Update::TurnEnded(Stop::Cancelled)], cx);
    assert_eq!(pane.heard(), []);
    pane.click("queue-resume", cx);
    assert_eq!(pane.heard(), [Command::Prompt(text("next"))]);
    assert!(pane.queued(cx).is_empty());
}

#[gpui::test]
fn streamed_chunks_appear_and_a_burst_is_one_frame(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready(), Update::TurnStarted], cx);
    pane.say([Update::AssistantChunk("Hel".into())], cx);
    pane.say([Update::AssistantChunk("lo **wor".into())], cx);
    pane.say(
        [Update::AssistantChunk("ld**\n\n```rust\nfn main".into())],
        cx,
    );
    assert_eq!(
        pane.entries(cx),
        [Entry::Assistant(
            "Hello **world**\n\n```rust\nfn main".into()
        )]
    );
    assert_eq!(
        pane.view.read_with(cx, |view, _| view.last_answer()),
        Some("Hello **world**\n\n```rust\nfn main".into())
    );
    // What is on screen is the parse of the text so far: an open fence is already code.
    pane.view.read_with(cx, |view, _| {
        let Some(Parsed::Document(blocks)) = &view.cache[0] else {
            panic!("the answer was drawn");
        };
        let Markdown::Paragraph(first) = &blocks[0] else {
            panic!("a paragraph: {blocks:?}");
        };
        assert_eq!(first.text, "Hello world");
        assert!(matches!(&blocks[1], Markdown::Code { text, .. } if text == "fn main"));
    });

    // Three hundred chunks waiting in the channel are one change and one frame.
    let frames = pane.view.read_with(cx, |view, _| view.frames);
    pane.say(
        (0..300).map(|n| Update::AssistantChunk(format!(" {n}"))),
        cx,
    );
    assert_eq!(pane.view.read_with(cx, |view, _| view.frames), frames + 1);
    let Entry::Assistant(answer) = &pane.entries(cx)[0] else {
        panic!("still one answer");
    };
    assert!(answer.ends_with(" 298 299"));
}

#[gpui::test]
fn a_permission_click_sends_permit_and_unblocks(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready(), Update::TurnStarted, ask(7, "t1")], cx);
    assert_eq!(pane.state(cx), ThreadState::Blocked);
    // Every option is a button, in the backend's order.
    let reject = cx.debug_bounds("answer-reject").expect("a reject button");
    let once = cx.debug_bounds("answer-once").expect("an allow button");
    let always = cx.debug_bounds("answer-always").expect("an always button");
    assert!(reject.left() < once.left() && once.left() < always.left());

    pane.click("answer-always", cx);
    assert_eq!(
        pane.heard(),
        [Command::Permit {
            request: 7,
            option: "always".into()
        }]
    );
    assert_eq!(pane.state(cx), ThreadState::Working);
    assert!(
        cx.debug_bounds("answer-once").is_none(),
        "the buttons are gone"
    );
    assert_eq!(
        pane.events.borrow().as_slice(),
        [
            ThreadViewEvent::State(ThreadState::Blocked),
            ThreadViewEvent::State(ThreadState::Working)
        ]
    );
}

#[gpui::test]
fn the_default_answer_has_a_key_and_tab_reaches_every_option(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [ready(), Update::TurnStarted, ask(7, "t1"), ask(8, "t2")],
        cx,
    );
    let allow = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    // The oldest request first, with its first allow option.
    cx.simulate_keystrokes(allow);
    assert_eq!(
        pane.heard(),
        [Command::Permit {
            request: 7,
            option: "once".into()
        }]
    );
    assert_eq!(pane.state(cx), ThreadState::Blocked);

    // Shift+Tab from the composer walks back through the remaining request's
    // buttons: always, once, reject. Enter on the third presses it.
    cx.simulate_keystrokes("shift-tab shift-tab shift-tab");
    let enter = Keystroke::parse("enter").expect("a key");
    cx.simulate_event(KeyDownEvent {
        keystroke: enter.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke: enter });
    assert_eq!(
        pane.heard(),
        [Command::Permit {
            request: 8,
            option: "reject".into()
        }]
    );
    assert_eq!(pane.state(cx), ThreadState::Working);
}

#[gpui::test]
fn escape_pressed_twice_stops_a_running_turn_and_only_that(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready()], cx);
    cx.simulate_keystrokes("escape escape");
    assert_eq!(pane.heard(), [], "nothing runs, nothing to stop");

    // One Escape is too easily pressed for what it would end: it says
    // that a second stops the turn, and stops nothing itself.
    pane.say([Update::TurnStarted], cx);
    cx.simulate_keystrokes("escape");
    assert_eq!(pane.heard(), []);
    assert_eq!(pane.state(cx), ThreadState::Working);
    assert!(
        notice(&pane, cx).contains("Esc again"),
        "{:?}",
        notice(&pane, cx)
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(pane.heard(), [Command::Cancel]);

    // The second has to come soon: later, it is a first one again.
    cx.simulate_keystrokes("escape");
    cx.executor()
        .advance_clock(crate::view::ESCAPE_TWICE + std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    assert_eq!(pane.heard(), [], "too late to be the second");
    cx.simulate_keystrokes("escape");
    assert_eq!(pane.heard(), [Command::Cancel]);

    // Blocked on a permission is still a running turn; an Escape pressed
    // before it was blocked does not count toward stopping it now.
    cx.simulate_keystrokes("escape");
    pane.say([ask(1, "t1")], cx);
    cx.simulate_keystrokes("escape");
    assert_eq!(pane.heard(), []);
    cx.simulate_keystrokes("escape");
    assert_eq!(pane.heard(), [Command::Cancel]);

    pane.say([Update::TurnEnded(Stop::Cancelled)], cx);
    cx.simulate_keystrokes("escape escape");
    assert_eq!(pane.heard(), []);
}

#[gpui::test]
fn state_and_title_events_follow_the_thread(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    assert_eq!(pane.state(cx), ThreadState::Starting);
    pane.say([ready()], cx);
    pane.say([Update::Title("Retry queue".into())], cx);
    pane.say([Update::TurnStarted], cx);
    pane.say([Update::AssistantChunk("text".into())], cx);
    pane.say([ask(1, "t1")], cx);
    pane.say([Update::TurnEnded(Stop::Failed("rate limited".into()))], cx);
    pane.say([Update::Title("Retry queue".into())], cx);
    pane.say([Update::Closed(Some("exit status 2".into()))], cx);
    assert_eq!(
        pane.events.borrow().as_slice(),
        [
            ThreadViewEvent::State(ThreadState::Idle),
            ThreadViewEvent::Title("Retry queue".into()),
            ThreadViewEvent::State(ThreadState::Working),
            ThreadViewEvent::State(ThreadState::Blocked),
            ThreadViewEvent::State(ThreadState::Idle),
            ThreadViewEvent::State(ThreadState::Closed(Some("exit status 2".into()))),
        ]
    );
    // The failed turn left its reason in the transcript.
    assert_eq!(
        pane.entries(cx).last(),
        Some(&Entry::Notice(Level::Error, "rate limited".into()))
    );
    // A closed session takes no prompts.
    pane.view
        .update(cx, |view, cx| view.send("anyone there?", cx));
    cx.simulate_input("hello");
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.heard(), []);
}

#[gpui::test]
fn a_backend_that_vanishes_closes_the_session_with_a_reason(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready()], cx);
    let Pane { view, end, events } = pane;
    drop(end);
    cx.run_until_parked();
    assert!(matches!(
        view.read_with(cx, |view, _| view.thread().state().clone()),
        ThreadState::Closed(Some(_))
    ));
    assert!(matches!(
        events.borrow().last(),
        Some(ThreadViewEvent::State(ThreadState::Closed(Some(_))))
    ));
}

#[gpui::test]
fn the_transcript_follows_the_end_only_while_the_user_is_there(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready(), Update::TurnStarted], cx);
    let filler = |from: usize| {
        (from..from + 60).flat_map(|n| {
            [
                Update::Notice(Level::Info, format!("note {n}")),
                Update::AssistantChunk(format!("paragraph {n}\n\nwith a second paragraph")),
            ]
        })
    };
    pane.say(filler(0), cx);
    let list = pane.view.read_with(cx, |view, _| view.list.clone());
    assert!(list.is_following_tail());
    let at_end = list.logical_scroll_top();

    // More arrives: the view stays at the end, which is now further down.
    pane.say([Update::AssistantChunk(" and more".into())], cx);
    pane.say(filler(60), cx);
    assert!(list.is_following_tail());
    assert!(list.logical_scroll_top().item_ix > at_end.item_ix);

    // The user scrolls up with the wheel: from here on the view stays where it is.
    let viewport = list.viewport_bounds();
    cx.simulate_event(ScrollEvent {
        position: viewport.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(400.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        source: Default::default(),
    });
    cx.run_until_parked();
    assert!(!list.is_following_tail());
    assert!(
        cx.debug_bounds("latest").is_some(),
        "a way back to the end is offered"
    );
    let reading = list.logical_scroll_top();
    pane.say(filler(120), cx);
    pane.say([Update::AssistantChunk("streaming on".into())], cx);
    pane.say([Update::AssistantChunk(" and on".into())], cx);
    let still = list.logical_scroll_top();
    assert_eq!(
        (still.item_ix, still.offset_in_item),
        (reading.item_ix, reading.offset_in_item),
        "new output must not move what the user is reading"
    );

    // Page Up and Home move further back; End returns to the live end.
    cx.simulate_keystrokes("pageup");
    cx.run_until_parked();
    assert!(list.logical_scroll_top().item_ix < reading.item_ix);
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    let top = list.logical_scroll_top();
    assert_eq!((top.item_ix, top.offset_in_item), (0, px(0.)));
    cx.simulate_keystrokes("pagedown");
    cx.run_until_parked();
    assert!(list.logical_scroll_top().item_ix > 0);
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    assert!(list.is_following_tail());
    assert!(cx.debug_bounds("latest").is_none());
    pane.say(filler(180), cx);
    assert!(list.is_following_tail());

    // The button does what End does.
    cx.simulate_keystrokes("pageup");
    cx.run_until_parked();
    pane.click("latest", cx);
    assert!(list.is_following_tail());
    assert!(cx.debug_bounds("latest").is_none());
}

#[gpui::test]
fn home_and_end_stay_in_a_draft_that_has_text(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready()], cx);
    pane.say(
        (0..80).map(|n| Update::Notice(Level::Info, format!("note {n}"))),
        cx,
    );
    let list = pane.view.read_with(cx, |view, _| view.list.clone());
    cx.simulate_input("draft");
    cx.simulate_keystrokes("home");
    cx.simulate_input("a ");
    cx.simulate_keystrokes("end");
    cx.simulate_input("!");
    assert_eq!(pane.draft(cx), "a draft!");
    assert!(list.is_following_tail(), "the transcript did not move");
}

#[gpui::test]
fn only_what_is_on_screen_is_parsed_and_laid_out(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready(), Update::TurnStarted], cx);
    for batch in 0..20 {
        pane.say(
            (0..150).flat_map(|n| {
                let n = batch * 150 + n;
                [
                    Update::AssistantChunk(format!("## Answer {n}\n\nSome *text* with `code`.")),
                    Update::Tool(tool(&format!("t{n}"), ToolStatus::Done)),
                ]
            }),
            cx,
        );
    }
    let (entries, parsed) = pane.view.read_with(cx, |view, _| {
        (
            view.thread().entries().len(),
            view.cache.iter().filter(|parsed| parsed.is_some()).count(),
        )
    });
    assert_eq!(entries, 6000);
    assert!(
        parsed > 0 && parsed < 1000,
        "{parsed} of {entries} entries were parsed"
    );

    // Paging to the far end of a long thread touches only the rows passed on the way.
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    cx.simulate_keystrokes("pagedown pagedown");
    cx.run_until_parked();
    let parsed = pane.view.read_with(cx, |view, _| {
        view.cache.iter().filter(|parsed| parsed.is_some()).count()
    });
    assert!(parsed < 1200, "{parsed} entries parsed after paging");
}

#[gpui::test]
fn a_running_call_is_closed_on_its_last_lines_and_a_failed_one_opens(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready(),
            Update::TurnStarted,
            Update::Tool(tool("run", ToolStatus::Running)),
            Update::Tool(tool("done", ToolStatus::Running)),
            Update::Tool(tool("bad", ToolStatus::Running)),
        ],
        cx,
    );
    let open = |pane: &Pane, cx: &mut VisualTestContext| -> Vec<bool> {
        pane.view.read_with(cx, |view, _| {
            view.thread()
                .entries()
                .iter()
                .enumerate()
                .map(|(index, entry)| match entry {
                    Entry::Tool(call) => view.tool_open(index, call),
                    _ => false,
                })
                .collect()
        })
    };
    // As in the web cockpit, a call that runs is a closed card that shows
    // the last lines its command printed.
    assert_eq!(open(&pane, cx), [false, false, false]);
    let patch = |id: &str, status| {
        Update::ToolPatch(ToolPatch {
            id: id.into(),
            status: Some(status),
            ..ToolPatch::default()
        })
    };
    pane.say(
        [
            patch("done", ToolStatus::Done),
            patch("bad", ToolStatus::Failed),
        ],
        cx,
    );
    assert_eq!(open(&pane, cx), [false, false, true]);

    // A click on the row opens a folded call and folds an open one, and that choice sticks.
    pane.click("tool-1", cx);
    pane.click("tool-2", cx);
    assert_eq!(open(&pane, cx), [false, true, false]);
    pane.say(
        [
            patch("run", ToolStatus::Done),
            patch("done", ToolStatus::Done),
        ],
        cx,
    );
    assert_eq!(open(&pane, cx), [false, true, false]);
}

#[gpui::test]
fn reasoning_is_a_line_that_opens_on_a_click(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready(),
            Update::TurnStarted,
            Update::ThoughtChunk("First thought.\n\nSecond paragraph of it.".into()),
        ],
        cx,
    );
    let height = |cx: &mut VisualTestContext| -> Pixels {
        let list = pane.view.read_with(cx, |view, _| view.list.clone());
        list.bounds_for_item(0)
            .expect("the thought is on screen")
            .size
            .height
    };
    // As in the web cockpit, reasoning is a line that says what it is about
    // while it is written. Once the answer comes it folds with the rest of
    // the work, into a line as tall; opened, it is that line again.
    let folded = height(cx);
    pane.say([Update::AssistantChunk("The answer.".into())], cx);
    assert_eq!(height(cx), folded);
    assert!(cx.debug_bounds("thought-0").is_none());
    pane.click("worked-0", cx);
    let folded = height(cx);
    pane.click("thought-0", cx);
    let open = height(cx);
    assert!(
        folded < open,
        "{folded:?} is one line, {open:?} is two paragraphs"
    );
    pane.click("thought-0", cx);
    assert_eq!(height(cx), folded);
}

#[gpui::test]
fn links_open_in_the_browser_and_only_web_links_do(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready(),
            Update::AssistantChunk(
                "Not this [local file](file:///etc/passwd), but [the docs](https://example.com/docs) \
                 and https://example.com/issues/412."
                    .into(),
            ),
        ],
        cx,
    );
    // The file link is text; the two controls are the web links.
    let docs: Bounds<Pixels> = cx
        .debug_bounds("link-0")
        .expect("the web link is a control");
    assert!(cx.debug_bounds("link-1").is_some());
    assert!(cx.debug_bounds("link-2").is_none());
    cx.simulate_click(docs.center(), Modifiers::none());
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/docs"));
    // The pointer opened it; the keyboard is still in the field.
    cx.simulate_input("typed");
    assert_eq!(pane.draft(cx), "typed");

    // By keyboard: Shift+Tab from the field reaches the last link, Enter opens it.
    cx.simulate_keystrokes("shift-tab");
    let enter = Keystroke::parse("enter").expect("a key");
    cx.simulate_event(KeyDownEvent {
        keystroke: enter.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke: enter });
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/issues/412")
    );
    assert_eq!(
        pane.draft(cx),
        "typed",
        "Enter went to the link, not the field"
    );
}

#[gpui::test]
fn a_link_that_wraps_is_one_tab_stop(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    cx.simulate_resize(size(px(260.), px(700.)));
    pane.say(
        [
            ready(),
            Update::AssistantChunk(
                "See [a link whose text is long enough to wrap over several rows of a narrow pane](https://example.com/a) \
                 and [another](https://example.com/b)."
                    .into(),
            ),
        ],
        cx,
    );
    let press_enter = |cx: &mut VisualTestContext| {
        let enter = Keystroke::parse("enter").expect("a key");
        cx.simulate_event(KeyDownEvent {
            keystroke: enter.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke: enter });
    };
    cx.simulate_keystrokes("shift-tab");
    press_enter(cx);
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/b"));
    // One more step back is the whole wrapped link, however many rows it covers.
    cx.simulate_keystrokes("shift-tab");
    press_enter(cx);
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/a"));
    cx.simulate_keystrokes("tab tab");
    cx.simulate_input("x");
    assert_eq!(pane.draft(cx), "x", "two stops forward is the field again");
}

#[gpui::test]
fn mode_and_model_pickers_appear_only_when_offered(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready()], cx);
    assert!(cx.debug_bounds("mode-picker").is_none());
    assert!(cx.debug_bounds("model-picker").is_none());

    pane.say(
        [Update::Ready {
            session: "s1".into(),
            capabilities: Capabilities::default(),
            modes: vec![choice("ask", "Ask"), choice("auto", "Auto-edit")],
            mode: Some("ask".into()),
            models: Vec::new(),
            model: None,
        }],
        cx,
    );
    assert!(cx.debug_bounds("model-picker").is_none());
    pane.click("mode-picker", cx);
    pane.click("choice-1", cx);
    assert_eq!(pane.heard(), [Command::SetMode("auto".into())]);
    assert_eq!(
        pane.view
            .read_with(cx, |view, _| view.thread().mode.clone()),
        Some("auto".into())
    );
    assert!(cx.debug_bounds("choice-0").is_none(), "the menu closed");
    // Focus is back in the field: typing goes on.
    cx.simulate_input("ok");
    assert_eq!(pane.draft(cx), "ok");

    // Escape closes an open menu without choosing and without cancelling anything.
    pane.click("mode-picker", cx);
    assert!(cx.debug_bounds("choice-0").is_some());
    cx.simulate_keystrokes("down escape");
    assert!(cx.debug_bounds("choice-0").is_none());
    assert_eq!(pane.heard(), []);
}

#[gpui::test]
fn up_in_an_empty_field_recalls_earlier_prompts(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready()], cx);
    for prompt in ["first", "second"] {
        cx.simulate_input(prompt);
        cx.simulate_keystrokes("enter");
        pane.say([Update::TurnStarted, Update::TurnEnded(Stop::Finished)], cx);
    }
    pane.heard();
    cx.simulate_keystrokes("up");
    assert_eq!(pane.draft(cx), "second");
    cx.simulate_keystrokes("up");
    assert_eq!(pane.draft(cx), "first");
    cx.simulate_keystrokes("up");
    assert_eq!(
        pane.draft(cx),
        "first",
        "the oldest prompt is the end of the road"
    );
    cx.simulate_keystrokes("down");
    assert_eq!(pane.draft(cx), "second");
    cx.simulate_keystrokes("down");
    assert_eq!(pane.draft(cx), "");
    // A recalled prompt is sent like a typed one.
    cx.simulate_keystrokes("up enter");
    assert_eq!(pane.heard(), [Command::Prompt(text("second"))]);
}

#[gpui::test]
fn slash_commands_from_the_backend_reach_the_field(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready(),
            Update::Commands(vec![choice("compact", "compact")]),
        ],
        cx,
    );
    cx.simulate_input("/c");
    assert!(cx.debug_bounds("slash-command-0").is_some());
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.draft(cx), "/compact ");
    assert_eq!(pane.heard(), [], "choosing a command does not send it");
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.heard(), [Command::Prompt(text("/compact "))]);
}

#[gpui::test]
fn the_usage_line_is_there_only_for_reported_values(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready()], cx);
    assert!(cx.debug_bounds("usage").is_none());
    pane.say(
        [Update::Usage(Usage {
            output_tokens: Some(640),
            ..Usage::default()
        })],
        cx,
    );
    assert!(cx.debug_bounds("usage").is_some());
}

#[gpui::test]
fn the_keyboard_returns_to_the_field_when_a_focused_control_leaves(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready(), Update::TurnStarted, ask(1, "t1")], cx);
    // Focus sits on a permission button when the turn is cancelled from elsewhere.
    cx.simulate_keystrokes("shift-tab");
    cx.simulate_input("lost?");
    assert_eq!(pane.draft(cx), "", "the button has the keyboard");
    pane.say([Update::TurnEnded(Stop::Cancelled)], cx);
    assert!(cx.debug_bounds("answer-once").is_none());
    cx.simulate_input("back");
    assert_eq!(pane.draft(cx), "back");
}

#[gpui::test]
fn a_pointer_click_in_the_pane_leaves_the_keyboard_in_the_field(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready(),
            Update::TurnStarted,
            Update::Tool(tool("t1", ToolStatus::Done)),
        ],
        cx,
    );
    // On a control: it acts, and typing still lands in the field.
    pane.click("tool-0", cx);
    cx.simulate_input("a");
    // On nothing in particular: the pane is the field.
    let list = pane.view.read_with(cx, |view, _| view.list.clone());
    cx.simulate_click(list.viewport_bounds().center(), Modifiers::none());
    cx.simulate_input("b");
    assert_eq!(pane.draft(cx), "ab");
    assert!(pane.view.read_with(cx, |view, _| {
        let Entry::Tool(call) = &view.thread().entries()[0] else {
            panic!("a tool call");
        };
        view.tool_open(0, call)
    }));
}

#[gpui::test]
fn typing_does_not_rebuild_the_transcript(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [ready(), Update::AssistantChunk("A long answer.".into())],
        cx,
    );
    // The first character changes what the send button is; the rest change only the field.
    cx.simulate_input("a");
    cx.run_until_parked();
    let frames = pane.view.read_with(cx, |view, _| view.frames);
    cx.simulate_input("bcdef");
    cx.simulate_keystrokes("left left shift-left backspace");
    cx.run_until_parked();
    assert_eq!(pane.draft(cx), "abcef");
    assert_eq!(pane.view.read_with(cx, |view, _| view.frames), frames);
}

#[gpui::test]
fn the_header_can_be_left_to_the_pane_around_the_view(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready(), Update::Title("Retry queue".into())], cx);
    let list = pane.view.read_with(cx, |view, _| view.list.clone());
    let header = cx.debug_bounds("header").expect("shown by default");
    assert_eq!(list.viewport_bounds().top(), header.bottom());

    pane.view
        .update(cx, |view, cx| view.set_header_visible(false, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("header").is_none());
    assert_eq!(
        list.viewport_bounds().top(),
        px(0.),
        "the transcript takes the room"
    );
    // What the header showed still reaches the pane that now shows it.
    pane.say([Update::TurnStarted], cx);
    assert_eq!(
        pane.events.borrow().as_slice(),
        [
            ThreadViewEvent::State(ThreadState::Idle),
            ThreadViewEvent::Title("Retry queue".into()),
            ThreadViewEvent::State(ThreadState::Working),
        ]
    );

    pane.view
        .update(cx, |view, cx| view.set_header_visible(true, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("header").is_some());
}

// ---------------------------------------------------------------- the prompt's goodies

fn ready_with(capabilities: Capabilities) -> Update {
    Update::Ready {
        session: "s1".into(),
        capabilities,
        modes: vec![choice("low", "Effort: low"), choice("high", "Effort: high")],
        mode: Some("high".into()),
        models: vec![choice("opus", "Opus 4.5"), choice("sonnet", "Sonnet 4.5")],
        model: Some("opus".into()),
    }
}

fn pictures_ok() -> Capabilities {
    Capabilities {
        images: true,
        ..Capabilities::default()
    }
}

/// A small PNG.
fn png() -> Vec<u8> {
    crate::pictures::tests::picture(24, 16, image::ImageFormat::Png)
}

fn paste_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-v"
    }
}

fn force_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    }
}

/// Runs the pane until `done` holds, giving threads of its own (the file
/// index) time to finish.
fn settle_until(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
    for _ in 0..400 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the pane did not settle");
}

/// A project with a few files and a folder.
fn project() -> tempfile::TempDir {
    let folder = tempfile::tempdir().expect("a directory");
    let root = folder.path();
    std::fs::create_dir_all(root.join("src/composer")).expect("folders");
    std::fs::write(
        root.join("src/main.rs"),
        "fn main() {}\nfn other() {}\nfn third() {}\n",
    )
    .expect("a file");
    std::fs::write(root.join("src/composer/view.rs"), "x").expect("a file");
    std::fs::write(root.join("a b.txt"), "spaced").expect("a file");
    std::fs::write(root.join("README.md"), "# readme").expect("a file");
    folder
}

#[gpui::test]
fn a_dollar_lists_the_project_s_files_and_the_link_goes_with_the_prompt(cx: &mut TestAppContext) {
    let folder = project();
    let (pane, cx) = Pane::open_in(cx, folder.path());
    pane.say([ready()], cx);
    cx.simulate_input("look at $mai");
    settle_until(cx, |cx| cx.debug_bounds("file-match-0").is_some());
    let first = pane.view.read_with(cx, |view, cx| {
        view.dock.read(cx).composer.read(cx).matches()[0]
            .name
            .clone()
    });
    assert_eq!(first, "src/main.rs");
    // Tab takes it, with a space after: the link is done.
    cx.simulate_keystrokes("tab");
    assert_eq!(pane.draft(cx), "look at $src/main.rs ");
    settle_until(cx, |cx| cx.debug_bounds("link-chip-0").is_some());
    assert!(cx.debug_bounds("file-match-0").is_none(), "the list closed");

    // Escape closes a list and leaves the text; the link stays a chip.
    cx.simulate_input("and $src/");
    settle_until(cx, |cx| cx.debug_bounds("file-match-0").is_some());
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("file-match-0").is_none());
    assert_eq!(pane.draft(cx), "look at $src/main.rs and $src/");
    cx.simulate_keystrokes(
        "backspace backspace backspace backspace backspace backspace backspace backspace backspace",
    );
    cx.simulate_input("please");
    cx.simulate_keystrokes("enter");
    // kou's backend reads the link from the text, as the cockpits write it.
    assert_eq!(
        pane.heard(),
        [Command::Prompt(text("look at $src/main.rs please"))]
    );
}

#[gpui::test]
fn a_backend_that_takes_files_gets_their_lines_and_quoted_paths_complete(cx: &mut TestAppContext) {
    let folder = project();
    let (pane, cx) = Pane::open_in(cx, folder.path());
    pane.say(
        [ready_with(Capabilities {
            embedded_files: true,
            ..Capabilities::default()
        })],
        cx,
    );
    cx.simulate_input("$\"a b");
    settle_until(cx, |cx| cx.debug_bounds("file-match-0").is_some());
    cx.simulate_keystrokes("tab");
    assert_eq!(pane.draft(cx), "$\"a b.txt\" ");
    cx.simulate_input("and $src/main.rs:2-3");
    settle_until(cx, |cx| {
        pane.view.read_with(cx, |view, cx| {
            view.dock.read(cx).composer.read(cx).links().len()
        }) == 2
    });
    let details: Vec<(String, u32, u32)> = pane.view.read_with(cx, |view, cx| {
        view.dock
            .read(cx)
            .composer
            .read(cx)
            .links()
            .iter()
            .map(|link| (link.label.clone(), link.start_line, link.end_line))
            .collect()
    });
    assert_eq!(
        details,
        [
            ("$\"a b.txt\"".to_owned(), 0, 0),
            ("$src/main.rs:2-3".to_owned(), 2, 3)
        ]
    );
    cx.simulate_keystrokes("enter");
    let root = folder.path();
    assert_eq!(
        pane.heard(),
        [Command::Prompt(vec![
            Block::Text("$\"a b.txt\" and $src/main.rs:2-3".into()),
            Block::File {
                path: root.join("a b.txt"),
                text: Some("spaced".into()),
            },
            Block::File {
                path: root.join("src/main.rs"),
                text: Some("fn other() {}\nfn third() {}".into()),
            },
        ])]
    );
}

#[gpui::test]
fn a_link_chip_is_removed_by_its_cross_and_by_backspace(cx: &mut TestAppContext) {
    let folder = project();
    let (pane, cx) = Pane::open_in(cx, folder.path());
    pane.say([ready()], cx);
    cx.simulate_input("see $README.md and $src/main.rs now");
    settle_until(cx, |cx| cx.debug_bounds("link-chip-1").is_some());
    pane.click("link-remove-0", cx);
    assert_eq!(pane.draft(cx), "see and $src/main.rs now");
    settle_until(cx, |cx| cx.debug_bounds("link-chip-1").is_none());
    // By keyboard: Shift+Tab from the field reaches the chip, Backspace takes it out.
    cx.simulate_keystrokes("shift-tab");
    cx.simulate_keystrokes("backspace");
    assert_eq!(pane.draft(cx), "see and now");
    // The keyboard is back in the field.
    cx.simulate_input("!");
    assert_eq!(pane.draft(cx), "see and now!");
}

#[gpui::test]
fn a_slash_runs_the_view_s_commands_and_sends_the_backend_s(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready_with(Capabilities::default()),
            Update::Commands(vec![
                choice("compact", "compact"),
                choice("continue", "continue"),
            ]),
            Update::Offers(Offers {
                steer: false,
                rewind: false,
                arguments: vec![("compact".into(), "[focus]".into())],
            }),
            Update::AssistantChunk("The **answer**.".into()),
            Update::TurnStarted,
            Update::TurnEnded(Stop::Finished),
        ],
        cx,
    );
    // Copy, a command of the view's: it runs here and sends nothing.
    cx.simulate_input("/cop");
    assert!(cx.debug_bounds("slash-command-0").is_some());
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.heard(), []);
    assert_eq!(pane.draft(cx), "");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("The **answer**.")
    );

    // A command whose argument is optional runs at once, and goes to the backend.
    cx.simulate_input("/comp");
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.heard(), [Command::Prompt(text("/compact"))]);
    pane.say([Update::TurnStarted, Update::TurnEnded(Stop::Finished)], cx);

    // An argument stage: the backend's models, the current one first.
    cx.simulate_input("/model ");
    let names: Vec<String> = pane.view.read_with(cx, |view, cx| {
        view.dock
            .read(cx)
            .composer
            .read(cx)
            .matches()
            .iter()
            .map(|item| item.name.clone())
            .collect()
    });
    assert_eq!(names, ["opus", "sonnet"]);
    cx.simulate_input("son");
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.heard(), [Command::SetModel("sonnet".into())]);
    // By its alias, effort is the mode.
    cx.simulate_input("/effort low");
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.heard(), [Command::SetMode("low".into())]);
    // Expand and collapse open and fold everything.
    pane.say([Update::Tool(tool("t1", ToolStatus::Done))], cx);
    let open = |pane: &Pane, cx: &mut VisualTestContext| {
        pane.view.read_with(cx, |view, _| {
            let (index, call) = view
                .thread()
                .entries()
                .iter()
                .enumerate()
                .find_map(|(index, entry)| match entry {
                    Entry::Tool(call) => Some((index, call)),
                    _ => None,
                })
                .expect("a tool call");
            view.tool_open(index, call)
        })
    };
    assert!(!open(&pane, cx), "a finished call is folded");
    cx.simulate_input("/expand");
    cx.simulate_keystrokes("enter");
    assert!(open(&pane, cx));
    cx.simulate_input("/collapse");
    cx.simulate_keystrokes("enter");
    assert!(!open(&pane, cx));
}

#[gpui::test]
fn the_app_s_commands_leave_as_requests_once_the_app_answers_them(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready()], cx);
    cx.simulate_input("/new");
    assert!(
        pane.view.read_with(cx, |view, cx| view
            .dock
            .read(cx)
            .composer
            .read(cx)
            .catalog()
            .named("new")
            .is_none()),
        "not offered while the app does not answer"
    );
    pane.view.update(cx, |view, cx| {
        view.set_app_commands(true, cx);
        view.set_history_file(None, cx);
    });
    let asked = Rc::new(RefCell::new(Vec::new()));
    let sink = asked.clone();
    cx.update(|_, cx| {
        cx.subscribe(
            &pane.view,
            move |_, request: &crate::view::ThreadViewRequest, _| {
                sink.borrow_mut().push(request.clone())
            },
        )
        .detach();
    });
    cx.simulate_keystrokes("backspace backspace backspace backspace");
    cx.simulate_input("/new");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("/resume 0198");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        asked.borrow().as_slice(),
        [
            crate::view::ThreadViewRequest::NewSession,
            crate::view::ThreadViewRequest::ResumeSession("0198".into())
        ]
    );
    assert_eq!(pane.heard(), []);
}

#[gpui::test]
fn a_pasted_picture_is_named_shown_and_sent_as_an_image(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready_with(pictures_ok())], cx);
    let bytes = png();
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            bytes.clone(),
        )))
    });
    cx.simulate_keystrokes(paste_key());
    cx.run_until_parked();
    assert_eq!(pane.draft(cx), "[Image 1] ");
    assert!(cx.debug_bounds("picture-0").is_some(), "the strip shows it");
    cx.simulate_input("what is wrong here?");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        pane.heard(),
        [Command::Prompt(vec![
            Block::Image {
                mime: "image/png".into(),
                data: bytes.into(),
            },
            Block::Text(" what is wrong here?".into()),
        ])]
    );
    // In the transcript the picture is a thumbnail, its label in the text.
    cx.run_until_parked();
    assert!(cx.debug_bounds("message-picture-0-0").is_some());
    let shown = pane.view.read_with(cx, |view, _| match &view.cache[0] {
        Some(Parsed::User(message)) => message.text.to_string(),
        _ => panic!("the prompt was drawn"),
    });
    assert_eq!(shown, "[Image 1] what is wrong here?");
    // A click shows it large; Escape closes it.
    pane.click("message-picture-0-0", cx);
    assert!(cx.debug_bounds("picture-overlay").is_some());
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("picture-overlay").is_none());
}

/// A call that starts a subagent, two calls that subagent makes, and one
/// call of the agent's own, as an agent's adapter tells them apart.
fn delegation() -> Vec<Update> {
    let mut launch = tool("a1", ToolStatus::Running);
    launch.title = "Check the tests".into();
    launch.kind = ToolKind::Think;
    launch.agent = Some("Explore".into());
    launch.raw_input = Some(serde_json::json!({
        "description": "Check the tests",
        "subagent_type": "Explore",
        "prompt": "Run the tests of the parser and say which fail.",
    }));
    launch.content = vec![ToolContent::Text(
        "Run the tests of the parser and say which fail.".into(),
    )];
    let child = |id: &str, title: &str, status| {
        let mut call = tool(id, status);
        call.title = title.into();
        call.parent = Some("a1".into());
        call
    };
    vec![
        ready(),
        Update::UserChunk(Block::Text("are the tests fine?".into())),
        Update::TurnStarted,
        Update::Tool(launch),
        Update::Tool(child("c1", "Read src/parser.rs", ToolStatus::Done)),
        Update::Tool(child("c2", "Run cargo test -p parser", ToolStatus::Running)),
        Update::Tool(tool("own", ToolStatus::Done)),
    ]
}

#[gpui::test]
fn an_agent_the_agent_started_itself_shows_as_an_agent_with_how_it_goes(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    // Wide enough for the card beside the transcript.
    cx.simulate_resize(size(px(1900.), px(800.)));
    pane.say(delegation(), cx);
    // How the agent started by the call at entry 1 goes, and its latest.
    let goes = |cx: &mut VisualTestContext| {
        pane.view.read_with(cx, |view, _| {
            let Entry::Tool(call) = &view.thread().entries()[1] else {
                panic!("the call that starts it");
            };
            let state = view.subagent_state(1, call);
            (state, view.subagent_detail(call, state), call.work.steps)
        })
    };
    let waited = |cx: &mut VisualTestContext| {
        pane.view
            .read_with(cx, |view, _| view.around.waiting.borrow().clone())
    };
    // At work: its card says so, with the call it is making; the line over
    // the field names it among those the agent waits on.
    let (state, latest, steps) = goes(cx);
    assert_eq!((state, steps), (NestedState::Running, 2));
    assert!(latest.contains("cargo test -p parser"), "{latest}");
    assert!(cx.debug_bounds("agent-state-1").is_some());
    assert!(cx.debug_bounds("agent-latest-1").is_some());
    assert_eq!(waited(cx), ["Check the tests"]);
    // What it says is its latest, once it has said it.
    pane.say(
        [Update::AgentSaid {
            call: "a1".into(),
            text: "Two of them fail.\nLooking at the **first**.".into(),
        }],
        cx,
    );
    assert_eq!(goes(cx).1, "Looking at the first.");
    // A call of its that waits for an answer stands in the transcript with
    // its answers, and the agent's card says it needs the user.
    assert!(cx.debug_bounds("tool-3").is_none());
    pane.say([ask(7, "c2")], cx);
    assert_eq!(goes(cx).0, NestedState::Asking);
    assert!(cx.debug_bounds("tool-3").is_some(), "the call that asks");
    assert!(cx.debug_bounds("answer-once").is_some());
    assert_eq!(waited(cx), Vec::<String>::new());
    pane.click("answer-once", cx);
    assert_eq!(goes(cx).0, NestedState::Running);
    assert!(
        cx.debug_bounds("tool-3").is_none(),
        "back in its agent's card"
    );
    // Started to run in the background, it runs on after its call is over,
    // for as long as the turn that started it runs.
    pane.say(
        [Update::ToolPatch(ToolPatch {
            id: "a1".into(),
            status: Some(ToolStatus::Done),
            content: Some(vec![ToolContent::Text("Async agent launched.".into())]),
            background: Some(true),
            ..ToolPatch::default()
        })],
        cx,
    );
    let (state, latest, _) = goes(cx);
    assert_eq!(state, NestedState::Running);
    assert_eq!(latest, "Looking at the first.", "not that it was launched");
    assert_eq!(waited(cx), ["Check the tests"]);
    // The card beside lists it among the agents at work; a click on its row
    // opens its card in the transcript.
    pane.view.update(cx, |view, cx| {
        view.set_beside(
            Some(Beside {
                project: "neko-mini".into(),
                path: "/work/neko-mini".into(),
                ..Beside::default()
            }),
            cx,
        )
    });
    cx.run_until_parked();
    assert!(!pane.view.read_with(cx, |view, _| {
        let Entry::Tool(call) = &view.thread().entries()[1] else {
            panic!("the call");
        };
        view.tool_open(1, call)
    }));
    pane.click("lineage-call:a1", cx);
    assert!(pane.view.read_with(cx, |view, _| {
        let Entry::Tool(call) = &view.thread().entries()[1] else {
            panic!("the call");
        };
        view.tool_open(1, call)
    }));
    assert!(
        pane.events
            .borrow()
            .iter()
            .all(|event| !matches!(event, ThreadViewEvent::OpenNested(_)))
    );
    // The turn over, it is done: no row of it beside, nobody waited on.
    pane.say([Update::TurnEnded(Stop::Finished)], cx);
    assert_eq!(goes(cx).0, NestedState::Finished);
    assert!(cx.debug_bounds("lineage-call:a1").is_none());
    assert_eq!(waited(cx), Vec::<String>::new());
}

#[gpui::test]
fn a_command_left_running_in_the_background_shows_as_running_until_it_ends(
    cx: &mut TestAppContext,
) {
    let (pane, cx) = Pane::open(cx);
    cx.simulate_resize(size(px(1900.), px(800.)));
    let mut command = tool("b1", ToolStatus::Running);
    command.title = "`npm run dev`".into();
    pane.say(
        [
            ready(),
            Update::UserChunk(Block::Text("start the server".into())),
            Update::TurnStarted,
            Update::Tool(command),
        ],
        cx,
    );
    let behind = |cx: &mut VisualTestContext| {
        pane.view.read_with(cx, |view, _| {
            let Entry::Tool(call) = &view.thread().entries()[1] else {
                panic!("the command's call");
            };
            (
                crate::entries::runs_behind(call),
                view.around.behind.borrow().clone(),
            )
        })
    };
    assert_eq!(behind(cx), (false, Vec::new()), "it runs in the turn");
    // Its call ends while its process runs on, and the turn ends after it.
    pane.say(
        [
            Update::ToolPatch(ToolPatch {
                id: "b1".into(),
                status: Some(ToolStatus::Done),
                background: Some(true),
                ..ToolPatch::default()
            }),
            Update::AssistantChunk("Started.".into()),
            Update::TurnEnded(Stop::Finished),
        ],
        cx,
    );
    assert_eq!(pane.state(cx), ThreadState::Idle);
    assert_eq!(behind(cx), (true, vec!["npm run dev".to_owned()]));
    // The agent is idle, and the line over the field still stands: it says
    // what runs in the background, without a turn's meter or its stop.
    assert!(cx.debug_bounds("activity-background").is_some());
    assert!(cx.debug_bounds("activity-meter").is_none());
    // Its card counts for as long as it is on screen, turn or no turn.
    assert!(pane.view.read_with(cx, |view, _| view.ticking_for_tests()));
    // The card beside lists it among what runs; a click shows its card.
    pane.view.update(cx, |view, cx| {
        view.set_beside(
            Some(Beside {
                project: "neko-mini".into(),
                path: "/work/neko-mini".into(),
                ..Beside::default()
            }),
            cx,
        )
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("lineage-call:b1").is_some());
    // The agent goes on by itself when something comes of it: a turn with
    // no prompt before it, in which the command still counts as running.
    pane.say(
        [
            Update::TurnStarted,
            Update::AssistantChunk("Looking.".into()),
        ],
        cx,
    );
    assert_eq!(pane.state(cx), ThreadState::Working);
    assert!(cx.debug_bounds("activity-meter").is_some());
    assert!(cx.debug_bounds("activity-background").is_some());
    pane.say([Update::TurnEnded(Stop::Finished)], cx);
    // It ends: nothing runs any more, and the line goes.
    pane.say(
        [Update::ToolPatch(ToolPatch {
            id: "b1".into(),
            ended: Some(true),
            ..ToolPatch::default()
        })],
        cx,
    );
    assert_eq!(behind(cx), (false, Vec::new()));
    assert!(cx.debug_bounds("activity").is_none());
    assert!(cx.debug_bounds("lineage-call:b1").is_none());
}

#[gpui::test]
fn a_subagent_of_the_agent_s_own_is_a_card_that_holds_what_it_did(cx: &mut TestAppContext) {
    for look in [Look::Cockpit, Look::Studio] {
        let (pane, cx) = Pane::open(cx);
        pane.look(look, cx);
        pane.say(delegation(), cx);
        // Entries: the prompt, the call that starts the subagent, what the
        // subagent does (two calls), and a call of the agent's own.
        let told = pane.view.read_with(cx, |view, _| {
            (
                (0..5)
                    .map(|row| view.made_by_subagent(row))
                    .collect::<Vec<_>>(),
                view.subagent_calls("a1")
                    .iter()
                    .map(|call| call.title.clone())
                    .collect::<Vec<_>>(),
            )
        });
        assert_eq!(told.0, [false, false, true, true, false], "{look:?}");
        assert_eq!(
            told.1,
            ["Read src/parser.rs", "Run cargo test -p parser"],
            "{look:?}"
        );
        // The subagent is a card; what it does is no row of the transcript;
        // the agent's own call is a row as ever.
        let card = cx.debug_bounds("tool-1").expect("the subagent's card");
        assert!(cx.debug_bounds("tool-2").is_none(), "{look:?}");
        assert!(cx.debug_bounds("tool-3").is_none(), "{look:?}");
        assert!(cx.debug_bounds("tool-4").is_some(), "{look:?}");
        // Opened, it is taller by what it was asked and what it did.
        pane.click("tool-1", cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        let below = cx.debug_bounds("tool-4").expect("the agent's own call");
        assert!(
            below.top() > card.bottom() + px(100.),
            "{look:?}: the card opened over {:?}",
            below.top() - card.bottom()
        );
        // It reports, and the turn ends: the card stays where it stood.
        pane.say(
            [
                Update::ToolPatch(ToolPatch {
                    id: "c2".into(),
                    status: Some(ToolStatus::Done),
                    ..ToolPatch::default()
                }),
                Update::ToolPatch(ToolPatch {
                    id: "a1".into(),
                    status: Some(ToolStatus::Done),
                    content: Some(vec![ToolContent::Text("All 12 pass.".into())]),
                    ..ToolPatch::default()
                }),
                Update::AssistantChunk("They are fine.".into()),
            ],
            cx,
        );
        pane.say([Update::TurnEnded(Stop::Finished)], cx);
        assert!(cx.debug_bounds("tool-1").is_some(), "{look:?}");
        if look == Look::Studio {
            // The agent's own work folds into a line that counts one step:
            // what the subagent did is counted in its card.
            assert!(cx.debug_bounds("tool-4").is_none(), "{look:?}");
            let steps = pane.view.read_with(cx, |view, _| {
                let (run, _, _) = view.fold_of(4).expect("the agent's call folds");
                (run.start, run.end, view.steps_of(&run))
            });
            assert_eq!(steps, (2, 5, 1));
            assert!(cx.debug_bounds("worked-2").is_some());
        }
    }
}

#[gpui::test]
fn a_picture_a_tool_gave_back_is_shown_in_its_card_and_opens_large(cx: &mut TestAppContext) {
    for look in [Look::Cockpit, Look::Studio] {
        let (pane, cx) = Pane::open(cx);
        pane.look(look, cx);
        let mut read = tool("t1", ToolStatus::Done);
        read.title = "Read shot.png".into();
        read.kind = ToolKind::Read;
        read.content = vec![
            ToolContent::Image {
                mime: "image/png".into(),
                data: png().into(),
            },
            // One whose bytes are no picture is said to be unreadable.
            ToolContent::Image {
                mime: "image/png".into(),
                data: b"not a picture".to_vec().into(),
            },
        ];
        pane.say(
            [
                ready(),
                Update::UserChunk(Block::Text("look at it".into())),
                Update::TurnStarted,
                Update::Tool(read),
            ],
            cx,
        );
        // Closed, nothing of it is decoded.
        assert!(
            pane.view.read_with(cx, |view, _| view.shots.is_empty()),
            "{look:?}"
        );
        assert!(cx.debug_bounds("tool-picture-1-0").is_none(), "{look:?}");
        // Opened: the picture, whole, no larger than it is.
        pane.click("tool-1", cx);
        cx.run_until_parked();
        let picture = cx
            .debug_bounds("tool-picture-1-0")
            .unwrap_or_else(|| panic!("{look:?}: the picture is in the card"));
        assert_eq!(picture.size, size(px(24.), px(16.)), "{look:?}");
        assert!(cx.debug_bounds("tool-picture-1-1").is_none(), "{look:?}");
        let failed = pane.view.read_with(cx, |view, _| {
            matches!(view.shots.get(&(1, 1)), Some(crate::view::Shot::Failed))
        });
        assert!(failed, "{look:?}: the second could not be read");
        // A click shows it large; Escape closes that.
        pane.click("tool-picture-1-0", cx);
        assert!(cx.debug_bounds("picture-overlay").is_some(), "{look:?}");
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds("picture-overlay").is_none(), "{look:?}");
    }
}

#[gpui::test]
fn backspace_takes_a_picture_after_its_label_or_the_last_at_the_start(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready_with(pictures_ok())], cx);
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            png(),
        )))
    });
    cx.simulate_keystrokes(paste_key());
    cx.simulate_keystrokes(paste_key());
    cx.run_until_parked();
    assert_eq!(pane.draft(cx), "[Image 1] [Image 2] ");
    assert!(cx.debug_bounds("picture-1").is_some());
    // Right after a label, Backspace takes the whole label.
    cx.simulate_keystrokes("backspace backspace");
    assert_eq!(pane.draft(cx), "[Image 1] ");
    cx.run_until_parked();
    assert!(cx.debug_bounds("picture-1").is_none());
    // At the start of the field, it takes the last picture.
    let start = if cfg!(target_os = "macos") {
        "cmd-up"
    } else {
        "ctrl-home"
    };
    cx.simulate_keystrokes(start);
    cx.simulate_keystrokes("backspace");
    assert_eq!(pane.draft(cx).trim(), "");
    // And the cross does the same.
    cx.simulate_keystrokes(paste_key());
    cx.run_until_parked();
    pane.click("picture-remove-0", cx);
    assert!(!pane.draft(cx).contains("[Image"));
}

#[gpui::test]
fn limits_are_said_before_anything_is_sent(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    // This agent takes no pictures: a paste says so and attaches nothing.
    pane.say([ready()], cx);
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            png(),
        )))
    });
    cx.simulate_keystrokes(paste_key());
    cx.run_until_parked();
    assert_eq!(pane.draft(cx), "");
    let notice = pane.view.read_with(cx, |view, cx| {
        view.dock
            .read(cx)
            .composer
            .read(cx)
            .notice()
            .map(str::to_owned)
    });
    assert!(notice.is_some_and(|notice| notice.contains("takes no pictures")));
    // Something that is no picture says why, and leaves no label behind.
    pane.say([ready_with(pictures_ok())], cx);
    pane.view.update(cx, |view, cx| {
        view.dock.update(cx, |dock, cx| {
            dock.composer.update(cx, |composer, cx| {
                composer.attach(b"not a picture".to_vec(), cx)
            })
        })
    });
    cx.run_until_parked();
    assert_eq!(pane.draft(cx).trim(), "");
    let notice = pane.view.read_with(cx, |view, cx| {
        view.dock
            .read(cx)
            .composer
            .read(cx)
            .notice()
            .map(str::to_owned)
    });
    assert!(
        notice
            .as_deref()
            .is_some_and(|notice| notice.contains("not a picture")),
        "{notice:?}"
    );
}

#[gpui::test]
fn dropped_files_attach_pictures_and_link_the_rest(cx: &mut TestAppContext) {
    let folder = project();
    std::fs::write(folder.path().join("shot.png"), png()).expect("a picture");
    let (pane, cx) = Pane::open_in(cx, folder.path());
    pane.say([ready_with(pictures_ok())], cx);
    let frame = cx.debug_bounds("frame").expect("the field's frame");
    let paths = gpui::ExternalPaths(
        [
            folder.path().join("shot.png"),
            folder.path().join("README.md"),
        ]
        .into_iter()
        .collect(),
    );
    cx.simulate_event(gpui::FileDropEvent::Entered {
        position: frame.center(),
        paths,
    });
    cx.simulate_event(gpui::FileDropEvent::Submit {
        position: frame.center(),
    });
    cx.run_until_parked();
    assert_eq!(pane.draft(cx), "[Image 1] $README.md ");
    assert!(cx.debug_bounds("picture-0").is_some());
}

#[gpui::test]
fn history_is_shared_with_the_cockpits_and_searched_with_ctrl_r(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().expect("a directory");
    let file = folder.path().join(".harness").join("ui-history.json");
    std::fs::create_dir_all(file.parent().expect("a folder")).expect("a folder");
    std::fs::write(
        &file,
        r#"[{"Prompt":"from the terminal","SessionID":"t","Title":"","Model":"","CreatedAt":"2026-11-10T10:00:00Z"},
            {"Prompt":"fix the retry queue","SessionID":"t","Title":"","Model":"","CreatedAt":"2026-11-10T11:00:00Z"}]"#,
    )
    .expect("history");
    let (pane, cx) = Pane::open_in(cx, folder.path());
    pane.say([ready()], cx);
    cx.simulate_keystrokes("up");
    assert_eq!(pane.draft(cx), "fix the retry queue");
    cx.simulate_keystrokes("up");
    assert_eq!(pane.draft(cx), "from the terminal");
    cx.simulate_keystrokes("down down");
    assert_eq!(pane.draft(cx), "");

    // Sent here, a prompt joins the file the terminal reads.
    cx.simulate_input("from the desktop");
    cx.simulate_keystrokes("enter");
    settle_until(cx, |_| {
        std::fs::read_to_string(&file).is_ok_and(|text| text.contains("from the desktop"))
    });
    let written = std::fs::read_to_string(&file).expect("history");
    assert!(written.contains("from the terminal"), "{written}");

    // Ctrl+R lists earlier prompts that hold what is typed.
    cx.simulate_input("retry");
    cx.simulate_keystrokes("ctrl-r");
    assert!(cx.debug_bounds("history-item-0").is_some());
    assert!(cx.debug_bounds("history-item-1").is_none());
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.draft(cx), "fix the retry queue");
}

#[gpui::test]
fn the_queue_reorders_drops_and_runs_when_the_turn_ends(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready(),
            Update::Offers(Offers {
                steer: true,
                rewind: false,
                arguments: Vec::new(),
            }),
            Update::TurnStarted,
        ],
        cx,
    );
    for prompt in ["first", "second", "third"] {
        cx.simulate_input(prompt);
        cx.simulate_keystrokes("enter");
    }
    assert_eq!(
        pane.queued(cx),
        [
            ("first".to_owned(), false),
            ("second".to_owned(), false),
            ("third".to_owned(), false)
        ]
    );
    // Up from the empty field goes to the queue's last row; Alt+Up moves it up.
    cx.simulate_keystrokes("up");
    cx.simulate_keystrokes("alt-up");
    assert_eq!(pane.queued(cx)[1].0, "third");
    // Backspace drops it; the keyboard moves to a neighbour.
    cx.simulate_keystrokes("backspace");
    assert_eq!(
        pane.queued(cx),
        [("first".to_owned(), false), ("second".to_owned(), false)]
    );
    // Force: the running turn gets it, and it waits as forced until the agent takes it.
    cx.simulate_keystrokes(force_key());
    assert_eq!(pane.requests(), [Request::Steer(text("second"))]);
    assert_eq!(
        pane.queued(cx),
        [("second".to_owned(), true), ("first".to_owned(), false)]
    );
    pane.say([Update::UserChunk(Block::Text("second".into()))], cx);
    assert_eq!(pane.queued(cx), [("first".to_owned(), false)]);
    // The turn ends well: the next queued prompt runs.
    pane.say([Update::TurnEnded(Stop::Finished)], cx);
    assert_eq!(pane.heard(), [Command::Prompt(text("first"))]);
    assert!(pane.queued(cx).is_empty());
}

#[gpui::test]
fn force_in_hands_the_running_turn_a_message(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready(),
            Update::Offers(Offers {
                steer: true,
                rewind: false,
                arguments: Vec::new(),
            }),
            Update::TurnStarted,
        ],
        cx,
    );
    cx.simulate_input("also check the tests");
    assert!(
        cx.debug_bounds("force").is_some(),
        "the force button is offered"
    );
    cx.simulate_keystrokes(force_key());
    assert_eq!(
        pane.requests(),
        [Request::Steer(text("also check the tests"))]
    );
    assert_eq!(pane.heard(), [], "not a prompt of its own");
    // The run ends before the agent took it: it waits as the next prompt.
    pane.say([Update::TurnEnded(Stop::Finished)], cx);
    assert_eq!(
        pane.heard(),
        [Command::Prompt(text("also check the tests"))]
    );
}

#[gpui::test]
fn a_message_forced_on_an_agent_that_takes_none_at_work_stops_its_turn_and_goes_next(
    cx: &mut TestAppContext,
) {
    for look in [Look::Cockpit, Look::Studio] {
        let (pane, cx) = Pane::open(cx);
        pane.look(look, cx);
        // An agent that offers no way into its running turn.
        pane.say([ready(), Update::TurnStarted], cx);
        cx.simulate_input("later");
        cx.simulate_keystrokes("enter");
        assert_eq!(pane.heard(), [], "{look:?}: queued, nothing said yet");
        cx.simulate_input("do this instead");
        assert!(
            cx.debug_bounds("force").is_some(),
            "{look:?}: the force button is offered for such an agent too"
        );
        cx.simulate_keystrokes(force_key());
        // The turn is stopped, and the message heads the queue meanwhile.
        assert_eq!(pane.heard(), [Command::Cancel], "{look:?}");
        assert_eq!(pane.requests(), [], "{look:?}: nothing goes into the turn");
        assert_eq!(
            pane.queued(cx),
            [
                ("do this instead".to_owned(), false),
                ("later".to_owned(), false)
            ],
            "{look:?}"
        );
        assert_eq!(pane.draft(cx), "", "{look:?}");
        // Stopped: it goes at once, though a stop pauses the queue.
        pane.say([Update::TurnEnded(Stop::Cancelled)], cx);
        assert_eq!(
            pane.heard(),
            [Command::Prompt(text("do this instead"))],
            "{look:?}"
        );
        assert_eq!(pane.queued(cx), [("later".to_owned(), false)], "{look:?}");
        // The rest of the queue is as it was: it runs when this turn ends.
        pane.say([Update::TurnStarted], cx);
        pane.say([Update::TurnEnded(Stop::Finished)], cx);
        assert_eq!(pane.heard(), [Command::Prompt(text("later"))], "{look:?}");
        assert!(pane.queued(cx).is_empty(), "{look:?}");
    }
}

#[gpui::test]
fn the_force_key_over_an_empty_field_forces_what_heads_the_queue(cx: &mut TestAppContext) {
    for steers in [false, true] {
        let (pane, cx) = Pane::open(cx);
        pane.say(
            [
                ready(),
                Update::Offers(Offers {
                    steer: steers,
                    rewind: false,
                    arguments: Vec::new(),
                }),
                Update::TurnStarted,
            ],
            cx,
        );
        // Nothing queued: the key is not ours.
        cx.simulate_keystrokes(force_key());
        assert_eq!(pane.heard(), [], "steers: {steers}");
        for prompt in ["first", "second"] {
            cx.simulate_input(prompt);
            cx.simulate_keystrokes("enter");
        }
        cx.simulate_keystrokes(force_key());
        if steers {
            // Handed to the running turn.
            assert_eq!(pane.requests(), [Request::Steer(text("first"))]);
            assert_eq!(pane.heard(), []);
        } else {
            // The turn is stopped for it, and it goes when it has stopped.
            assert_eq!(pane.heard(), [Command::Cancel]);
            // Pressed again meanwhile, nothing more happens.
            cx.simulate_keystrokes(force_key());
            assert_eq!(pane.heard(), []);
            pane.say([Update::TurnEnded(Stop::Cancelled)], cx);
            assert_eq!(pane.heard(), [Command::Prompt(text("first"))]);
            assert_eq!(pane.queued(cx), [("second".to_owned(), false)]);
        }
    }
}

#[gpui::test]
fn a_queued_message_forced_on_such_an_agent_goes_first_and_a_stop_by_hand_still_pauses(
    cx: &mut TestAppContext,
) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready(), Update::TurnStarted], cx);
    for prompt in ["first", "second"] {
        cx.simulate_input(prompt);
        cx.simulate_keystrokes("enter");
    }
    // Up from the empty field is the queue's last row: forced, it goes first.
    cx.simulate_keystrokes("up");
    cx.simulate_keystrokes(force_key());
    assert_eq!(pane.heard(), [Command::Cancel]);
    assert_eq!(
        pane.queued(cx),
        [("second".to_owned(), false), ("first".to_owned(), false)]
    );
    pane.say([Update::TurnEnded(Stop::Cancelled)], cx);
    assert_eq!(pane.heard(), [Command::Prompt(text("second"))]);
    // That turn stopped by hand: what is queued waits, as after any stop.
    pane.say([Update::TurnStarted], cx);
    cx.simulate_keystrokes("escape escape");
    assert_eq!(pane.heard(), [Command::Cancel]);
    pane.say([Update::TurnEnded(Stop::Cancelled)], cx);
    assert_eq!(pane.heard(), []);
    assert_eq!(pane.queued(cx), [("first".to_owned(), false)]);
}

#[gpui::test]
fn an_earlier_prompt_goes_back_in_the_field_with_its_pictures(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready_with(pictures_ok()),
            Update::UserChunk(Block::Text("compare ".into())),
            Update::UserChunk(Block::Image {
                mime: "image/png".into(),
                data: png().into(),
            }),
            Update::TurnStarted,
            Update::AssistantChunk("They match.\n\n```rust\nfn main() {}\n```".into()),
            Update::TurnEnded(Stop::Finished),
        ],
        cx,
    );
    // A backend that does not rewind cannot edit a prompt: its control
    // puts the prompt back in the field, and is called so.
    assert!(cx.debug_bounds("edit-prompt-0").is_none());
    pane.click("reuse-prompt-0", cx);
    cx.run_until_parked();
    assert_eq!(pane.draft(cx), "compare [Image 1]");
    assert_eq!(
        edit_help(&pane, cx).as_deref(),
        Some("Put prompt n, by default the last, back in the field")
    );
    assert!(cx.debug_bounds("picture-0").is_some());
    // The code block's control copies the code.
    pane.click("copy-code-0", cx);
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("fn main() {}")
    );
}

#[gpui::test]
fn what_moves_in_the_prompt_area_does_not_rebuild_the_transcript(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready_with(pictures_ok()),
            Update::Commands(vec![choice("compact", "compact")]),
            Update::AssistantChunk("A long answer.".into()),
            Update::TurnStarted,
        ],
        cx,
    );
    let frames = |cx: &mut VisualTestContext| {
        cx.run_until_parked();
        pane.view.read_with(cx, |view, _| view.frames)
    };
    // The first character turns the stop button into queue and force.
    cx.simulate_input("a");
    let start = frames(cx);
    // Typing on, a list opening, moving and closing: the prompt area alone.
    cx.simulate_input("bcd");
    cx.simulate_keystrokes("backspace backspace backspace backspace");
    cx.simulate_input("/c");
    cx.simulate_keystrokes("down up escape backspace backspace");
    cx.simulate_input("later");
    assert_eq!(
        frames(cx),
        start,
        "nothing of the transcript was built again"
    );
    // The queue appears: the prompt area grew, and the transcript makes room once.
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.queued(cx), [("later".to_owned(), false)]);
    assert_eq!(frames(cx), start + 1);
    // A picture's strip appears: once more, once.
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            png(),
        )))
    });
    cx.simulate_keystrokes(paste_key());
    assert_eq!(frames(cx), start + 2);
    // The transcript's last row keeps the room the prompt area takes.
    let room = pane.view.read_with(cx, |view, _| view.dock_height);
    let frame = cx.debug_bounds("frame").expect("the field");
    assert!(room > frame.size.height.as_f32(), "{room} for {frame:?}");
}

// ------------------------------------------------------------------ zoom

/// Opens every run of work that has folded, as a click on each of their
/// lines would: for what looks at a call an answer has followed.
fn unfold_work(pane: &Pane, cx: &mut VisualTestContext) {
    pane.view.update(cx, |view, cx| {
        for row in 0..view.thread().entries().len() {
            if let Some((run, false, true)) = view.fold_of(row) {
                view.toggle_worked(run.start, cx);
            }
        }
    });
    cx.run_until_parked();
}

/// A conversation with a bit of everything the transcript draws: a prompt, a
/// tool call, an answer whose first paragraph is long, a request, the pickers.
fn zoom_conversation() -> Vec<Update> {
    vec![
        ready_with(Capabilities::default()),
        Update::Commands(vec![
            choice("compact", "compact"),
            choice("review", "review"),
        ]),
        Update::UserChunk(Block::Text("Объясни очередь повторов".into())),
        Update::TurnStarted,
        Update::Tool(tool("t1", ToolStatus::Done)),
        Update::AssistantChunk(format!(
            "{}\n\n```rust\nfn backoff(attempt: u32) -> u64 {{ 1 << attempt }}\n```",
            LONG_PARAGRAPH
        )),
        ask(7, "t2"),
    ]
}

const LONG_PARAGRAPH: &str = "The task stays in the queue until a worker acknowledges it. \
     Instead the worker takes a lease of thirty seconds and renews it while it works; if the \
     worker dies, the lease runs out and the next worker takes the task. A retry can happen, \
     so handlers have to be idempotent, and the queue counts attempts so that a task that \
     keeps failing ends up in the dead letters instead of going round for ever.";

/// What the zoom tests compare: elements with a name, as drawn. Rows further
/// down the transcript stand where the rows above them end, each rounded to
/// whole device pixels on its own, so only the size of those is compared;
/// the others are compared edge by edge.
const NAMED: [&str; 9] = [
    "header",
    "tool-1",
    "reply-2",
    "answer-once",
    "frame",
    "attach",
    "mode-picker",
    "model-picker",
    "stop",
];

/// How many one-pixel borders, which do not scale, lie between the window's
/// edge and an edge of a named element, at most: the code block's around an
/// answer's text, the field's frame, the pickers' and the buttons' own. The
/// mode's control comes after the model's, whose two borders and two texts
/// (each measured in whole pixels) it is moved by too.
fn borders(name: &str) -> f32 {
    match name {
        "header" => 0.,
        // The card around the call's head.
        "tool-1" => 1.,
        "mode-picker" => 9.,
        "model-picker" => 5.,
        _ => 2.,
    }
}

/// Whether only the size of a named element is compared. See [`NAMED`].
fn size_only(name: &str) -> bool {
    ["reply-2", "answer-once"].contains(&name)
}

impl Pane {
    /// A pane drawn on a canvas at `zoom`: the window is the 900 by 700 pane
    /// times the zoom, and the view follows the zoom through `handle`.
    fn open_zoomed(
        cx: &mut TestAppContext,
        zoom: f32,
    ) -> (Self, Rc<std::cell::Cell<f32>>, &mut VisualTestContext) {
        let (pane, cx) = Self::open(cx);
        let handle = Rc::new(std::cell::Cell::new(zoom));
        pane.view
            .update(cx, |view, cx| view.follow_zoom(Some(handle.clone()), cx));
        cx.simulate_resize(size(px(900. * zoom), px(700. * zoom)));
        cx.run_until_parked();
        (pane, handle, cx)
    }

    fn named(&self, cx: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
        NAMED
            .iter()
            .map(|name| {
                cx.debug_bounds(name)
                    .unwrap_or_else(|| panic!("{name} is on screen"))
            })
            .collect()
    }
}

/// Whether `zoomed` is `one` times `factor` within a pixel (edge by edge, or
/// in size for [`size_only`] elements), give or take what the [`borders`]
/// before an edge move it. A row of the transcript stands where the rows
/// above it end, each rounded to half a pixel: half a pixel more for the one
/// row above the tool call.
fn scaled_by(name: &str, one: Bounds<Pixels>, zoomed: Bounds<Pixels>, factor: f32) -> bool {
    let rows = if name == "tool-1" { 0.5 } else { 0. };
    let slack = px(1. + rows + borders(name) * (factor - 1.).abs());
    let pairs = if size_only(name) {
        vec![
            (one.size.width, zoomed.size.width),
            (one.size.height, zoomed.size.height),
        ]
    } else {
        vec![
            (one.left(), zoomed.left()),
            (one.top(), zoomed.top()),
            (one.right(), zoomed.right()),
            (one.bottom(), zoomed.bottom()),
        ]
    };
    pairs
        .into_iter()
        .all(|(one, zoomed)| (one * factor - zoomed).abs() <= slack)
}

/// The named elements of the zoom conversation, drawn at `zoom` in a window
/// of its own (or at the pane's own size without one).
fn draw_named(cx: &mut TestAppContext, zoom: Option<f32>) -> Vec<Bounds<Pixels>> {
    let (pane, cx) = match zoom {
        Some(zoom) => {
            let (pane, _, cx) = Pane::open_zoomed(cx, zoom);
            (pane, cx)
        }
        None => Pane::open(cx),
    };
    pane.say(zoom_conversation(), cx);
    unfold_work(&pane, cx);
    pane.named(cx)
}

#[gpui::test]
fn zoomed_the_pane_is_its_picture_at_one_scaled(cx: &mut TestAppContext) {
    let one = draw_named(cx, None);
    // Any zoom, not only the sizes glyphs are rasterised at: the content is
    // laid out at the zoom itself and fills the window.
    for zoom in [0.6, 0.83, 1.5, 2., 2.83] {
        assert_ne!(
            kou_ui::scale::quantize(1.5),
            1.5,
            "1.5 is between two glyph sizes"
        );
        let zoomed = draw_named(cx, Some(zoom));
        for ((name, one), drawn) in NAMED.iter().zip(&one).zip(zoomed) {
            assert!(
                scaled_by(name, *one, drawn, zoom),
                "{name} at {zoom}: {drawn:?}, at 1: {one:?}"
            );
        }
    }
}

#[gpui::test]
fn a_long_paragraph_keeps_its_lines_at_any_zoom(cx: &mut TestAppContext) {
    let lines = |cx: &mut TestAppContext, zoom: Option<f32>| {
        let (pane, cx) = match zoom {
            Some(zoom) => {
                let (pane, _, cx) = Pane::open_zoomed(cx, zoom);
                (pane, cx)
            }
            None => Pane::open(cx),
        };
        pane.say([ready(), Update::AssistantChunk(LONG_PARAGRAPH.into())], cx);
        let reply = cx.debug_bounds("reply-0").expect("the answer is drawn");
        let factor = zoom.unwrap_or(1.);
        (reply.size.height / (px(crate::prose::LINE) * factor)).round() as usize
    };
    let one = lines(cx, None);
    assert!(one >= 4, "{one} lines: not long enough to wrap anywhere");
    for zoom in [0.6, 1., 1.5, 2.5] {
        assert_eq!(lines(cx, Some(zoom)), one, "lines at {zoom}");
    }
}

/// Every text drawn in one frame, in the order they were placed: the bounds
/// of each, drawn again from scratch at the zoom the pane is at.
fn drawn_texts(cx: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
    kou_ui::scale::count_text_work(true);
    kou_ui::scale::take_text_places();
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    kou_ui::scale::take_text_places()
}

/// The largest distance of `values` (taken at `zooms`) from the straight line
/// through them that fits them best: how far they are from moving with the
/// zoom. A place in a zoomed pane is its place at 1 times the zoom, plus the
/// borders before it, which do not scale.
fn off_the_line(zooms: &[f32], values: &[f32]) -> f32 {
    let count = zooms.len() as f32;
    let (mean_z, mean_v) = (
        zooms.iter().sum::<f32>() / count,
        values.iter().sum::<f32>() / count,
    );
    let slope = zooms
        .iter()
        .zip(values)
        .map(|(z, v)| (z - mean_z) * (v - mean_v))
        .sum::<f32>()
        / zooms.iter().map(|z| (z - mean_z).powi(2)).sum::<f32>();
    zooms
        .iter()
        .zip(values)
        .map(|(z, v)| (v - (mean_v + slope * (z - mean_z))).abs())
        .fold(0., f32::max)
}

#[gpui::test]
fn a_zoom_gesture_moves_everything_every_frame_and_no_word_to_another_line(
    cx: &mut TestAppContext,
) {
    // One pane zoomed as a pinch zooms it: a little more every frame, from
    // 60% to 250%, on a display of one device pixel to a pixel, where
    // rounding shows most. Paragraphs of several lengths, so that some line
    // or other ends within a hair of the edge at one zoom or another.
    // A pane tall enough for all six paragraphs at the transcript's size.
    const TALL: f32 = 1200.;
    let (pane, zoom, cx) = Pane::open_zoomed(cx, 0.6);
    cx.simulate_resize(size(px(900. * 0.6), px(TALL * 0.6)));
    cx.simulate_scale_factor_change(1.);
    let mut said = vec![ready()];
    let words: Vec<&str> = LONG_PARAGRAPH.split(' ').collect();
    let paragraphs: Vec<String> = (0..6).map(|skip| words[skip * 3..].join(" ")).collect();
    said.push(Update::AssistantChunk(paragraphs.join("\n\n")));
    pane.say(said, cx);
    // The answer, and where each of its paragraphs starts below its top.
    let measure = |cx: &mut VisualTestContext| {
        cx.run_until_parked();
        let texts = drawn_texts(cx);
        let reply = cx.debug_bounds("reply-0").expect("the answer is drawn");
        // The header and the prompt area lie over the transcript's ends:
        // their texts are not the answer's.
        let field = cx.debug_bounds("frame").expect("the field is drawn");
        let header = cx.debug_bounds("header").expect("the header is drawn");
        let tops: Vec<f32> = texts
            .iter()
            .filter(|text| {
                // Under the answer's line of capitals: its paragraphs.
                text.left() >= reply.left() - px(1.)
                    && text.top() >= reply.top() + px(20. * 0.6)
                    && text.top() < reply.bottom()
                    && text.top() < field.top()
                    && text.top() >= header.bottom()
            })
            .map(|text| f32::from(text.top() - reply.top()))
            .collect();
        (reply, tops)
    };
    let rows =
        |reply: Bounds<Pixels>, zoom: f32| reply.size.height / (px(crate::prose::LINE) * zoom);
    let (first, first_tops) = measure(cx);
    assert_eq!(first_tops.len(), 6, "{first_tops:?}");
    // Lines and the gaps between paragraphs, in line heights.
    let lines = rows(first, 0.6);
    assert!(lines >= 20., "{lines} lines: not enough to tell");
    let (mut value, mut last) = (0.6f32, first);
    let mut frames = vec![(0.6, first, first_tops)];
    while value < 2.5 {
        value *= 1.007;
        zoom.set(value);
        cx.simulate_resize(size(px(900. * value), px(TALL * value)));
        let (reply, tops) = measure(cx);
        // The same lines: its height is theirs at this zoom, to a fraction of a line.
        let now = rows(reply, value);
        assert!(
            (now - lines).abs() < 0.2,
            "{now} lines at {value}, {lines} at 0.6"
        );
        // It grows with every frame: no frame waits for the rounding of
        // its parts to catch up, and none catches up for others.
        assert!(
            reply.size.height > last.size.height,
            "{:?} tall at {value}, {:?} the frame before",
            reply.size.height,
            last.size.height
        );
        assert!(
            (reply.size.width / (px(900.) * value) - first.size.width / px(540.)).abs() < 0.01,
            "as wide a share of the pane at {value} as at 0.6: {reply:?}"
        );
        // Within a fifth of a percent of 1 it is drawn as the pane without
        // a zoom, in GPUI's own text.
        if (value - 1.).abs() >= 0.002 {
            assert_eq!(tops.len(), 6, "every paragraph at {value}");
        }
        frames.push((value, reply, tops));
        last = reply;
    }
    // Every frame has its own zoom's height, to a pixel: its top and its
    // bottom each stand where the zoom puts them, rounded once.
    let (end, tallest, _) = frames[frames.len() - 1].clone();
    for (value, reply, _) in &frames {
        // A pixel for its own top and bottom, and half of one for the
        // tallest's, which this is worked out from.
        let exact = tallest.size.height * (value / end);
        assert!(
            (reply.size.height - exact).abs() <= px(1.5),
            "{:?} tall at {value}, {exact:?} to the zoom",
            reply.size.height
        );
    }
    // And so does every paragraph in it, however many lines and gaps are above it.
    let scaled: Vec<&(f32, Bounds<Pixels>, Vec<f32>)> = frames
        .iter()
        .filter(|(_, _, tops)| !tops.is_empty())
        .collect();
    let zooms: Vec<f32> = scaled.iter().map(|(zoom, _, _)| *zoom).collect();
    for paragraph in 0..6 {
        let tops: Vec<f32> = scaled.iter().map(|(_, _, tops)| tops[paragraph]).collect();
        let off = off_the_line(&zooms, &tops);
        assert!(off <= 1., "paragraph {paragraph} is {off} px off its place");
    }
}

/// A long answer, as agents write them: a heading, prose, a numbered list,
/// code, a table.
const LONG_ANSWER: &str = "## The retry queue\n\n\
    A task stays in the queue until a worker acknowledges it. Instead the worker takes a \
    lease of thirty seconds and renews it while it works.\n\n\
    1. The worker claims the task and sets `leased_until`.\n\
    2. Every ten seconds it renews the lease.\n\
    3. When it is done it writes the result and deletes the row.\n\n\
    ```go\nfunc (q *Queue) Lease(ctx context.Context) (*Task, error) {\n\treturn q.store.Claim(ctx)\n}\n```\n\n\
    | Field | Type | What for |\n|:--|:--|:--|\n| `attempt` | int | how many times it ran |\n\
    | `leased_until` | time | until when it is taken |\n| `last_error` | text | how it failed |\n\n\
    If the worker dies, the lease runs out and the next worker takes the task.";

#[gpui::test]
fn rows_far_up_a_long_transcript_keep_their_places_through_a_zoom(cx: &mut TestAppContext) {
    // A transcript taller than the pane, followed to its end, zoomed as a
    // pinch zooms it on a display of one device pixel to a pixel. Every text
    // on screen stands where the zoom puts it, give or take a pixel, at
    // every frame: laid out at the zoom alone, the rounding of every gap
    // and line height below a text added up, and texts at the top of the
    // pane stalled and then jumped by up to a line every few frames.
    let (pane, zoom, cx) = Pane::open_zoomed(cx, 0.6);
    cx.simulate_scale_factor_change(1.);
    let mut said = vec![ready()];
    for turn in 0..4 {
        said.push(Update::UserChunk(Block::Text(format!(
            "Turn {turn}: and then?"
        ))));
        said.push(Update::TurnStarted);
        said.push(Update::Tool(tool(&format!("t{turn}"), ToolStatus::Done)));
        said.push(Update::AssistantChunk(LONG_ANSWER.into()));
        said.push(Update::TurnEnded(Stop::Finished));
    }
    pane.say(said, cx);
    let mut zooms = Vec::new();
    let mut frames: Vec<Vec<f32>> = Vec::new();
    let mut value = 0.6f32;
    while value < 1.6 {
        zoom.set(value);
        cx.simulate_resize(size(px(900. * value), px(700. * value)));
        cx.run_until_parked();
        let texts = drawn_texts(cx);
        // The transcript's, above the prompt area and below the row cut at
        // the top: how far each is from the pane's bottom, which is where
        // the transcript ends.
        let field = cx.debug_bounds("frame").expect("the field");
        let bottom = 700. * value;
        frames.push(
            texts
                .iter()
                .filter(|text| text.top() > px(60. * value))
                .filter(|text| text.bottom() < field.top() - px(40. * value))
                .map(|text| bottom - f32::from(text.top()))
                .collect(),
        );
        zooms.push(value);
        value *= 1.012;
    }
    // The same texts in every frame, counted from the end of the transcript.
    let count = frames.iter().map(Vec::len).min().unwrap_or(0);
    assert!(count >= 20, "{count} texts on screen: not enough to tell");
    let mut worst: f32 = 0.;
    for text in 0..count {
        let places: Vec<f32> = frames
            .iter()
            .map(|frame| frame[frame.len() - count + text])
            .collect();
        worst = worst.max(off_the_line(&zooms, &places));
    }
    // Its row's top, its place in its row and its place in its block, each
    // rounded once, and the pane's own bottom: within two pixels. Laid out
    // at the zoom alone, texts at the top of the pane were 13 pixels off.
    assert!(worst <= 2., "a text {worst} px off its place");
}

#[gpui::test]
fn a_frame_of_a_zoom_builds_only_the_rows_on_screen(cx: &mut TestAppContext) {
    let (pane, zoom, cx) = Pane::open_zoomed(cx, 0.8);
    let mut said = vec![ready()];
    for turn in 0..12 {
        said.push(Update::UserChunk(Block::Text(format!("Turn {turn}"))));
        said.push(Update::AssistantChunk(LONG_ANSWER.into()));
    }
    pane.say(said, cx);
    let built = |cx: &mut VisualTestContext| pane.view.read_with(cx, |view, _| view.rows_built);
    let on_screen = |cx: &mut VisualTestContext| {
        (0..=24)
            .filter(|row| {
                cx.debug_bounds(Box::leak(format!("reply-{row}").into_boxed_str()))
                    .is_some()
            })
            .count()
            + 1
    };
    for value in [0.81f32, 0.83, 0.85] {
        let before = built(cx);
        zoom.set(value);
        cx.simulate_resize(size(px(900. * value), px(700. * value)));
        cx.run_until_parked();
        // The rows a zoom shows, and the last row with the prompt area's
        // room: not a screenful more to measure them again.
        let shown = on_screen(cx) + 1;
        let now = built(cx) - before;
        assert!(
            now <= shown * 2,
            "{now} rows built at {value} for {shown} on screen"
        );
    }
}

#[gpui::test]
fn a_zoom_does_not_move_the_room_kept_for_the_prompt_area(cx: &mut TestAppContext) {
    // Rounded at each zoom, the prompt area measures a pixel or two more or
    // less from one frame to the next. Taken for a change of its height, that
    // moved the whole transcript above it by as much, every few frames.
    let (pane, zoom, cx) = Pane::open_zoomed(cx, 0.6);
    cx.simulate_scale_factor_change(1.);
    pane.say(zoom_conversation(), cx);
    unfold_work(&pane, cx);
    let room = |cx: &mut VisualTestContext| pane.view.read_with(cx, |view, _| view.dock_height);
    let at_first = room(cx);
    assert!(at_first > 0., "the room is measured");
    let mut value = 0.6f32;
    while value < 2. {
        value *= 1.01;
        zoom.set(value);
        cx.simulate_resize(size(px(900. * value), px(700. * value)));
        cx.run_until_parked();
        assert_eq!(room(cx), at_first, "the room at {value}");
    }
    // A real change of the prompt area still makes room: a second line.
    cx.simulate_input("a");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("b");
    cx.run_until_parked();
    assert!(
        room(cx) > at_first + 10.,
        "{} after a second line",
        room(cx)
    );
}

#[gpui::test]
fn a_zoom_that_rests_is_drawn_once_more_at_rest(cx: &mut TestAppContext) {
    // While a zoom moves, glyphs stand on whole pixels; once it rests, the
    // view is drawn once more with each glyph at its own place.
    let (pane, zoom, cx) = Pane::open_zoomed(cx, 0.8);
    pane.say(zoom_conversation(), cx);
    unfold_work(&pane, cx);
    zoom.set(0.85);
    cx.simulate_resize(size(px(900. * 0.85), px(700. * 0.85)));
    cx.run_until_parked();
    let frames = pane.view.read_with(cx, |view, _| view.frames);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(
        pane.view.read_with(cx, |view, _| view.frames),
        frames + 1,
        "drawn once more, at rest"
    );
    // And not again while it stays at rest.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(pane.view.read_with(cx, |view, _| view.frames), frames + 1);
}

/// Where the centre of `name` is at 1, times `zoom`: the place the user
/// points at on a canvas at that zoom.
fn zoomed_centre(one: Bounds<Pixels>, zoom: f32) -> gpui::Point<Pixels> {
    point(one.center().x * zoom, one.center().y * zoom)
}

#[gpui::test]
fn a_click_at_a_zoomed_place_hits_what_is_drawn_there(cx: &mut TestAppContext) {
    let zoom = 2f32.powf(0.5);
    let (once, tool, row) = {
        let (pane, cx) = Pane::open(cx);
        pane.say(zoom_conversation(), cx);
        unfold_work(&pane, cx);
        let once = cx.debug_bounds("answer-once").expect("a button");
        // Answered, the request leaves the transcript, which follows its end:
        // the call is measured where it is then, as it is clicked below.
        pane.click("answer-once", cx);
        let tool = cx.debug_bounds("tool-1").expect("a tool call");
        cx.simulate_input("/re");
        let row = cx
            .debug_bounds("slash-command-0")
            .expect("a row of the list");
        (once, tool, row)
    };
    let (pane, _, cx) = Pane::open_zoomed(cx, zoom);
    pane.say(zoom_conversation(), cx);
    unfold_work(&pane, cx);

    // A button: the answer goes to the backend.
    cx.simulate_click(zoomed_centre(once, zoom), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        pane.heard(),
        [Command::Permit {
            request: 7,
            option: "once".into()
        }]
    );

    // A row of the transcript: the tool call opens.
    let open = |pane: &Pane, cx: &mut VisualTestContext| {
        pane.view
            .read_with(cx, |view, _| view.toggled.get(&1).copied())
    };
    assert_eq!(open(&pane, cx), None);
    cx.simulate_click(zoomed_centre(tool, zoom), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(open(&pane, cx), Some(true));

    // A row of the field's list: the command it names goes in the field.
    cx.simulate_input("/re");
    let drawn = cx
        .debug_bounds("slash-command-0")
        .expect("the list is open");
    assert!(
        drawn.contains(&zoomed_centre(row, zoom)),
        "{drawn:?} for {row:?}"
    );
    cx.simulate_click(zoomed_centre(row, zoom), Modifiers::none());
    cx.run_until_parked();
    assert!(
        pane.draft(cx).starts_with("/re") && pane.draft(cx).len() > 3,
        "{}",
        pane.draft(cx)
    );
}

#[gpui::test]
fn following_no_zoom_again_draws_the_pane_at_its_own_size(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(zoom_conversation(), cx);
    unfold_work(&pane, cx);
    let one = pane.named(cx);

    let zoom = Rc::new(std::cell::Cell::new(2.));
    pane.view
        .update(cx, |view, cx| view.follow_zoom(Some(zoom.clone()), cx));
    cx.simulate_resize(size(px(1800.), px(1400.)));
    cx.run_until_parked();
    let doubled = pane.named(cx);
    assert!(doubled[0].size.height == one[0].size.height * 2.);

    pane.view.update(cx, |view, cx| view.follow_zoom(None, cx));
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.run_until_parked();
    assert_eq!(pane.named(cx), one);
    assert_eq!(
        pane.view.read_with(cx, |view, _| view.scale.get()),
        1.,
        "the prompt area draws at 1 again"
    );
}

#[gpui::test]
fn the_transcript_is_measured_again_when_the_zoom_moves_and_only_then(cx: &mut TestAppContext) {
    let (pane, zoom, cx) = Pane::open_zoomed(cx, 1.);
    pane.say(zoom_conversation(), cx);
    unfold_work(&pane, cx);
    let counts = |cx: &mut VisualTestContext| {
        cx.run_until_parked();
        pane.view
            .read_with(cx, |view, _| (view.frames, view.relayouts))
    };
    let (frames, relayouts) = counts(cx);
    // Nothing changes: nothing is built.
    cx.run_until_parked();
    assert_eq!(counts(cx), (frames, relayouts));

    // A gesture: every frame of it is drawn at its own zoom.
    for (step, value) in [1.004f32, 1.008, 1.012].into_iter().enumerate() {
        zoom.set(value);
        cx.simulate_resize(size(px(900. * value), px(700. * value)));
        let (now, again) = counts(cx);
        assert!(now > frames + step, "drawn for the new bounds");
        assert_eq!(again, relayouts + step + 1, "measured for {value}");
    }
    // The zoom rests: so does the view.
    let rest = counts(cx);
    cx.run_until_parked();
    assert_eq!(counts(cx), rest);
    // A zoom that is 100% to the eye is drawn as the pane without one.
    zoom.set(1.001);
    cx.simulate_resize(size(px(901.), px(701.)));
    cx.run_until_parked();
    assert_eq!(pane.view.read_with(cx, |view, _| view.scale.get()), 1.);
}

#[gpui::test]
fn typing_at_a_zoom_keeps_the_field_at_the_zoom(cx: &mut TestAppContext) {
    let (pane, _, cx) = Pane::open_zoomed(cx, 2.);
    pane.say(
        [ready(), Update::AssistantChunk("A long answer.".into())],
        cx,
    );
    cx.simulate_input("a");
    cx.run_until_parked();
    let frames = pane.view.read_with(cx, |view, _| view.frames);
    let frame = cx.debug_bounds("frame").expect("the field");
    // The field is built again on its own as it is typed in, at the zoom, and
    // the transcript is not built again.
    cx.simulate_input("bcdef");
    cx.simulate_keystrokes("left left shift-left backspace");
    cx.run_until_parked();
    assert_eq!(pane.draft(cx), "abcef");
    assert_eq!(pane.view.read_with(cx, |view, _| view.frames), frames);
    assert_eq!(cx.debug_bounds("frame"), Some(frame));
    // A second line: at the zoom's line height.
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("ghi");
    cx.run_until_parked();
    let taller = cx.debug_bounds("frame").expect("the field");
    assert_eq!(taller.size.width, frame.size.width);
    assert_eq!(
        taller.size.height - frame.size.height,
        px(crate::prose::LINE * 2.),
        "a second line, at twice its height"
    );
}

// ------------------------------------------------------------------ the model picker

/// kou's own agent as `kou-native` presents it: its models, then who serves them.
fn kou_models() -> Vec<Update> {
    let model = |id: &str, name: &str| Choice {
        id: id.into(),
        name: name.into(),
        description: None,
    };
    let provider = |id: &str, name: &str, models: &[&str]| kou_thread::Provider {
        id: id.into(),
        name: name.into(),
        models: models.iter().map(|model| (*model).to_owned()).collect(),
    };
    vec![
        Update::Ready {
            session: "s1".into(),
            capabilities: Capabilities::default(),
            modes: Vec::new(),
            mode: None,
            models: vec![
                model("claude-opus-5", "Claude Opus 5"),
                model("claude-fable-5", "Claude Fable 5"),
                model("gpt-6.1-sol", "gpt-6.1-sol"),
                model("gpt-6.1-mini", "gpt-6.1-mini"),
                model("grok-4.7", "grok-4.7"),
            ],
            model: Some("gpt-6.1-mini".into()),
        },
        Update::Providers(vec![
            provider(
                "anthropic",
                "Anthropic",
                &["claude-opus-5", "claude-fable-5"],
            ),
            provider("openai", "OpenAI", &["gpt-6.1-sol", "gpt-6.1-mini"]),
            provider("xai", "xAI", &["grok-4.7"]),
        ]),
    ]
}

/// What the open picker shows: its lines, as the picker's state has them.
fn picker_lines(pane: &Pane, cx: &mut VisualTestContext) -> Option<Vec<crate::models::Line>> {
    pane.view.read_with(cx, |view, cx| {
        let dock = view.dock.read(cx);
        let picker = dock.model_picker.as_ref()?;
        Some(picker.browse.lines(&picker.catalog, &dock.sync.models))
    })
}

#[gpui::test]
fn the_model_picker_opens_on_the_providers_closed_with_the_current_one_marked(
    cx: &mut TestAppContext,
) {
    use crate::models::Line;
    let (pane, cx) = Pane::open(cx);
    pane.say(kou_models(), cx);
    pane.click("model-picker", cx);
    // Providers, closed: no model is listed until one is opened.
    assert_eq!(
        picker_lines(&pane, cx),
        Some(vec![
            Line::Provider(0),
            Line::Provider(1),
            Line::Provider(2)
        ])
    );
    assert!(cx.debug_bounds("provider-openai").is_some());
    // The cursor starts on the provider of the model in force.
    let current = pane.view.read_with(cx, |view, cx| {
        view.dock
            .read(cx)
            .model_picker
            .as_ref()
            .map(|picker| picker.browse.cursor)
    });
    assert_eq!(current, Some(1));
    // A click on a provider opens it on its models, under the way back.
    pane.click("model-line-0", cx);
    assert_eq!(
        picker_lines(&pane, cx),
        Some(vec![Line::Back(0), Line::Model(0), Line::Model(1)])
    );
    pane.click("model-back", cx);
    assert_eq!(picker_lines(&pane, cx).map(|lines| lines.len()), Some(3));
}

#[test]
fn the_quieter_look_draws_with_the_app_s_theme_accent_and_all() {
    use crate::studio::{chrome_theme, studio_theme};
    let plain = kou_ui::Theme::new(true);
    let chosen = kou_ui::Theme::new(true).accented(Some(0x4d9fe0));
    assert_eq!(studio_theme(&plain).palette, plain.palette);
    assert_eq!(studio_theme(&chosen).palette, chosen.palette);
    assert_eq!(studio_theme(&chosen).accent, chosen.accent);
    // What frames the conversation: the same colours in the text's face.
    assert_eq!(chrome_theme(&chosen).palette, chosen.palette);
    assert_eq!(chrome_theme(&chosen).mono_font, chosen.ui_font);
    assert_eq!(studio_theme(&chosen).mono_font, chosen.mono_font);
    // Made once for a palette, and found again from the look's own theme.
    assert!(std::ptr::eq(studio_theme(&chosen), studio_theme(&chosen)));
    assert!(std::ptr::eq(
        chrome_theme(studio_theme(&chosen)),
        chrome_theme(&chosen)
    ));
    assert!(!std::ptr::eq(studio_theme(&chosen), studio_theme(&plain)));
}

#[gpui::test]
fn the_model_picker_rises_from_the_model_s_control_in_each_look(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(kou_models(), cx);
    for look in [Look::Cockpit, Look::Studio] {
        pane.look(look, cx);
        pane.click("model-picker", cx);
        cx.run_until_parked();
        let control = cx
            .debug_bounds("model-picker")
            .expect("the model's control");
        let list = cx.debug_bounds("model-choices").expect("the open list");
        // Over the control, a gap above it.
        assert!(
            list.bottom() <= control.top(),
            "{look:?}: the list ends at {:?}, the control starts at {:?}",
            list.bottom(),
            control.top()
        );
        // The cockpit's control is at the left of the foot, and its list
        // stands over the field at that end; the studio's is at the right,
        // and its list rises from the control itself and ends where it does.
        match look {
            Look::Cockpit => assert!(
                list.left() <= control.left() && control.left() - list.left() < px(64.),
                "{look:?}: list {list:?}, control {control:?}"
            ),
            Look::Studio => {
                assert!(control.top() - list.bottom() < px(12.), "{look:?}");
                assert_eq!(list.right(), control.right(), "{look:?}");
            }
        }
        pane.click("model-picker", cx);
        cx.run_until_parked();
    }
}

#[gpui::test]
fn the_keys_open_a_provider_and_choose_a_model_and_the_field_has_them_back(
    cx: &mut TestAppContext,
) {
    let (pane, cx) = Pane::open(cx);
    pane.say(kou_models(), cx);
    pane.click("model-picker", cx);
    // OpenAI, the current model's provider: opened with →, its second model.
    cx.simulate_keystrokes("right down enter");
    assert_eq!(pane.heard(), [Command::SetModel("gpt-6.1-mini".into())]);
    assert!(picker_lines(&pane, cx).is_none(), "the picker closed");
    cx.simulate_input("ok");
    assert_eq!(pane.draft(cx), "ok");

    // ← goes back to the providers, Enter opens one, Escape steps back, then closes.
    pane.view
        .update_in(cx, |view, window, cx| view.focus_field(window, cx));
    pane.click("model-picker", cx);
    cx.simulate_keystrokes("up enter");
    assert_eq!(picker_lines(&pane, cx).map(|lines| lines.len()), Some(3));
    cx.simulate_keystrokes("left");
    assert_eq!(
        picker_lines(&pane, cx).map(|lines| lines[0].clone()),
        Some(crate::models::Line::Provider(0))
    );
    cx.simulate_keystrokes("enter escape");
    assert!(matches!(
        picker_lines(&pane, cx).as_deref(),
        Some([crate::models::Line::Provider(_), ..])
    ));
    cx.simulate_keystrokes("escape");
    assert!(picker_lines(&pane, cx).is_none());
    assert_eq!(pane.heard(), [], "nothing chosen, nothing cancelled");
}

#[gpui::test]
fn typing_in_the_picker_finds_models_across_providers(cx: &mut TestAppContext) {
    use crate::models::Line;
    let (pane, cx) = Pane::open(cx);
    pane.say(kou_models(), cx);
    pane.click("model-picker", cx);
    cx.simulate_input("o");
    // "o" is in Claude Opus, in OpenAI's name, and in grok: under each provider.
    let lines = picker_lines(&pane, cx).expect("open");
    assert_eq!(
        lines[..4],
        [
            Line::Heading(0),
            Line::Model(0),
            Line::Model(1),
            Line::Heading(1)
        ]
    );
    cx.simulate_input("pus");
    assert_eq!(
        picker_lines(&pane, cx).expect("open"),
        [Line::Heading(0), Line::Model(0), Line::Free("opus".into())]
    );
    cx.simulate_keystrokes("enter");
    assert_eq!(pane.heard(), [Command::SetModel("claude-opus-5".into())]);
}

#[gpui::test]
fn an_agent_that_lists_models_flat_gets_a_flat_list_or_its_prefixes(cx: &mut TestAppContext) {
    use crate::models::Line;
    let (pane, cx) = Pane::open(cx);
    let ready = |models: Vec<Choice>| Update::Ready {
        session: "s1".into(),
        capabilities: Capabilities::default(),
        modes: Vec::new(),
        mode: None,
        models,
        model: None,
    };
    pane.say(
        [ready(vec![
            choice("sonnet", "Sonnet"),
            choice("opus", "Opus"),
        ])],
        cx,
    );
    pane.click("model-picker", cx);
    assert_eq!(
        picker_lines(&pane, cx),
        Some(vec![Line::Model(0), Line::Model(1)])
    );
    cx.simulate_keystrokes("down enter");
    assert_eq!(pane.heard(), [Command::SetModel("opus".into())]);

    pane.say(
        [ready(vec![
            choice("openrouter/qwen3", "Qwen 3"),
            choice("anthropic/claude-opus-5", "Opus"),
            choice("openrouter/kimi-k3", "Kimi"),
        ])],
        cx,
    );
    pane.view
        .update_in(cx, |view, window, cx| view.focus_field(window, cx));
    pane.click("model-picker", cx);
    assert_eq!(
        picker_lines(&pane, cx),
        Some(vec![Line::Provider(0), Line::Provider(1)])
    );
    assert!(cx.debug_bounds("provider-openrouter").is_some());
}

// ------------------------------------------------------------------ the field alone

struct Field {
    field: Entity<crate::PromptField>,
    events: Rc<RefCell<Vec<crate::PromptFieldEvent>>>,
}

fn field(cx: &mut TestAppContext, width: f32) -> (Field, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(kou_ui::Theme::new(true));
        cx.set_reduce_motion(true);
    });
    let (field, cx) = cx.add_window_view(|window, cx| {
        let mut field = crate::PromptField::new(window, cx);
        field.set_placeholder("What should be done?", cx);
        field
    });
    cx.simulate_resize(size(px(width), px(300.)));
    cx.update(|window, _| window.activate_window());
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&field, move |_, event: &crate::PromptFieldEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    cx.run_until_parked();
    (Field { field, events }, cx)
}

impl Field {
    fn submitted(&self) -> Vec<crate::Prompt> {
        self.events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                crate::PromptFieldEvent::Submit(prompt) => Some(prompt.clone()),
                _ => None,
            })
            .collect()
    }
}

#[gpui::test]
fn the_field_alone_takes_focus_and_hands_over_what_is_sent(cx: &mut TestAppContext) {
    let (field, cx) = field(cx, 320.);
    field
        .field
        .update_in(cx, |field, window, cx| field.focus(window, cx));
    cx.simulate_input("почини");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("тест");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        field.submitted(),
        [crate::Prompt {
            text: "почини\nтест".into(),
            blocks: text("почини\nтест"),
        }]
    );
    assert_eq!(field.field.read_with(cx, |field, cx| field.text(cx)), "");
    // Blank is not a prompt, and a slash is only a slash: no command here.
    cx.simulate_keystrokes("space enter");
    assert_eq!(field.submitted().len(), 1);
    field.field.update(cx, |field, cx| field.clear(cx));
    cx.simulate_input("/c");
    let open = field.field.read_with(cx, |field, cx| {
        field.dock().read(cx).composer.read(cx).menu_open()
    });
    assert!(!open, "no command list without a backend");
    // The run button does what Enter does; at 320 pixels the frame fits.
    let frame = cx.debug_bounds("frame").expect("the field is drawn");
    assert!(frame.size.width <= px(320.), "{frame:?}");
    let send = cx.debug_bounds("send").expect("the button");
    assert!(send.right() <= frame.right(), "{send:?} in {frame:?}");
    cx.simulate_click(send.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(field.submitted()[1].text, "/c");
}

#[gpui::test]
fn a_picture_pasted_in_the_field_alone_goes_with_the_prompt(cx: &mut TestAppContext) {
    let (field, cx) = field(cx, 600.);
    field
        .field
        .update_in(cx, |field, window, cx| field.focus(window, cx));
    cx.simulate_input("вот:");
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            png(),
        )))
    });
    cx.simulate_keystrokes(paste_key());
    cx.run_until_parked();
    assert!(cx.debug_bounds("picture-0").is_some(), "its card shows");
    cx.simulate_keystrokes("enter");
    let sent = field.submitted();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].text, "вот: [Image 1] ");
    assert!(
        sent[0]
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Image { mime, .. } if mime == "image/png")),
        "{:?}",
        sent[0].blocks
    );
}

#[gpui::test]
fn the_field_alone_draws_the_app_s_foot_and_tells_its_height(cx: &mut TestAppContext) {
    let (field, cx) = field(cx, 600.);
    field.field.update(cx, |field, cx| {
        field.set_foot(
            Some(|_: &mut gpui::Window, _: &mut gpui::App| {
                use gpui::{
                    InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _,
                };
                gpui::div()
                    .debug_selector(|| "recipient".into())
                    .w(px(80.))
                    .h(px(28.))
                    .child("kou")
                    .into_any_element()
            }),
            cx,
        )
    });
    cx.run_until_parked();
    let recipient = cx.debug_bounds("recipient").expect("the app's button");
    let send = cx.debug_bounds("send").expect("the run button");
    assert!(recipient.right() < send.left(), "at the start of the foot");
    let heights = |field: &Field| -> Vec<f32> {
        field
            .events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                crate::PromptFieldEvent::Height(height) => Some(*height),
                _ => None,
            })
            .collect()
    };
    field
        .field
        .update_in(cx, |field, window, cx| field.focus(window, cx));
    cx.simulate_input("a");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("b");
    cx.run_until_parked();
    let two = *heights(&field).last().expect("its height is told");
    // A line less: told again, a line shorter.
    cx.simulate_keystrokes("backspace backspace");
    cx.run_until_parked();
    let one = *heights(&field).last().expect("told again");
    assert!(
        (two - one - crate::prose::LINE).abs() < 1.,
        "{two} with two lines, {one} with one"
    );
}

// ------------------------------------------------------------------ motion

/// A pane whose motion runs, as for a user who did not ask for less of it.
fn moving_pane(cx: &mut TestAppContext) -> (Pane, &mut VisualTestContext) {
    let (pane, cx) = Pane::open(cx);
    cx.update(|_, cx| cx.set_reduce_motion(false));
    (pane, cx)
}

/// How many frames the window asks for now: the callbacks of the next frame.
fn frames_asked(cx: &mut VisualTestContext) -> usize {
    let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    asked
}

#[gpui::test]
fn nothing_asks_for_frames_once_what_moved_has_settled(cx: &mut TestAppContext) {
    let (pane, cx) = moving_pane(cx);
    pane.say(
        [
            ready(),
            Update::TurnStarted,
            Update::AssistantChunk("The delay is doubled twice.".into()),
        ],
        cx,
    );
    // Arriving, the answer rises and its text fades in: frames are asked for.
    assert!(frames_asked(cx) > 0);
    pane.say([Update::TurnEnded(Stop::Finished)], cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    // Drain what was asked before it settled; then nothing more.
    for _ in 0..3 {
        frames_asked(cx);
    }
    assert_eq!(frames_asked(cx), 0, "no frames at rest");
    let blinking = pane.view.read_with(cx, |view, _| view.blinking_for_tests());
    assert!(!blinking, "no timer waits to blink anything");
}

#[gpui::test]
fn a_tool_call_opens_into_its_card_and_closes_over_a_few_frames(cx: &mut TestAppContext) {
    for look in [Look::Cockpit, Look::Studio] {
        let (pane, cx) = moving_pane(cx);
        pane.look(look, cx);
        pane.say(
            [
                ready(),
                Update::UserChunk(Block::Text("run it".into())),
                Update::TurnStarted,
                Update::Tool(tool("t1", ToolStatus::Done)),
                Update::AssistantChunk("Done.".into()),
                Update::TurnEnded(Stop::Finished),
            ],
            cx,
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        cx.run_until_parked();
        // A finished turn's work has folded: open it.
        pane.click("worked-1", cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        let unfolding = |pane: &Pane, cx: &mut VisualTestContext| {
            pane.view.read_with(cx, |view, cx| {
                view.motion.unfolding(1, cx.background_executor().now())
            })
        };
        // How many times the transcript is built over a few frames: none at
        // rest. (The field's caret may ask for frames of its own meanwhile.)
        let built = |pane: &Pane, cx: &mut VisualTestContext| {
            for _ in 0..4 {
                frames_asked(cx);
            }
            let before = pane.view.read_with(cx, |view, _| view.frames);
            for _ in 0..3 {
                frames_asked(cx);
            }
            pane.view.read_with(cx, |view, _| view.frames) - before
        };
        let closed = cx.debug_bounds("tool-1").expect("the call").size.height;
        assert_eq!(unfolding(&pane, cx), None, "{look:?}: at rest");
        assert_eq!(built(&pane, cx), 0, "{look:?}: at rest");

        // Opened: on its way, the transcript built at each frame.
        pane.click("tool-1", cx);
        let level = unfolding(&pane, cx).expect("it opens over time");
        assert!(level < 0.5, "{look:?}: it starts closed, at {level}");
        let before = pane.view.read_with(cx, |view, _| view.frames);
        assert!(frames_asked(cx) > 0, "{look:?}");
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(60));
        frames_asked(cx);
        assert!(
            pane.view.read_with(cx, |view, _| view.frames) > before,
            "{look:?}: drawn again on the way"
        );
        let half = unfolding(&pane, cx).expect("still on its way");
        assert!(half > level && half < 1., "{look:?}: {half}");
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        assert_eq!(built(&pane, cx), 0, "{look:?}: open, it rests");
        assert_eq!(unfolding(&pane, cx), None, "{look:?}: it came to rest");
        // Open, the same card in either look: the quieter look's line has
        // given way to the card's head, which is taller.
        let head = cx.debug_bounds("tool-1").expect("the card's head");
        match look {
            Look::Cockpit => assert_eq!(head.size.height, closed, "{look:?}"),
            Look::Studio => assert!(head.size.height > closed, "{look:?}"),
        }

        // Closed again: what it showed stays until it has closed.
        pane.click("tool-1", cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(60));
        frames_asked(cx);
        let shown = pane.view.read_with(cx, |view, _| {
            matches!(
                view.cache.get(1),
                Some(Some(crate::entries::Parsed::Tool(pieces)))
                    if !matches!(pieces.first(), Some(crate::entries::Piece::Folded))
            )
        });
        assert!(
            shown,
            "{look:?}: its content is still drawn while it closes"
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        assert_eq!(built(&pane, cx), 0, "{look:?}: closed, it rests");
        assert_eq!(unfolding(&pane, cx), None, "{look:?}");
        let back = cx.debug_bounds("tool-1").expect("the call").size.height;
        assert_eq!(back, closed, "{look:?}: as it was");
    }
}

#[gpui::test]
fn what_was_there_does_not_rise_again_and_a_replayed_session_never_rises(cx: &mut TestAppContext) {
    let (pane, cx) = moving_pane(cx);
    // A session replayed before the turn: entries are simply there.
    pane.say(
        [
            Update::UserChunk(Block::Text("earlier".into())),
            Update::AssistantChunk("an earlier answer".into()),
            ready(),
        ],
        cx,
    );
    let rising = |pane: &Pane, cx: &mut VisualTestContext| {
        pane.view.read_with(cx, |view, cx| {
            let now = cx.background_executor().now();
            (0..view.thread().entries().len())
                .filter(|index| view.motion.rise(*index, now).is_some())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(rising(&pane, cx), Vec::<usize>::new());
    // What comes while a turn runs rises, once.
    pane.say(
        [Update::TurnStarted, Update::AssistantChunk("now".into())],
        cx,
    );
    assert_eq!(rising(&pane, cx), [2]);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(rising(&pane, cx), Vec::<usize>::new());
    // Drawn anew (a zoom, the pane shown again), nothing starts over.
    pane.view.update(cx, |view, cx| {
        view.follow_zoom(Some(Rc::new(std::cell::Cell::new(1.5))), cx)
    });
    cx.run_until_parked();
    pane.view.update(cx, |view, cx| view.follow_zoom(None, cx));
    cx.run_until_parked();
    assert_eq!(rising(&pane, cx), Vec::<usize>::new());
    let fading = pane.view.read_with(cx, |view, cx| {
        let now = cx.background_executor().now();
        view.motion.fades(2, now)
    });
    assert!(fading.is_empty(), "{fading:?}");
}

#[gpui::test]
fn with_less_motion_nothing_moves_and_no_frame_is_asked_for(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready(),
            Update::TurnStarted,
            Update::ThoughtChunk("**Reading** the queue".into()),
            Update::Tool(tool("t1", ToolStatus::Running)),
            Update::AssistantChunk("streaming".into()),
        ],
        cx,
    );
    // What was asked for as the window first took its size; nothing after.
    for _ in 0..2 {
        frames_asked(cx);
    }
    assert_eq!(frames_asked(cx), 0);
    pane.view.read_with(cx, |view, _| {
        assert!(view.lights.is_empty(), "no light runs");
        assert!(!view.blinking_for_tests(), "nothing blinks");
    });
}

#[gpui::test]
fn a_light_that_runs_does_not_build_the_transcript_again(cx: &mut TestAppContext) {
    let (pane, cx) = moving_pane(cx);
    let mut said = vec![ready(), Update::TurnStarted];
    // A long transcript, the call that runs at its end.
    for at in 0..40 {
        said.push(Update::UserChunk(Block::Text(format!("prompt {at}"))));
        said.push(Update::AssistantChunk(format!("answer {at}")));
        said.push(Update::Tool(tool(&format!("done-{at}"), ToolStatus::Done)));
    }
    said.push(Update::Tool(tool("running", ToolStatus::Running)));
    pane.say(said, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    let lights = pane.view.read_with(cx, |view, _| view.lights.len());
    assert_eq!(lights, 1, "the call's light");
    for _ in 0..3 {
        frames_asked(cx);
    }
    let frames = pane.view.read_with(cx, |view, _| view.frames);
    // Frames of the light alone: the transcript is drawn from what it drew.
    for _ in 0..10 {
        assert!(frames_asked(cx) > 0, "the light goes on");
    }
    assert_eq!(pane.view.read_with(cx, |view, _| view.frames), frames);
}

#[gpui::test]
fn a_picture_dropped_on_the_field_alone_is_attached(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().expect("a directory");
    std::fs::write(folder.path().join("shot.png"), png()).expect("a picture");
    let (field, cx) = field(cx, 600.);
    let frame = cx.debug_bounds("frame").expect("the field's frame");
    let paths = gpui::ExternalPaths(std::iter::once(folder.path().join("shot.png")).collect());
    cx.simulate_event(gpui::FileDropEvent::Entered {
        position: frame.center(),
        paths,
    });
    cx.simulate_event(gpui::FileDropEvent::Submit {
        position: frame.center(),
    });
    cx.run_until_parked();
    assert_eq!(
        field.field.read_with(cx, |field, cx| field.text(cx)),
        "[Image 1] "
    );
    assert!(cx.debug_bounds("picture-0").is_some());
}

// ---------------------------------------------------------------- editing a prompt

fn rewinds() -> Update {
    Update::Offers(Offers {
        steer: false,
        rewind: true,
        arguments: Vec::new(),
    })
}

/// Two prompts and what they set off, from a backend that rewinds: the user
/// entries are 0 and 2, a call and an answer follow the second.
fn edited_conversation() -> Vec<Update> {
    vec![
        ready_with(pictures_ok()),
        rewinds(),
        Update::UserChunk(Block::Text("first question".into())),
        Update::PromptId("p1".into()),
        Update::TurnStarted,
        Update::AssistantChunk("first answer".into()),
        Update::TurnEnded(Stop::Finished),
        Update::UserChunk(Block::Text("second question".into())),
        Update::PromptId("p2".into()),
        Update::TurnStarted,
        Update::Tool(tool("t1", ToolStatus::Done)),
        Update::AssistantChunk("second answer".into()),
        Update::TurnEnded(Stop::Finished),
    ]
}

/// The prompt being edited and what its field holds.
fn editing(pane: &Pane, cx: &mut VisualTestContext) -> Option<(usize, String)> {
    pane.view.read_with(cx, |view, cx| {
        view.editor
            .as_ref()
            .map(|editor| (editor.index, editor.composer.read(cx).text().to_owned()))
    })
}

/// Whether the keyboard is in the editor, and in the field under the transcript.
fn focus(pane: &Pane, cx: &mut VisualTestContext) -> (bool, bool) {
    let (editor, field) = pane.view.read_with(cx, |view, _| {
        (
            view.editor.as_ref().map(|editor| editor.focus.clone()),
            view.field_focus().clone(),
        )
    });
    cx.update(|window, _| {
        (
            editor.is_some_and(|focus| focus.is_focused(window)),
            field.is_focused(window),
        )
    })
}

/// How opaque each entry is drawn, at rest.
fn fades(pane: &Pane, cx: &mut VisualTestContext) -> Vec<Option<f32>> {
    pane.view.read_with(cx, |view, cx| {
        let now = cx.background_executor().now();
        (0..view.thread().entries().len())
            .map(|index| view.motion.faded(index, now, true))
            .collect()
    })
}

fn notice(pane: &Pane, cx: &mut VisualTestContext) -> String {
    pane.view.read_with(cx, |view, cx| {
        view.dock
            .read(cx)
            .composer
            .read(cx)
            .notice()
            .unwrap_or_default()
            .to_owned()
    })
}

fn edit_help(pane: &Pane, cx: &mut VisualTestContext) -> Option<String> {
    pane.view.read_with(cx, |view, cx| {
        view.dock
            .read(cx)
            .composer
            .read(cx)
            .catalog()
            .specs
            .iter()
            .find(|spec| spec.name == "edit")
            .map(|spec| spec.help.clone())
    })
}

const FADED: Option<f32> = Some(crate::motion::FADED);

#[gpui::test]
fn a_prompt_is_edited_where_it_stands_and_runs_from_there(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(edited_conversation(), cx);
    let before = pane.entries(cx);
    assert!(cx.debug_bounds("reuse-prompt-2").is_none());
    assert_eq!(
        edit_help(&pane, cx).as_deref(),
        Some("Edit prompt n, by default the last, and run it again from there")
    );

    // The editor takes the prompt's place with its text, the keyboard in it.
    pane.click("edit-prompt-2", cx);
    assert_eq!(editing(&pane, cx), Some((2, "second question".into())));
    assert_eq!(focus(&pane, cx), (true, false));
    assert!(cx.debug_bounds("editor").is_some());
    assert!(cx.debug_bounds("editing-flag").is_some());
    assert!(cx.debug_bounds("edit-note").is_some());
    assert_eq!(
        crate::edit::note(2),
        "Replaces the 2 entries below · changes to files stay"
    );
    // What it replaces fades; what comes before does not.
    assert_eq!(fades(&pane, cx), [None, None, None, FADED, FADED]);

    // Typed at the end of the prompt; Shift+Enter is a new line.
    cx.simulate_input(", edited");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("and more");
    assert_eq!(
        editing(&pane, cx),
        Some((2, "second question, edited\nand more".into()))
    );
    assert_eq!(
        pane.draft(cx),
        "",
        "the field under the transcript is untouched"
    );
    assert_eq!(pane.entries(cx), before, "nothing changes until it runs");

    // Enter runs it: the session goes back to before the prompt.
    cx.simulate_keystrokes("enter");
    assert_eq!(
        pane.requests(),
        [Request::Rewind {
            prompt: Some("p2".into()),
            blocks: text("second question, edited\nand more"),
        }]
    );
    assert_eq!(pane.heard(), [], "no prompt of its own");
    assert_eq!(editing(&pane, cx), None);
    assert_eq!(
        pane.entries(cx),
        [
            Entry::User(text("first question")),
            Entry::Assistant("first answer".into()),
            Entry::User(text("second question, edited\nand more")),
        ]
    );
    assert_eq!(fades(&pane, cx), [None, None, None]);
    assert_eq!(focus(&pane, cx), (false, true));
    // Not again while it is on its way.
    assert!(cx.debug_bounds("edit-prompt-0").is_none());

    // It ran: the edited prompt is the session's, and its turn goes on.
    pane.say(
        [
            Update::Rewound(Ok("p2b".into())),
            Update::TurnStarted,
            Update::AssistantChunk("edited answer".into()),
            Update::TurnEnded(Stop::Finished),
        ],
        cx,
    );
    assert_eq!(pane.entries(cx).len(), 4);
    assert_eq!(
        pane.view
            .read_with(cx, |view, _| view.thread().prompt_id(2).map(str::to_owned)),
        Some("p2b".into())
    );
    assert!(cx.debug_bounds("edit-prompt-2").is_some());
}

#[gpui::test]
fn a_failed_edit_gives_back_what_it_replaced_and_opens_again(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(edited_conversation(), cx);
    let before = pane.entries(cx);
    // The call was opened by hand: it comes back open.
    pane.view.update(cx, |view, cx| view.toggle(3, true, cx));

    // `/edit 1` edits the first prompt.
    cx.simulate_input("/edit 1");
    cx.simulate_keystrokes("enter");
    assert_eq!(editing(&pane, cx), Some((0, "first question".into())));
    assert_eq!(fades(&pane, cx), [None, FADED, FADED, FADED, FADED]);
    cx.simulate_input(" again");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        pane.requests(),
        [Request::Rewind {
            prompt: Some("p1".into()),
            blocks: text("first question again"),
        }]
    );
    assert_eq!(pane.entries(cx).len(), 1);

    pane.say(
        [Update::Rewound(Err(
            "The session no longer holds that prompt: it was changed elsewhere.".into(),
        ))],
        cx,
    );
    assert_eq!(pane.entries(cx), before, "what it replaced is back");
    assert_eq!(
        editing(&pane, cx),
        Some((0, "first question again".into())),
        "the editor is open again with what was typed"
    );
    assert_eq!(focus(&pane, cx), (true, false));
    assert_eq!(fades(&pane, cx), [None, FADED, FADED, FADED, FADED]);
    assert!(
        notice(&pane, cx).contains("changed elsewhere"),
        "{}",
        notice(&pane, cx)
    );
    assert_eq!(
        pane.view
            .read_with(cx, |view, _| view.toggled.get(&3).copied()),
        Some(true)
    );
    assert_eq!(
        pane.view
            .read_with(cx, |view, _| view.thread().prompt_id(2).map(str::to_owned)),
        Some("p2".into())
    );
}

#[gpui::test]
fn an_edit_is_cancelled_by_esc_in_it_outside_it_and_by_its_button(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(edited_conversation(), cx);
    let before = pane.entries(cx);

    // Esc in the editor: the prompt stays as it was, the keyboard goes to the field.
    pane.click("edit-prompt-2", cx);
    cx.simulate_input(" changed");
    cx.simulate_keystrokes("escape");
    assert_eq!(editing(&pane, cx), None);
    assert_eq!(pane.entries(cx), before);
    assert_eq!(focus(&pane, cx), (false, true));
    assert_eq!(fades(&pane, cx), [None; 5]);
    assert_eq!(pane.requests(), []);

    // Opened again, it starts from the prompt; `/edit` alone is the last.
    cx.simulate_input("/edit");
    cx.simulate_keystrokes("enter");
    assert_eq!(editing(&pane, cx), Some((2, "second question".into())));

    // Esc pressed outside the editor closes it too, and stops nothing.
    pane.view
        .update_in(cx, |view, window, cx| view.focus_field(window, cx));
    assert_eq!(focus(&pane, cx), (false, true));
    cx.simulate_keystrokes("escape");
    assert_eq!(editing(&pane, cx), None);
    assert_eq!(pane.heard(), []);

    // And its Cancel.
    pane.click("edit-prompt-0", cx);
    assert_eq!(editing(&pane, cx).map(|(index, _)| index), Some(0));
    pane.click("edit-cancel", cx);
    assert_eq!(editing(&pane, cx), None);
    assert_eq!(pane.entries(cx), before);
}

#[gpui::test]
fn one_edit_is_open_at_a_time_and_the_fade_follows_it(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(edited_conversation(), cx);
    pane.click("edit-prompt-2", cx);
    cx.simulate_input(" typed");
    assert_eq!(fades(&pane, cx), [None, None, None, FADED, FADED]);

    // A second edit replaces the first, which keeps nothing of what was typed.
    pane.click("edit-prompt-0", cx);
    assert_eq!(editing(&pane, cx), Some((0, "first question".into())));
    assert_eq!(fades(&pane, cx), [None, FADED, FADED, FADED, FADED]);
    assert!(
        cx.debug_bounds("edit-prompt-2").is_some(),
        "the second prompt is a prompt again"
    );

    // An entry under the pointer lifts to 70%; the others stay faded.
    let answer = cx.debug_bounds("reply-4").expect("the last answer");
    cx.simulate_mouse_move(answer.center(), None, Modifiers::none());
    cx.run_until_parked();
    let lifted = Some(crate::motion::LIFTED);
    assert_eq!(fades(&pane, cx), [None, FADED, FADED, FADED, lifted]);
    let first = cx.debug_bounds("reply-1").expect("the first answer");
    cx.simulate_mouse_move(first.center(), None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(fades(&pane, cx), [None, lifted, FADED, FADED, FADED]);

    // Sending a prompt from the field leaves the edit behind.
    pane.view
        .update_in(cx, |view, window, cx| view.focus_field(window, cx));
    cx.simulate_input("something else");
    cx.simulate_keystrokes("enter");
    assert_eq!(editing(&pane, cx), None);
    assert_eq!(pane.heard(), [Command::Prompt(text("something else"))]);
    assert_eq!(fades(&pane, cx), [None; 6]);
}

#[gpui::test]
fn an_edit_the_session_moved_past_is_dropped_and_what_was_typed_kept(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(edited_conversation(), cx);

    // A run the edit did not start: the edit closes, and what was typed
    // waits in the empty field.
    pane.click("edit-prompt-2", cx);
    cx.simulate_input(" changed");
    pane.say([Update::TurnStarted], cx);
    assert_eq!(editing(&pane, cx), None);
    assert_eq!(pane.draft(cx), "second question changed");
    assert!(
        notice(&pane, cx).contains("moved on"),
        "{}",
        notice(&pane, cx)
    );
    assert_eq!(fades(&pane, cx), [None; 5]);
    // While the agent works there is nothing to edit.
    assert!(cx.debug_bounds("edit-prompt-2").is_none());
    pane.view.update_in(cx, |view, window, cx| {
        view.begin_edit(2, window, cx);
    });
    assert_eq!(editing(&pane, cx), None);
    pane.say([Update::TurnEnded(Stop::Finished)], cx);

    // The prompt as it was is not worth keeping; the field keeps its own.
    pane.click("edit-prompt-0", cx);
    pane.say([Update::TurnStarted, Update::TurnEnded(Stop::Finished)], cx);
    assert_eq!(editing(&pane, cx), None);
    assert_eq!(pane.draft(cx), "second question changed");

    // The session ending takes the edit with it, what it holds kept even
    // as it was, in a field that is empty.
    pane.view.update_in(cx, |view, window, cx| {
        view.dock.update(cx, |dock, cx| {
            dock.composer.update(cx, |composer, cx| composer.clear(cx))
        });
        view.begin_edit(0, window, cx);
    });
    pane.say([Update::Closed(Some("the agent exited".into()))], cx);
    assert_eq!(editing(&pane, cx), None);
    assert_eq!(pane.draft(cx), "first question");
}

#[gpui::test]
fn the_editor_is_drawn_at_the_zoom_and_works_there(cx: &mut TestAppContext) {
    let zoom = 2f32.powf(0.5);
    let (one, run) = {
        let (pane, cx) = Pane::open(cx);
        pane.say(edited_conversation(), cx);
        pane.click("edit-prompt-2", cx);
        (
            cx.debug_bounds("editor").expect("the editor"),
            cx.debug_bounds("edit-run").expect("its Run"),
        )
    };
    let (pane, _, cx) = Pane::open_zoomed(cx, zoom);
    pane.say(edited_conversation(), cx);
    cx.simulate_input("/edit 2");
    cx.simulate_keystrokes("enter");
    assert_eq!(editing(&pane, cx), Some((2, "second question".into())));
    let zoomed = cx.debug_bounds("editor").expect("the editor");
    for (at_one, at_zoom) in [
        (one.size.width, zoomed.size.width),
        (one.size.height, zoomed.size.height),
        (one.left(), zoomed.left()),
        (one.top(), zoomed.top()),
    ] {
        assert!(
            (at_one * zoom - at_zoom).abs() <= px(2.),
            "{one:?} at {zoom} is {zoomed:?}"
        );
    }
    // Typed into at the zoom, and run by a click where Run is drawn.
    cx.simulate_input("!");
    cx.simulate_click(zoomed_centre(run, zoom), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        pane.requests(),
        [Request::Rewind {
            prompt: Some("p2".into()),
            blocks: text("second question!"),
        }]
    );
}

#[gpui::test]
fn prompts_the_interface_sent_are_edited_by_the_names_the_backend_gave(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say([ready(), rewinds()], cx);
    cx.simulate_input("sent here");
    cx.simulate_keystrokes("enter");
    pane.say(
        [
            Update::PromptTaken(Some("m1".into())),
            Update::TurnStarted,
            Update::AssistantChunk("ok".into()),
            Update::TurnEnded(Stop::Finished),
        ],
        cx,
    );
    pane.click("edit-prompt-0", cx);
    cx.simulate_keystrokes("enter");
    assert_eq!(
        pane.requests(),
        [Request::Rewind {
            prompt: Some("m1".into()),
            blocks: text("sent here"),
        }]
    );
    // What a failed edit gives back is no message the agent took from the
    // queue: nothing forced into a turn is marked delivered by it.
    pane.say([Update::Rewound(Err("busy".into()))], cx);
    assert_eq!(pane.entries(cx).len(), 2);
}

#[gpui::test]
fn an_edited_prompt_keeps_its_pictures(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(
        [
            ready_with(pictures_ok()),
            rewinds(),
            Update::UserChunk(Block::Text("compare ".into())),
            Update::UserChunk(Block::Image {
                mime: "image/png".into(),
                data: png().into(),
            }),
            Update::PromptId("p1".into()),
            Update::TurnStarted,
            Update::AssistantChunk("They match.".into()),
            Update::TurnEnded(Stop::Finished),
        ],
        cx,
    );
    pane.click("edit-prompt-0", cx);
    cx.run_until_parked();
    assert_eq!(editing(&pane, cx), Some((0, "compare [Image 1]".into())));
    cx.simulate_input(" again");
    cx.simulate_keystrokes("enter");
    let requests = pane.requests();
    let [Request::Rewind { prompt, blocks }] = requests.as_slice() else {
        panic!("one rewind: {requests:?}");
    };
    assert_eq!(prompt.as_deref(), Some("p1"));
    let pictures: Vec<&[u8]> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Image { data, .. } => Some(&data[..]),
            _ => None,
        })
        .collect();
    assert_eq!(pictures, [png().as_slice()], "the picture goes with it");
    // The edited prompt shows its picture at once.
    assert!(cx.debug_bounds("message-picture-0-1").is_some());
}

// ------------------------------------------------------- nested agents, studio

use crate::nested::{Beside, Look, Nested, NestedState};

/// The wall clock the cards count against in these tests.
fn wall() -> std::time::SystemTime {
    std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000)
}

fn agent(id: &str, state: NestedState, after: Option<usize>) -> Nested {
    Nested {
        id: id.into(),
        title: format!("{id}: a worker of ours"),
        harness: "claude".into(),
        code: "CC".into(),
        state,
        detail: String::new(),
        since: wall() - std::time::Duration::from_secs(90),
        after,
    }
}

/// A prompt, a call, an answer: three entries, the turn over.
fn three_entries() -> Vec<Update> {
    vec![
        ready(),
        Update::UserChunk(Block::Text("Измерь всё".into())),
        Update::TurnStarted,
        Update::Tool(tool("t1", ToolStatus::Done)),
        Update::AssistantChunk("Запустил замер.".into()),
        Update::TurnEnded(Stop::Finished),
    ]
}

impl Pane {
    fn nested(&self, agents: Vec<Nested>, cx: &mut VisualTestContext) {
        self.view.update(cx, |view, cx| {
            view.pin_clock(Some(wall()), cx);
            view.set_nested(agents, cx);
        });
        cx.run_until_parked();
    }

    fn look(&self, look: Look, cx: &mut VisualTestContext) {
        self.view.update(cx, |view, cx| view.set_look(look, cx));
        cx.run_until_parked();
    }

    fn top(&self, selector: &'static str, cx: &mut VisualTestContext) -> Pixels {
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is on screen"))
            .top()
    }
}

#[gpui::test]
fn nested_cards_stand_after_the_entry_they_were_started_at_in_order(cx: &mut TestAppContext) {
    for look in [Look::Cockpit, Look::Studio] {
        let (pane, cx) = Pane::open(cx);
        pane.look(look, cx);
        pane.say(three_entries(), cx);
        pane.nested(
            vec![
                agent("a", NestedState::Running, Some(2)),
                agent("b", NestedState::Finished, None),
                agent("c", NestedState::Failed, Some(1)),
            ],
            cx,
        );
        let (c, a, b) = (
            pane.top("nested-c", cx),
            pane.top("nested-a", cx),
            pane.top("nested-b", cx),
        );
        let reply = pane.top("reply-2", cx);
        assert!(
            c < a,
            "{look:?}: c was started after the prompt, a after the call"
        );
        assert!(a < reply, "{look:?}: a stands before the answer");
        assert!(reply < b, "{look:?}: b came after the last entry");
        // What comes later stands after the cards: they keep their place.
        pane.say([Update::AssistantChunk("ещё".into())], cx);
        pane.say(
            [
                Update::TurnStarted,
                Update::AssistantChunk("И ещё одно.".into()),
            ],
            cx,
        );
        assert!(
            pane.top("nested-b", cx) < pane.top("reply-3", cx),
            "{look:?}"
        );
    }
}

#[gpui::test]
fn a_nested_agent_updated_in_place_keeps_its_card_and_its_place(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.look(Look::Studio, cx);
    pane.say(three_entries(), cx);
    pane.nested(vec![agent("a", NestedState::Running, None)], cx);
    let before = pane.top("nested-a", cx);
    let places = pane.view.read_with(cx, |view, _| view.nest.places.clone());
    // More entries, then the agent finishes with its latest words.
    pane.say(
        [Update::TurnStarted, Update::AssistantChunk("Жду.".into())],
        cx,
    );
    let mut done = agent("a", NestedState::Finished, None);
    done.detail = "Всё готово: отчёт в report.md".into();
    pane.nested(vec![done.clone()], cx);
    assert_eq!(
        pane.view.read_with(cx, |view, _| view.nest.places.clone()),
        places,
        "heard of later, it stays where it was first heard of"
    );
    assert!((pane.top("nested-a", cx) - before).abs() < px(1.));
    let said = crate::nested::words(&done, wall());
    assert_eq!(said.detail, "Всё готово: отчёт в report.md");
    assert!(!said.counting);
}

#[gpui::test]
fn the_queue_stands_between_the_line_that_says_the_agent_works_and_the_field(
    cx: &mut TestAppContext,
) {
    for look in [Look::Cockpit, Look::Studio] {
        let (pane, cx) = Pane::open(cx);
        pane.look(look, cx);
        pane.say([ready(), Update::TurnStarted], cx);
        if look == Look::Cockpit {
            // Over the field alone, the line begins at the field's edge.
            let meter = cx.debug_bounds("activity-meter").expect("its meter");
            let field = cx.debug_bounds("frame").expect("the field");
            assert!(
                (meter.left() - field.left() - px(2.)).abs() <= px(0.5),
                "{meter:?} {field:?}"
            );
        }
        cx.simulate_input("later");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let works = cx
            .debug_bounds("activity")
            .expect("the line over the field");
        let queue = cx.debug_bounds("queue").expect("the queue");
        let field = cx.debug_bounds("frame").expect("the field");
        if look == Look::Cockpit {
            // Over the queue's card, which stands in from the field's
            // sides, it begins where the card does.
            let meter = cx.debug_bounds("activity-meter").expect("its meter");
            assert!(queue.left() > field.left() + px(10.), "{queue:?} {field:?}");
            assert!(
                (meter.left() - queue.left()).abs() <= px(0.5),
                "{meter:?} {queue:?}"
            );
        }
        assert!(
            works.bottom() <= queue.top(),
            "{look:?}: {works:?} {queue:?}"
        );
        // A tray on the field's top edge.
        assert!(
            (queue.bottom() - field.top()).abs() <= px(2.),
            "{look:?}: {queue:?} {field:?}"
        );
    }
}

#[gpui::test]
fn work_folded_away_is_neither_read_nor_slow_to_scroll_past(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    cx.simulate_resize(size(px(1000.), px(700.)));
    pane.look(Look::Studio, cx);
    // Three long turns: hundreds of tool calls each, with changes to files
    // (whose lines a card would count), folded once the turn is over.
    const CALLS: usize = 300;
    let old: String = (0..400).map(|line| format!("line {line}\n")).collect();
    let new: String = (0..400)
        .map(|line| format!("line {line} changed\n"))
        .collect();
    let mut updates = vec![ready()];
    for turn in 0..3 {
        updates.push(Update::UserChunk(Block::Text(format!("turn {turn}"))));
        updates.push(Update::TurnStarted);
        for call in 0..CALLS {
            let mut edit = tool(&format!("t{turn}-{call}"), ToolStatus::Done);
            edit.kind = ToolKind::Edit;
            edit.content = vec![ToolContent::Diff {
                path: format!("/work/file-{call}.rs").into(),
                old: Some(old.clone()),
                new: new.clone(),
            }];
            updates.push(Update::Tool(edit));
        }
        updates.push(Update::AssistantChunk(format!("answer {turn}")));
        updates.push(Update::TurnEnded(Stop::Finished));
    }
    pane.say(updates, cx);
    let rows = 3 * (CALLS + 2);
    assert_eq!(pane.entries(cx).len(), rows);
    // (While a turn ran, its steps were on screen and were read; what is
    // looked at here is the folded work afterwards, as in a session opened
    // again.)
    pane.view.update(cx, |view, cx| {
        view.cache.iter_mut().for_each(|parsed| *parsed = None);
        cx.notify();
    });
    cx.run_until_parked();
    // Scrolled from the end to the start and back, every row has been asked for.
    let list = pane.view.read_with(cx, |view, _| view.list.clone());
    let started = std::time::Instant::now();
    for step in 0..40 {
        let up = step < 20;
        cx.simulate_event(ScrollEvent {
            position: list.viewport_bounds().center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(if up { 600. } else { -600. }))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
            source: Default::default(),
        });
        cx.run_until_parked();
    }
    let took = started.elapsed();
    // Nothing of the folded work was read: no change was compared line by line.
    let read = pane.view.read_with(cx, |view, _| {
        (0..rows)
            .filter(|row| matches!(view.thread().entries()[*row], Entry::Tool(_)))
            .filter(|row| matches!(view.cache.get(*row), Some(Some(_))))
            .count()
    });
    assert_eq!(read, 0, "folded calls were parsed");
    // And forty frames of it take no time to speak of. (Before, each folded
    // row looked back over its whole run at every frame, and every change
    // was compared line by line: this took minutes.)
    assert!(
        took < std::time::Duration::from_secs(8),
        "scrolling past folded work took {took:?}"
    );
    // Opened, a run's calls are there, and only then are they read.
    let (first, last) = (2 * (CALLS + 2) + 1, 3 * (CALLS + 2) - 2);
    pane.click(Box::leak(format!("worked-{first}").into_boxed_str()), cx);
    cx.run_until_parked();
    let read = pane.view.read_with(cx, |view, _| {
        (first..=last)
            .filter(|row| matches!(view.cache.get(*row), Some(Some(_))))
            .count()
    });
    assert!(read > 0, "an opened run's calls are read as they are shown");
    assert!(
        read < CALLS,
        "and only those on screen: {read} of {CALLS} were read"
    );
}

#[gpui::test]
fn the_field_stands_under_the_text_from_edge_to_edge(cx: &mut TestAppContext) {
    // With the card beside the transcript and without: the field goes with
    // the text, which the card never moves.
    let mut moved_over = false;
    for beside in [false, true] {
        for width in [1400., 1150., 1000., 900., 620.] {
            let (pane, cx) = Pane::open(cx);
            cx.simulate_resize(size(px(width), px(800.)));
            pane.say(
                [
                    ready(),
                    Update::UserChunk(Block::Text("run it".into())),
                    Update::TurnStarted,
                    Update::Tool(tool("t1", ToolStatus::Done)),
                ],
                cx,
            );
            if beside {
                pane.view.update(cx, |view, cx| {
                    view.set_beside(
                        Some(Beside {
                            project: "neko-mini".into(),
                            path: "/work/neko-mini".into(),
                            branch: Some("master".into()),
                            changes: None,
                            parent: None,
                            earlier: 0,
                        }),
                        cx,
                    );
                });
            }
            cx.run_until_parked();
            // A tool call's card spans the text's column, a pixel of border
            // around its head.
            let head = cx.debug_bounds("tool-1").expect("the call's card");
            let field = cx.debug_bounds("frame").expect("the field");
            assert!(
                (field.left() - (head.left() - px(1.))).abs() <= px(0.5),
                "{width}, beside {beside}: the field begins at {:?}, the text at {:?}",
                field.left(),
                head.left() - px(1.)
            );
            assert!(
                (field.right() - (head.right() + px(1.))).abs() <= px(0.5),
                "{width}, beside {beside}: the field ends at {:?}, the text at {:?}",
                field.right(),
                head.right() + px(1.)
            );
            // The card never moves the text: it is around the pane's
            // middle at every width, with the card or without.
            let text = (head.left() + head.right()) / 2.;
            moved_over |= (text - px(width / 2.)).abs() > px(20.);
        }
    }
    assert!(!moved_over, "the text stands off the pane's middle");
}

#[gpui::test]
fn the_way_back_to_the_end_stands_over_the_middle_of_the_field(cx: &mut TestAppContext) {
    for look in [Look::Cockpit, Look::Studio] {
        let (pane, cx) = Pane::open(cx);
        cx.simulate_resize(size(px(1400.), px(800.)));
        pane.look(look, cx);
        pane.say([ready()], cx);
        // Enough turns to scroll through.
        pane.say(
            (0..80).flat_map(|turn| {
                [
                    Update::UserChunk(Block::Text(format!("prompt {turn}"))),
                    Update::TurnStarted,
                    Update::AssistantChunk(format!("answer {turn}")),
                    Update::TurnEnded(Stop::Finished),
                ]
            }),
            cx,
        );
        // With the card beside the transcript: the quieter look's column
        // and field stand off the pane's right edge by the room it takes.
        pane.view.update(cx, |view, cx| {
            view.set_beside(
                Some(Beside {
                    project: "neko-mini".into(),
                    path: "/work/neko-mini".into(),
                    branch: Some("master".into()),
                    changes: None,
                    parent: None,
                    earlier: 0,
                }),
                cx,
            );
        });
        cx.run_until_parked();
        let list = pane.view.read_with(cx, |view, _| view.list.clone());
        cx.simulate_event(ScrollEvent {
            position: list.viewport_bounds().center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(400.))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
            source: Default::default(),
        });
        cx.run_until_parked();
        // The card shows only beside a centred column, so nothing moves
        // over for it; the button is over the field at every width.
        for width in [1400., 1150., 1000., 900.] {
            cx.simulate_resize(size(px(width), px(800.)));
            cx.run_until_parked();
            let latest = cx.debug_bounds("latest").expect("a way back to the end");
            let field = cx.debug_bounds("frame").expect("the field");
            assert!(
                (latest.center().x - field.center().x).abs() <= px(1.),
                "{look:?} at {width}: the button is at {:?}, the field at {:?}",
                latest.center().x,
                field.center().x
            );
            assert!(latest.bottom() < field.top(), "{look:?} at {width}");
        }
    }
}

#[gpui::test]
fn clicks_on_a_card_a_lineage_row_and_the_folder_tell_the_app(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    cx.simulate_resize(size(px(1400.), px(800.)));
    pane.look(Look::Studio, cx);
    pane.say(three_entries(), cx);
    pane.nested(vec![agent("a", NestedState::Running, Some(2))], cx);
    pane.view.update(cx, |view, cx| {
        view.set_beside(
            Some(Beside {
                project: "neko-mini".into(),
                path: "/work/neko-mini".into(),
                branch: Some("master".into()),
                changes: Some((3, 1)),
                parent: Some(agent("parent", NestedState::Finished, None)),
                earlier: 2,
            }),
            cx,
        )
    });
    cx.run_until_parked();
    pane.events.borrow_mut().clear();
    pane.click("nested-a", cx);
    pane.click("lineage-a", cx);
    pane.click("lineage-parent", cx);
    pane.click("beside-folder", cx);
    assert_eq!(
        *pane.events.borrow(),
        [
            ThreadViewEvent::OpenNested("a".into()),
            ThreadViewEvent::OpenNested("a".into()),
            ThreadViewEvent::OpenNested("parent".into()),
            ThreadViewEvent::OpenFolder("/work/neko-mini".into()),
        ]
    );
}

#[gpui::test]
fn the_waiting_line_names_the_agents_and_its_stop_ends_the_turn(cx: &mut TestAppContext) {
    for look in [Look::Cockpit, Look::Studio] {
        let (pane, cx) = Pane::open(cx);
        pane.look(look, cx);
        pane.say(three_entries(), cx);
        pane.nested(
            vec![
                agent("a", NestedState::Running, None),
                agent("b", NestedState::Running, None),
            ],
            cx,
        );
        assert!(cx.debug_bounds("waiting-titles").is_none(), "{look:?}");
        pane.say([Update::TurnStarted], cx);
        pane.view.update(cx, |view, cx| {
            view.set_waiting_on(vec!["a".into(), "b".into()], cx)
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("waiting-titles").is_some(), "{look:?}");
        assert_eq!(
            pane.view
                .read_with(cx, |view, _| view.around.waiting.borrow().clone()),
            ["a: a worker of ours", "b: a worker of ours"]
        );
        pane.heard();
        pane.click("waiting-stop", cx);
        assert_eq!(pane.heard(), [Command::Cancel], "{look:?}");
        // It waits on none: the line goes.
        pane.view
            .update(cx, |view, cx| view.set_waiting_on(Vec::new(), cx));
        pane.say([Update::TurnEnded(Stop::Cancelled)], cx);
        assert!(cx.debug_bounds("waiting-titles").is_none(), "{look:?}");
    }
}

#[gpui::test]
fn a_running_card_s_time_ticks_only_while_one_is_on_screen(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.look(Look::Studio, cx);
    pane.say(three_entries(), cx);
    pane.nested(vec![agent("a", NestedState::Finished, Some(2))], cx);
    assert!(
        !pane.view.read_with(cx, |view, _| view.ticking_for_tests()),
        "nothing counts at rest"
    );
    pane.nested(vec![agent("a", NestedState::Running, Some(2))], cx);
    assert!(pane.view.read_with(cx, |view, _| view.ticking_for_tests()));
    // A second later it is drawn again, and asks for the next.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(pane.view.read_with(cx, |view, _| view.ticking_for_tests()));
    // Finished, the timer runs out once and is not asked for again.
    pane.nested(vec![agent("a", NestedState::Finished, Some(2))], cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(!pane.view.read_with(cx, |view, _| view.ticking_for_tests()));
}

/// A card beside the transcript, as the app gives one.
fn a_card_beside(pane: &Pane, cx: &mut VisualTestContext) {
    pane.view.update(cx, |view, cx| {
        view.set_beside(
            Some(Beside {
                project: "neko-mini".into(),
                path: "/work/neko-mini".into(),
                branch: Some("master".into()),
                ..Beside::default()
            }),
            cx,
        )
    });
    // Drawn twice: the first frame learns the width.
    cx.run_until_parked();
    cx.run_until_parked();
}

#[gpui::test]
fn the_card_beside_is_pulled_out_by_a_handle_and_put_away_by_its_button(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    cx.simulate_resize(size(px(1000.), px(800.)));
    pane.say(three_entries(), cx);
    a_card_beside(&pane, cx);
    let middle = |cx: &mut VisualTestContext| {
        let reply = cx.debug_bounds("reply-2").expect("the answer");
        (reply.left() + reply.right()) / 2.
    };
    // No room beside a centred column: no card, a handle at the pane's edge.
    assert!(cx.debug_bounds("beside").is_none());
    let handle = cx.debug_bounds("beside-pull").expect("the handle");
    assert_eq!(handle.right(), px(1000.));
    let before = middle(cx);
    // Pulled out, the card is there, over the text, which has not moved.
    pane.click("beside-pull", cx);
    let card = cx.debug_bounds("beside").expect("pulled out");
    assert!(
        card.right() < px(1000.) && card.left() > px(600.),
        "{card:?}"
    );
    assert!(cx.debug_bounds("beside-pull").is_none());
    assert_eq!(middle(cx), before);
    // Its own button puts it away again.
    pane.click("beside-hide", cx);
    assert!(cx.debug_bounds("beside").is_none());
    assert!(cx.debug_bounds("beside-pull").is_some());

    // Where the pane has room it is there by itself; it can be put away,
    // and pulled out again.
    cx.simulate_resize(size(px(1500.), px(800.)));
    cx.run_until_parked();
    cx.run_until_parked();
    assert!(cx.debug_bounds("beside").is_some());
    assert!(cx.debug_bounds("beside-pull").is_none());
    pane.click("beside-hide", cx);
    assert!(cx.debug_bounds("beside").is_none());
    pane.click("beside-pull", cx);
    assert!(cx.debug_bounds("beside").is_some());
    // That was what the room gives anyway: narrower again, it goes.
    cx.simulate_resize(size(px(1000.), px(800.)));
    cx.run_until_parked();
    cx.run_until_parked();
    assert!(cx.debug_bounds("beside").is_none());
    assert!(cx.debug_bounds("beside-pull").is_some());
}

#[gpui::test]
fn the_card_beside_comes_out_and_goes_back_over_a_few_frames(cx: &mut TestAppContext) {
    let (pane, cx) = moving_pane(cx);
    cx.simulate_resize(size(px(1500.), px(800.)));
    pane.say(three_entries(), cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    let level = |pane: &Pane, cx: &mut VisualTestContext| {
        pane.view.read_with(cx, |view, cx| {
            view.beside_level(cx.background_executor().now(), false)
        })
    };
    a_card_beside(&pane, cx);
    // Coming: part of the way out, frames asked for, drawn already.
    assert!(frames_asked(cx) > 0);
    let (out, going) = level(&pane, cx);
    assert!(out < 1. && !going, "{out}");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(120));
    frames_asked(cx);
    let (further, _) = level(&pane, cx);
    assert!(further > out && further < 1., "{out} then {further}");
    assert!(cx.debug_bounds("beside").is_some());
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    for _ in 0..4 {
        frames_asked(cx);
    }
    assert_eq!(level(&pane, cx), (1., false));
    let at_rest = cx.debug_bounds("beside").expect("out").right();
    // Put away: it goes back the way it came, still drawn until it is gone.
    pane.click("beside-hide", cx);
    frames_asked(cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(120));
    frames_asked(cx);
    let (left, going) = level(&pane, cx);
    assert!(going && left > 0. && left < 1., "{left}");
    let leaving = cx.debug_bounds("beside").expect("still drawn").right();
    assert!(leaving > at_rest, "it slides to the edge: {leaving:?}");
    assert!(cx.debug_bounds("beside-pull").is_none());
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    for _ in 0..4 {
        frames_asked(cx);
    }
    assert_eq!(level(&pane, cx), (0., false));
    assert!(cx.debug_bounds("beside").is_none());
    assert!(cx.debug_bounds("beside-pull").is_some());
}

#[gpui::test]
fn the_card_beside_shows_only_where_it_covers_no_text(cx: &mut TestAppContext) {
    for look in [Look::Cockpit, Look::Studio] {
        let (pane, cx) = Pane::open(cx);
        cx.simulate_resize(size(px(1500.), px(800.)));
        pane.look(look, cx);
        pane.say(three_entries(), cx);
        pane.view.update(cx, |view, cx| {
            view.set_beside(
                Some(Beside {
                    project: "neko-mini".into(),
                    ..Beside::default()
                }),
                cx,
            )
        });
        cx.run_until_parked();
        // Drawn twice: the first frame learns the width.
        cx.run_until_parked();
        let beside = cx.debug_bounds("beside").expect("room for it");
        let reply = cx.debug_bounds("reply-2").expect("the answer");
        assert!(
            reply.right() <= beside.left(),
            "{look:?}: {reply:?} {beside:?}"
        );
        // The text is in the middle of the pane, as it is without the card.
        let middle = (reply.left() + reply.right()) / 2.;
        assert!(
            (middle - px(750.)).abs() < px(40.),
            "{look:?}: the answer is around {middle:?}"
        );
        // Narrower, the text would have to move over to clear it: no card,
        // and the text stays in the middle.
        for width in [1300., 1000., 900.] {
            cx.simulate_resize(size(px(width), px(800.)));
            cx.run_until_parked();
            cx.run_until_parked();
            if let Some(beside) = cx.debug_bounds("beside") {
                let reply = cx.debug_bounds("reply-2").expect("the answer");
                assert!(reply.right() <= beside.left(), "{look:?} {width}");
            }
            let reply = cx.debug_bounds("reply-2").expect("the answer");
            let middle = (reply.left() + reply.right()) / 2.;
            assert!(
                (middle - px(width / 2.)).abs() < px(40.),
                "{look:?} at {width}: the answer is around {middle:?}"
            );
        }
        cx.simulate_resize(size(px(1000.), px(800.)));
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("beside").is_none(),
            "{look:?}: no room beside a centred column"
        );
        cx.simulate_resize(size(px(600.), px(800.)));
        cx.run_until_parked();
        cx.run_until_parked();
        assert!(cx.debug_bounds("beside").is_none(), "{look:?}: too narrow");
        // Taken away, it is gone at any width.
        cx.simulate_resize(size(px(1500.), px(800.)));
        pane.view.update(cx, |view, cx| view.set_beside(None, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("beside").is_none(), "{look:?}");
    }
}

#[gpui::test]
fn the_studio_look_folds_finished_work_into_a_line_that_opens(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.look(Look::Studio, cx);
    pane.say(three_entries(), cx);
    assert!(
        cx.debug_bounds("tool-1").is_none(),
        "the call is folded away"
    );
    pane.click("worked-1", cx);
    assert!(
        cx.debug_bounds("tool-1").is_some(),
        "opened, the call shows"
    );
    pane.click("worked-1", cx);
    assert!(cx.debug_bounds("tool-1").is_none());
    // A turn that runs shows its work as it goes.
    pane.say(
        [
            Update::UserChunk(Block::Text("Ещё".into())),
            Update::TurnStarted,
            Update::Tool(tool("t2", ToolStatus::Running)),
        ],
        cx,
    );
    assert!(cx.debug_bounds("tool-4").is_some());
    // The cockpit folds what is over in the same way, into a line on its
    // rail, and shows the work of the turn that runs.
    pane.look(Look::Cockpit, cx);
    assert!(cx.debug_bounds("tool-1").is_none());
    assert!(cx.debug_bounds("worked-1").is_some());
    assert!(cx.debug_bounds("tool-4").is_some());
    pane.click("worked-1", cx);
    assert!(
        cx.debug_bounds("tool-1").is_some(),
        "opened, the call shows"
    );
}

#[gpui::test]
fn an_agent_named_in_an_answer_opens_from_its_name(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.look(Look::Studio, cx);
    pane.nested(vec![agent("lite", NestedState::Running, Some(0))], cx);
    pane.say(
        [
            ready(),
            Update::UserChunk(Block::Text("Что с замером?".into())),
            Update::TurnStarted,
            Update::AssistantChunk(
                "Запустил 🦀 lite: a worker of ours. Он измерит всё, офлайн.".into(),
            ),
            Update::TurnEnded(Stop::Finished),
        ],
        cx,
    );
    pane.events.borrow_mut().clear();
    pane.click("link-0", cx);
    assert_eq!(
        *pane.events.borrow(),
        [ThreadViewEvent::OpenNested("lite".into())]
    );
    // The cockpit leaves the name as it is written.
    pane.look(Look::Cockpit, cx);
    assert!(cx.debug_bounds("link-0").is_none());
}

#[gpui::test]
fn the_cockpit_draws_as_before_until_it_is_told_of_agents(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.say(three_entries(), cx);
    pane.say(
        [
            Update::UserChunk(Block::Text("Второй вопрос".into())),
            Update::TurnStarted,
            Update::AssistantChunk("Ответ.".into()),
            Update::TurnEnded(Stop::Finished),
        ],
        cx,
    );
    assert!(cx.debug_bounds("ticks").is_none(), "no ticks of its own");
    pane.look(Look::Studio, cx);
    assert!(cx.debug_bounds("ticks").is_some());
    assert!(cx.debug_bounds("tick-0").is_some());
    pane.look(Look::Cockpit, cx);
    pane.nested(vec![agent("a", NestedState::Running, None)], cx);
    assert!(
        cx.debug_bounds("ticks").is_some(),
        "told of agents, it has them"
    );
}

#[gpui::test]
fn the_studio_look_draws_cards_at_any_zoom(cx: &mut TestAppContext) {
    for zoom in [0.6, 1.5] {
        let (pane, _, cx) = Pane::open_zoomed(cx, zoom);
        pane.look(Look::Studio, cx);
        pane.say(three_entries(), cx);
        pane.nested(vec![agent("a", NestedState::Running, Some(2))], cx);
        let card = cx.debug_bounds("nested-a").expect("the card");
        let reply = cx.debug_bounds("reply-2").expect("the answer");
        assert!(card.bottom() <= reply.top() + px(1.), "{zoom}");
        // The card is as tall as the look has it, times the zoom.
        assert!(
            (card.size.height - px(crate::studio::pitch(40.) * zoom)).abs() <= px(2.),
            "{zoom}: {:?}",
            card.size
        );
    }
}

#[gpui::test]
fn a_prompt_s_bubble_is_one_flat_fill_even_under_the_top_edge(cx: &mut TestAppContext) {
    let (pane, cx) = Pane::open(cx);
    pane.look(Look::Studio, cx);
    pane.say(three_entries(), cx);
    let flat_over = |cx: &mut VisualTestContext, at: &'static str| {
        let bubble = cx.debug_bounds(at).expect("the bubble");
        let scale = cx.update(|window, _| window.scale_factor());
        let quads = cx.update(|window, _| window.painted_quads());
        let over = quads
            .iter()
            .filter(|quad| quad.background.as_solid().is_none())
            .filter(|quad| {
                let bounds = quad.bounds;
                let (top, bottom) = (
                    bounds.origin.y.0 / scale,
                    (bounds.origin.y.0 + bounds.size.height.0) / scale,
                );
                let (left, right) = (
                    bounds.origin.x.0 / scale,
                    (bounds.origin.x.0 + bounds.size.width.0) / scale,
                );
                top < f32::from(bubble.bottom())
                    && bottom > f32::from(bubble.top())
                    && left < f32::from(bubble.right())
                    && right > f32::from(bubble.left())
            })
            .count();
        assert_eq!(over, 0, "a gradient over {at} at {bubble:?}");
    };
    // At the top of the transcript, where the first prompt stands.
    flat_over(cx, "bubble-0");
    // Further down a long conversation, scrolled so that a prompt stands
    // right under the top edge, with what came before it out of sight.
    for _ in 0..6 {
        pane.say(
            [
                Update::UserChunk(Block::Text("Ещё раз".into())),
                Update::TurnStarted,
                Update::AssistantChunk("Ответ.\n\n".repeat(8)),
                Update::TurnEnded(Stop::Finished),
            ],
            cx,
        );
    }
    let prompt = pane
        .entries(cx)
        .iter()
        .rposition(|entry| matches!(entry, Entry::User(_)))
        .expect("a prompt");
    for offset in [0., 8., 14.] {
        pane.view.update(cx, |view, _| {
            view.list.scroll_to(gpui::ListOffset {
                item_ix: prompt,
                offset_in_item: px(offset),
            })
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let at: &'static str = Box::leak(format!("bubble-{prompt}").into_boxed_str());
        let bubble = cx.debug_bounds(at).expect("the bubble in sight");
        assert!(bubble.top() >= px(0.), "{offset}: {bubble:?}");
        flat_over(cx, at);
    }
}
