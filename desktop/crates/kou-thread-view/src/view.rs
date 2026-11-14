//! The pane: the pump that folds a backend's updates into the thread, the
//! transcript list and everything the keyboard does outside the field. The
//! prompt area under it is [`crate::dock`]; how each entry looks is in
//! [`crate::entries`].

use crate::entries::EDGE;
use crate::{
    commands::Act,
    dock::{self, Dock, DockEvent, MENU_CONTEXT},
    draft::{self, Draft},
    entries::Parsed,
    lights::{Lights, Spots},
    motion::{Fresh, Motion},
    pictures,
    prose::{LINE, PROSE},
    trace::{self, Part},
};
use gpui::{
    AnimationExt as _, AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Div,
    Entity, EventEmitter, FocusHandle, Focusable, FollowMode, Global, ImageSource,
    InteractiveElement as _, IntoElement, KeyBinding, ListAlignment, ListOffset, ListState,
    ParentElement as _, Pixels, Render, RenderImage, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, actions, canvas, div, img, list,
};
use kou_thread::{
    Backend, Block, Change, Command, Entry, PermissionKind, Request, Thread, ThreadState, Update,
};
use kou_ui::{
    Icon, Mark, Theme, icon, metric, motion,
    scale::{self, exact, px},
    status_mark,
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    rc::Rc,
    sync::Arc,
    task::Poll,
    time::Duration,
};

/// The key context of the whole pane.
const CONTEXT: &str = "ThreadView";
/// Updates folded in before the pump lets a frame through.
const BATCH: usize = 512;
/// How far past the viewport the transcript is measured: more than a page, so
/// paging lands on rows whose height is known.
const OVERDRAW: f32 = 1200.;
/// How long a copy's check mark stays.
const COPIED: Duration = Duration::from_millis(1400);
/// How long after its last change a zoom is taken to rest: its text is then
/// drawn once more with every glyph at its own place (see
/// [`kou_ui::scale::Scaled::moving`]).
const SETTLE: Duration = Duration::from_millis(150);
/// How far, in device pixels at the current zoom, the view's unzoomed width may
/// drift before its content is laid out at another width. Layout rounds a zoomed
/// node to whole device pixels, which divided by a small zoom is most of an
/// unzoomed pixel; without this margin a word at the end of a line could come
/// and go between frames of a zoom.
const ZOOM_SLACK: f32 = 1.5;

actions!(
    kou_thread_view,
    [
        PageUp,
        PageDown,
        ScrollToTop,
        ScrollToBottom,
        FocusNext,
        FocusPrevious,
        Cancel,
        AllowDefault,
        MenuUp,
        MenuDown,
    ]
);

/// Marks that this crate's key bindings are in the keymap.
struct KeysBound;

impl Global for KeysBound {}

/// Bind the pane's keys, once per application.
pub(crate) fn bind_keys(cx: &mut App) {
    if cx.has_global::<KeysBound>() {
        return;
    }
    cx.set_global(KeysBound);
    let mut keys = crate::composer::bindings();
    keys.extend(crate::picker::bindings());
    let allow = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    keys.extend([
        KeyBinding::new("pageup", PageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", PageDown, Some(CONTEXT)),
        KeyBinding::new("home", ScrollToTop, Some(CONTEXT)),
        KeyBinding::new("end", ScrollToBottom, Some(CONTEXT)),
        KeyBinding::new("tab", FocusNext, Some(CONTEXT)),
        KeyBinding::new("shift-tab", FocusPrevious, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new(allow, AllowDefault, Some(CONTEXT)),
        KeyBinding::new("up", MenuUp, Some(MENU_CONTEXT)),
        KeyBinding::new("down", MenuDown, Some(MENU_CONTEXT)),
        KeyBinding::new("shift-tab", MenuUp, Some(MENU_CONTEXT)),
        KeyBinding::new("tab", MenuDown, Some(MENU_CONTEXT)),
    ]);
    cx.bind_keys(keys);
}

/// The key that answers a permission request with its default option, as shown beside it.
pub(crate) fn allow_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd+enter"
    } else {
        "ctrl+enter"
    }
}

/// What a thread view shows about where it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// The agent's name as the user knows it: `kou`, `OpenCode`.
    pub agent: SharedString,
    /// Its harness's code, as its tile is drawn by (`CC`, `K`): the agents
    /// it starts by itself wear it on their cards.
    pub code: SharedString,
    /// The directory the agent works in.
    pub cwd: PathBuf,
}

/// How soon after an Escape a second one stops the running turn.
pub(crate) const ESCAPE_TWICE: std::time::Duration = std::time::Duration::from_millis(1500);

/// What a thread view tells the pane, the rail and the canvas around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadViewEvent {
    /// The thread started doing something else: working, blocked, idle, closed.
    State(ThreadState),
    /// The backend named the session.
    Title(SharedString),
    /// A nested agent's card, or its row beside the transcript, was clicked:
    /// the app's name for it.
    OpenNested(String),
    /// The project's folder button beside the transcript was clicked.
    OpenFolder(PathBuf),
    /// The model the next prompts go to changed during the session: chosen
    /// in the field or by a command, or changed by the agent. Not said for
    /// the model a session merely starts with.
    Model(String),
}

/// What a thread view asks of the app around it: the slash commands that are
/// the app's. They are offered in the field only once the app says it
/// answers them, with [`ThreadView::set_app_commands`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadViewRequest {
    /// `/new`: start a new session of the same agent in this pane.
    NewSession,
    /// `/sessions`, or `/resume` alone: show the sessions to open one.
    OpenSessions,
    /// `/resume <session>`: open this session, by ID or title.
    ResumeSession(String),
    /// `/rename <title>`: title the session.
    Rename(String),
    /// `/settings`: the connection's settings.
    OpenSettings,
}

/// A picture of a message, decoded for the transcript.
#[derive(Clone)]
pub(crate) enum Shot {
    Loading,
    Ready {
        thumb: Arc<RenderImage>,
        large: Arc<RenderImage>,
        width: u32,
        height: u32,
    },
    Failed,
}

/// The native agent pane: one conversation and the prompt area under it.
pub struct ThreadView {
    pub(crate) thread: Thread,
    commands: async_channel::Sender<Command>,
    pub(crate) requests: async_channel::Sender<Request>,
    pub(crate) header: Header,
    pub(crate) dock: Entity<Dock>,
    /// The field's focus handle: focusing the pane puts the cursor in the field.
    focus: FocusHandle,
    /// Focus of the pane as a whole: everything focusable in it is below this.
    pane_focus: FocusHandle,
    pub(crate) list: ListState,
    /// Where the prompts' bubbles in sight stand this frame, from their
    /// rows' prepaint, for the fade under the transcript's top edge: it
    /// stops above a bubble, so a bubble at rest is one flat fill. Emptied
    /// each frame once the fade has read it.
    pub(crate) bubble_tops: Rc<RefCell<Vec<Pixels>>>,
    /// What each entry parsed to, by index; `None` until it is first shown or after it changed.
    pub(crate) cache: Vec<Option<Parsed>>,
    /// Entries the user opened or closed by hand, which then stop following their status.
    pub(crate) toggled: HashMap<usize, bool>,
    /// How tall what opens and closes is when whole, unscaled, as it noted
    /// while it was drawn: a tool call's content (entry, 0) and its card
    /// (entry, 1). What the frames of an opening or a closing are cut from.
    pub(crate) unfold_heights: Rc<RefCell<HashMap<(usize, u8), f32>>>,
    /// Terminal outputs (entry, content index) shown from their first line.
    pub(crate) unfolded: HashSet<(usize, usize)>,
    /// Whether the view draws its own header line. Off when the pane around it has one.
    header_visible: bool,
    /// The transcript is scrolled away from its end, as of the last layout.
    away: bool,
    /// Entries before this one are out of sight after `/clear`.
    pub(crate) cleared: usize,
    /// Pictures of messages, by entry and block.
    pub(crate) shots: HashMap<(usize, usize), Shot>,
    /// A message's picture shown large over the transcript.
    pub(crate) viewing: Option<(usize, usize)>,
    /// What was copied last, for its check mark.
    pub(crate) copied: Option<SharedString>,
    copies: u64,
    /// How tall the prompt area over the transcript's bottom is, as it last
    /// settled, unscaled: the room the last row keeps.
    pub(crate) dock_height: f32,
    /// The zoom of the canvas the view is drawn on, while it follows one.
    zoom: Option<Rc<Cell<f32>>>,
    /// The factor everything is drawn at: the zoom in steps, 1 without one.
    /// The prompt area and the field read it too, to draw at it when GPUI
    /// builds them again without the view around them.
    pub(crate) scale: Rc<Cell<f32>>,
    /// The view's unzoomed width its content was last laid out at.
    held_width: Rc<Cell<Option<Pixels>>>,
    /// What each frame costs, while `KOU_ZOOM_TRACE` is set. See [`crate::trace`].
    pub(crate) trace: trace::Trace,
    /// Where the blocks of each row drawn at a zoom stand at 1, by row: see
    /// [`Self::placed_row`]. Dropped when the row changes.
    plans: HashMap<usize, Rc<exact::Plan>>,
    /// Where this frame places the transcript's rows from: from its end
    /// while it follows its end, otherwise from the row at its top. Taken
    /// before the list is laid out: the list cannot be asked while it lays
    /// out its rows.
    rows_from: RowsFrom,
    /// When the zoom last changed, and the task that draws the view once
    /// more when it has rested.
    moved_at: Option<std::time::Instant>,
    settling: Option<Task<()>>,
    /// What of the transcript moves, and since when. See [`crate::motion`].
    pub(crate) motion: Motion,
    /// The loops rows draw while what they show runs or is written, by row
    /// and kind. See [`crate::lights`].
    pub(crate) lights: std::collections::BTreeMap<(usize, u8), Entity<Lights>>,
    /// Where the rows the lights go over were laid out last.
    pub(crate) spots: Spots,
    /// The model each prompt went to, by entry, as far as the view knows.
    pub(crate) prompt_models: HashMap<usize, String>,
    /// The timer that draws the view again when what blinks changes.
    blinking: Option<Task<()>>,
    /// The prompt being edited where it stands. See [`crate::edit`].
    pub(crate) editor: Option<crate::edit::Editor>,
    /// An edit on its way to the backend.
    pub(crate) sent_edit: Option<crate::edit::Sent>,
    /// The editor takes the keyboard once the view is drawn.
    pub(crate) focus_editor: bool,
    /// When Escape was pressed while a turn ran: pressed again within
    /// [`ESCAPE_TWICE`], it stops the turn.
    stop_armed: Option<std::time::Instant>,
    /// How the conversation is drawn, shared with the prompt area. See [`crate::nested`].
    pub(crate) around: Rc<crate::nested::Around>,
    /// The agents this one started, and what stands beside the conversation.
    pub(crate) nest: crate::nested::Nest,
    /// The run of folded work each row is in, as worked out once (see
    /// [`ThreadView::fold_of`]): a long run is asked about by every one of
    /// its rows at every frame. Emptied whenever what it follows from
    /// changes: the entries, the thread's state, what is open, the look.
    pub(crate) fold_memo: RefCell<HashMap<usize, Option<(crate::nested::Run, bool)>>>,
    /// How many times the pane was built, to check that a burst of updates is one frame.
    #[cfg(test)]
    pub(crate) frames: usize,
    /// How many times a new factor laid the transcript out again.
    #[cfg(test)]
    pub(crate) relayouts: usize,
    /// How many rows of the transcript were built, to check what a zoom builds.
    #[cfg(test)]
    pub(crate) rows_built: usize,
    _pump: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadViewEvent> for ThreadView {}
impl EventEmitter<ThreadViewRequest> for ThreadView {}

impl Focusable for ThreadView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ThreadView {
    /// A view of the conversation `backend` drives. The view receives the
    /// backend's updates from here on and sends it the user's commands.
    pub fn new(
        backend: Backend,
        header: Header,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        bind_keys(cx);
        let thread = Thread::new();
        let scale = Rc::new(Cell::new(1.));
        let held_width: Rc<Cell<Option<Pixels>>> = Rc::new(Cell::new(None));
        let trace = trace::new();
        let dock = cx.new(|cx| {
            Dock::new(
                header.agent.clone(),
                Some(header.cwd.clone()),
                scale.clone(),
                held_width.clone(),
                trace.clone(),
                window,
                cx,
            )
        });
        let focus = dock.read(cx).focus_handle(cx);
        let around = dock.read(cx).around.clone();
        let pane_focus = cx.focus_handle();
        let subscriptions = vec![
            cx.subscribe_in(&dock, window, Self::dock_event),
            // A click on the pane itself, or the app focusing it, means the field.
            cx.on_focus(&pane_focus, window, |this, window, cx| {
                window.focus(&this.focus, cx)
            }),
            // A focused control can leave the screen: its call finished, its request was
            // answered, it scrolled away. The keyboard then goes back to the field
            // instead of nowhere.
            cx.on_focus_out(&pane_focus, window, |this, event, window, cx| {
                let focused = window.focused(cx);
                let stranded = focused.is_none() || focused == event.blurred.upgrade();
                if stranded && window.is_window_active() {
                    window.focus(&this.focus, cx);
                }
            }),
        ];

        // One row per entry and a last row for what the thread is doing.
        let list = ListState::new(1, ListAlignment::Top, px(OVERDRAW));
        list.set_follow_mode(FollowMode::Tail);

        let updates = backend.updates;
        let pump = cx.spawn(async move |this, cx| {
            while let Ok(first) = updates.recv().await {
                // Everything that arrived since the last look is one change to the view.
                let mut batch = vec![first];
                while batch.len() < BATCH
                    && let Ok(update) = updates.try_recv()
                {
                    batch.push(update);
                }
                if this.update(cx, |view, cx| view.absorb(batch, cx)).is_err() {
                    return;
                }
                // A backend that never pauses must not keep the window from drawing.
                YieldNow(false).await;
            }
            this.update(cx, |view, cx| {
                if !matches!(view.thread.state(), ThreadState::Closed(_)) {
                    let gone = "the backend went away without giving a reason".to_owned();
                    view.absorb(vec![Update::Closed(Some(gone))], cx);
                }
            })
            .ok();
        });

        Self {
            thread,
            commands: backend.commands,
            requests: backend.requests,
            header,
            dock,
            focus,
            pane_focus,
            list,
            bubble_tops: Rc::default(),
            cache: Vec::new(),
            toggled: HashMap::new(),
            fold_memo: RefCell::default(),
            unfold_heights: Rc::default(),
            unfolded: HashSet::new(),
            header_visible: true,
            away: false,
            cleared: 0,
            shots: HashMap::new(),
            viewing: None,
            copied: None,
            copies: 0,
            dock_height: 0.,
            zoom: None,
            scale,
            held_width,
            trace,
            plans: HashMap::new(),
            rows_from: RowsFrom::End(1),
            moved_at: None,
            settling: None,
            motion: Motion::new(cx.background_executor().now()),
            lights: std::collections::BTreeMap::new(),
            spots: Spots::default(),
            prompt_models: HashMap::new(),
            blinking: None,
            editor: None,
            sent_edit: None,
            focus_editor: false,
            stop_armed: None,
            around,
            nest: crate::nested::Nest::default(),
            #[cfg(test)]
            frames: 0,
            #[cfg(test)]
            relayouts: 0,
            #[cfg(test)]
            rows_built: 0,
            _pump: pump,
            _subscriptions: subscriptions,
        }
    }

    /// The conversation as it stands.
    pub fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Send a prompt that did not come from the composer: from another agent, or from the app.
    /// It is echoed as a user entry at once. Unlike the composer, this does not wait for
    /// a running turn to end: whether a prompt sent mid-turn is queued or dropped is up
    /// to the backend. Nothing is sent to a closed session.
    pub fn send(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.trim().is_empty() || matches!(self.thread.state(), ThreadState::Closed(_)) {
            return;
        }
        self.prompt(vec![Block::Text(text.to_owned())], cx);
    }

    /// Whether anything has been said in this conversation: a prompt of the
    /// user's, now or in what a resumed session replayed. A session nobody
    /// has said anything in is one most agents have not saved yet.
    pub fn has_conversation(&self) -> bool {
        self.thread
            .entries()
            .iter()
            .any(|entry| matches!(entry, Entry::User(_)))
    }

    /// Send a prompt made of blocks (text, pictures, files), as
    /// [`PromptField`](crate::PromptField) hands one over. Echoed and sent
    /// like [`send`](Self::send).
    pub fn send_blocks(&mut self, blocks: Vec<Block>, cx: &mut Context<Self>) {
        if blocks.is_empty() || matches!(self.thread.state(), ThreadState::Closed(_)) {
            return;
        }
        self.prompt(blocks, cx);
    }

    /// Show or hide the view's own header line (agent, directory, session title,
    /// state). It is shown by default; a pane that draws a header of its own
    /// hides it, and the transcript then starts at the top of the view. Nothing
    /// is lost by hiding it: the state is also the transcript's last row, and
    /// the title and state reach the pane as [`ThreadViewEvent`]s.
    pub fn set_header_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.header_visible != visible {
            self.header_visible = visible;
            cx.notify();
        }
    }

    /// Follows a zoom factor, or stops following one with `None`.
    ///
    /// While a handle is set, its value is read on every frame the view draws
    /// and everything is drawn scaled by it: text, paddings, gaps, radii, icons,
    /// controls, pictures, the prompt field with its lists, the queue. The
    /// view's content is laid out at its unzoomed size (its bounds divided by
    /// the zoom), so zooming does not wrap the conversation anew, and it is
    /// drawn at the zoom itself on every frame: nothing in it moves in steps.
    /// Only the pictures of the glyphs are stepped (see
    /// [`kou_ui::scale::quantize`]). `None` draws at the view's own size again.
    ///
    /// A view whose bounds did not change and that was not notified is drawn
    /// from what it drew last; a zoom that changes without the view's bounds
    /// changing needs `cx.notify()` on it.
    pub fn follow_zoom(&mut self, zoom: Option<Rc<Cell<f32>>>, cx: &mut Context<Self>) {
        self.zoom = zoom;
        self.held_width.set(None);
        self.plans.clear();
        cx.notify();
    }

    /// Rows `range` show something else: measured, and planned, again.
    pub(crate) fn remeasure_rows(&mut self, range: std::ops::Range<usize>) {
        self.plans.retain(|row, _| !range.contains(row));
        self.list.remeasure_items(range);
    }

    /// Row `row` is planned again at the next zoom frame, without being measured again.
    pub(crate) fn plans_forget(&mut self, row: usize) {
        self.plans.remove(&row);
    }

    /// Every row shows something else.
    fn remeasure_all(&mut self) {
        self.plans.clear();
        self.list.remeasure();
    }

    /// The zoom the view is drawn at this frame: the handle's value, 1 without one.
    fn zoom(&self) -> f32 {
        let value = self.zoom.as_ref().map_or(1., |zoom| zoom.get());
        if value.is_finite() && value > 0. {
            value.clamp(scale::ZOOM_RANGE.0, scale::ZOOM_RANGE.1)
        } else {
            1.
        }
    }

    /// Takes in the factor this frame is drawn at. A new one measures the
    /// rows of the transcript on screen again (their text is not shaped
    /// again: it is shaped at its unscaled size) and keeps what was at the
    /// top of a scrolled transcript at the top. Returns whether the factor
    /// was new.
    ///
    /// Only the rows on screen: a zoom keeps the same rows in sight, and the
    /// others would be built and laid out on every frame of a zoom gesture
    /// only to be measured (the list measures a screenful past what it
    /// shows). They keep the height they had at an earlier zoom until they
    /// come into sight, which places nothing on screen.
    fn rescale(&mut self, factor: f32) -> bool {
        let was = self.scale.replace(factor);
        if was == factor {
            return false;
        }
        #[cfg(test)]
        {
            self.relayouts += 1;
        }
        let top = self.list.logical_scroll_top();
        let following = self.list.is_following_tail();
        let count = self.list.item_count();
        let mut end = (top.item_ix + 1).min(count);
        while end < count && self.list.item_is_below_viewport(end) == Some(false) {
            end += 1;
        }
        // Following its end, the list lays its last row out first.
        if following {
            end = count;
        }
        self.list.remeasure_items(top.item_ix..end);
        if !following {
            self.list.scroll_to(ListOffset {
                item_ix: top.item_ix,
                offset_in_item: top.offset_in_item * (factor / was),
            });
        }
        true
    }

    /// Whether the zoom is moving: it changed within the last [`SETTLE`].
    /// While it moves, a task waits for it to rest and then draws the view
    /// once more, at rest.
    fn track_motion(&mut self, rescaled: bool, cx: &mut Context<Self>) -> bool {
        let now = cx.background_executor().now();
        if rescaled {
            self.moved_at = Some(now);
        }
        let moving = self
            .moved_at
            .is_some_and(|at| now.saturating_duration_since(at) < SETTLE);
        if moving && self.settling.is_none() {
            self.settling = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(SETTLE).await;
                    let rested = this.update(cx, |view, cx| {
                        let now = cx.background_executor().now();
                        let rested = view
                            .moved_at
                            .is_none_or(|at| now.saturating_duration_since(at) >= SETTLE);
                        if rested {
                            view.settling = None;
                            cx.notify();
                        }
                        rested
                    });
                    if rested.unwrap_or(true) {
                        return;
                    }
                }
            }));
        }
        moving
    }

    /// Where prompt history is shared with kou-conveyor's other cockpits. By
    /// default `<cwd>/.harness/ui-history.json`, which the terminal cockpit
    /// keeps; `None` keeps the history of this view in memory only.
    pub fn set_history_file(&mut self, file: Option<PathBuf>, cx: &mut Context<Self>) {
        self.dock
            .update(cx, |dock, cx| dock.set_history_file(file, cx));
    }

    /// Whether the app answers [`ThreadViewRequest`]s. Off by default: the
    /// field offers `/new`, `/sessions`, `/resume`, `/rename` and `/settings`
    /// only once the app subscribes to the requests and turns this on.
    pub fn set_app_commands(&mut self, answered: bool, cx: &mut Context<Self>) {
        self.dock
            .update(cx, |dock, cx| dock.set_app_commands(answered, cx));
    }

    /// The assistant's latest text, for another agent that reads this one.
    pub fn last_answer(&self) -> Option<String> {
        self.thread
            .entries()
            .iter()
            .rev()
            .find_map(|entry| match entry {
                Entry::Assistant(text) => Some(text.clone()),
                _ => None,
            })
    }

    /// Whether a timer waits to draw what blinks, for tests.
    #[cfg(test)]
    pub(crate) fn blinking_for_tests(&self) -> bool {
        self.blinking.is_some()
    }

    /// The composer's draft, for tests.
    #[cfg(test)]
    pub(crate) fn composer_text(&self, cx: &App) -> String {
        self.dock.read(cx).composer.read(cx).text().to_owned()
    }

    /// Fold a batch of updates into the thread and mark what has to be laid out again.
    fn absorb(&mut self, updates: Vec<Update>, cx: &mut Context<Self>) {
        let state = self.thread.state().clone();
        let title = self.thread.title.clone();
        let mut changed = BTreeSet::new();
        let mut meta = false;
        let mut appended = false;
        let mut messages = 0;
        let mut started = false;
        changed.extend(
            self.thread
                .permissions()
                .iter()
                .map(|pending| pending.entry),
        );
        let now = cx.background_executor().now();
        // A burst of entries is a session replayed, not one being written.
        let burst = updates
            .iter()
            .filter(|update| {
                matches!(
                    update,
                    Update::UserChunk(_) | Update::Tool(_) | Update::Notice(..)
                )
            })
            .count()
            > 8;
        for update in updates {
            let model = match &update {
                Update::Model(model) => Some(model.clone()),
                _ => None,
            };
            started |= matches!(update, Update::TurnStarted);
            match &update {
                Update::TurnStarted => self.note_turn(true, now),
                Update::TurnEnded(_) | Update::Closed(_) => self.note_turn(false, now),
                _ => {}
            }
            // What a failed edit gives back is no message the agent took.
            let rewound = match &update {
                Update::Rewound(result) => Some(result.clone()),
                _ => None,
            };
            let changes = self.thread.apply(update);
            self.forget_folds();
            for change in changes {
                if let Change::Appended(index) = change {
                    appended = true;
                    if rewound.is_none()
                        && matches!(self.thread.entries().get(index), Some(Entry::User(_)))
                    {
                        messages += 1;
                    }
                }
                if !burst {
                    self.observe(change, now);
                }
                meta |= self.note(change, &mut changed);
                // What a subagent does shows in the card of the call that
                // started it: that card's row is another height, too.
                if let Change::Appended(index) | Change::Updated(index) = change
                    && let Some(Entry::Tool(step)) = self.thread.entries().get(index)
                    && let Some(launch) = step
                        .parent
                        .as_deref()
                        .and_then(|launch| self.thread.tool_entry(launch))
                {
                    changed.insert(launch);
                }
            }
            if let Some(model) = model {
                // The model a turn runs with is the one its prompt went to.
                if let Some(prompt) = self
                    .thread
                    .entries()
                    .iter()
                    .rposition(|entry| matches!(entry, Entry::User(_)))
                {
                    self.prompt_models.insert(prompt, model.clone());
                }
                // For the app to keep: a session opened again goes on with it.
                if !model.trim().is_empty() {
                    cx.emit(ThreadViewEvent::Model(model));
                }
            }
            if let Some(result) = rewound {
                self.rewound(&result, cx);
            }
        }
        self.follow_edit(started, cx);
        // A turn that began or ended, a prompt that came: the look folds
        // the work of a finished turn, so its rows are measured again.
        if *self.thread.state() != state || messages > 0 || appended {
            self.restudy_last();
        }
        self.prune_lights();
        // Who the agent waits on: the agents it started by itself, too.
        self.share_waiting(cx);
        if meta {
            // State, permissions or the end of a turn: the rows that show them.
            changed.extend(
                self.thread
                    .permissions()
                    .iter()
                    .map(|pending| pending.entry),
            );
            let count = self.thread.entries().len();
            changed.insert(count);
            changed.extend(count.checked_sub(1));
        }
        for index in changed {
            self.remeasure_rows(index..index + 1);
        }
        if meta || appended {
            let sync = self.dock_sync();
            self.dock.update(cx, |dock, cx| {
                // A message of the user's the backend shows is one the agent took
                // from the running turn.
                for _ in 0..messages {
                    dock.delivered(cx);
                }
                dock.sync(sync, cx);
            });
        }
        self.after_change(state, title, cx);
    }

    /// What the prompt area needs to know of the thread.
    fn dock_sync(&self) -> dock::Sync {
        let thread = &self.thread;
        let entries = thread.entries();
        dock::Sync {
            state: thread.state().clone(),
            last_stop: thread.last_stop.clone(),
            session: thread.session.clone(),
            capabilities: thread.capabilities,
            offers: thread.offers.clone(),
            commands: thread.commands.clone(),
            modes: thread.modes.clone(),
            mode: thread.mode.clone(),
            models: thread.models.clone(),
            model: thread.model.clone(),
            providers: thread.providers.clone(),
            usage: thread.usage,
            answered: entries
                .iter()
                .any(|entry| matches!(entry, Entry::Assistant(_))),
            entries: !entries.is_empty(),
            prompts: entries
                .iter()
                .filter(|entry| matches!(entry, Entry::User(_)))
                .count(),
            ready: !matches!(thread.state(), ThreadState::Starting),
        }
    }

    fn sync_dock(&self, cx: &mut Context<Self>) {
        let sync = self.dock_sync();
        self.dock.update(cx, |dock, cx| dock.sync(sync, cx));
    }

    /// Notes what a change starts moving: an answer or reasoning that comes
    /// while a turn runs rises and fades in, a message the agent takes in the
    /// middle of its turn lights up, a call that runs is timed. What comes
    /// while no turn runs (a session replayed) is simply there.
    fn observe(&mut self, change: Change, now: std::time::Instant) {
        let live = matches!(self.thread.state(), ThreadState::Working);
        let (index, appended) = match change {
            Change::Appended(index) => (index, true),
            Change::Updated(index) => (index, false),
            Change::Meta | Change::Truncated(_) => return,
        };
        // When it came, for how long the work before it took.
        if appended {
            self.nest.seen.insert(index, now);
        }
        match self.thread.entries().get(index) {
            Some(Entry::Assistant(_) | Entry::Thought(_)) if live => {
                if appended {
                    self.motion.arrived.insert(index, now);
                    self.motion.fresh.insert(index, Fresh::default());
                } else {
                    self.motion.fresh.entry(index).or_default();
                }
                self.motion.grew = Some((index, now));
            }
            Some(Entry::User(_)) if appended => {
                if let Some(model) = self.thread.model.clone() {
                    self.prompt_models.insert(index, model);
                }
                if live {
                    self.motion.flashed.insert(index, now);
                }
            }
            Some(Entry::Tool(call)) if live || self.motion.started.contains_key(&index) => {
                let running = call.status == kou_thread::ToolStatus::Running;
                self.motion.tool(index, running, now);
            }
            _ => {}
        }
    }

    /// Lets go of the lights of rows that no longer run or are written.
    pub(crate) fn prune_lights(&mut self) {
        let entries = self.thread.entries();
        let working = matches!(self.thread.state(), ThreadState::Working);
        let last = entries.len().checked_sub(1);
        let asked: HashSet<usize> = self
            .thread
            .permissions()
            .iter()
            .map(|pending| pending.entry)
            .collect();
        let cards = self.card_lights_wanted();
        self.lights
            .retain(|(index, kind), _| match (entries.get(*index), kind) {
                (_, 2..) => cards.contains(&(*index, *kind)),
                (Some(Entry::Tool(call)), 0) => {
                    call.status == kou_thread::ToolStatus::Running && !asked.contains(index)
                }
                (Some(Entry::Thought(_)), 1) => working && Some(*index) == last,
                _ => false,
            });
    }

    /// Whether anything of the transcript blinks now: a call that runs, an
    /// answer or reasoning being written, a step of the plan being worked on.
    fn blinks(&self) -> bool {
        if !matches!(self.thread.state(), ThreadState::Working) {
            return false;
        }
        let entries = self.thread.entries();
        let last = entries.len().saturating_sub(1);
        entries
            .iter()
            .enumerate()
            .skip(self.cleared)
            .any(|(index, entry)| match entry {
                // A subagent left to run in the background runs on after
                // its call: its card's time goes on counting.
                Entry::Tool(call) => {
                    call.status == kou_thread::ToolStatus::Running
                        || (call.agent.is_some() && self.subagent_state(index, call).live())
                }
                Entry::Plan(steps) => steps
                    .iter()
                    .any(|step| step.status == kou_thread::PlanStatus::Running),
                Entry::Assistant(_) | Entry::Thought(_) => index == last,
                _ => false,
            })
    }

    /// Asks for what the transcript's motion needs from now: a frame while
    /// something rises or fades in, a frame when what blinks changes. Nothing
    /// at rest, and nothing at all with reduced motion.
    fn pace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cx.reduce_motion() {
            return;
        }
        let now = cx.background_executor().now();
        // A tool call's card opening or closing: its row is another height
        // at each frame; once it has closed, its content is let go.
        for (index, open) in self.motion.unfolded(now) {
            if !open && let Some(parsed) = self.cache.get_mut(index) {
                *parsed = None;
            }
            self.remeasure_rows(index..index + 1);
        }
        let unfolding: Vec<usize> = self.motion.unfolds.keys().copied().collect();
        for index in unfolding {
            self.remeasure_rows(index..index + 1);
        }
        if self.motion.busy(now) {
            window.request_animation_frame();
        } else {
            self.motion.settle(now);
        }
        // The card beside coming out or going back: over a few frames.
        let shows = self.beside_shows();
        if shows != self.nest.beside_was {
            self.nest.beside_was = shows;
            self.nest.beside_moved = Some((now, shows));
        }
        if let Some((since, _)) = self.nest.beside_moved {
            if now.saturating_duration_since(since) < kou_ui::motion::studio::BESIDE {
                window.request_animation_frame();
            } else {
                self.nest.beside_moved = None;
            }
        }
        // A nested agent's card arriving.
        let arrive = kou_ui::motion::studio::ARRIVE;
        self.nest
            .arrived
            .retain(|_, at| now.saturating_duration_since(*at) < arrive);
        if !self.nest.arrived.is_empty() {
            window.request_animation_frame();
        }
        if self.blinking.is_some() || !self.blinks() {
            return;
        }
        let mut next = self.motion.blink(now).1;
        let last = self.thread.entries().len().saturating_sub(1);
        if self.writing(last)
            && let (_, Some(caret)) = self.motion.caret(last, now)
        {
            next = next.min(caret);
        }
        let timer = cx.background_executor().timer(next);
        self.blinking = Some(cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |view, cx| {
                view.blinking = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Record one change of the thread. Returns whether it was a change outside the entries.
    fn note(&mut self, change: Change, changed: &mut BTreeSet<usize>) -> bool {
        match change {
            Change::Appended(index) => {
                self.cache.resize_with(index + 1, || None);
                self.list.splice(index..index, 1);
                // The rows after it moved: their plans are for other indices.
                self.plans.retain(|row, _| *row < index);
                // The entry before may fold now that something follows it.
                changed.extend(index.checked_sub(1));
                false
            }
            Change::Updated(index) => {
                if let Some(parsed) = self.cache.get_mut(index) {
                    *parsed = None;
                }
                changed.insert(index);
                false
            }
            Change::Truncated(at) => {
                self.forget_from(at);
                changed.retain(|index| *index < at);
                changed.extend(at.checked_sub(1));
                false
            }
            Change::Meta => true,
        }
    }

    /// The entries from `at` on are gone: their rows go, and everything the
    /// view kept of them by index.
    fn forget_from(&mut self, at: usize) {
        if self.list.item_count() > at + 1 {
            // The last row, what the thread is doing, stays.
            let gone = self.list.item_count() - 1 - at;
            self.list.splice(at..at + gone, 0);
        }
        self.cache.truncate(at);
        self.plans.retain(|row, _| *row < at);
        self.toggled.retain(|index, _| *index < at);
        self.unfold_heights
            .borrow_mut()
            .retain(|(index, _), _| *index < at);
        self.unfolded.retain(|(index, _)| *index < at);
        self.shots.retain(|(index, _), _| *index < at);
        self.prompt_models.retain(|index, _| *index < at);
        self.lights.retain(|(index, _), _| *index < at);
        self.motion.forget_from(at);
        self.nest
            .turns
            .retain(|index, _| *index < at || *index == crate::nested::NO_PROMPT);
        self.nest.worked_open.retain(|index| *index < at);
        self.forget_folds();
        self.nest.seen.retain(|index, _| *index < at);
        if self.viewing.is_some_and(|(index, _)| index >= at) {
            self.viewing = None;
        }
        self.cleared = self.cleared.min(at);
    }

    /// Tell whoever listens what changed outside the entries, and redraw.
    fn after_change(&mut self, state: ThreadState, title: Option<String>, cx: &mut Context<Self>) {
        if *self.thread.state() != state {
            // An Escape pressed in a turn does not count toward another's.
            self.stop_armed = None;
            cx.emit(ThreadViewEvent::State(self.thread.state().clone()));
        }
        if self.thread.title != title
            && let Some(title) = &self.thread.title
        {
            cx.emit(ThreadViewEvent::Title(title.clone().into()));
        }
        cx.notify();
    }

    /// Takes in changes the view made to the thread itself, as it takes in
    /// the backend's.
    pub(crate) fn take_changes(&mut self, changes: Vec<Change>, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let mut changed = BTreeSet::new();
        for change in changes {
            self.observe(change, now);
            self.note(change, &mut changed);
        }
        for index in changed {
            self.remeasure_rows(index..index + 1);
        }
        self.sync_dock(cx);
    }

    fn prompt(&mut self, blocks: Vec<Block>, cx: &mut Context<Self>) -> bool {
        // A prompt at the end leaves an edit further up behind.
        self.shut_edit(cx);
        if self
            .commands
            .try_send(Command::Prompt(blocks.clone()))
            .is_err()
        {
            log::warn!(
                "the backend of {} no longer takes prompts",
                self.header.agent
            );
            return false;
        }
        let mut changed = BTreeSet::new();
        let change = self.thread.push_user(blocks);
        self.forget_folds();
        self.observe(change, cx.background_executor().now());
        self.note(change, &mut changed);
        for index in changed {
            self.remeasure_rows(index..index + 1);
        }
        // What was just sent is what the user wants to see.
        self.list.scroll_to_end();
        self.sync_dock(cx);
        cx.notify();
        true
    }

    fn dock_event(
        &mut self,
        _: &Entity<Dock>,
        event: &DockEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DockEvent::Send { blocks, draft } => self.send_draft(blocks.clone(), draft, window, cx),
            DockEvent::Steer { blocks } => {
                if self
                    .requests
                    .try_send(Request::Steer(blocks.clone()))
                    .is_err()
                {
                    self.dock.update(cx, |dock, cx| {
                        dock.tell(
                            "The agent's backend is gone: the message was not sent.",
                            true,
                            cx,
                        )
                    });
                }
            }
            DockEvent::Cancel => {
                self.cancel_turn();
            }
            DockEvent::SetMode(id) => self.choose(true, id.clone(), cx),
            DockEvent::SetModel(id) => self.choose(false, id.clone(), cx),
            DockEvent::View(act) => self.view_command(*act, cx),
            DockEvent::Edit(number) => self.edit(*number, window, cx),
            DockEvent::Ask(request) => cx.emit(request.clone()),
            // Only a field standing alone says so.
            DockEvent::Blank => {}
            DockEvent::Height(height) => {
                if (self.dock_height - height).abs() > 0.5 {
                    self.dock_height = *height;
                    let tail = self.thread.entries().len();
                    self.remeasure_rows(tail..tail + 1);
                    cx.notify();
                }
            }
        }
    }

    /// Sends what the field held; its pictures' thumbnails are the transcript's at once.
    fn send_draft(
        &mut self,
        blocks: Vec<Block>,
        draft: &Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.thread.entries().len();
        if !self.prompt(blocks.clone(), cx) {
            let draft = draft.clone();
            let text = draft.text.clone();
            let pictures = draft
                .pictures
                .iter()
                .map(|picture| (picture.label.clone(), picture.prepared.data.clone()))
                .collect();
            self.dock.update(cx, |dock, cx| {
                dock.restore(text, pictures, window, cx);
                dock.tell(
                    "The agent's backend is gone: the message was not sent.",
                    true,
                    cx,
                );
            });
            return;
        }
        for (at, block) in blocks.iter().enumerate() {
            let Block::Image { data, .. } = block else {
                continue;
            };
            if let Some(picture) = draft
                .pictures
                .iter()
                .find(|picture| Arc::ptr_eq(&picture.prepared.data, data))
            {
                let prepared = &picture.prepared;
                self.shots.insert(
                    (index, at),
                    Shot::Ready {
                        thumb: prepared.thumb.clone(),
                        large: prepared.large.clone(),
                        width: prepared.width,
                        height: prepared.height,
                    },
                );
            }
        }
    }

    /// A picture of a message for the transcript, decoded off the main thread
    /// the first time it is shown.
    pub(crate) fn shot(
        &mut self,
        entry: usize,
        block: usize,
        data: &Arc<[u8]>,
        cx: &mut Context<Self>,
    ) -> Shot {
        if let Some(shot) = self.shots.get(&(entry, block)) {
            return shot.clone();
        }
        self.shots.insert((entry, block), Shot::Loading);
        let data = data.clone();
        let decoded = cx.background_spawn(async move { pictures::decode(&data) });
        cx.spawn(async move |this, cx| {
            let decoded = decoded.await;
            this.update(cx, |this, cx| {
                let shot = match decoded {
                    Some((thumb, large, width, height)) => Shot::Ready {
                        thumb,
                        large,
                        width,
                        height,
                    },
                    None => Shot::Failed,
                };
                this.shots.insert((entry, block), shot);
                this.remeasure_rows(entry..entry + 1);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Shot::Loading
    }

    /// Shows a message's picture large over the transcript.
    pub(crate) fn view_picture(&mut self, entry: usize, block: usize, cx: &mut Context<Self>) {
        self.viewing = Some((entry, block));
        cx.notify();
    }

    /// Copies text and marks what was copied for a moment.
    pub(crate) fn copy(&mut self, key: SharedString, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copies += 1;
        let copy = self.copies;
        self.copied = Some(key);
        let timer = cx.background_executor().timer(COPIED);
        cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |this, cx| {
                if this.copies == copy {
                    this.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn view_command(&mut self, act: Act, cx: &mut Context<Self>) {
        match act {
            Act::Copy => match self.last_answer() {
                Some(answer) => {
                    self.copy("answer".into(), answer, cx);
                    self.dock.update(cx, |dock, cx| {
                        dock.tell("Copied the last answer.", false, cx)
                    });
                }
                None => self.dock.update(cx, |dock, cx| {
                    dock.tell("There is no answer to copy yet.", false, cx)
                }),
            },
            Act::Expand | Act::Collapse => {
                let open = act == Act::Expand;
                // The finished turns' work, folded by the quieter look.
                self.nest.worked_open.clear();
                self.forget_folds();
                if open {
                    let entries = self.thread.entries();
                    for (index, entry) in entries.iter().enumerate() {
                        if !matches!(entry, Entry::User(_))
                            && (index == 0 || matches!(entries[index - 1], Entry::User(_)))
                        {
                            self.nest.worked_open.insert(index);
                        }
                    }
                }
                for (index, entry) in self.thread.entries().iter().enumerate() {
                    if matches!(entry, Entry::Tool(_) | Entry::Thought(_)) {
                        self.toggled.insert(index, open);
                        if let Some(parsed) = self.cache.get_mut(index) {
                            *parsed = None;
                        }
                    }
                }
                self.remeasure_all();
                cx.notify();
            }
            Act::Clear => {
                self.cleared = self.thread.entries().len();
                self.remeasure_all();
                self.list.scroll_to_end();
                cx.notify();
            }
            _ => {}
        }
    }

    /// Shows the entries `/clear` put out of sight again.
    pub(crate) fn unclear(&mut self, cx: &mut Context<Self>) {
        self.cleared = 0;
        self.remeasure_all();
        cx.notify();
    }

    /// Edits prompt `number` (1 is the first), or the last, where it stands;
    /// a backend that does not rewind gets it back in the field instead.
    pub(crate) fn edit(
        &mut self,
        number: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompts: Vec<usize> = self
            .thread
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry, Entry::User(_)))
            .map(|(index, _)| index)
            .collect();
        let index = match number {
            None => prompts.last().copied(),
            Some(number) => prompts.get(number - 1).copied(),
        };
        let Some(index) = index else {
            let text = match number {
                Some(number) => format!(
                    "There is no prompt {number}: the conversation has {}.",
                    prompts.len()
                ),
                None if self.edits() => "There is no prompt to edit yet.".to_owned(),
                None => "There is no prompt to put back yet.".to_owned(),
            };
            self.dock.update(cx, |dock, cx| dock.tell(text, false, cx));
            return;
        };
        if self.edits() {
            self.begin_edit(index, window, cx);
        } else {
            self.reuse(index, window, cx);
        }
    }

    /// Puts the prompt of entry `index` back in the field, its pictures too.
    pub(crate) fn reuse(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Entry::User(blocks)) = self.thread.entries().get(index) else {
            return;
        };
        let (text, labels) = draft::shown(blocks);
        let pictures = labels
            .into_iter()
            .filter_map(|(label, at)| match &blocks[at] {
                Block::Image { data, .. } => Some((label, data.clone())),
                _ => None,
            })
            .collect();
        self.dock
            .update(cx, |dock, cx| dock.restore(text, pictures, window, cx));
    }

    /// After a click on a control in the transcript: a pointer click leaves the
    /// keyboard in the field, where the next thing typed belongs; a key press
    /// leaves focus on the control it pressed.
    pub(crate) fn rest_focus(
        &self,
        click: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(click, ClickEvent::Keyboard(_)) {
            window.focus(&self.focus, cx);
        }
    }

    /// The field's focus handle.
    pub(crate) fn field_focus(&self) -> &FocusHandle {
        &self.focus
    }

    /// Put the keyboard in the field.
    pub(crate) fn focus_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    /// Open or close an entry by hand.
    pub(crate) fn toggle(&mut self, index: usize, open: bool, cx: &mut Context<Self>) {
        self.toggled.insert(index, open);
        // A tool call's card opens and closes over a few frames.
        if !cx.reduce_motion() && matches!(self.thread.entries().get(index), Some(Entry::Tool(_))) {
            self.motion
                .unfold(index, open, cx.background_executor().now());
        }
        if let Some(parsed) = self.cache.get_mut(index) {
            *parsed = None;
        }
        self.remeasure_rows(index..index + 1);
        cx.notify();
    }

    /// How far open tool call `index`'s card is now, from 0 to 1: on its way
    /// while it opens or closes, else where `open` says.
    pub(crate) fn unfold_level(&self, index: usize, open: bool, now: std::time::Instant) -> f32 {
        self.motion
            .unfolding(index, now)
            .unwrap_or(if open { 1. } else { 0. })
    }

    /// What the folds were worked out from has changed: they are worked out
    /// again when next asked for.
    pub(crate) fn forget_folds(&self) {
        self.fold_memo.borrow_mut().clear();
    }

    /// Show a terminal output from its first line.
    pub(crate) fn unfold(&mut self, index: usize, piece: usize, cx: &mut Context<Self>) {
        self.unfolded.insert((index, piece));
        self.remeasure_rows(index..index + 1);
        cx.notify();
    }

    /// Answer a permission request.
    pub(crate) fn permit(&mut self, request: u64, option: String, cx: &mut Context<Self>) {
        let Some(entry) = self
            .thread
            .permissions()
            .iter()
            .find(|pending| pending.request == request)
            .map(|pending| pending.entry)
        else {
            return;
        };
        if self
            .commands
            .try_send(Command::Permit { request, option })
            .is_err()
        {
            return;
        }
        let state = self.thread.state().clone();
        self.thread.answered(request);
        let count = self.thread.entries().len();
        self.remeasure_rows(entry..entry + 1);
        self.remeasure_rows(count..count + 1);
        self.sync_dock(cx);
        self.after_change(state, self.thread.title.clone(), cx);
    }

    fn choose(&mut self, mode: bool, id: String, cx: &mut Context<Self>) {
        let command = if mode {
            Command::SetMode(id.clone())
        } else {
            Command::SetModel(id.clone())
        };
        if self.commands.try_send(command).is_ok() {
            // Shown at once; the backend's own report of the mode or model overrides it.
            if mode {
                self.thread.mode = Some(id);
            } else {
                self.thread.model = Some(id);
            }
            self.sync_dock(cx);
        }
    }

    fn cancel_turn(&mut self) -> bool {
        matches!(
            self.thread.state(),
            ThreadState::Working | ThreadState::Blocked
        ) && self.commands.try_send(Command::Cancel).is_ok()
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.viewing.take().is_some() {
            cx.notify();
        } else if self.editor.is_some() {
            // Esc anywhere in the pane cancels the edit open.
            self.close_edit(true, window, cx);
        } else if matches!(
            self.thread.state(),
            ThreadState::Working | ThreadState::Blocked
        ) {
            // One Escape is too easily pressed for what it would end: the
            // first says so, and a second soon after stops the turn.
            let now = cx.background_executor().now();
            let again = self
                .stop_armed
                .take()
                .is_some_and(|at| now.saturating_duration_since(at) <= ESCAPE_TWICE);
            if again {
                self.cancel_turn();
            } else {
                self.stop_armed = Some(now);
                let agent = self.header.agent.clone();
                self.dock.update(cx, |dock, cx| {
                    dock.tell_for(format!("Press Esc again to stop {agent}"), ESCAPE_TWICE, cx)
                });
            }
        } else {
            // Nothing of ours to cancel: Escape belongs to whatever is around the pane.
            cx.propagate();
        }
    }

    fn allow_default(&mut self, _: &AllowDefault, window: &mut Window, cx: &mut Context<Self>) {
        let default = self.thread.permissions().first().and_then(|pending| {
            default_option(&pending.options)
                .map(|option| (pending.request, pending.options[option].id.clone()))
        });
        match default {
            Some((request, option)) => self.permit(request, option, cx),
            // Nothing to allow: over an empty field the same key forces in
            // what heads the queue, while the agent works.
            None => {
                if !self.dock.update(cx, |dock, cx| dock.force_head(window, cx)) {
                    cx.propagate();
                }
            }
        }
    }

    fn page(&mut self, down: bool, cx: &mut Context<Self>) {
        // An action runs outside the scaled subtree: the factor is the view's own.
        let factor = self.scale.get();
        let shown = self.list.viewport_bounds().size.height - gpui::px(self.dock_height * factor);
        let page = shown.max(gpui::px(LINE * factor)) * 0.85;
        self.list.scroll_by(if down { page } else { -page });
        cx.notify();
    }

    fn scroll_to_top(&mut self, _: &ScrollToTop, _: &mut Window, cx: &mut Context<Self>) {
        self.list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: Pixels::ZERO,
        });
        cx.notify();
    }

    fn scroll_to_bottom(&mut self, _: &ScrollToBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.list.scroll_to_end();
        cx.notify();
    }

    fn header(&self, theme: &Theme) -> Div {
        let (mark, word) = match self.thread.state() {
            ThreadState::Starting => (Mark::Idle, "starting"),
            ThreadState::Idle => (Mark::Idle, Mark::Idle.label()),
            ThreadState::Working => (Mark::Working, Mark::Working.label()),
            ThreadState::Blocked => (Mark::Blocked, Mark::Blocked.label()),
            ThreadState::Closed(Some(_)) => (Mark::Failed, "stopped"),
            ThreadState::Closed(None) => (Mark::Idle, "closed"),
        };
        div()
            .debug_selector(|| "header".into())
            .flex_none()
            .h(px(32.))
            .px(px(EDGE))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_b_1()
            .border_color(theme.line)
            .child(
                div()
                    .flex_none()
                    .w(px(crate::entries::GUTTER))
                    .flex()
                    .justify_center()
                    .child(status_mark(mark, theme)),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(scale::text(self.header.agent.clone())),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .font_family(theme.mono_font.clone())
                    .text_size(px(metric::TEXT_SMALL))
                    .text_color(theme.fg_muted)
                    .child(scale::text(display_path(&self.header.cwd, None))),
            )
            .children(self.thread.title.clone().map(|title| {
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.fg_dim)
                    .child(scale::text(title))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .text_size(px(metric::TEXT_SMALL))
                    .text_color(theme.fg_muted)
                    .child(scale::text(word)),
            )
    }

    /// Keeps [`Self::away`] true to the list. Whether the list is at its end is
    /// only settled when it is laid out, so this looks after the list was, and
    /// asks for one more frame when the answer changed.
    fn watch_end(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let (list, away, view) = (self.list.clone(), self.away, cx.weak_entity());
        canvas(
            move |_, _, cx| {
                let now = !list.is_following_tail();
                if now != away {
                    cx.defer(move |cx| {
                        view.update(cx, |view, cx| {
                            view.away = now;
                            cx.notify();
                        })
                        .ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_0()
    }

    /// A message's picture over the transcript, until Escape or a click.
    fn picture_overlay(
        &self,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let (entry, block) = self.viewing?;
        let Shot::Ready {
            large,
            width,
            height,
            ..
        } = self.shots.get(&(entry, block))?.clone()
        else {
            return None;
        };
        // The viewport as drawn, in the unscaled sizes everything here is
        // written in. Zoomed, the list is wider than what it shows (see
        // `content`): the view's own unzoomed width is the width shown.
        let mut viewport = self
            .list
            .viewport_bounds()
            .size
            .map(|side| side / scale::factor());
        if let Some(width) = self.unzoomed_width() {
            viewport.width = width;
        }
        let size = large.size(0);
        let (shown_w, shown_h) = (size.width.0.max(1) as f32, size.height.0.max(1) as f32);
        let room_w = (viewport.width.as_f32() - 64.).max(120.);
        let room_h = (viewport.height.as_f32() - 96.).max(120.);
        let scale = (room_w / shown_w).min(room_h / shown_h).min(0.5);
        let label = self
            .thread
            .entries()
            .get(entry)
            .and_then(|found| match found {
                Entry::User(blocks) => draft::shown(blocks)
                    .1
                    .into_iter()
                    .find(|(_, at)| *at == block)
                    .map(|(label, _)| label),
                _ => None,
            })
            .unwrap_or_default();
        let mime = match self.thread.entries().get(entry) {
            Some(Entry::User(blocks)) => match blocks.get(block) {
                Some(Block::Image { mime, data }) => {
                    pictures::describe(width, height, mime, data.len())
                }
                _ => String::new(),
            },
            _ => String::new(),
        };
        let scrim = theme.pane.opacity(0.86);
        Some(
            div()
                .id("picture-overlay")
                .debug_selector(|| "picture-overlay".into())
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(scrim)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.viewing = None;
                    this.focus_field(window, cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .p(px(8.))
                        .rounded(px(metric::RADIUS_LARGE))
                        .border_1()
                        .border_color(theme.line_strong)
                        .bg(theme.raised)
                        .shadow(crate::picker::shadow(theme))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div().rounded(px(metric::RADIUS)).overflow_hidden().child(
                                img(ImageSource::Render(large))
                                    .w(px(shown_w * scale))
                                    .h(px(shown_h * scale)),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(10.))
                                .font_family(theme.mono_font.clone())
                                .text_size(px(metric::TEXT_SMALL))
                                .text_color(theme.fg_dim)
                                .child(
                                    div()
                                        .text_color(theme.accent)
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child(scale::text(label)),
                                )
                                .child(scale::text(mime))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .text_color(theme.fg_muted)
                                        .child(scale::text("esc or a click closes it")),
                                ),
                        ),
                )
                .with_animation(
                    ("picture-overlay-in", entry * 1000 + block),
                    motion::enter(motion::BASE),
                    |this, t| this.opacity(t),
                ),
        )
    }
}

impl ThreadView {
    /// The quieter look's transcript fading out under its top edge: over
    /// what is scrolled out above it, no deeper than the look's fade, and
    /// never over a prompt's bubble whose top is in sight, so that a bubble
    /// at rest is one flat fill wherever it stands. Painted after the rows,
    /// which say where their bubbles are as they are laid out.
    fn top_fade(&self, theme: &Theme) -> impl IntoElement {
        let (list, tops) = (self.list.clone(), self.bubble_tops.clone());
        let (deepest, pane) = (px(crate::studio::FADE), theme.pane);
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let tops = std::mem::take(&mut *tops.borrow_mut());
                let top = list.logical_scroll_top();
                let above = if top.item_ix == 0 {
                    top.offset_in_item
                } else {
                    deepest
                };
                let mut depth = deepest.min(above);
                for bubble in tops {
                    if bubble >= bounds.top() {
                        depth = depth.min(bubble - bounds.top());
                    }
                }
                if depth <= Pixels::ZERO {
                    return;
                }
                window.paint_quad(gpui::fill(
                    gpui::Bounds::new(bounds.origin, gpui::size(bounds.size.width, depth)),
                    gpui::linear_gradient(
                        180.,
                        gpui::linear_color_stop(pane, 0.),
                        gpui::linear_color_stop(pane.opacity(0.), 1.),
                    ),
                ));
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// Everything the view shows, in the sizes of the factor in effect.
    fn content(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let transcript = list(
            self.list.clone(),
            cx.processor(|this, index, window, cx| {
                trace::row_built(&this.trace);
                #[cfg(test)]
                {
                    this.rows_built += 1;
                }
                let row = this.placed_row(index, window, cx);
                trace::traced(&this.trace, Part::Row(index), row)
            }),
        );
        // A list whose width changes forgets the height of every row, and
        // builds and lays out a screenful more rows than it shows to measure
        // them again. Zoomed, its rows have their own exact width (see
        // `placed_row`), so the list itself is as wide as the content at
        // the largest zoom, the same at every zoom, and what is past the
        // content is cut off by the pane. A new zoom measures the rows on
        // screen again (see `rescale`), a new unzoomed width all of them
        // (see `watch_width`).
        let transcript = match self.unzoomed_width() {
            Some(width) => transcript
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .w(width * scale::ZOOM_RANGE.1),
            None => transcript.size_full(),
        };
        let (size, line) = self.text_base();
        let studio = self.around.studio();
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.fg)
            .font_family(theme.ui_font.clone())
            .text_size(px(size))
            .line_height(px(line))
            .children(self.header_visible.then(|| self.header(theme)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(transcript)
                    .child(self.watch_end(cx))
                    .child(self.watch_room(cx))
                    // The quieter look's transcript fades out under its top edge.
                    .children(studio.then(|| self.top_fade(theme)))
                    .children(self.ticks(theme, cx))
                    // The lights over the rows: views of their own, which
                    // move without the transcript being built again.
                    .children((!self.lights.is_empty()).then(|| {
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .overflow_hidden()
                            .children(self.lights.values().map(|light| {
                                light.clone().cached(
                                    gpui::StyleRefinement::default()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .size_full(),
                                )
                            }))
                    }))
                    // The prompt area, over the transcript's bottom: an entity
                    // drawn apart, so what moves there never rebuilds the transcript.
                    .child(
                        div().absolute().top_0().left_0().size_full().child(
                            self.dock
                                .clone()
                                .cached(gpui::StyleRefinement::default().size_full()),
                        ),
                    )
                    // `.jump`: back to the end, centred over the field (which
                    // stands off the pane's right by the room the card beside
                    // the transcript takes), in the text's colour on the
                    // surface's.
                    .children(self.away.then(|| {
                        // The quieter look's: a muted word on a raised button.
                        let (ink, fill, ring) = if studio {
                            (theme.fg_dim, theme.raised, theme.accent)
                        } else {
                            (theme.pane, theme.fg, theme.accent)
                        };
                        let frame = if studio { theme.line_strong } else { fill };
                        let face = if studio {
                            theme.ui_font.clone()
                        } else {
                            theme.mono_font.clone()
                        };
                        div()
                            .absolute()
                            .bottom(px(self.dock_height + 10.))
                            .left_0()
                            .right_0()
                            // As the column and the field are: clear of the
                            // room the card beside the transcript takes.
                            .pr(px(self.around.inset.get()))
                            // The cockpit's field stands under the text,
                            // clear of the rail: its middle is the text's.
                            .pl(px(if studio {
                                0.
                            } else {
                                crate::entries::TEXT_EDGE - crate::entries::EDGE
                            }))
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .id("latest")
                                    .debug_selector(|| "latest".into())
                                    .h(px(28.))
                                    .px(px(12.))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .rounded(px(metric::RADIUS))
                                    .border_1()
                                    .border_color(frame)
                                    .bg(fill)
                                    .shadow(crate::picker::shadow(theme))
                                    .cursor_pointer()
                                    .tab_index(0)
                                    .hover(|style| style.opacity(0.9))
                                    .focus_visible(move |style| style.border_color(ring))
                                    .font_family(face)
                                    .font_weight(if studio {
                                        gpui::FontWeight::MEDIUM
                                    } else {
                                        gpui::FontWeight::BOLD
                                    })
                                    .text_size(px(if studio { 11.5 } else { 10.5 }))
                                    .text_color(ink)
                                    .child(icon(Icon::ChevronDown, ink).size(px(12.)))
                                    .child(scale::text(if studio { "Latest" } else { "LATEST" }))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.list.scroll_to_end();
                                        this.away = false;
                                        this.focus_field(window, cx);
                                        cx.notify();
                                    })),
                            )
                    }))
                    .children(self.beside_card(theme, cx))
                    .children(self.picture_overlay(theme, cx)),
            )
    }

    /// The text size and line pitch of the transcript in the look it has.
    fn text_base(&self) -> (f32, f32) {
        if self.around.studio() {
            (crate::studio::PROSE, crate::studio::LINE)
        } else {
            (PROSE, LINE)
        }
    }

    /// Keeps what the card beside and the ticks need to know of the last
    /// layout: the view's width, unscaled, and the rows on screen. A width
    /// that moves the column for the card, or rows that scrolled, ask for
    /// one more frame.
    fn watch_room(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let (width, shown) = (self.nest.width.clone(), self.nest.shown.clone());
        let (list, view) = (self.list.clone(), cx.weak_entity());
        let factor = scale::factor();
        canvas(
            move |bounds, _, cx| {
                let unscaled = bounds.size.width.as_f32() / factor.max(0.01);
                let resized = (width.get() - unscaled).abs() > 0.5;
                if resized {
                    width.set(unscaled);
                }
                let count = list.item_count();
                let top = list.logical_scroll_top().item_ix;
                let mut end = (top + 1).min(count);
                while end < count && list.item_is_below_viewport(end) == Some(false) {
                    end += 1;
                }
                let scrolled = shown.replace((top, end)) != (top, end);
                if resized || scrolled {
                    let view = view.clone();
                    cx.defer(move |cx| {
                        view.update(cx, |view, cx| {
                            if resized {
                                view.fit_beside(cx);
                            }
                            cx.notify();
                        })
                        .ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// The view's unzoomed width, while it follows a zoom and once it is known.
    fn unzoomed_width(&self) -> Option<Pixels> {
        self.zoom.as_ref()?;
        self.held_width.get()
    }

    /// Row `index` of the transcript as the list places it. At a zoom it is
    /// drawn following its plan (see `kou_ui::scale::exact`): its blocks where
    /// they stand at 1 times the zoom, its texts in their lines at 1, and the
    /// row itself as tall as its place at 1 times the zoom, its top and its
    /// bottom each rounded once (see [`Self::placed_height`]). Laid out at the
    /// zoom alone, every gap and line height is rounded on its own and a row
    /// far up the transcript is pixels off its place, by more every few
    /// frames of a zoom gesture.
    fn placed_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let factor = self.scale.get();
        let Some(unzoomed) = self.unzoomed_width() else {
            return self.item(index, window, cx);
        };
        // A row is as wide as the list, which is rounded to whole pixels:
        // at a zoom, a different fraction every frame. Its exact width keeps
        // its lines where they are.
        let row = div().w(unzoomed * factor);
        if factor == 1. {
            return row.child(self.item(index, window, cx)).into_any_element();
        }
        let plan = match self.plans.get(&index).filter(|plan| !plan.is_stale()) {
            Some(plan) => plan.clone(),
            None => {
                let plan = Rc::new(self.plan_row(index, unzoomed, window, cx));
                self.plans.insert(index, plan.clone());
                plan
            }
        };
        let device = window.scale_factor();
        let item = exact::replaying(&plan, device, || self.item(index, window, cx));
        let row = row.child(item);
        exact::anchored(match self.placed_height(index, factor, device) {
            Some(height) => row.h(height),
            None => row,
        })
    }

    /// Lays row `index` out at 1, in the text style the transcript has there,
    /// `unzoomed` wide, for where its blocks stand.
    fn plan_row(
        &mut self,
        index: usize,
        unzoomed: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> exact::Plan {
        let (item, recording) = exact::record(|| self.item(index, window, cx));
        let theme = crate::studio::look_theme(self.around.look.get(), cx);
        let (size, line) = self.text_base();
        let row = div()
            .w(unzoomed)
            .text_color(theme.fg)
            .font_family(theme.ui_font.clone())
            .text_size(gpui::px(size))
            .line_height(gpui::px(line))
            .child(item)
            .into_any_element();
        recording.plan(row, unzoomed, window, cx)
    }

    /// How tall row `index` is drawn at `factor`: from where its top stands at
    /// 1 times the factor to where its bottom does, each rounded to a whole
    /// device pixel once, counted from the transcript's end while it follows
    /// its end and otherwise from the row at its top (see [`Self::rows_from`]).
    /// The list puts each row under the one before: so each row stands where
    /// its place at 1 times the zoom puts it, give or take a pixel, however
    /// many rows come between. `None` while a row it is counted from is not
    /// planned yet.
    fn placed_height(&self, index: usize, factor: f32, device: f32) -> Option<Pixels> {
        let height = |row: usize| self.plans.get(&row).map(|plan| plan.height());
        let sum =
            |rows: std::ops::Range<usize>| -> Option<f32> { rows.map(height).sum::<Option<f32>>() };
        let place = |unscaled: f32| (unscaled * factor * device).round() / device;
        let (top, bottom) = match self.rows_from {
            RowsFrom::Row(from) if index >= from => {
                let top = sum(from..index)?;
                (top, top + height(index)?)
            }
            RowsFrom::Row(from) => {
                let bottom = -sum(index + 1..from)?;
                (bottom - height(index)?, bottom)
            }
            RowsFrom::End(count) => {
                let bottom = -sum(index + 1..count)?;
                (bottom - height(index)?, bottom)
            }
        };
        Some(gpui::px(place(bottom) - place(top)))
    }

    /// Keeps [`Self::held_width`]: the view's unzoomed width, held while
    /// layout rounding moves it by less than [`ZOOM_SLACK`] device pixels. Its
    /// bounds are only known once it is laid out, so this looks after layout
    /// and asks for one more frame when the width changed: the view's own
    /// width, which a zoom alone does not change.
    fn watch_width(
        &self,
        zoom: f32,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let (held, view) = (self.held_width.clone(), cx.weak_entity());
        let device = window.scale_factor().max(1.);
        let trace = self.trace.clone();
        canvas(
            move |bounds, window, cx| {
                trace::view_bounds(&trace, bounds, window.scale_factor());
                let unzoomed = bounds.size.width / zoom;
                let slack = gpui::px(ZOOM_SLACK / (zoom * device));
                if held
                    .get()
                    .is_some_and(|width| (width - unzoomed).abs() <= slack)
                {
                    return;
                }
                held.set(Some(unzoomed));
                let view = view.clone();
                // Another width wraps every row anew: all are measured again.
                cx.defer(move |cx| {
                    view.update(cx, |view, cx| {
                        view.remeasure_all();
                        cx.notify();
                    })
                    .ok();
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.frames += 1;
        }
        let zoom = self.zoom.is_some().then(|| self.zoom());
        // Drawn at the zoom itself, frame by frame; a zoom that is 100% to
        // within a fifth of a percent is drawn as the pane is without one.
        let factor = zoom.map_or(1., |zoom| if (zoom - 1.).abs() < 0.002 { 1. } else { zoom });
        let rescaled = self.rescale(factor);
        let moving = self.track_motion(rescaled, cx);
        self.pace(window, cx);
        if std::mem::take(&mut self.focus_editor) {
            cx.defer_in(window, |this, window, cx| {
                if let Some(editor) = &this.editor {
                    window.focus(&editor.focus, cx);
                }
            });
        }
        // The rows laid out this frame note their places anew.
        self.spots.borrow_mut().clear();
        self.forget_folds();
        self.rows_from = if self.list.is_following_tail() {
            RowsFrom::End(self.list.item_count())
        } else {
            RowsFrom::Row(self.list.logical_scroll_top().item_ix)
        };
        let cx = &*cx;
        let theme = crate::studio::look_theme(self.around.look.get(), cx);
        let Some(zoom) = zoom else {
            return self
                .pane(self.content(theme, cx), theme, cx)
                .into_any_element();
        };
        let started = trace::begin(&self.trace, zoom, factor, rescaled, || {
            let top = self.list.logical_scroll_top();
            format!(
                "held={:.3} room={:.2} top={}:{:.2}{}",
                self.held_width.get().map_or(0., f32::from),
                self.dock_height,
                top.item_ix,
                f32::from(top.offset_in_item),
                if self.list.is_following_tail() {
                    " tail"
                } else {
                    ""
                },
            )
        });
        // The content is built, laid out, prepainted and painted at the factor,
        // at the view's unzoomed width times the factor (the view's width,
        // give or take the rounding the width is held against): its
        // lines are the lines it has at 1. Until the width is known, the
        // first frame takes it from the zoom.
        let width: gpui::Length = match self.held_width.get() {
            Some(width) => (width * factor).into(),
            None => gpui::relative(factor / zoom).into(),
        };
        let content = scale::with_factor(factor, || self.content(theme, cx))
            .flex_none()
            .w(width);
        let content = trace::traced(
            &self.trace,
            Part::Content,
            scale::scaled(factor, content).moving(moving),
        );
        trace::built(&self.trace, started);
        self.pane(div(), theme, cx)
            .relative()
            .flex()
            .justify_center()
            .overflow_hidden()
            .child(self.watch_width(zoom, window, cx))
            .child(content)
            .into_any_element()
    }
}

impl ThreadView {
    /// `element` as the pane: its focus, its keys, its surface.
    fn pane(&self, element: Div, theme: &Theme, cx: &Context<Self>) -> gpui::Stateful<Div> {
        element
            .id("thread-view")
            .key_context(CONTEXT)
            .track_focus(&self.pane_focus)
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.page(false, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.page(true, cx)))
            .on_action(cx.listener(Self::scroll_to_top))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_action(cx.listener(|_, _: &FocusNext, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPrevious, window, cx| window.focus_prev(cx)))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::allow_default))
            .size_full()
            .bg(theme.pane)
    }
}

/// Where a frame places the transcript's rows from. See [`ThreadView::placed_height`].
#[derive(Clone, Copy)]
enum RowsFrom {
    /// From the end of the list, which has this many rows.
    End(usize),
    /// From the row at the top of the list.
    Row(usize),
}

/// The option a permission request answers with by default: its first allow.
pub(crate) fn default_option(options: &[kou_thread::PermissionOption]) -> Option<usize> {
    options.iter().position(|option| {
        matches!(
            option.kind,
            PermissionKind::AllowOnce | PermissionKind::AllowAlways
        )
    })
}

/// A path as the pane shows it: relative to `base` when inside it, with the
/// home directory as `~` otherwise.
pub(crate) fn display_path(path: &Path, base: Option<&Path>) -> SharedString {
    if let Some(base) = base
        && let Ok(relative) = path.strip_prefix(base)
        && !relative.as_os_str().is_empty()
    {
        return relative.display().to_string().into();
    }
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    if let Some(home) = home
        && let Ok(relative) = path.strip_prefix(&home)
    {
        if relative.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~{}{}", std::path::MAIN_SEPARATOR, relative.display()).into();
    }
    path.display().to_string().into()
}

/// One figure of the line under the field: its name, its value, and the
/// share it is of a whole, for its meter.
pub(crate) struct Figure {
    pub key: &'static str,
    pub value: String,
    pub share: Option<f32>,
}

/// The figures under the field (`.stats`). Only what the backend reported;
/// nothing is estimated.
pub(crate) fn usage_figures(usage: &kou_thread::Usage) -> Vec<Figure> {
    let mut figures = Vec::new();
    match (usage.context_used, usage.context_limit) {
        (Some(used), Some(limit)) if limit > 0 => figures.push(Figure {
            key: "CONTEXT",
            value: format!(
                "{}/{} · {}%",
                tokens(used),
                tokens(limit),
                used.saturating_mul(100) / limit
            ),
            share: Some(used as f32 / limit as f32),
        }),
        (Some(used), _) => figures.push(Figure {
            key: "CONTEXT",
            value: tokens(used),
            share: None,
        }),
        (None, _) => {}
    }
    if let Some(input) = usage.input_tokens {
        figures.push(Figure {
            key: "IN",
            value: tokens(input),
            share: None,
        });
    }
    if let Some(output) = usage.output_tokens {
        figures.push(Figure {
            key: "OUT",
            value: tokens(output),
            share: None,
        });
    }
    if let Some(cost) = usage.cost_usd {
        figures.push(Figure {
            key: "COST",
            // Fractions of a cent still show as something.
            value: if cost > 0. && cost < 0.01 {
                format!("${cost:.4}")
            } else {
                format!("${cost:.2}")
            },
            share: None,
        });
    }
    figures
}

/// The usage line's parts. Only what the backend reported; nothing is estimated.
#[cfg(test)]
pub(crate) fn usage_parts(usage: &kou_thread::Usage) -> Vec<String> {
    let mut parts = Vec::new();
    match (usage.context_used, usage.context_limit) {
        (Some(used), Some(limit)) if limit > 0 => parts.push(format!(
            "context {} of {} ({}%)",
            tokens(used),
            tokens(limit),
            used.saturating_mul(100) / limit
        )),
        (Some(used), _) => parts.push(format!("context {}", tokens(used))),
        (None, _) => {}
    }
    if let Some(input) = usage.input_tokens {
        parts.push(format!("{} in", tokens(input)));
    }
    if let Some(output) = usage.output_tokens {
        parts.push(format!("{} out", tokens(output)));
    }
    if let Some(cost) = usage.cost_usd {
        // Fractions of a cent still show as something.
        parts.push(if cost > 0. && cost < 0.01 {
            format!("${cost:.4}")
        } else {
            format!("${cost:.2}")
        });
    }
    parts
}

/// A token count, shortened: `842`, `42.1k`, `1.25M`.
pub(crate) fn tokens(count: u64) -> String {
    if count < 1_000 {
        count.to_string()
    } else if count < 1_000_000 {
        let thousands = count as f64 / 1_000.;
        if count.is_multiple_of(1_000) {
            format!("{thousands:.0}k")
        } else {
            format!("{thousands:.1}k")
        }
    } else {
        let millions = count as f64 / 1_000_000.;
        if count.is_multiple_of(1_000_000) {
            format!("{millions:.0}M")
        } else {
            format!("{millions:.2}M")
        }
    }
}

/// Lets the executor run whatever else is queued, once.
struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kou_thread::Usage;

    #[test]
    fn token_counts_are_shortened_not_invented() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(842), "842");
        assert_eq!(tokens(1_000), "1k");
        assert_eq!(tokens(42_130), "42.1k");
        assert_eq!(tokens(200_000), "200k");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_250_000), "1.25M");
    }

    #[test]
    fn the_usage_line_shows_only_what_was_reported() {
        assert!(usage_parts(&Usage::default()).is_empty());
        assert_eq!(
            usage_parts(&Usage {
                context_used: Some(42_130),
                context_limit: Some(200_000),
                input_tokens: Some(12_300),
                output_tokens: Some(1_204),
                cost_usd: Some(0.4217),
            }),
            [
                "context 42.1k of 200k (21%)",
                "12.3k in",
                "1.2k out",
                "$0.42"
            ]
        );
        // A used count without a limit is shown as it is: no percentage is made up.
        assert_eq!(
            usage_parts(&Usage {
                context_used: Some(900),
                cost_usd: Some(0.0042),
                ..Usage::default()
            }),
            ["context 900", "$0.0042"]
        );
        // A limit alone says nothing about this session.
        assert!(
            usage_parts(&Usage {
                context_limit: Some(200_000),
                ..Usage::default()
            })
            .is_empty()
        );
    }

    #[test]
    fn paths_are_shown_relative_to_the_working_directory_or_home() {
        let base = Path::new("/work/project");
        assert_eq!(
            display_path(Path::new("/work/project/src/main.rs"), Some(base)),
            "src/main.rs"
        );
        assert_eq!(
            display_path(Path::new("/etc/hosts"), Some(base)),
            "/etc/hosts"
        );
        if let Some(home) = std::env::var_os("HOME") {
            let inside = Path::new(&home).join("notes").join("a.md");
            let shown = display_path(&inside, Some(base));
            assert_eq!(
                shown,
                format!("~{0}notes{0}a.md", std::path::MAIN_SEPARATOR)
            );
            assert_eq!(display_path(Path::new(&home), None), "~");
        }
    }

    #[test]
    fn the_default_answer_is_the_first_allow() {
        let option = |id: &str, kind| kou_thread::PermissionOption {
            id: id.into(),
            label: id.into(),
            kind,
        };
        let options = [
            option("no", PermissionKind::RejectOnce),
            option("always", PermissionKind::AllowAlways),
            option("once", PermissionKind::AllowOnce),
        ];
        assert_eq!(default_option(&options), Some(1));
        assert_eq!(default_option(&options[..1]), None);
    }
}
