//! The area under the transcript where prompts are written and wait: the
//! queue, the field in its frame with the controls beside it (the mode and
//! model pickers, attach, send, queue, force, stop), the usage line, and the
//! help card. It decides what Enter does with what is written: a command of
//! the view or the app runs here, a prompt goes to the backend when it is
//! free and waits in the queue while it works. Forced (Cmd+Enter), a prompt
//! does not wait for the turn to end: an agent that takes messages while it
//! works is handed it, and any other has its turn stopped and gets it next.
//!
//! It is an entity of its own so that what moves here (a menu opening, the
//! queue reordering, a button changing) redraws only this, never the
//! transcript above.

mod studio;

use crate::lights::{Light, Lights};
use crate::{
    commands::{self, Act, Offer},
    composer::parts::{GLIDE, SETTLE, grow},
    composer::{self, Composer, ComposerEvent},
    draft::Draft,
    entries::{EDGE, MEASURE, TEXT_EDGE},
    history,
    queue::{Item, Queue},
    trace,
    view::{Cancel, MenuDown, MenuUp, ThreadViewRequest, allow_key, usage_figures},
};
use gpui::{
    Anchor, AnimationExt as _, AnyElement, App, AppContext as _, ClickEvent, Context, Div, Entity,
    EventEmitter, ExternalPaths, FocusHandle, Focusable, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, PathPromptOptions, Render, SharedString,
    SpringAnimation, StatefulInteractiveElement as _, Styled as _, Subscription, Window, anchored,
    deferred, div, point, prelude::FluentBuilder as _,
};
use kou_thread::{Block, Capabilities, Choice, Offers, Provider, Stop, ThreadState, Usage};
use kou_ui::{
    Icon, Theme, icon, key_hint, metric, motion,
    scale::{self, px},
    theme,
};
use std::{
    cell::Cell,
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    time::{Instant, SystemTime},
};

/// The key context of an open mode or model menu.
pub(crate) const MENU_CONTEXT: &str = "ThreadMenu";
/// Height of a queued prompt's row (`.queue-item`: 36px at least).
const QUEUE_ROW: f32 = 36.;
/// How far in from the field's sides the queue's card stands: clear of the
/// field's corners, on which it sits.
const QUEUE_INSET: f32 = 9. + metric::RADIUS_LARGE;

/// What the thread is and offers, as the dock needs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Sync {
    pub state: ThreadState,
    pub last_stop: Option<Stop>,
    pub session: Option<String>,
    pub capabilities: Capabilities,
    pub offers: Offers,
    pub commands: Vec<Choice>,
    pub modes: Vec<Choice>,
    pub mode: Option<String>,
    pub models: Vec<Choice>,
    pub model: Option<String>,
    /// Who serves the models, when the backend says.
    pub providers: Vec<Provider>,
    pub usage: Usage,
    /// There is an answer to copy, and anything at all to fold or clear.
    pub answered: bool,
    pub entries: bool,
    /// How many prompts the conversation has.
    pub prompts: usize,
    /// Whether the backend has said it is ready, so what it takes is known.
    pub ready: bool,
}

/// What the dock asks of the view around it.
#[derive(Clone, Debug)]
pub(crate) enum DockEvent {
    /// Send these blocks as a prompt; the draft keeps the pictures' thumbnails.
    Send {
        blocks: Vec<Block>,
        draft: Draft,
    },
    /// Hand these blocks to the running turn.
    Steer {
        blocks: Vec<Block>,
    },
    /// Enter, or the run button, on a field standing alone with nothing in
    /// it: there is no prompt, and the app around it may still have a use
    /// for the gesture.
    Blank,
    /// Stop the running turn.
    Cancel,
    SetMode(String),
    SetModel(String),
    /// A command about the transcript: copy, expand, collapse, clear.
    View(Act),
    /// Put prompt `n` (1 is the first), or the last, back in the field.
    Edit(Option<usize>),
    /// Something for the app around the view.
    Ask(ThreadViewRequest),
    /// The dock's height changed and has settled: the transcript keeps this
    /// much room under its last row.
    Height(f32),
}

/// Which of the backend's lists of choices a picker shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickerKind {
    Mode,
    Model,
}

impl PickerKind {
    fn title(self) -> &'static str {
        match self {
            PickerKind::Mode => "Mode",
            PickerKind::Model => "Model",
        }
    }

    fn id(self) -> &'static str {
        match self {
            PickerKind::Mode => "mode-picker",
            PickerKind::Model => "model-picker",
        }
    }
}

/// An open picker: one focus handle per choice, so the choices are real tab stops.
pub(crate) struct Picker {
    kind: PickerKind,
    items: Vec<FocusHandle>,
    opened: u64,
}

/// A queued prompt dragged to another place.
#[derive(Clone, Debug)]
struct QueueDrag {
    id: u64,
    text: SharedString,
}

/// What a dragged prompt looks like under the pointer, at the factor of the
/// queue it was dragged from.
struct DragGhost(SharedString, f32);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let ghost = scale::with_factor(self.1, || {
            div()
                .max_w(px(320.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(metric::RADIUS))
                .border_1()
                .border_color(theme.accent)
                .bg(theme.rail)
                .text_size(px(metric::TEXT))
                .text_color(theme.fg)
                .truncate()
                .child(scale::text(self.0.clone()))
        });
        scaled(self.1, ghost)
    }
}

/// `element` drawn at `factor` by a view nested in the thread view: as it is
/// at 1, and in a scaled subtree of its own at any other factor, which keeps
/// the factor when GPUI builds the view again without the views around it.
pub(crate) fn scaled(factor: f32, element: impl IntoElement) -> gpui::AnyElement {
    if factor == 1. {
        element.into_any_element()
    } else {
        scale::scaled(factor, element)
            .inherit_rem()
            .into_any_element()
    }
}

/// The prompt area of a thread view.
pub(crate) struct Dock {
    pub(crate) composer: Entity<Composer>,
    pub(crate) focus: FocusHandle,
    agent: SharedString,
    cwd: PathBuf,
    pub(crate) sync: Sync,
    draft_empty: bool,
    pub(crate) picker: Option<Picker>,
    /// The model picker, while it is open. See [`crate::picker`].
    pub(crate) model_picker: Option<crate::picker::ModelPicker>,
    /// When the turn that runs began, for the activity line's clock.
    busy_since: Option<Instant>,
    /// The activity line's meter and clock: a view of their own, which moves
    /// without the prompt area being built again.
    activity: Option<Entity<Lights>>,
    /// The epoch the activity meter's phase is counted from.
    epoch: Instant,
    /// The prompt area is narrow (under 520 pixels): the controls' names go.
    narrow: Rc<Cell<bool>>,
    /// Drawn on its own, without a conversation: see [`crate::PromptField`].
    pub(crate) standalone: bool,
    /// What the app puts at the start of the field's foot, built every frame.
    pub(crate) foot: Option<crate::field::Foot>,
    pickers_opened: u64,
    pub(crate) queue: Queue,
    /// The queued prompt a turn is being stopped for (see
    /// [`Dock::interrupt`]), and whether the queue was paused before: once
    /// the turn has ended that prompt goes, and the queue is as it was.
    interrupting: Option<(u64, bool)>,
    queue_focus: HashMap<u64, FocusHandle>,
    /// A queued prompt taken into the field to be edited, with its place.
    editing: Option<(usize, Item)>,
    /// Queued prompts on their way out, with where they were.
    leaving: Vec<(Item, usize, Instant)>,
    shown_queue: Vec<(Item, usize)>,
    queue_height: Rc<Cell<f32>>,
    /// The dock's height as last drawn, and as last told to the view, unscaled,
    /// with the factor each was measured at.
    seen_height: Rc<Cell<(f32, f32)>>,
    reported_height: Rc<Cell<(f32, f32)>>,
    history_file: Option<PathBuf>,
    history: Vec<history::Entry>,
    /// How many prompts back the recall is.
    recall: Option<usize>,
    /// What the field held when the recall began, back after the newest.
    put_aside: Option<Draft>,
    help: bool,
    helps_opened: u64,
    /// The view asks the app for what is the app's.
    app: bool,
    /// The factor the thread view draws at, which the dock draws at too.
    scale: Rc<Cell<f32>>,
    /// The thread view's unzoomed width while it follows a zoom: the dock is
    /// that width times the factor, to the fraction.
    width: Rc<Cell<Option<gpui::Pixels>>>,
    /// The thread view's trace, which notes the frames the dock is built in.
    trace: trace::Trace,
    /// How the view around it looks, and what it tells the dock of its
    /// nested agents. See [`crate::nested::Around`].
    pub(crate) around: Rc<crate::nested::Around>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DockEvent> for Dock {}

impl Focusable for Dock {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Dock {
    /// The prompt area of a conversation whose agent works in `cwd`, or,
    /// without one, of a field standing alone.
    pub(crate) fn new(
        agent: SharedString,
        cwd: Option<PathBuf>,
        scale: Rc<Cell<f32>>,
        width: Rc<Cell<Option<gpui::Pixels>>>,
        trace: trace::Trace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let around = Rc::new(crate::nested::Around::default());
        let composer = cx.new(|cx| {
            let mut composer =
                Composer::new(placeholder(&ThreadState::Starting, &agent, false).0, cx);
            composer.set_project(cwd.clone());
            composer.set_scale(scale.clone(), trace.clone());
            composer.set_around(around.clone());
            composer
        });
        let history_file = cwd.as_deref().map(history::path);
        let cwd = cwd.unwrap_or_default();
        let focus = composer.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(&composer, window, Self::composer_event),
            // The frame shows where the keyboard is.
            cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()),
            cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()),
        ];
        let mut dock = Self {
            composer,
            focus,
            agent,
            cwd,
            sync: Sync::default(),
            draft_empty: true,
            picker: None,
            model_picker: None,
            busy_since: None,
            activity: None,
            epoch: cx.background_executor().now(),
            narrow: Rc::new(Cell::new(false)),
            standalone: false,
            foot: None,
            pickers_opened: 0,
            queue: Queue::default(),
            interrupting: None,
            queue_focus: HashMap::new(),
            editing: None,
            leaving: Vec::new(),
            shown_queue: Vec::new(),
            queue_height: Rc::new(Cell::new(0.)),
            seen_height: Rc::new(Cell::new((0., 1.))),
            reported_height: Rc::new(Cell::new((0., 1.))),
            history_file: None,
            history: Vec::new(),
            recall: None,
            put_aside: None,
            help: false,
            helps_opened: 0,
            app: false,
            scale,
            width,
            trace,
            around,
            _subscriptions: subscriptions,
        };
        dock.set_history_file(history_file, cx);
        dock.push_offer(cx);
        dock
    }

    /// The view around it changed its look: the field says other things,
    /// and the line over it has another light.
    pub(crate) fn look_changed(&mut self, cx: &mut Context<Self>) {
        self.activity = None;
        let (long, short) = self.placeholders();
        self.composer.update(cx, |composer, cx| {
            composer.set_placeholders(long, short, cx);
            cx.notify();
        });
        cx.notify();
    }

    /// What the empty field says now, in the look the view has.
    fn placeholders(&self) -> (SharedString, SharedString) {
        if self.around.studio() {
            studio::placeholder(&self.sync.state, &self.agent)
        } else {
            placeholder(&self.sync.state, &self.agent, self.sync.offers.steer)
        }
    }

    /// Where prompt history is shared; `None` keeps it in memory. It is read
    /// off the main thread.
    pub(crate) fn set_history_file(&mut self, file: Option<PathBuf>, cx: &mut Context<Self>) {
        self.history_file = file.clone();
        let Some(file) = file else {
            return;
        };
        let loaded = cx.background_spawn(async move { history::load(&file) });
        cx.spawn(async move |this, cx| {
            let loaded = loaded.await;
            this.update(cx, |this, cx| {
                // What was sent while the file was read stays.
                let ours = std::mem::take(&mut this.history);
                this.history = history::merge(loaded, &ours);
                this.push_history(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The folder prompts are sent from: what `$` links are found in.
    pub(crate) fn set_cwd(&mut self, cwd: Option<PathBuf>, cx: &mut Context<Self>) {
        self.cwd = cwd.unwrap_or_default();
        cx.notify();
    }

    /// Whether the app answers the commands that are the app's: new session,
    /// sessions, resume, rename, settings.
    pub(crate) fn set_app_commands(&mut self, app: bool, cx: &mut Context<Self>) {
        self.app = app;
        self.push_offer(cx);
    }

    /// Takes in what the thread is now. A turn that ended lets the queue go on.
    /// What the dock was last told of the conversation.
    pub(crate) fn synced(&self) -> &Sync {
        &self.sync
    }

    pub(crate) fn sync(&mut self, sync: Sync, cx: &mut Context<Self>) {
        if sync == self.sync {
            return;
        }
        let was = std::mem::replace(&mut self.sync, sync);
        let busy =
            |state: &ThreadState| matches!(state, ThreadState::Working | ThreadState::Blocked);
        if was.state != self.sync.state || was.offers.steer != self.sync.offers.steer {
            let (long, short) = self.placeholders();
            self.composer.update(cx, |composer, cx| {
                composer.set_placeholders(long, short, cx)
            });
            if matches!(self.sync.state, ThreadState::Closed(_)) {
                self.picker = None;
                self.model_picker = None;
            }
        }
        // The activity line's clock counts from when the turn began.
        if busy(&self.sync.state) && !busy(&was.state) {
            self.busy_since = Some(cx.background_executor().now());
        } else if !busy(&self.sync.state) {
            self.busy_since = None;
            self.activity = None;
        }
        if busy(&was.state) && !busy(&self.sync.state) {
            let stop = self.sync.last_stop.clone();
            self.queue.settle(stop.as_ref());
            // A turn stopped to take a forced prompt: that prompt goes now,
            // if it still heads the queue, and the queue is otherwise as it
            // was before the stop (which, as any stop, has paused it).
            let forced = self.interrupting.take().filter(|(id, _)| {
                self.queue
                    .items()
                    .first()
                    .is_some_and(|item| item.id == *id)
            });
            if self.sync.state == ThreadState::Idle {
                match forced {
                    Some((_, paused)) => {
                        self.queue.paused = false;
                        self.run_next(cx);
                        self.queue.paused = paused;
                    }
                    None => self.run_next(cx),
                }
            }
        }
        let takes = self.sync.ready.then_some(self.sync.capabilities.images);
        let commands = self.sync.commands.clone();
        self.composer.update(cx, |composer, cx| {
            composer.set_takes_pictures(takes);
            composer.set_commands(commands, cx);
        });
        self.push_offer(cx);
        cx.notify();
    }

    /// The agent took the oldest prompt forced into its turn.
    pub(crate) fn delivered(&mut self, cx: &mut Context<Self>) {
        if self.queue.delivered().is_some() {
            cx.notify();
        }
    }

    /// Puts a draft in the field: an earlier prompt to send again.
    pub(crate) fn restore(
        &mut self,
        text: String,
        pictures: Vec<(String, std::sync::Arc<[u8]>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer.update(cx, |composer, cx| {
            composer.clear(cx);
            composer.reuse(&text, pictures, cx);
        });
        window.focus(&self.focus, cx);
    }

    /// Says something short over the field.
    pub(crate) fn tell(
        &mut self,
        text: impl Into<SharedString>,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        self.composer
            .update(cx, |composer, cx| composer.tell(text, error, cx));
    }

    /// Says something short over the field for as long as `shown`.
    pub(crate) fn tell_for(
        &mut self,
        text: impl Into<SharedString>,
        shown: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        self.composer
            .update(cx, |composer, cx| composer.tell_for(text, false, shown, cx));
    }

    fn push_offer(&mut self, cx: &mut Context<Self>) {
        let offer = Offer {
            backend: Vec::new(),
            arguments: self.sync.offers.arguments.clone(),
            models: self.sync.models.clone(),
            model: self.sync.model.clone(),
            modes: self.sync.modes.clone(),
            mode: self.sync.mode.clone(),
            running: self.busy(),
            queued: self.queue.queued(),
            paused: self.queue.paused,
            answered: self.sync.answered,
            entries: self.sync.entries,
            prompts: self.sync.prompts > 0,
            rewind: self.sync.offers.rewind,
            closed: matches!(self.sync.state, ThreadState::Closed(_)),
            app: self.app,
        };
        self.composer
            .update(cx, |composer, cx| composer.set_offer(offer, cx));
    }

    fn push_history(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now();
        let list = self
            .history
            .iter()
            .map(|entry| (entry.prompt.clone(), history::ago(entry.at(), now)))
            .collect();
        self.composer
            .update(cx, |composer, _| composer.set_history(list));
    }

    fn busy(&self) -> bool {
        matches!(self.sync.state, ThreadState::Working | ThreadState::Blocked)
    }

    /// The draft in the field, or why it cannot go, said over the field.
    fn take_draft(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(Draft, Vec<kou_thread::files::FileLink>)> {
        match self.composer.read(cx).draft() {
            Ok(draft) => Some(draft),
            Err(reason) => {
                self.tell(format!("Not yet: {reason}."), true, cx);
                None
            }
        }
    }

    fn remember(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.trim().is_empty() {
            return;
        }
        let entry = history::Entry::now(
            text,
            self.sync.session.as_deref().unwrap_or_default(),
            self.sync.model.as_deref().unwrap_or_default(),
        );
        if !history::append(&mut self.history, entry) {
            return;
        }
        self.push_history(cx);
        if let Some(file) = self.history_file.clone() {
            let ours = self.history.clone();
            cx.background_spawn(async move {
                if let Err(error) = history::save(&file, &ours) {
                    log::debug!("prompt history was not saved: {error}");
                }
            })
            .detach();
        }
    }

    fn clear_field(&mut self, cx: &mut Context<Self>) {
        self.recall = None;
        self.put_aside = None;
        self.composer.update(cx, |composer, cx| composer.clear(cx));
    }

    fn composer_event(
        &mut self,
        _: &Entity<Composer>,
        event: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Submit(text) => self.submit(text.clone(), false, window, cx),
            ComposerEvent::Force(text) => self.submit(text.clone(), true, window, cx),
            ComposerEvent::Emptied(empty) => {
                self.draft_empty = *empty;
                cx.notify();
            }
            // Nothing to send: in a conversation Enter then does nothing;
            // standing alone, the field passes the gesture on.
            ComposerEvent::Blank => {
                if self.standalone && self.draft_empty {
                    cx.emit(DockEvent::Blank);
                }
            }
            ComposerEvent::Recall { older } => self.recall(*older, window, cx),
        }
    }

    /// What Enter (or Cmd+Enter, `force`) does with what is written.
    fn submit(&mut self, text: String, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.finish_edit(true, cx);
            return;
        }
        if !force && let Some((name, argument)) = commands::parse(&text) {
            let catalog = self.composer.read(cx).catalog().clone();
            if let Some(at) = catalog.named(&name)
                && catalog.specs[at].act != Act::Backend
            {
                let act = catalog.specs[at].act;
                self.clear_field(cx);
                self.run(act, argument, window, cx);
                return;
            }
        }
        match self.sync.state {
            ThreadState::Closed(_) => {
                self.tell(
                    "This session has ended: nothing more can be sent to it.",
                    true,
                    cx,
                );
            }
            ThreadState::Starting => {
                self.tell(
                    format!("{} is starting: send once it is ready.", self.agent),
                    false,
                    cx,
                );
            }
            ThreadState::Idle => self.send_now(cx),
            ThreadState::Working | ThreadState::Blocked if force => self.force_now(cx),
            ThreadState::Working | ThreadState::Blocked => {
                let Some((draft, _)) = self.take_draft(cx) else {
                    return;
                };
                self.remember(&draft.text, cx);
                self.queue.add(draft);
                self.clear_field(cx);
                self.push_offer(cx);
                cx.notify();
            }
        }
    }

    fn send_now(&mut self, cx: &mut Context<Self>) {
        let Some((draft, links)) = self.take_draft(cx) else {
            return;
        };
        if draft.is_blank() {
            return;
        }
        let blocks = draft.blocks(&links, &self.cwd, self.sync.capabilities.embedded_files);
        self.remember(&draft.text, cx);
        self.clear_field(cx);
        cx.emit(DockEvent::Send { blocks, draft });
    }

    fn force_now(&mut self, cx: &mut Context<Self>) {
        let Some((draft, links)) = self.take_draft(cx) else {
            return;
        };
        if !self.sync.offers.steer {
            // An agent that takes no messages while it works is stopped to
            // take this one: it heads the queue and goes as soon as the
            // turn has ended.
            self.remember(&draft.text, cx);
            let id = self.queue.add(draft);
            self.queue.move_to(id, 0);
            self.clear_field(cx);
            self.interrupt(id, cx);
            return;
        }
        let blocks = draft.blocks(&links, &self.cwd, self.sync.capabilities.embedded_files);
        self.remember(&draft.text, cx);
        self.queue.add_forced(draft);
        self.clear_field(cx);
        cx.emit(DockEvent::Steer { blocks });
        cx.notify();
    }

    /// Stops the running turn so that queued prompt `id`, at the head of the
    /// queue, is sent at once: what forcing a prompt in means for an agent
    /// that takes none while it works. Should the agent not stop, the prompt
    /// goes when the turn ends by itself.
    fn interrupt(&mut self, id: u64, cx: &mut Context<Self>) {
        let paused = self
            .interrupting
            .map_or(self.queue.paused, |(_, paused)| paused);
        self.interrupting = Some((id, paused));
        cx.emit(DockEvent::Cancel);
        self.push_offer(cx);
        cx.notify();
    }

    /// Sends the next queued prompt, if the queue runs.
    fn run_next(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.queue.next() else {
            return;
        };
        let links = if item.draft.text.contains('$') {
            kou_thread::files::file_links(&item.draft.text, &self.cwd)
        } else {
            Vec::new()
        };
        let blocks = item
            .draft
            .blocks(&links, &self.cwd, self.sync.capabilities.embedded_files);
        self.push_offer(cx);
        cx.emit(DockEvent::Send {
            blocks,
            draft: item.draft,
        });
    }

    /// Carries out a command that is not the backend's.
    fn run(&mut self, act: Act, argument: String, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Act::Backend => {}
            Act::Stop => cx.emit(DockEvent::Cancel),
            Act::Queue => self.queue_command(&argument, window, cx),
            Act::Model | Act::Mode => {
                let (kind, choices) = if act == Act::Model {
                    (PickerKind::Model, &self.sync.models)
                } else {
                    (PickerKind::Mode, &self.sync.modes)
                };
                if argument.is_empty() {
                    self.toggle_picker(kind, window, cx);
                    return;
                }
                let wanted = argument.to_lowercase();
                let found = choices.iter().find(|choice| {
                    choice.id.to_lowercase() == wanted
                        || choice.name.to_lowercase() == wanted
                        || choice.name.to_lowercase().ends_with(&format!(": {wanted}"))
                });
                match (found, kind) {
                    (Some(choice), PickerKind::Mode) => {
                        cx.emit(DockEvent::SetMode(choice.id.clone()))
                    }
                    (Some(choice), PickerKind::Model) => {
                        cx.emit(DockEvent::SetModel(choice.id.clone()))
                    }
                    // A model the list does not name may still be one the connection reaches.
                    (None, PickerKind::Model) => cx.emit(DockEvent::SetModel(argument)),
                    (None, PickerKind::Mode) => {
                        let names: Vec<&str> =
                            choices.iter().map(|choice| choice.id.as_str()).collect();
                        self.tell(format!("Modes: {}.", names.join(", ")), true, cx);
                    }
                }
            }
            Act::Copy | Act::Expand | Act::Collapse | Act::Clear => cx.emit(DockEvent::View(act)),
            Act::Help => {
                self.help = !self.help;
                self.helps_opened += 1;
                cx.notify();
            }
            Act::History => {
                window.dispatch_action(Box::new(composer::SearchHistory), cx);
            }
            Act::Edit => {
                let number = argument.trim().parse::<usize>().ok();
                if !argument.trim().is_empty() && number.is_none_or(|n| n == 0) {
                    self.tell(
                        "/edit takes the number of a prompt, 1 for the first.",
                        true,
                        cx,
                    );
                    return;
                }
                cx.emit(DockEvent::Edit(number));
            }
            Act::New => cx.emit(DockEvent::Ask(ThreadViewRequest::NewSession)),
            Act::Sessions => cx.emit(DockEvent::Ask(ThreadViewRequest::OpenSessions)),
            Act::Settings => cx.emit(DockEvent::Ask(ThreadViewRequest::OpenSettings)),
            Act::Resume => {
                if argument.is_empty() {
                    cx.emit(DockEvent::Ask(ThreadViewRequest::OpenSessions));
                } else {
                    cx.emit(DockEvent::Ask(ThreadViewRequest::ResumeSession(argument)));
                }
            }
            Act::Rename => {
                if argument.is_empty() {
                    self.tell("/rename takes the title: /rename Retry queue.", true, cx);
                } else {
                    cx.emit(DockEvent::Ask(ThreadViewRequest::Rename(argument)));
                }
            }
        }
    }

    fn queue_command(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        match argument.trim() {
            "" => {
                if self.queue.is_empty() {
                    self.tell(
                        "The queue is empty: while the agent works, Enter queues a message and Cmd+Enter forces one in.",
                        false,
                        cx,
                    );
                } else {
                    self.focus_queued(self.queue.items().len() - 1, window, cx);
                }
            }
            "resume" => self.resume(cx),
            "pause" => {
                self.queue.paused = true;
                self.push_offer(cx);
                cx.notify();
            }
            "clear" => {
                self.queue.clear();
                self.push_offer(cx);
                cx.notify();
            }
            _ => self.tell("/queue takes resume, pause or clear.", true, cx),
        }
    }

    fn resume(&mut self, cx: &mut Context<Self>) {
        self.queue.paused = false;
        if self.sync.state == ThreadState::Idle {
            self.run_next(cx);
        }
        self.push_offer(cx);
        cx.notify();
    }

    /// Up and Down through earlier prompts, as a shell does: what was in the
    /// field comes back after the newest.
    fn recall(&mut self, older: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Over an empty field, Up goes to what waits for the agent first.
        if older && self.recall.is_none() && !self.queue.is_empty() && self.draft_empty {
            self.focus_queued(self.queue.items().len() - 1, window, cx);
            return;
        }
        let prompts: Vec<String> = self
            .history
            .iter()
            .map(|entry| entry.prompt.clone())
            .collect();
        if prompts.is_empty() {
            return;
        }
        let field_empty = self.composer.read(cx).text().is_empty();
        // An empty field is a fresh start, wherever an earlier walk stopped.
        let at = self.recall.filter(|_| !field_empty);
        let next = match (at, older) {
            (None, true) => Some(0),
            (Some(back), true) => Some((back + 1).min(prompts.len() - 1)),
            (Some(0) | None, false) => None,
            (Some(back), false) => Some(back - 1),
        };
        self.recall = next;
        let text = next.map(|back| prompts[prompts.len() - 1 - back].clone());
        self.composer
            .update(cx, |composer, cx| composer.recall(text, cx));
    }

    // ------------------------------------------------------------ the queue

    fn focus_queued(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.queue.items().get(index) else {
            return;
        };
        let handle = self
            .queue_focus
            .entry(item.id)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn drop_queued(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.queue.items().iter().position(|item| item.id == id);
        if self.queue.take(id).is_none() {
            self.tell(
                "The agent has this message already: it cannot be taken back.",
                false,
                cx,
            );
            return;
        }
        // The keyboard goes to a neighbour, or back to the field.
        match index {
            Some(index) if !self.queue.is_empty() => {
                self.focus_queued(index.min(self.queue.items().len() - 1), window, cx)
            }
            _ => window.focus(&self.focus, cx),
        }
        self.push_offer(cx);
        cx.notify();
    }

    fn move_queued(&mut self, id: u64, by: isize, cx: &mut Context<Self>) {
        let Some(position) = self.queue.position(id) else {
            return;
        };
        let to = (position as isize + by).max(0) as usize;
        if self.queue.move_to(id, to) {
            cx.notify();
        }
    }

    fn force_queued(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.busy() {
            // Nothing runs: forcing it in is running it now.
            if let Some((_, item)) = self.queue.take(id) {
                self.queue.insert(0, item);
                let paused = std::mem::replace(&mut self.queue.paused, false);
                self.run_next(cx);
                self.queue.paused = paused;
            }
            cx.notify();
            return;
        }
        if !self.sync.offers.steer {
            // As for a prompt forced from the field: the turn is stopped
            // and this one goes first.
            self.queue.move_to(id, 0);
            if self.queue.position(id) == Some(0) {
                self.interrupt(id, cx);
            }
            return;
        }
        if let Some(draft) = self.queue.force(id) {
            let links = kou_thread::files::file_links(&draft.text, &self.cwd);
            let blocks = draft.blocks(&links, &self.cwd, self.sync.capabilities.embedded_files);
            cx.emit(DockEvent::Steer { blocks });
        }
        cx.notify();
    }

    /// Forces in the prompt that heads the queue, while the agent works:
    /// what the force key does over an empty field. Whether there was one.
    /// (With the keys on a row of the queue, the key is that row's.)
    pub(crate) fn force_head(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.busy() || self.interrupting.is_some() || !self.focus.is_focused(window) {
            return false;
        }
        let Some(id) = self
            .queue
            .items()
            .iter()
            .find(|item| !item.forced)
            .map(|item| item.id)
        else {
            return false;
        };
        self.force_queued(id, cx);
        true
    }

    /// Takes a queued prompt into the field to edit it.
    fn edit_queued(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.composer.read(cx).text().trim().is_empty() {
            self.tell(
                "Send or clear what is in the field first, then edit the queued message.",
                false,
                cx,
            );
            return;
        }
        if self.editing.is_some() {
            self.finish_edit(false, cx);
        }
        let Some((position, item)) = self.queue.take(id) else {
            return;
        };
        let banner = format!(
            "Editing queued message {} · ↵ puts it back · esc keeps it as it was",
            position + 1
        );
        let draft = item.draft.clone();
        self.editing = Some((position, item));
        self.composer.update(cx, |composer, cx| {
            composer.restore(draft, cx);
            composer.set_banner(Some(banner.into()), cx);
        });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Puts the prompt being edited back in its place: as edited, or as it was.
    fn finish_edit(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some((position, mut item)) = self.editing.take() else {
            return;
        };
        if keep {
            match self.composer.read(cx).draft() {
                Ok((draft, _)) if !draft.is_blank() => item.draft = draft,
                Ok(_) => {
                    // Emptied: the prompt is dropped.
                    self.composer.update(cx, |composer, cx| {
                        composer.set_banner(None, cx);
                        composer.clear(cx);
                    });
                    cx.notify();
                    return;
                }
                Err(reason) => {
                    self.editing = Some((position, item));
                    self.tell(format!("Not yet: {reason}."), true, cx);
                    return;
                }
            }
        }
        self.queue.insert(position, item);
        self.composer.update(cx, |composer, cx| {
            composer.set_banner(None, cx);
            composer.clear(cx);
        });
        cx.notify();
    }

    // ------------------------------------------------------------ pickers

    fn choices(&self, kind: PickerKind) -> (&[Choice], Option<&str>) {
        match kind {
            PickerKind::Mode => (&self.sync.modes, self.sync.mode.as_deref()),
            PickerKind::Model => (&self.sync.models, self.sync.model.as_deref()),
        }
    }

    fn toggle_picker(&mut self, kind: PickerKind, window: &mut Window, cx: &mut Context<Self>) {
        if kind == PickerKind::Model {
            self.toggle_model_picker(window, cx);
            return;
        }
        self.model_picker = None;
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.kind == kind)
        {
            self.close_picker(window, cx);
            return;
        }
        let (choices, current) = self.choices(kind);
        let at = choices
            .iter()
            .position(|choice| Some(choice.id.as_str()) == current)
            .unwrap_or(0);
        let items: Vec<FocusHandle> = choices
            .iter()
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect();
        if let Some(item) = items.get(at) {
            window.focus(item, cx);
        }
        self.pickers_opened += 1;
        self.picker = Some(Picker {
            kind,
            items,
            opened: self.pickers_opened,
        });
        cx.notify();
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn picker_step(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.picker else {
            return;
        };
        let count = picker.items.len();
        if count == 0 {
            return;
        }
        let at = picker.items.iter().position(|item| item.is_focused(window));
        let next = match (at, down) {
            (Some(at), true) => (at + 1) % count,
            (Some(at), false) => (at + count - 1) % count,
            (None, true) => 0,
            (None, false) => count - 1,
        };
        window.focus(&picker.items[next], cx);
        cx.notify();
    }

    fn choose(
        &mut self,
        kind: PickerKind,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(match kind {
            PickerKind::Mode => DockEvent::SetMode(id),
            PickerKind::Model => DockEvent::SetModel(id),
        });
        self.close_picker(window, cx);
    }

    /// Escape: the innermost thing the dock has open closes.
    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_picker.is_some() {
            self.picker_key("escape", window, cx);
        } else if self.picker.is_some() {
            self.close_picker(window, cx);
        } else if self.help {
            self.help = false;
            cx.notify();
        } else if self.editing.is_some() {
            self.finish_edit(false, cx);
        } else if !self.focus.is_focused(window)
            && self
                .queue_focus
                .values()
                .any(|handle| handle.is_focused(window))
        {
            window.focus(&self.focus, cx);
        } else {
            cx.propagate();
        }
    }

    /// Opens the system's file dialog to attach pictures or link files.
    fn attach_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        let focus = self.focus.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                window.focus(&focus, cx);
                this.composer
                    .update(cx, |composer, cx| composer.attach_paths(paths, cx));
            })
            .ok();
        })
        .detach();
    }
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let factor = self.scale.get();
        trace::dock_built(&self.trace, factor);
        let dock = scale::with_factor(factor, || self.dock(window, cx));
        // The room it is given is rounded to whole pixels, at a zoom to a
        // different fraction every frame; laid out in its exact width, what
        // is typed in the field keeps its lines while the zoom moves.
        match self.width.get() {
            Some(width) => scaled(factor, div().h_full().w(width * factor).child(dock)),
            None => scaled(factor, dock),
        }
    }
}

impl Dock {
    fn dock(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.around.studio() {
            return self.studio_dock(window, cx);
        }
        let queue = self.queue_element(window, cx);
        let measure = self.measure(cx);
        let activity = self.activity_line(cx);
        let model_picker = self.model_picker_element(Anchor::BottomLeft, window, cx);
        // What the app puts at the start of the foot, built as the field is.
        let foot = self.foot.clone().map(|foot| foot(window, cx));
        let theme = theme(cx);
        let frame = self.frame(window, foot, theme, cx);
        let usage = self.usage(theme);
        let help = self.help.then(|| self.help_element(window, theme, cx));
        let standalone = self.standalone;
        // The prompt area keeps to the transcript's column, centred as it
        // is, and within it to the text: the field begins where the text
        // does, past the rail, and ends where it ends. (Standing alone there
        // is no rail to stand clear of.)
        let lead = if standalone { EDGE } else { TEXT_EDGE };
        let column = div()
            .relative()
            .w_full()
            .max_w(px(MEASURE))
            .flex_none()
            .pl(px(lead))
            .pr(px(EDGE))
            .pb(px(if standalone { 0. } else { 10. }))
            .flex()
            .flex_col()
            .child(measure)
            .children(activity)
            .child(queue)
            .child(trace::traced(&self.trace, trace::Part::FieldFrame, frame))
            .children(usage)
            .children(help)
            .children(
                model_picker.map(|picker| div().absolute().top_0().left(px(lead)).child(picker)),
            );
        // Where the card beside the transcript takes room from the pane,
        // the transcript's column is centred in what is left: the prompt
        // area stands under that column, wherever it is, not in the middle
        // of the pane.
        let inset = if standalone {
            0.
        } else {
            self.around.inset.get()
        };
        let band = div()
            .w_full()
            .flex_none()
            .pr(px(inset))
            .flex()
            .justify_center()
            .when(!standalone, |band| band.bg(theme.pane))
            .child(column);
        // The dock lies over the bottom of the transcript, whose last row
        // keeps as much room as the dock takes; above its content it is
        // empty and lets the pointer through to the transcript. Alone, it is
        // as tall as what it holds.
        div()
            .id("dock")
            .when(!standalone, |dock| dock.size_full().justify_end())
            .when(standalone, |dock| dock.w_full())
            .flex()
            .flex_col()
            // Drawn alone, the field takes the theme's type itself; what the
            // app puts in its foot takes it from here.
            .text_color(theme.fg)
            .font_family(theme.ui_font.clone())
            .text_size(px(metric::TEXT))
            .on_action(cx.listener(Self::cancel))
            .on_action(
                cx.listener(|this, _: &MenuUp, window, cx| this.picker_step(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MenuDown, window, cx| this.picker_step(true, window, cx)),
            )
            .child(band)
            .into_any_element()
    }

    /// The line over the field while the agent works (`.activity`): a meter
    /// with a light passing along it and how long the turn has run, what it
    /// does, and the key that stops it. It also stands while nothing of the
    /// agent's own runs but something it left running does: the commands
    /// in the background, which a stop of the turn does not end.
    fn activity_line(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let waiting = self.around.waiting.borrow().clone();
        let behind = self.around.behind.borrow().clone();
        let busy = self.busy();
        if (!busy && waiting.is_empty() && behind.is_empty()) || self.standalone {
            return None;
        }
        // The meter and its time are the turn's (or of what it waits on).
        let turn = busy || !waiting.is_empty();
        let since = self.busy_since;
        let (epoch, scale) = (self.epoch, self.scale.clone());
        let meter = turn.then(|| {
            self.activity
                .get_or_insert_with(|| {
                    cx.new(|_| Lights::new(Light::Meter { since }, epoch, scale))
                })
                .clone()
        });
        let theme = theme(cx);
        let blocked = self.sync.state == ThreadState::Blocked;
        // Over the queue's card it keeps to that card's edges, which stand
        // in from the field's; over the field alone, to the field's.
        let queued = !self.queue.items().is_empty() || !self.leaving.is_empty();
        let doing = if blocked {
            Some("Waiting for your answer".to_owned())
        } else if !waiting.is_empty() {
            Some(crate::nested::waiting_on(waiting.len()))
        } else if busy {
            Some("Working".to_owned())
        } else {
            None
        };
        let in_background = match behind.len() {
            0 => None,
            1 => Some("1 command in background".to_owned()),
            count => Some(format!("{count} commands in background")),
        };
        // Which of them, where the agents it waits on are not named already.
        let commands =
            (!behind.is_empty() && (waiting.is_empty() || blocked)).then(|| behind.join(", "));
        Some(
            div()
                .debug_selector(|| "activity".into())
                .h(px(32.))
                .px(px(if queued { QUEUE_INSET } else { 2. }))
                .flex()
                .items_center()
                .gap(px(10.))
                .font_family(theme.mono_font.clone())
                .text_size(px(11.5))
                .text_color(if blocked { theme.blocked } else { theme.fg_dim })
                .children(meter.map(|meter| {
                    div()
                        .debug_selector(|| "activity-meter".into())
                        .flex_none()
                        .child(meter)
                }))
                .children(doing.map(|doing| div().flex_none().child(scale::text(doing))))
                // The agents it waits on, by their titles.
                .when(!waiting.is_empty() && !blocked, |line| {
                    line.child(
                        div()
                            .debug_selector(|| "waiting-titles".into())
                            .min_w_0()
                            .flex_shrink(1.)
                            .truncate()
                            .text_color(theme.fg_muted)
                            .child(scale::text(waiting.join(", "))),
                    )
                })
                // What it left running in the background.
                .children(in_background.map(|said| {
                    div()
                        .debug_selector(|| "activity-background".into())
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_color(theme.fg_dim)
                        .child(icon(Icon::Terminal, theme.fg_muted).size(px(12.)))
                        .child(scale::text(said))
                }))
                .children(commands.map(|commands| {
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .truncate()
                        .text_color(theme.fg_muted)
                        .child(scale::text(commands))
                }))
                .child(div().flex_1())
                .when(busy && (waiting.is_empty() || blocked), |line| {
                    line.child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_size(px(10.5))
                            .text_color(theme.fg_muted)
                            .child(key_hint("esc", theme))
                            .child(scale::text("stop")),
                    )
                })
                .when(!waiting.is_empty() && !blocked && busy, |line| {
                    line.child(
                        crate::prose::act("waiting-stop", "Stop", theme)
                            .debug_selector(|| "waiting-stop".into())
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.emit(DockEvent::Cancel);
                                window.focus(&this.focus, cx);
                            })),
                    )
                }),
        )
    }
}

/// How far, in pixels as drawn, rounding and unscaled borders can move the
/// dock's height between two zooms. See [`Dock::measure`].
const ZOOM_NOISE: f32 = 6.;

impl Dock {
    /// Measures how tall the dock is as drawn and tells the view around it,
    /// once the height has settled: while something grows it lies over the
    /// transcript, and the transcript makes room once, not at every frame.
    ///
    /// The height is unscaled, so that a zoom is not a change of height. It is
    /// not exactly the same at every zoom all the same: drawn at a zoom, the
    /// dock's parts are rounded to whole device pixels and its borders do not
    /// scale. Told to the view, every such difference would move the whole
    /// transcript by a pixel or two in the middle of a zoom gesture, so a
    /// height measured at another factor than the last one told is told only
    /// when it differs by more than that: [`ZOOM_NOISE`] pixels as drawn.
    fn measure(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let (dock, still) = (cx.weak_entity(), cx.reduce_motion());
        let (seen, reported) = (self.seen_height.clone(), self.reported_height.clone());
        let narrow = self.narrow.clone();
        let factor = scale::factor();
        gpui::canvas(
            move |bounds, window, cx| {
                // `@container composer-row (max-width: 520px)`: the labels go.
                let is_narrow = bounds.size.width.as_f32() / factor < 520.;
                if narrow.replace(is_narrow) != is_narrow {
                    let dock = dock.clone();
                    cx.defer(move |cx| {
                        dock.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
                let height = bounds.size.height.as_f32() / factor;
                let tolerance = |(_, at): (f32, f32)| {
                    if at == factor {
                        0.5
                    } else {
                        0.5 + ZOOM_NOISE / factor
                    }
                };
                let differs = |(was, at): (f32, f32)| (height - was).abs() > tolerance((was, at));
                let changed = differs(seen.get());
                seen.set((height, factor));
                if changed && !still {
                    // Look again next frame: reported once it holds still.
                    window.request_animation_frame();
                    return;
                }
                if differs(reported.get()) {
                    reported.set((height, factor));
                    let dock = dock.clone();
                    cx.defer(move |cx| {
                        dock.update(cx, |_, cx| cx.emit(DockEvent::Height(height)))
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
}

impl Dock {
    /// The field in its frame (`.composer`), with its foot: what the app puts
    /// there, attach, the model and the mode, and the buttons that send.
    fn frame(
        &self,
        window: &Window,
        foot: Option<AnyElement>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let focused = self.focus.is_focused(window);
        let busy = self.busy();
        let (resting, lit, accent) = (theme.line_strong, theme.fg_faint, theme.accent);
        let drop_fill = theme.selected;
        let pickers: Vec<AnyElement> = [PickerKind::Model, PickerKind::Mode]
            .into_iter()
            .filter_map(|kind| self.picker_button(kind, theme, cx))
            .collect();
        let closed = matches!(self.sync.state, ThreadState::Closed(_));
        let attach = (!closed).then(|| {
            kou_ui::icon_button("attach", Icon::Paperclip, theme)
                .debug_selector(|| "attach".into())
                .size(px(28.))
                .on_click(cx.listener(|this, _, window, cx| this.attach_files(window, cx)))
        });
        let frame = div()
            .id("frame")
            .debug_selector(|| "frame".into())
            .relative()
            .rounded(px(metric::RADIUS_LARGE))
            .border_1()
            .bg(theme.raised)
            .pl(px(14.))
            .pr(px(10.))
            .pt(px(12.))
            .pb(px(10.))
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(accent).bg(drop_fill)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.composer
                    .update(cx, |composer, cx| composer.drop_paths(paths, window, cx));
            }))
            .child(self.composer.clone())
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .children(foot.map(|foot| {
                                div()
                                    .debug_selector(|| "field-foot".into())
                                    .flex_none()
                                    .min_w_0()
                                    .child(foot)
                            }))
                            .children(attach)
                            .children(pickers),
                    )
                    .child(self.controls(theme, cx)),
            );
        if busy {
            // `.composer[data-busy]`: the accent in its border while the agent works.
            return frame
                .border_color(crate::lights::mix(resting, accent, 0.35))
                .into_any_element();
        }
        frame
            .with_spring(
                "frame-focus",
                SpringAnimation::new(SETTLE).to(focused),
                move |this, phase| {
                    let (from, to) = (gpui::Rgba::from(resting), gpui::Rgba::from(lit));
                    this.border_color(phase.interpolate_clamped(from, to))
                },
            )
            .into_any_element()
    }

    /// The control of a picker (`.model-pick`, `.think`): framed, with its
    /// name in capitals while there is room, then what is chosen.
    fn picker_button(
        &self,
        kind: PickerKind,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let (choices, current) = self.choices(kind);
        // Nothing to choose, or nobody left to tell.
        if choices.is_empty() || matches!(self.sync.state, ThreadState::Closed(_)) {
            return None;
        }
        let chosen = current.and_then(|id| choices.iter().find(|choice| choice.id == id));
        let name: SharedString = match (chosen, current) {
            (Some(choice), _) => choice.name.clone(),
            (None, Some(id)) => id.to_owned(),
            (None, None) => kind.title().to_lowercase(),
        }
        .into();
        let open = match kind {
            PickerKind::Model => self.model_picker.is_some(),
            PickerKind::Mode => self
                .picker
                .as_ref()
                .is_some_and(|picker| picker.kind == kind),
        };
        let menu = self
            .picker
            .as_ref()
            .filter(|picker| picker.kind == kind)
            .map(|picker| self.picker_menu(kind, picker, choices, current, theme, cx));
        // kou's own agent offers its efforts as modes: drawn as the web's
        // effort, its bars filled to the level.
        let effort = kind == PickerKind::Mode
            && choices
                .iter()
                .all(|choice| choice.name.starts_with("Effort: "));
        let wide = !self.narrow.get();
        let (hover, frame, ring) = (theme.hover, theme.line_strong, theme.accent);
        let mut control = div()
            .id(kind.id())
            .debug_selector(|| kind.id().to_owned())
            .h(px(28.))
            .min_w_0()
            .max_w(px(300.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(metric::RADIUS))
            .border_1()
            .border_color(if open { theme.line_strong } else { theme.line })
            .bg(if open { theme.hover } else { theme.pane })
            .cursor_pointer()
            .tab_index(0)
            .hover(move |style| style.bg(hover).border_color(frame))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener(move |this, _, window, cx| this.toggle_picker(kind, window, cx)));
        if effort {
            let level = choices
                .iter()
                .position(|choice| Some(choice.id.as_str()) == current)
                .map_or(0, |at| at + 1);
            let bars = choices.len().min(5);
            control = control
                .children(wide.then(|| crate::picker::label("Effort", theme.fg_muted, theme)))
                .child(
                    div()
                        .flex_none()
                        .h(px(17.))
                        .flex()
                        .items_end()
                        .gap(px(3.))
                        .children((0..bars).map(|bar| {
                            div()
                                .w(px(5.))
                                .h(px(5. + 3. * bar as f32))
                                .rounded(px(metric::RADIUS_MARK))
                                .bg(if bar < level {
                                    theme.accent
                                } else {
                                    theme.line_strong
                                })
                        })),
                )
                .child(crate::picker::label(
                    name.trim_start_matches("Effort: "),
                    theme.fg_dim,
                    theme,
                ));
        } else {
            let provider = (kind == PickerKind::Model)
                .then(|| {
                    current.and_then(|id| crate::models::provider_of(id, &self.sync.providers))
                })
                .flatten();
            control = control
                .children(wide.then(|| crate::picker::label(kind.title(), theme.fg_muted, theme)))
                .children((kind == PickerKind::Model).then(|| {
                    div()
                        .flex_none()
                        .size(px(7.))
                        .rounded_full()
                        .bg(theme.provider(provider.as_deref().unwrap_or_default()))
                }))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme.mono_font.clone())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(12.))
                        .text_color(theme.fg)
                        .child(scale::text(name)),
                )
                .child(icon(Icon::ChevronDown, theme.fg_muted).size(px(11.)));
        }
        Some(
            div()
                .relative()
                .min_w_0()
                .children(menu)
                .child(control)
                .into_any_element(),
        )
    }

    /// The modes' menu (`.menu`): over the control, a rounded block under
    /// the pointer or the keyboard.
    fn picker_menu(
        &self,
        kind: PickerKind,
        picker: &Picker,
        choices: &[Choice],
        current: Option<&str>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let (hover, ring) = (theme.hover, theme.accent);
        let still = cx.reduce_motion();
        let rows =
            choices
                .iter()
                .zip(&picker.items)
                .enumerate()
                .map(|(index, (choice, handle))| {
                    let id = choice.id.clone();
                    let is_current = Some(choice.id.as_str()) == current;
                    div()
                        .id(("choice", index))
                        .debug_selector(|| format!("choice-{index}"))
                        .track_focus(handle)
                        .mx(px(4.))
                        .pl(px(10.))
                        .pr(px(8.))
                        .py(px(6.))
                        .rounded(px(metric::RADIUS))
                        .border_1()
                        .border_color(gpui::transparent_black())
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .focus(move |style| style.bg(hover))
                        .focus_visible(move |style| style.border_color(ring))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose(kind, id.clone(), window, cx)
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(if is_current {
                                            theme.accent
                                        } else {
                                            theme.fg
                                        })
                                        .child(scale::text(choice.name.clone())),
                                )
                                .child(div().flex_1())
                                .children(
                                    is_current.then(|| {
                                        crate::picker::label("current", theme.accent, theme)
                                    }),
                                ),
                        )
                        .children(choice.description.clone().map(|description| {
                            div()
                                .font_family(theme.mono_font.clone())
                                .text_size(px(10.5))
                                .line_height(px(15.))
                                .text_color(theme.fg_muted)
                                .child(scale::text(description))
                        }))
                });
        // Laid out after everything else, outside the scaled subtree: sizes are
        // taken now. It rises into place (`.menu`: `drop 0.1s ease-out`).
        let unit = px(1.);
        let duration = if still {
            std::time::Duration::from_millis(1)
        } else {
            motion::cockpit::MENU
        };
        deferred(
            anchored()
                .anchor(Anchor::BottomLeft)
                .offset(point(px(0.), px(-6.)))
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .id("choices")
                        .key_context(MENU_CONTEXT)
                        .occlude()
                        .on_mouse_down_out(
                            cx.listener(|this, _, window, cx| this.close_picker(window, cx)),
                        )
                        .w(px(264.))
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .py(px(4.))
                        .rounded(px(metric::RADIUS_LARGE))
                        .border_1()
                        .border_color(theme.line_strong)
                        .bg(theme.raised)
                        .shadow(crate::picker::shadow(theme))
                        .text_size(px(13.))
                        .text_color(theme.fg)
                        .child(
                            div()
                                .px(px(14.))
                                .pt(px(6.))
                                .pb(px(6.))
                                .child(crate::picker::label(kind.title(), theme.fg_muted, theme)),
                        )
                        .children(rows)
                        .with_animation(
                            ("picker-in", picker.opened),
                            gpui::Animation::new(duration).with_easing(motion::cockpit::ease_out),
                            move |this, t| this.opacity(t).mt(unit * (6. * (1. - t))),
                        ),
                ),
        )
        .with_priority(2)
    }

    /// The buttons at the end of the field (`.composer-send`): run; while a
    /// turn runs, stop, or force and queue once something is written.
    fn controls(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let busy = self.busy();
        let closed = matches!(self.sync.state, ThreadState::Closed(_));
        let typed = !self.draft_empty && self.editing.is_none();
        // Always two places, the first empty but while force is offered: the
        // row keeps its shape, and a change of button lays out only itself.
        let row = div().flex_none().flex().items_center().gap(px(8.));
        let empty = || div().flex_none().size_0().into_any_element();
        let press = |action: fn(&mut Self, &mut Window, &mut Context<Self>)| -> Press {
            Box::new(
                cx.listener(move |this: &mut Self, _: &ClickEvent, window, cx| {
                    action(this, window, cx);
                    window.focus(&this.focus, cx);
                }),
            )
        };
        if busy && !typed && !self.standalone {
            let stop = main_button(
                "stop",
                Icon::Stop,
                SendMode::Stop,
                theme,
                Some(Box::new(cx.listener(|this, _, window, cx| {
                    cx.emit(DockEvent::Cancel);
                    // The button is about to turn back into run: the field keeps the keyboard.
                    window.focus(&this.focus, cx);
                }))),
            );
            return row.child(empty()).child(stop);
        }
        if busy && !self.standalone {
            // Force is there for every agent: one that takes messages while
            // it works is handed this one, any other is stopped to take it.
            let force = div()
                .flex_none()
                .debug_selector(|| "force".into())
                .child(main_button(
                    "force",
                    Icon::Bolt,
                    SendMode::Force,
                    theme,
                    Some(press(|this, window, cx| {
                        let text = this.composer.read(cx).text().to_owned();
                        this.submit(text, true, window, cx);
                    })),
                ));
            let queue = main_button(
                "queue",
                Icon::Queue,
                SendMode::Run,
                theme,
                Some(press(|this, window, cx| {
                    let text = this.composer.read(cx).text().to_owned();
                    this.submit(text, false, window, cx);
                })),
            );
            return row.child(force).child(queue);
        }
        let ready = (self.sync.state == ThreadState::Idle || self.standalone) && typed && !closed;
        let press = ready.then(|| {
            press(|this, window, cx| {
                let text = this.composer.read(cx).text().to_owned();
                this.submit(text, false, window, cx);
            })
        });
        row.child(empty()).child(main_button(
            "send",
            Icon::Play,
            if ready { SendMode::Run } else { SendMode::Idle },
            theme,
            press,
        ))
    }

    /// The figures under the field (`.stats-dock[data-style="line"]`): what
    /// the backend reported about context, tokens and cost, a dot between.
    fn usage(&self, theme: &Theme) -> Option<Div> {
        let figures = usage_figures(&self.sync.usage);
        if figures.is_empty() || self.standalone {
            return None;
        }
        Some(
            div()
                .debug_selector(|| "usage".into())
                .min_h(px(22.))
                .pl(px(4.))
                .pr(px(4.))
                .flex()
                .flex_wrap()
                .items_center()
                .font_family(theme.mono_font.clone())
                .text_size(px(10.5))
                .line_height(px(22.))
                .children(figures.into_iter().enumerate().map(|(at, figure)| {
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(9.))
                        .when(at > 0, |stat| {
                            stat.child(
                                div()
                                    .absolute()
                                    .left(px(-1.5))
                                    .size(px(3.))
                                    .rounded_full()
                                    .bg(theme.line_strong),
                            )
                        })
                        .relative()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(9.5))
                                .text_color(theme.fg_muted)
                                .child(scale::text(figure.key)),
                        )
                        .children(figure.share.map(|share| {
                            // `.stat-meter`, round: a bar of four cells' length.
                            div()
                                .flex_none()
                                .w(px(22.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(theme.line_strong)
                                .child(
                                    div()
                                        .h_full()
                                        .w(px(22. * share.clamp(0., 1.)))
                                        .rounded_full()
                                        .bg(if share >= 0.9 {
                                            theme.error
                                        } else if share >= 0.75 {
                                            theme.blocked
                                        } else {
                                            theme.accent
                                        }),
                                )
                        }))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.fg_muted)
                                .child(scale::text(figure.value)),
                        )
                })),
        )
    }

    /// The queue over the field: forced prompts first, then the queued ones in
    /// the order they run, each row sliding to its place.
    fn queue_element(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let now = cx.background_executor().now();
        // What left since the last look leaves where it was.
        let current: Vec<(Item, usize)> = self
            .queue
            .items()
            .iter()
            .cloned()
            .enumerate()
            .map(|(at, item)| (item, at))
            .collect();
        for (old, at) in &self.shown_queue {
            if !current.iter().any(|(item, _)| item.id == old.id) {
                self.leaving.push((old.clone(), *at, now));
            }
        }
        self.leaving.retain(|(item, _, since)| {
            now.saturating_duration_since(*since) < composer::LEAVING
                && !current.iter().any(|(new, _)| new.id == item.id)
        });
        if !self.leaving.is_empty() {
            let timer = cx.background_executor().timer(composer::LEAVING);
            cx.spawn(async move |this, cx| {
                timer.await;
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        }
        self.shown_queue = current.clone();
        let ids: Vec<u64> = current.iter().map(|(item, _)| item.id).collect();
        self.queue_focus.retain(|id, _| ids.contains(id));
        for id in &ids {
            if !self.queue_focus.contains_key(id) {
                let handle = cx.focus_handle().tab_stop(true);
                self.queue_focus.insert(*id, handle);
            }
        }
        let theme = crate::studio::chrome_look_theme(self.around.look.get(), cx);

        let content = (!current.is_empty() || !self.leaving.is_empty()).then(|| {
            let forced = self.queue.forced();
            let queued = self.queue.queued();
            let waiting = self.queue.paused || !self.busy();
            let state: SharedString = if self.interrupting.is_some() && self.busy() {
                "Stopping the turn · the first message goes as soon as it has stopped".into()
            } else if self.queue.paused {
                "Paused · the queued messages wait for you".into()
            } else if !self.busy() {
                "Waiting · the next runs when you resume".into()
            } else if forced > 0 && queued > 0 {
                format!(
                    "{forced} forced in after the running tools · {queued} when the agent finishes"
                )
                .into()
            } else if forced > 0 {
                "Forced in: the agent reads it after its running tools".into()
            } else {
                "Runs when the agent finishes, one after another".into()
            };
            // `.act`: the queue's tools, words in capitals of the monospace.
            let tool = |id: &'static str, glyph: Icon, label: &'static str| {
                let (ring, frame, lit) = (theme.accent, theme.line_strong, theme.fg);
                div()
                    .id(id)
                    .debug_selector(move || id.to_owned())
                    .h(px(22.))
                    .px(px(7.))
                    .rounded(px(metric::RADIUS))
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .cursor_pointer()
                    .tab_index(0)
                    .hover(move |style| style.border_color(frame).text_color(lit))
                    .focus_visible(move |style| style.border_color(ring))
                    .font_family(theme.mono_font.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(10.))
                    .text_color(theme.fg_muted)
                    .child(icon(glyph, theme.fg_muted).size(px(11.)))
                    .child(scale::text(label.to_uppercase()))
            };
            // `.queue-head`: the count on the accent, what the queue does.
            let header =
                div()
                    .h(px(32.))
                    .pl(px(12.))
                    .pr(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .font_family(theme.mono_font.clone())
                    .text_size(px(11.))
                    .child(
                        div()
                            .debug_selector(|| "queue-count".into())
                            .min_w(px(18.))
                            .h(px(16.))
                            .px(px(4.))
                            .rounded(px(metric::RADIUS_SMALL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(if waiting { theme.blocked } else { theme.accent })
                            .text_color(theme.accent_ink)
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(10.))
                            .child(scale::text(current.len().to_string())),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_color(if self.queue.paused {
                                theme.blocked
                            } else {
                                theme.fg_muted
                            })
                            .child(scale::text(state)),
                    )
                    .when(waiting && queued > 0, |header| {
                        header.child(
                            tool("queue-resume", Icon::Play, "Resume")
                                .on_click(cx.listener(|this, _, _, cx| this.resume(cx))),
                        )
                    })
                    .when(!waiting && queued > 0, |header| {
                        header.child(tool("queue-pause", Icon::Pause, "Pause").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.queue.paused = true;
                                this.push_offer(cx);
                                cx.notify();
                            }),
                        ))
                    })
                    .when(queued > 0, |header| {
                        header.child(tool("queue-clear", Icon::Close, "Clear").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.queue.clear();
                                this.push_offer(cx);
                                window.focus(&this.focus, cx);
                                cx.notify();
                            }),
                        ))
                    });
            // In a stable order: a row's element keeps what it animates only
            // while it keeps its place among its siblings. Where it shows is
            // its spring's, by its place in the queue.
            let mut numbered: Vec<(&Item, usize, usize)> = Vec::new();
            let mut number = 0;
            for (item, at) in &current {
                if !item.forced {
                    number += 1;
                }
                numbered.push((item, *at, number));
            }
            numbered.sort_by_key(|(item, _, _)| item.id);
            let mut rows = Vec::new();
            for (item, at, number) in numbered {
                rows.push(
                    self.queue_row(item, at, number, waiting, window, theme, cx)
                        .into_any_element(),
                );
            }
            let unit = px(1.);
            for (item, at, _) in &self.leaving {
                let top = *at as f32 * QUEUE_ROW;
                rows.push(
                    queue_row_body(item, 0, false, false, theme)
                        .id(("queued-out", item.id))
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(top))
                        .with_animation(
                            ("queued-out", item.id),
                            gpui::Animation::new(composer::LEAVING).with_easing(motion::ease_in),
                            move |this, t| this.opacity(1. - t).ml(unit * (t * 24.)),
                        )
                        .into_any_element(),
                );
            }
            // `.queue`, round: a tray on the field's straight edge, clear of
            // its corners, rounded on top, its foot the field's border.
            div()
                .debug_selector(|| "queue".into())
                .mx(px(QUEUE_INSET))
                .mb(px(-1.))
                .rounded_t(px(metric::RADIUS))
                .border_1()
                .border_color(if self.queue.paused {
                    crate::lights::mix(theme.line_strong, theme.blocked, 0.45)
                } else {
                    theme.line_strong
                })
                .bg(theme.pane)
                .overflow_hidden()
                .child(header)
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(current.len() as f32 * QUEUE_ROW))
                        .border_t_1()
                        .border_color(theme.line)
                        .children(rows),
                )
        });
        grow(
            "queue",
            self.queue_height.clone(),
            div().w_full().children(content),
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_row(
        &self,
        item: &Item,
        at: usize,
        number: usize,
        waiting: bool,
        window: &Window,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let id = item.id;
        let Some(handle) = self.queue_focus.get(&id).cloned() else {
            return div().into_any_element();
        };
        let focused = handle.is_focused(window);
        let ring = theme.accent;
        let next = !item.forced && number == 1 && !waiting;
        let (factor, unit) = (scale::factor(), px(1.));
        let mut row = queue_row_body(item, number, next, focused, theme)
            .id(("queued", id))
            .debug_selector(move || format!("queued-{id}"))
            .track_focus(&handle)
            .focus_visible(move |style| style.border_color(ring))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                let modifiers = keystroke.modifiers;
                match keystroke.key.as_str() {
                    "up" | "down" => {
                        let down = keystroke.key == "down";
                        if modifiers.alt || modifiers.shift {
                            this.move_queued(id, if down { 1 } else { -1 }, cx);
                        } else {
                            let at = this
                                .queue
                                .items()
                                .iter()
                                .position(|item| item.id == id)
                                .unwrap_or(0);
                            if down && at + 1 >= this.queue.items().len() {
                                window.focus(&this.focus, cx);
                            } else {
                                let to = if down { at + 1 } else { at.saturating_sub(1) };
                                this.focus_queued(to, window, cx);
                            }
                        }
                        cx.stop_propagation();
                    }
                    "enter" if modifiers.platform || modifiers.control => {
                        this.force_queued(id, cx);
                        cx.stop_propagation();
                    }
                    "enter" => {
                        this.edit_queued(id, window, cx);
                        cx.stop_propagation();
                    }
                    "backspace" | "delete" => {
                        this.drop_queued(id, window, cx);
                        cx.stop_propagation();
                    }
                    "escape" => {
                        window.focus(&this.focus, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .on_click(cx.listener(move |this, click: &ClickEvent, window, cx| {
                if click.click_count() >= 2 {
                    this.edit_queued(id, window, cx);
                }
            }))
            .group(SharedString::from(format!("queued-{id}")));
        if !item.forced {
            let ghost = QueueDrag {
                id,
                text: SharedString::from(first_line(&item.draft.text)),
            };
            let drop_line = theme.accent;
            row = row
                .cursor_grab()
                .on_drag(ghost, move |drag, _, _, cx| {
                    cx.new(|_| DragGhost(drag.text.clone(), factor))
                })
                .drag_over::<QueueDrag>(move |style, _, _, _| {
                    style.border_t_2().border_color(drop_line)
                })
                .on_drop(cx.listener(move |this, drag: &QueueDrag, _, cx| {
                    if let Some(to) = this.queue.position(id) {
                        this.queue.move_to(drag.id, to);
                        cx.notify();
                    }
                }));
            let action = |name: &'static str, glyph: Icon, color| {
                div()
                    .id((name, id))
                    .debug_selector(move || format!("{name}-{id}"))
                    .size(px(22.))
                    .rounded(px(metric::RADIUS))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.selected))
                    .child(icon(glyph, color).size(px(12.)))
            };
            // `.queue-actions`: there with the pointer or the keyboard on the row.
            let shown = focused;
            row = row.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(1.))
                    .when(!shown, |actions| {
                        actions
                            .opacity(0.)
                            .group_hover(SharedString::from(format!("queued-{id}")), |style| {
                                style.opacity(1.)
                            })
                    })
                    .when(self.busy(), |actions| {
                        actions.child(action("queued-force", Icon::Bolt, theme.accent).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.force_queued(id, cx);
                                cx.stop_propagation();
                            }),
                        ))
                    })
                    .child(
                        action("queued-edit", Icon::Pencil, theme.fg_muted).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.edit_queued(id, window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    )
                    .child(action("queued-drop", Icon::Close, theme.fg_muted).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.drop_queued(id, window, cx);
                            cx.stop_propagation();
                        }),
                    )),
            );
        }
        let top = at as f32 * QUEUE_ROW;
        // The row slides to its place when the order changes, and fades in when
        // it comes. The spring runs in unscaled pixels: a step of the zoom is
        // not a move.
        div()
            .absolute()
            .left_0()
            .right_0()
            .child(
                // `.queue-item[data-new]`: `rise 0.18s ease-out`.
                row.with_animation(
                    ("queued-in", id),
                    gpui::Animation::new(motion::cockpit::QUEUE_ROW)
                        .with_easing(motion::cockpit::ease_out),
                    move |this, t| this.opacity(t).mt(unit * (6. * (1. - t))),
                ),
            )
            .with_spring(
                ("queued-place", id),
                SpringAnimation::new(GLIDE)
                    .to(top)
                    .from(top + QUEUE_ROW * 0.5)
                    .with_epsilon(0.2),
                move |this, top: f32| this.top(unit * top),
            )
            .into_any_element()
    }

    /// Keys and commands, over the field.
    fn help_element(
        &self,
        window: &Window,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let keys: [(&str, &str); 13] = [
            (
                "↵",
                "Run the prompt; while the agent works, queue it for when it finishes",
            ),
            ("⇧↵", "New line"),
            (
                force_key(),
                if self.sync.offers.steer {
                    "Force it in: the agent reads it after the tool calls it is making"
                } else {
                    "Force it in: the agent's turn is stopped and it goes at once"
                },
            ),
            ("esc", "Close what is open; pressed twice, stop the run"),
            (
                "↑ ↓",
                "Earlier prompts, shared with kou's other cockpits; over an empty field, the queue first",
            ),
            ("ctrl+r", "Search earlier prompts"),
            ("/", "Commands: typing filters, tab completes, ↵ runs"),
            (
                "$",
                "Link a file or folder of the project: typing lists them; $path:10-40 links those lines",
            ),
            (
                paste_key(),
                "Paste a picture, which the prompt names [Image 1]; dropping files attaches them",
            ),
            (
                "⌫",
                "Right after [Image n], takes the picture out; at the start of the field, the last picture",
            ),
            ("tab", "Complete in a list; otherwise to the next control"),
            (
                allow_key(),
                "Allow what the agent asks, with its first allow",
            ),
            (
                "pgup pgdn",
                "Scroll the transcript; home and end from an empty field",
            ),
        ];
        let catalog = self.composer.read(cx).catalog().clone();
        // The card is laid out after everything else, outside the scaled subtree.
        let lift = px(10.);
        let row = |key: SharedString, text: SharedString| {
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .child(
                    div()
                        .flex_none()
                        .w(px(84.))
                        .flex()
                        .child(key_hint(key, theme)),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_color(theme.fg_dim)
                        .child(scale::text(text)),
                )
        };
        let card = div()
            .id(("help", self.helps_opened))
            .debug_selector(|| "help".into())
            .occlude()
            .w(px(560.))
            .max_w(window.viewport_size().width - px(24.))
            .max_h(px(440.))
            .overflow_y_scroll()
            .p(px(12.))
            .rounded(px(metric::RADIUS_LARGE))
            .border_1()
            .border_color(theme.line_strong)
            .bg(theme.raised)
            .shadow(crate::picker::shadow(theme))
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_size(px(metric::TEXT_SMALL))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.help = false;
                cx.notify();
            }))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.fg)
                    .child(scale::text("Keys")),
            )
            .children(
                keys.iter()
                    .map(|(key, text)| row((*key).into(), (*text).into())),
            )
            .child(
                div()
                    .pt(px(6.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.fg)
                    .child(scale::text("Commands")),
            )
            .children(catalog.specs.iter().map(|spec| {
                let name = match &spec.args {
                    Some(args) => format!("/{} {args}", spec.name),
                    None => format!("/{}", spec.name),
                };
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(180.))
                            .font_family(theme.mono_font.clone())
                            .text_color(theme.fg)
                            .child(scale::text(name)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_color(theme.fg_dim)
                            .child(scale::text(spec.help.clone())),
                    )
            }))
            .with_animation(
                ("help-in", self.helps_opened),
                motion::enter(motion::BASE),
                move |this, t| this.opacity(t).mt(lift * (1. - t)),
            );
        div().absolute().top_0().left(px(8.)).child(
            deferred(
                anchored()
                    .anchor(Anchor::BottomLeft)
                    .offset(point(px(0.), px(-8.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )
            .with_priority(2),
        )
    }

    /// The prompts waiting, for tests.
    #[cfg(test)]
    pub(crate) fn queued_texts(&self) -> Vec<(String, bool)> {
        self.queue
            .items()
            .iter()
            .map(|item| (item.draft.text.clone(), item.forced))
            .collect()
    }
}

/// The key that forces a message in, as shown.
fn force_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘↵"
    } else {
        "ctrl+↵"
    }
}

fn paste_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘V"
    } else {
        "ctrl+v"
    }
}

/// What a press of the main button does.
type Press = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// What the main button is now (`.send[data-mode]`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SendMode {
    /// Nothing to run yet: a control like its neighbours.
    Idle,
    /// Run, or queue: the accent's fill.
    Run,
    /// Stop the turn: the error's colour in a quiet frame.
    Stop,
    /// Force it in: the accent's colour in a quiet frame.
    Force,
}

/// A square button at the end of the field (`.send`): its accent fill fades
/// in and out with whether it can be pressed, and its glyph arrives with it.
fn main_button(
    id: &'static str,
    glyph: Icon,
    mode: SendMode,
    theme: &Theme,
    press: Option<Press>,
) -> impl IntoElement + use<> {
    // Ready, it is the one primary action, as the quieter look's send is.
    let (fill, ink) = (theme.primary, theme.primary_ink);
    let ready = mode == SendMode::Run;
    let (frame, ink_now) = match mode {
        SendMode::Idle => (theme.line, theme.fg_faint),
        SendMode::Run => (theme.primary_rim, ink),
        SendMode::Stop => (
            crate::lights::mix(theme.line, theme.error, 0.6),
            theme.error,
        ),
        SendMode::Force => (
            crate::lights::mix(theme.line_strong, theme.accent, 0.55),
            theme.accent,
        ),
    };
    let ring = if ready { ink } else { theme.accent };
    let hover = match mode {
        SendMode::Run => crate::lights::mix(fill, theme.fg, 0.15),
        SendMode::Stop => theme.error.opacity(0.09),
        _ => theme.accent.opacity(0.12),
    };
    let unit = px(1.);
    // A layer of the accent over the resting button, rather than a colour
    // that would pass through other hues on its way.
    let layer = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(px(metric::RADIUS - 1.))
        .bg(fill)
        .with_spring(
            gpui::ElementId::Name(format!("send-fill-{id}").into()),
            SpringAnimation::new(SETTLE).to(ready),
            |this, phase| this.opacity(phase.0.clamp(0., 1.)),
        );
    let mut button = div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .relative()
        .flex_none()
        .size(px(28.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(frame)
        .bg(theme.pane)
        .flex()
        .items_center()
        .justify_center()
        .child(layer)
        .child(icon(glyph, ink_now).with_animation(
            gpui::ElementId::Name(format!("{id}-glyph-{ready}").into()),
            gpui::Animation::new(motion::cockpit::HOVER).with_easing(motion::cockpit::ease_out),
            move |this, t| this.size(unit * motion::mix(12., 15., t)).opacity(t),
        ));
    if mode != SendMode::Idle {
        button = button
            .cursor_pointer()
            .tab_index(0)
            .hover(move |style| style.bg(hover))
            // Pressed, it gives a pixel (`.send:active`).
            .active(|style| style.top(px(1.)))
            .focus_visible(move |style| style.border_color(ring));
    }
    if let Some(press) = press {
        button = button.on_click(press);
    }
    button
}

/// A queued prompt's row, without what makes it a control.
fn queue_row_body(item: &Item, number: usize, next: bool, focused: bool, theme: &Theme) -> Div {
    let mark: gpui::AnyElement = if item.forced {
        icon(Icon::Bolt, theme.accent)
            .size(px(13.))
            .into_any_element()
    } else {
        div()
            .font_family(theme.mono_font.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(10.5))
            .text_color(theme.fg_muted)
            .child(scale::text(format!("{number:02}")))
            .into_any_element()
    };
    let pictures = item.draft.pictures.len();
    div()
        .h(px(QUEUE_ROW))
        .pr(px(6.))
        .flex()
        .items_center()
        .gap(px(10.))
        .relative()
        .border_1()
        .border_color(gpui::transparent_black())
        .bg(if focused { theme.hover } else { theme.raised })
        // `.queue-item`: a rule over each.
        .child(
            div()
                .absolute()
                .top(px(-1.))
                .left_0()
                .right_0()
                .h(px(1.))
                .bg(theme.line),
        )
        .when(item.forced, |row| {
            // On its way in: an accent edge along it.
            row.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(theme.accent),
            )
        })
        .child(
            div()
                .flex_none()
                .w(px(38.))
                .flex()
                .justify_center()
                .child(mark),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(px(13.5))
                .text_color(if item.forced || focused {
                    theme.fg
                } else {
                    theme.fg_dim
                })
                .child(scale::text(first_line(&item.draft.text))),
        )
        .when(pictures > 0, |row| {
            row.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .text_size(px(metric::TEXT_SMALL))
                    .text_color(theme.fg_muted)
                    .child(icon(Icon::Image, theme.fg_muted).size(px(12.)))
                    .child(scale::text(pictures.to_string())),
            )
        })
        .when(item.forced, |row| {
            row.child(
                div()
                    .flex_none()
                    .text_size(px(metric::TEXT_SMALL))
                    .text_color(theme.fg_muted)
                    .child(scale::text("after the running tools")),
            )
        })
        .when(next, |row| {
            row.child(
                div()
                    .flex_none()
                    .font_family(theme.mono_font.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(10.))
                    .text_color(theme.fg_dim)
                    .child(scale::text("NEXT")),
            )
        })
}

/// A prompt on one line.
fn first_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What the empty field says, at length and short: how to use it, or why
/// writing in it is pointless.
pub(crate) fn placeholder(
    state: &ThreadState,
    agent: &str,
    steers: bool,
) -> (SharedString, SharedString) {
    match state {
        ThreadState::Closed(_) => (
            "This session has ended".into(),
            "This session has ended".into(),
        ),
        ThreadState::Working | ThreadState::Blocked => (
            format!(
                "Message {agent}. Enter queues it for when it finishes; {} {}.",
                if cfg!(target_os = "macos") {
                    "Cmd+Enter"
                } else {
                    "Ctrl+Enter"
                },
                if steers {
                    "forces it in after its running tools"
                } else {
                    "stops its turn and sends it now"
                }
            )
            .into(),
            format!("Message {agent} at work").into(),
        ),
        _ => (
            "Describe the task. Enter runs it, Shift+Enter adds a line, / lists commands, $ links a file."
                .into(),
            "Describe the task · / commands · $ files".into(),
        ),
    }
}
