//! The window: what is open, what each agent is doing, and what the user asked for.
//! Drawing is in `rail.rs` and `workspace.rs`.

use crate::state::{Axis, Id, Layout, PaneKind, RAIL_WIDTHS, Seat, Store};
use gpui::{
    AnyView, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding,
    PathPromptOptions, Pixels, PromptLevel, SharedString, Subscription, Window, actions,
};
use kou_canvas::{
    AddRequest, Board, BoardEvent, CanvasEvent, CanvasView, EdgeId, EdgeMode, NodeId, NodeKind,
    NodeSpec, Offer, Pending, PortRef, Traffic,
};
use kou_ui::{
    Mark,
    motion::{Spring, Springs},
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

actions!(
    kou,
    [
        /// Show or hide the rail.
        ToggleRail,
        /// From one field of the account form to the other.
        AccountNext,
        /// Choose a harness to start in this session.
        NewAgent,
        NewShell,
        ShowUsage,
        /// Close the focused pane.
        ClosePane,
        Quit,
        /// The session's panes as tabs and splits.
        ShowPanes,
        /// The session's panes as a canvas.
        ShowCanvas,
        /// Divide the focused pane's room: a new terminal to its right.
        SplitRight,
        /// Divide the focused pane's room: a new terminal below it.
        SplitDown,
        /// Focus the next pane of the tab.
        NextPane,
        /// Focus the previous pane of the tab.
        PreviousPane,
        /// Give the keys to the next pane of the tab, and stay in the tab.
        SwapPane,
        /// Give the keys to the previous pane of the tab, and stay in the tab.
        SwapPaneBack,
        /// Show the next tab.
        NextTab,
        /// Show the previous tab.
        PreviousTab,
        /// Show the tab at this place in the strip, counted from one.
        ShowTab1,
        ShowTab2,
        ShowTab3,
        ShowTab4,
        ShowTab5,
        ShowTab6,
        ShowTab7,
        ShowTab8,
        ShowTab9,
        /// Give the tab on screen a name and an icon.
        RenameTab,
        /// Open the pane with the keys the other way: a chat as its
        /// harness's terminal interface, a terminal as a chat.
        SwitchInterface,
        /// Move keyboard focus to the next control.
        FocusNext,
        /// Move keyboard focus to the previous control.
        FocusPrevious,
        /// Close whatever is open on top.
        Dismiss,
        /// Take the keys out of the pane, to the window's own controls.
        LeavePane,
    ]
);

/// Install the app's keys. A terminal pane keeps Tab and Escape for its program.
pub fn bind_keys(cx: &mut App) {
    // On macOS the command key is free; elsewhere Control belongs to the
    // terminal's program, so the app's keys add Shift.
    #[cfg(target_os = "macos")]
    const APP: &str = "cmd";
    #[cfg(not(target_os = "macos"))]
    const APP: &str = "ctrl-shift";
    let key = |keys: &str| -> &'static str { Box::leak(format!("{APP}-{keys}").into_boxed_str()) };
    cx.bind_keys([
        KeyBinding::new(key("b"), ToggleRail, None),
        KeyBinding::new(key("t"), NewAgent, None),
        KeyBinding::new(key("alt-t"), NewShell, None),
        KeyBinding::new(key("w"), ClosePane, None),
        KeyBinding::new(key("q"), Quit, None),
        KeyBinding::new(key("u"), ShowUsage, None),
        // The digits are the tabs, as in a browser or a terminal; a tab's
        // two layouts take Option with them.
        KeyBinding::new(key("alt-1"), ShowPanes, None),
        KeyBinding::new(key("alt-2"), ShowCanvas, None),
        KeyBinding::new(key("1"), ShowTab1, None),
        KeyBinding::new(key("2"), ShowTab2, None),
        KeyBinding::new(key("3"), ShowTab3, None),
        KeyBinding::new(key("4"), ShowTab4, None),
        KeyBinding::new(key("5"), ShowTab5, None),
        KeyBinding::new(key("6"), ShowTab6, None),
        KeyBinding::new(key("7"), ShowTab7, None),
        KeyBinding::new(key("8"), ShowTab8, None),
        KeyBinding::new(key("9"), ShowTab9, None),
        // And the arrows walk them. (Only where the command key is the
        // app's: with Control and Shift the arrows select text by words.)
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-right", NextTab, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-left", PreviousTab, None),
        KeyBinding::new(key("d"), SplitRight, None),
        KeyBinding::new(key("shift-d"), SplitDown, None),
        KeyBinding::new(key("]"), NextPane, None),
        KeyBinding::new(key("["), PreviousPane, None),
        KeyBinding::new(key("`"), SwapPane, None),
        // With Shift a symbol key arrives as the symbol Shift makes of it,
        // without Shift: both spellings are bound, for every such key.
        KeyBinding::new(key("~"), SwapPaneBack, None),
        KeyBinding::new(key("shift-`"), SwapPaneBack, None),
        KeyBinding::new(key("}"), NextTab, None),
        KeyBinding::new(key("{"), PreviousTab, None),
        KeyBinding::new(key("shift-]"), NextTab, None),
        KeyBinding::new(key("shift-["), PreviousTab, None),
        KeyBinding::new(key("r"), RenameTab, None),
        KeyBinding::new(key("shift-i"), SwitchInterface, None),
        KeyBinding::new(key("e"), LeavePane, None),
        KeyBinding::new("tab", FocusNext, Some("!Terminal")),
        KeyBinding::new("shift-tab", FocusPrevious, Some("!Terminal")),
        // In the account form Tab goes from one field to the other. (After
        // the two above: of bindings that match alike the later one is taken.)
        KeyBinding::new("tab", AccountNext, Some(crate::accounts::FORM)),
        KeyBinding::new("shift-tab", AccountNext, Some(crate::accounts::FORM)),
        // Wherever the keys are: a pane's own Escape is bound deeper and wins there.
        KeyBinding::new("escape", Dismiss, None),
    ]);
    // The application's own menu, where the system looks for Quit (and shows
    // its key): chosen there it is the same action as the key, so it asks
    // twice with agents at work as the key does.
    cx.set_menus([gpui::Menu::new("kou").items([
        gpui::MenuItem::action("Usage", ShowUsage),
        gpui::MenuItem::separator(),
        gpui::MenuItem::action("Quit kou", Quit),
    ])]);
}

/// Quit when no window is there to be asked (see [`Shell::quit`], which
/// answers the key in a window): registered once, when the app starts.
pub fn quit_without_a_window(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
}

/// How soon after it was pressed a second press of the quit key quits, when
/// agents are at work.
pub(crate) const QUIT_TWICE: Duration = Duration::from_millis(2500);

/// A harness as the window lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct HarnessInfo {
    /// The id panes are started by.
    pub id: String,
    /// Its name.
    pub name: String,
    /// One or two letters for its mark.
    pub code: String,
    /// Whether it can be started now.
    pub ready: bool,
    /// Its version, when it reported one.
    pub version: Option<String>,
    /// Why it cannot be started, or a warning.
    pub message: Option<String>,
    /// Whether it can run as a native thread instead of its terminal interface.
    pub native: bool,
    /// What its chat is waiting for, when that is an adapter the app can
    /// install: what to tell the user, and the install itself when there is
    /// a way to run it.
    pub adapter: Option<(String, Option<kou_agents::Install>)>,
}

/// What a pane shows.
pub enum Content {
    /// Nothing is running in it. The text says why, when there is a reason.
    Stopped(Option<SharedString>),
    /// A live view: a terminal or a thread.
    View(AnyView),
}

/// A pane's state while the app runs.
pub struct Live {
    /// What its agent is doing.
    pub mark: Mark,
    /// When that last changed.
    pub changed: Instant,
    /// Whether the user has looked at it since it last finished.
    pub seen: bool,
    /// See [`Content`].
    pub content: Content,
    /// When the pane was made in this run, if it was: it comes in once.
    pub born: Option<Instant>,
}

impl Live {
    /// A pane with nothing running in it.
    pub fn stopped(reason: Option<SharedString>) -> Self {
        Self {
            mark: Mark::Idle,
            changed: Instant::now(),
            seen: true,
            content: Content::Stopped(reason),
            born: None,
        }
    }

    /// The mark to draw: finished work stays "done" until it has been looked at.
    pub fn shown(&self) -> Mark {
        match self.mark {
            Mark::Idle if !self.seen => Mark::Done,
            mark => mark,
        }
    }
}

// A name as it is kept: on one line, without space around it or runs of it.
fn one_line(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where a new pane's node goes on its tab's canvas.
pub enum Place {
    /// Beside its parent when it has one, otherwise where the board has room.
    Auto,
    /// Where the user asked on the canvas, wired as they drew it.
    Asked(AddRequest),
    /// Centred on a world point.
    At(f32, f32),
    /// On a node the canvas has already made (a template's).
    Node(NodeId),
}

/// A message that waits on an edge for the user to let it through.
pub(crate) struct Held {
    /// The tab whose canvas the edge is on.
    pub tab: Id,
    pub edge: EdgeId,
    pub from: Id,
    pub to: Id,
    pub text: String,
    /// How many times it will have been passed on once it is let through
    /// (see `wires.rs`).
    pub hops: u32,
}

/// The harness picker: where the new agent goes. Asked for as a new tab, it
/// is a tab of its own in the strip until an agent is chosen in it, beside
/// the others and no more than one; asked for on a tab's canvas, it stands
/// over that canvas.
pub struct Picker {
    /// The session to start it in.
    pub session: Id,
    /// The pane it is a child of, when it was asked for from one.
    pub parent: Option<Id>,
    /// The tab it joins, when it was asked for on that tab's canvas.
    pub tab: Option<Id>,
    /// Whether it is what its session shows. As a tab of its own it can be
    /// left for another tab and come back to.
    pub front: bool,
    /// The pane whose room it divides, and which way, when it was asked for
    /// as a split: the choice is made in the half the new pane will have.
    pub beside: Option<(Id, Axis)>,
}

impl Picker {
    /// Whether it is a tab of its own: an agent not chosen yet.
    pub fn pending(&self) -> bool {
        self.tab.is_none() && self.beside.is_none()
    }
}

/// The earlier conversations a pane can go back to, asked for with `/sessions`.
pub struct Resuming {
    /// The pane to open one in.
    pub pane: Id,
    /// Latest first. `None` while they are being read.
    pub sessions: Option<Vec<Past>>,
}

/// The models a harness offers a new session, as it last said.
#[derive(Clone, Debug, Default)]
pub struct Offered {
    /// The models. Empty while they are asked for, and for a harness that
    /// offers no choice.
    pub models: Vec<kou_thread::Choice>,
    /// Who serves them, where the harness says.
    pub providers: Vec<kou_thread::Provider>,
    /// The one a new session starts with.
    pub current: Option<String>,
    pub access: kou_thread::Access,
    pub account: Option<Id>,
    /// The folder they were asked for, and when.
    pub cwd: PathBuf,
    pub at: Option<Instant>,
}

#[derive(PartialEq)]
struct PromptChoices {
    harness: Id,
    kind: PaneKind,
    revision: u64,
    model: Option<String>,
    access: Option<String>,
}

/// An earlier conversation of a harness's, as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Past {
    /// What the harness resumes it by.
    pub id: String,
    /// Its name; empty when it has none.
    pub title: String,
    /// When it was last written to, when that is known.
    pub updated: Option<std::time::SystemTime>,
    /// Whether something has it open right now.
    pub running: bool,
    /// How many prompts the user sent in it, when that is known.
    pub prompts: Option<usize>,
}

/// A tab being given a name and an icon.
pub struct TabEdit {
    /// The session it is a tab of.
    pub session: Id,
    /// Which of its tabs, by id: the strip may change under an open field.
    pub id: Id,
    /// The name as typed so far.
    pub field: kou_canvas::field::Field,
    /// The icon chosen so far, by its saved name; `None` is the harness's mark.
    pub icon: Option<String>,
}

/// What in the rail has a name of its own to change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Named {
    /// A space, by id.
    Space(Id),
    /// A session, by id.
    Session(Id),
}

/// A space or a session being renamed where its name stands in the rail.
pub struct NameEdit {
    /// Which one.
    pub what: Named,
    /// The name as typed so far.
    pub field: kou_canvas::field::Field,
}

/// The window's root view.
pub struct Shell {
    pub(crate) store: Store,
    path: Option<PathBuf>,
    pub(crate) live: HashMap<Id, Live>,
    /// The canvas of each tab that has been shown as one, or had a node made.
    canvases: HashMap<Id, (Entity<CanvasView>, Subscription)>,
    pub(crate) harnesses: Vec<HarnessInfo>,
    pub(crate) focus: FocusHandle,
    pub(crate) picker: Option<Picker>,
    pub(crate) settings_open: bool,
    pub(crate) appearance: crate::appearance::Appearance,
    pub(crate) idle_open: bool,
    pub(crate) notice: Option<SharedString>,
    /// The notice is of a save that failed.
    save_failed: bool,
    /// The account being added or changed in the settings.
    pub(crate) account_form: Option<crate::accounts::AccountForm>,
    /// What has the keys while that form is open.
    pub(crate) account_focus: FocusHandle,
    /// The panes that run a sign-in, with the account each is for.
    pub(crate) signing_in: HashMap<Id, Id>,
    /// Where a harness's own home is, when it is not where the harness
    /// has it by itself (see `accounts::own_home`): the tests' own folders.
    pub(crate) own_homes: HashMap<String, PathBuf>,
    /// The panes the canvas asked to close since the last frame: every node
    /// of a selection that was deleted, not the last of them alone.
    close_requested: Vec<Id>,
    /// When quitting was asked for with agents at work: asked again within
    /// [`QUIT_TWICE`], the app quits.
    quit_armed: Option<Instant>,
    /// Whether a scan for harnesses is under way.
    pub(crate) scanning: bool,
    /// What the last scan found.
    pub(crate) detected: Vec<kou_agents::Detected>,
    /// The panes with a process in them.
    pub(crate) running: HashMap<Id, crate::panes::Running>,
    /// The panes with a native conversation in them.
    pub(crate) threads: HashMap<Id, crate::panes::Talking>,
    /// Requests waiting for panes to become free.
    pub(crate) waits: Vec<crate::panes::Wait>,
    /// The socket panes reach the app through.
    pub(crate) socket: PathBuf,
    /// Where the keepers of this window's Claude Codes listen (see
    /// `panes.rs`): set once the window listens on its own socket. `None`
    /// for one that does not (a scene, a test, a second instance of the
    /// app), which keeps nothing and ends nothing that is kept.
    pub(crate) keepers: Option<PathBuf>,
    /// Where the app keeps what it writes: the store, and each pane's
    /// generated harness settings.
    pub(crate) home: PathBuf,
    /// The listening end of that socket; dropping it closes it.
    pub(crate) server: Option<kou_ipc::Server>,
    /// Where a scripted scene stops its transitions.
    pub(crate) scene_time: f32,
    /// Everything that glides rather than jumps: widths, heights, places.
    pub(crate) springs: RefCell<Springs>,
    /// False until the first frame has been drawn: what is there at launch is
    /// simply there, and only what arrives later makes an entrance.
    pub(crate) mounted: Cell<bool>,
    /// What the user did on a canvas that needs the window to answer.
    board_events: Vec<(Id, BoardEvent)>,
    /// Messages waiting on edges set to ask first.
    pub(crate) held: Vec<Held>,
    /// How many times the message that started each pane's turn had been
    /// passed on, agent to agent; a pane the user wrote to is not here.
    pub(crate) hops: HashMap<Id, u32>,
    /// The questions panes asked each other, whose answers are still owed.
    pub(crate) asks: Vec<crate::wires::Ask>,
    /// The requests that wait for a harness to say its models, and what
    /// looks, a few times a second, whether it has.
    pub(crate) deferred: Vec<crate::wires::Deferred>,
    pub(crate) deferred_poll: Option<gpui::Task<()>>,
    /// How much has gone along each edge since the app started.
    pub(crate) traffic: HashMap<(Id, EdgeId), Traffic>,
    /// The list of earlier conversations, when one is open.
    pub(crate) resuming: Option<Resuming>,
    /// The agents each conversation's agent is waiting for right now, as
    /// its view was last told, and the changed lines of each one's folder.
    pub(crate) waiting_on: HashMap<Id, Vec<Id>>,
    pub(crate) changed_lines: HashMap<Id, (u32, u32)>,
    // The sessions a harness has open right now, by harness, as last asked
    // and when: asked again once that is a few seconds old.
    pub(crate) open_now: HashMap<Id, (Instant, Vec<kou_agents::OpenSession>)>,
    /// Whether notifications are really posted (see `notify.rs`). What would
    /// have been said is kept in `announced` either way.
    pub(crate) notifies: bool,
    /// Whether the system lets the app post them, as it last said.
    pub(crate) notice_permission: crate::notify::Permission,
    /// Where the system's answer to another look arrives from.
    pub(crate) notice_tell: Option<async_channel::Sender<crate::notify::Heard>>,
    /// The notifications of this run, newest last: title and body.
    pub(crate) announced: Vec<(String, String)>,
    /// The app's own program as panes call it (their hooks, the command
    /// line, the MCP server): a copy under the app's home, so that what runs
    /// in a pane goes on working when the program the app was started from
    /// is moved, rebuilt or deleted. `None` falls back to that program.
    pub(crate) bin: Option<PathBuf>,
    /// The app's own program as the keepers of its Claude Codes are started
    /// from: a copy that is this build's alone (see `main::keeper_program`),
    /// which no other build of the app replaces. `None` falls back to the
    /// program the app was started from.
    pub(crate) keeper_bin: Option<PathBuf>,
    /// When each pane's conversation was last opened again by itself, for
    /// a connection to its kept Claude Code that was lost (see
    /// `panes::take_up_again`).
    pub(crate) taken_up: HashMap<Id, std::time::Instant>,
    /// The guidance on working with the app's other agents that a harness
    /// started in a pane is given, as files under the app's home.
    pub(crate) guidance: Option<kou_mcp::Bundle>,
    /// The prompt field of an empty screen: a conversation's own, standing
    /// alone. What is sent from it is the first prompt of the agent it
    /// starts. Made when it is first shown, where a window is at hand.
    pub(crate) prompt: Option<Entity<kou_thread_view::PromptField>>,
    // The folder that field looks its file links up in, as last told to it.
    prompt_project: Option<PathBuf>,
    /// What an agent about to be started from that field is to be sent
    /// first: its words, and with pictures or files in it, its blocks.
    pub(crate) first_prompt: Option<(String, Vec<kou_thread::Block>)>,
    // A prompt sent while there was no folder to work in yet, and when: it
    // goes to the agent started once there is one.
    ask_pending: Option<(kou_thread_view::Prompt, Instant)>,
    /// Whether harnesses are started to be asked things (their models): in
    /// the app proper, not in a window made for a picture or a test.
    pub(crate) probes: bool,
    /// The models each harness offers a new session, as it last said, by
    /// harness: what the prompt field's model control lists before there is
    /// a session to ask.
    pub(crate) offered: HashMap<Id, Offered>,
    /// The model chosen in that control, and the harness it was chosen for:
    /// the agent started from the field runs with it.
    pub(crate) ask_model: Option<(Id, String)>,
    pub(crate) ask_access: Option<(PathBuf, String)>,
    // What the field's model control was last told, to tell it only of a change.
    prompt_models: Option<PromptChoices>,
    // Counts every change in `offered`.
    pub(crate) offered_changes: u64,
    // Whether that field has been given the keys since it came on screen.
    asked: bool,
    /// The harness chosen in the field's own list for what is typed there,
    /// and how it opens; without a choice it is the one started last.
    pub(crate) ask_choice: Option<(String, PaneKind)>,
    /// Whether that list is open under the field.
    pub(crate) ask_menu: bool,
    /// The account chosen with `ask_choice` in the field's list, by id;
    /// `None` is the harness's own.
    pub(crate) ask_profile: Option<Id>,
    /// The account the pane being started is to run under: set by whoever
    /// starts it, taken by the start.
    pub(crate) launch_profile: Option<Id>,
    /// What each account's agent last said of its plan's limits (see
    /// `usage.rs`).
    pub(crate) usage: crate::usage::Answers,
    /// Whether the usage page stands where the workspace would, as the
    /// settings do: one of the two at most.
    pub(crate) usage_open: bool,
    /// When the usage page was opened: its bars fill from then. `None`:
    /// they stand filled (a capture, a test).
    pub(crate) usage_opened: Option<Instant>,
    /// A wall clock for what is left of a window, pinned in captures.
    pub(crate) usage_now: Option<std::time::SystemTime>,
    /// Asks the accounts again while the page is open; gone with it.
    pub(crate) usage_tick: Option<gpui::Task<()>>,
    /// The app's plugins (see `plugins.rs`), started when first asked for
    /// and shared with the chats, which draw with them too; and whether
    /// they are read from disk and watched (not in a test or a capture).
    pub(crate) plugins: crate::plugins::SharedHost,
    pub(crate) plugins_live: bool,
    pub(crate) plugin_watch: Option<gpui::Task<()>>,
    /// Where the keys go while a field a plugin drew is typed in, and what
    /// keeps what was typed when they go elsewhere.
    pub(crate) plugin_focus: FocusHandle,
    pub(crate) plugin_blur: Option<Subscription>,
    /// The surfaces agents show in their chats (see `surfaces.rs`), and
    /// what looks at their files while they are on screen.
    pub(crate) surfaces: Rc<RefCell<crate::surfaces::Surfaces>>,
    pub(crate) surface_watch: Option<gpui::Task<()>>,
    /// The pages plugins add, as last listed, and the one of them that
    /// stands where the workspace would (the Usage page has its own flag).
    pub(crate) plugin_pages: Vec<kou_plugins::PageInfo>,
    pub(crate) plugin_page: Option<String>,
    /// What the window's notice says of a plugin's problem, while it does.
    pub(crate) plugin_said: Option<SharedString>,
    /// Whether the tab strip's plus shows what a new tab can be: an agent,
    /// or a shell.
    pub(crate) new_menu: bool,
    /// The harness whose chat adapter is being installed, while it is.
    pub(crate) installing: Option<String>,
    /// Panes whose conversations are to be started again where they left
    /// off, once the harnesses have been found: what a window put up again
    /// was told ran in them.
    pub(crate) revive: Vec<Id>,
    /// How a tab made in this run starts out, when the command line said
    /// (`--layout canvas`); a tab is panes otherwise.
    pub(crate) new_tabs: Option<Layout>,
    /// The tab being named, when one is, and where the keys are meanwhile.
    pub(crate) tab_edit: Option<TabEdit>,
    pub(crate) tab_focus: FocusHandle,
    /// The space or session being renamed in the rail, when one is. It has
    /// the same keys as the tab's field: only one of the two is ever open.
    pub(crate) name_edit: Option<NameEdit>,
    // Keeps what was typed when the keys leave the field for anything else.
    name_blur: Option<Subscription>,
    /// The pane the keys were last given to, so they are given again only when
    /// the pane that should have them changes: see [`Shell::give_keys`].
    keys_at: Option<Id>,
    /// The git branch of each folder asked about, and when it was read: the
    /// rail asks on every frame, and a branch changes a few times a day.
    branches: RefCell<HashMap<PathBuf, (Instant, Option<String>)>>,
    /// Where the panes were drawn last frame, for laying splits out by hand.
    pub(crate) content_area: Rc<Cell<Bounds<Pixels>>>,
    /// Width and height the panes will have, from the window's size, until measured.
    pub(crate) area_guess: Cell<(f32, f32)>,
    /// The rail and the tab strip, each a view of its own that is drawn
    /// again only when what it shows changed (see `workspace.rs`), and what
    /// each showed when last told to draw again.
    pub(crate) rail_part: Entity<crate::workspace::Part>,
    pub(crate) tabs_part: Entity<crate::workspace::Part>,
    pub(crate) rail_shows: Option<crate::workspace::Shows>,
    pub(crate) tabs_shows: Option<crate::workspace::Shows>,
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Shell {
    /// A window over a store. `path` is where changes are saved; `None` keeps them in memory.
    pub fn new(store: Store, path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        // The plugins, which tooltips are drawn by too (see `plugins.rs`).
        let plugins: crate::plugins::SharedHost = Rc::default();
        kou_ui::set_tip_face(Some(crate::plugins::tip_face(plugins.clone())), cx);
        // Nothing survives a restart running: every restored pane starts stopped.
        let live = store
            .panes
            .iter()
            .map(|pane| (pane.id.clone(), Live::stopped(None)))
            .collect();
        // Ages in the rail move on without anything else changing.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            store,
            path,
            live,
            canvases: HashMap::new(),
            harnesses: Vec::new(),
            focus: cx.focus_handle(),
            picker: None,
            settings_open: false,
            appearance: crate::appearance::Appearance::new(cx),
            idle_open: false,
            notice: None,
            save_failed: false,
            account_form: None,
            account_focus: cx.focus_handle(),
            signing_in: HashMap::new(),
            own_homes: HashMap::new(),
            close_requested: Vec::new(),
            quit_armed: None,
            scanning: false,
            detected: Vec::new(),
            running: HashMap::new(),
            threads: HashMap::new(),
            waits: Vec::new(),
            socket: kou_ipc::socket_path(),
            keepers: None,
            home: crate::state::home(),
            server: None,
            scene_time: 0.,
            springs: RefCell::default(),
            mounted: Cell::new(false),
            board_events: Vec::new(),
            held: Vec::new(),
            hops: HashMap::new(),
            asks: Vec::new(),
            deferred: Vec::new(),
            deferred_poll: None,
            traffic: HashMap::new(),
            resuming: None,
            waiting_on: HashMap::new(),
            changed_lines: HashMap::new(),
            open_now: HashMap::new(),
            bin: None,
            keeper_bin: None,
            taken_up: HashMap::new(),
            notifies: false,
            notice_permission: crate::notify::Permission::Unknown,
            notice_tell: None,
            announced: Vec::new(),
            guidance: None,
            prompt: None,
            prompt_project: None,
            first_prompt: None,
            ask_pending: None,
            probes: false,
            offered: HashMap::new(),
            ask_model: None,
            ask_access: None,
            prompt_models: None,
            offered_changes: 0,
            asked: false,
            ask_choice: None,
            ask_menu: false,
            ask_profile: None,
            launch_profile: None,
            usage: HashMap::new(),
            usage_open: false,
            usage_opened: None,
            usage_now: None,
            usage_tick: None,
            plugins,
            plugins_live: false,
            plugin_watch: None,
            plugin_focus: cx.focus_handle(),
            plugin_blur: None,
            surfaces: Rc::default(),
            surface_watch: None,
            plugin_pages: Vec::new(),
            plugin_page: None,
            plugin_said: None,
            new_menu: false,
            installing: None,
            revive: Vec::new(),
            new_tabs: None,
            tab_edit: None,
            tab_focus: cx.focus_handle(),
            name_edit: None,
            name_blur: None,
            keys_at: None,
            branches: RefCell::default(),
            content_area: Rc::default(),
            area_guess: Cell::new((0., 0.)),
            rail_part: crate::workspace::Part::new(crate::workspace::Which::Rail, cx),
            tabs_part: crate::workspace::Part::new(crate::workspace::Which::Tabs, cx),
            rail_shows: None,
            tabs_shows: None,
        }
    }

    /// What a window put up again needs of this one (see `main.rs`): where it
    /// keeps its files, everything it remembers, and which of its screens
    /// was open. As plain data, so that it can be read by code that lays
    /// this type out differently.
    pub(crate) fn kept(&self) -> serde_json::Value {
        serde_json::json!({
            "home": self.home,
            "path": self.path,
            "store": self.store,
            "new_tabs": self.new_tabs,
            "settings_open": self.settings_open,
            "usage_open": self.usage_open,
            "idle_open": self.idle_open,
            "notifies": self.notifies,
        })
    }

    /// Let go of everything that reaches outside the window, now: the socket
    /// agents talk to, what runs in the panes, the conversations, the
    /// canvases. For a window whose content is being taken down: the view
    /// itself may be kept a frame longer by what was last drawn, and what
    /// replaces it needs the socket's path free at once.
    pub(crate) fn let_go(&mut self) {
        self.server = None;
        // (What is kept beside the socket is the next window's to find.)
        self.keepers = None;
        self.waits.clear();
        self.running.clear();
        self.threads.clear();
        self.canvases.clear();
        self.held.clear();
        for live in self.live.values_mut() {
            live.content = Content::Stopped(None);
        }
    }

    /// Where the value called `key` is now, on its way to `target`.
    pub(crate) fn spring(&self, key: impl Into<SharedString>, target: f32, spring: Spring) -> f32 {
        self.springs.borrow_mut().to(key, target, spring)
    }

    /// The git branch a folder is on, read from disk at most once in a while.
    pub(crate) fn branch(&self, folder: &std::path::Path) -> Option<String> {
        const FRESH: Duration = Duration::from_secs(3);
        let mut branches = self.branches.borrow_mut();
        if let Some((read, branch)) = branches.get(folder)
            && read.elapsed() < FRESH
        {
            return branch.clone();
        }
        let branch = crate::state::branch_of(folder);
        branches.insert(folder.to_owned(), (Instant::now(), branch.clone()));
        branch
    }

    /// A harness by id.
    pub fn harness(&self, id: &str) -> Option<&HarnessInfo> {
        self.harnesses.iter().find(|harness| harness.id == id)
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        let Some(path) = &self.path else {
            return;
        };
        // Only a save that fails has something to say, and only what it
        // said is taken back by one that works: a notice of anything else
        // (agents cannot reach this window, a message that waits) is not a
        // save's to clear, and nearly every click saves.
        match self.store.save(path) {
            Err(error) => {
                self.notice = Some(format!("Could not save to {}: {error}", path.display()).into());
                self.save_failed = true;
            }
            Ok(()) => {
                if std::mem::take(&mut self.save_failed) {
                    self.notice = None;
                }
            }
        }
        cx.notify();
    }

    /// Tells every canvas what can be started now.
    pub(crate) fn offer_to_canvases(&mut self, cx: &mut Context<Self>) {
        let offers = self.offers();
        for (canvas, _) in self.canvases.values() {
            let offers = offers.clone();
            canvas.update(cx, |canvas, cx| canvas.set_offers(offers, cx));
        }
    }

    /// The canvas of a tab, created from its saved board on first use.
    pub(crate) fn canvas(
        &mut self,
        tab: &str,
        cx: &mut Context<Self>,
    ) -> Option<Entity<CanvasView>> {
        if let Some((canvas, _)) = self.canvases.get(tab) {
            return Some(canvas.clone());
        }
        let saved = self.store.tab(tab)?;
        let (board, camera) = (saved.board.clone(), saved.camera);
        let marks: Vec<_> = self
            .store
            .panes_in(tab)
            .into_iter()
            .filter_map(|pane| Some((pane.node, self.live.get(&pane.id)?.shown())))
            .collect();
        let offers = self.offers();
        // Its chrome is drawn by the `canvas` plugin (see `plugins.rs`).
        let face = self.canvas_face(cx);
        let canvas = cx.new(|cx| {
            let mut canvas = CanvasView::new(board, cx);
            canvas.set_face(Some(face), cx);
            if let Some(camera) = camera {
                canvas.set_camera(camera, cx);
            }
            for (node, mark) in marks {
                canvas.restore_mark(node, mark, cx);
            }
            canvas.set_offers(offers, cx);
            canvas
        });
        let id = tab.to_owned();
        let subscription = cx.subscribe(&canvas, move |this, canvas, event: &CanvasEvent, cx| {
            crate::hot::hot!(this.canvas_event(&id, &canvas, event, cx));
        });
        // What the user does to the board itself is answered on the next
        // frame, where the window is at hand to start panes with.
        let id = tab.to_owned();
        cx.subscribe(&canvas, move |this, _, event: &BoardEvent, cx| {
            this.board_events.push((id.clone(), event.clone()));
            cx.notify();
        })
        .detach();
        self.canvases
            .insert(tab.to_owned(), (canvas.clone(), subscription));
        let panes: Vec<Id> = self
            .store
            .panes_in(tab)
            .into_iter()
            .map(|pane| pane.id.clone())
            .collect();
        for pane in panes {
            self.show_on_canvas(&pane, cx);
        }
        Some(canvas)
    }

    /// The canvas of the tab a pane is in, if it has been made.
    pub(crate) fn canvas_of(&self, pane: &str) -> Option<(Id, Entity<CanvasView>)> {
        let tab = self.store.tab_of(pane)?.id.clone();
        let canvas = self.canvases.get(&tab)?.0.clone();
        Some((tab, canvas))
    }

    /// What a canvas may add: every harness found, with why not for the rest.
    pub(crate) fn offers(&self) -> Vec<Offer> {
        self.harnesses
            .iter()
            .map(|harness| Offer {
                id: harness.id.clone(),
                title: harness.name.clone(),
                code: harness.code.clone(),
                kind: if harness.id == "shell" {
                    NodeKind::Terminal
                } else {
                    NodeKind::Agent
                },
                detail: harness
                    .message
                    .clone()
                    .or_else(|| harness.version.clone())
                    .unwrap_or_default(),
                ready: harness.ready,
            })
            .collect()
    }

    /// Bring a pane's node up to date with the pane: its live view inside it,
    /// its name, and where it works.
    pub(crate) fn show_on_canvas(&mut self, pane: &str, cx: &mut Context<Self>) {
        let Some(saved) = self.store.pane(pane) else {
            return;
        };
        let Some((tab, canvas)) = self.canvas_of(pane) else {
            return;
        };
        let (node, title) = (saved.node, saved.title.clone());
        let code = self
            .harness(&saved.harness)
            .map(|harness| harness.code.clone());
        let place = match self.branch(&saved.cwd) {
            Some(branch) => format!("{} · {branch}", abbreviate(&saved.cwd)),
            None => abbreviate(&saved.cwd),
        };
        let body = match self.live.get(pane).map(|live| &live.content) {
            Some(Content::View(view)) => Some(view.clone()),
            _ => None,
        };
        let focus = self.body_focus(pane, cx);
        canvas.clone().update(cx, |canvas, cx| {
            canvas.update_node(
                node,
                |node| {
                    node.title = title;
                    node.place = place;
                    if let Some(code) = code {
                        node.tag = code;
                    }
                },
                cx,
            );
            if let Some(body) = body {
                canvas.set_body(node, body, cx);
                if let Some(focus) = focus {
                    canvas.set_body_focus(node, focus, cx);
                }
            }
        });
        self.follow_canvas(&tab, cx);
    }

    fn canvas_event(
        &mut self,
        tab: &str,
        canvas: &Entity<CanvasView>,
        event: &CanvasEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            CanvasEvent::Opened(node) => {
                if let Some(pane) = self.pane_of(tab, *node) {
                    self.focus_pane(&pane, cx);
                    self.set_layout(Layout::Panes, cx);
                }
            }
            CanvasEvent::CreateRequested { .. } => {
                if let Some(session) = self.store.session_of_tab(tab) {
                    self.picker = Some(Picker {
                        session: session.id.clone(),
                        parent: None,
                        tab: Some(tab.to_owned()),
                        front: true,
                        beside: None,
                    });
                    cx.notify();
                }
            }
            CanvasEvent::RemoveRequested(node) => {
                if let Some(pane) = self.pane_of(tab, *node) {
                    if !self.close_requested.contains(&pane) {
                        self.close_requested.push(pane);
                    }
                    cx.notify();
                }
            }
            CanvasEvent::Changed => {
                let (board, camera) = {
                    let canvas = canvas.read(cx);
                    (canvas.board().clone(), canvas.camera())
                };
                if let Some(saved) = self.store.tab_mut(tab) {
                    saved.board = board;
                    saved.camera = Some(camera);
                }
                self.save(cx);
            }
            // Whatever else the canvas says, the window has nothing to do for.
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    /// Ask for a folder and add it as a space with a first session.
    pub(crate) fn add_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add as a space".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| this.add_space_at(path, cx)).ok();
        })
        .detach();
    }

    /// Add a directory as a space, with a first session, and show it.
    pub fn add_space_at(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Id {
        let space = self.store.add_space(path);
        if self.store.sessions_of(&space, false).next().is_none() {
            self.store.add_session(&space);
        }
        self.close_pages();
        self.save(cx);
        space
    }

    pub(crate) fn add_session(&mut self, space: &str, cx: &mut Context<Self>) -> Option<Id> {
        let session = self.store.add_session(space)?;
        if let Some(space) = self.store.space_mut(space) {
            space.collapsed = false;
        }
        self.close_pages();
        self.save(cx);
        Some(session)
    }

    pub(crate) fn select_session(&mut self, session: &str, cx: &mut Context<Self>) {
        if let Some(saved) = self.store.session_mut(session) {
            saved.archived = false;
            self.store.selected = Some(session.to_owned());
            self.close_pages();
            // A tab waiting for its agent waits in its session.
            self.picker.take_if(|picker| !picker.pending());
            self.see_focused();
            self.save(cx);
        }
    }

    pub(crate) fn set_archived(
        &mut self,
        session: &str,
        archived: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An archived session runs nothing.
        if archived {
            let panes: Vec<Id> = self
                .store
                .panes_of(session)
                .map(|pane| pane.id.clone())
                .collect();
            for pane in panes {
                self.stop_pane(&pane, "Stopped when its session was archived.", cx);
            }
        }
        if let Some(saved) = self.store.session_mut(session) {
            saved.archived = archived;
        }
        if archived && self.store.selected.as_deref() == Some(session) {
            self.store.selected = None;
        }
        let _ = window;
        self.save(cx);
    }

    pub(crate) fn delete_session(&mut self, session: &str, cx: &mut Context<Self>) {
        let panes: Vec<Id> = self
            .store
            .panes_of(session)
            .map(|pane| pane.id.clone())
            .collect();
        for pane in panes {
            self.forget(&pane, cx);
            self.live.remove(&pane);
            self.store.remove_pane(&pane);
        }
        if let Some(saved) = self.store.session(session) {
            for tab in &saved.tabs {
                self.canvases.remove(&tab.id);
            }
        }
        self.store.sessions.retain(|saved| saved.id != session);
        if self.store.selected.as_deref() == Some(session) {
            self.store.selected = None;
        }
        self.save(cx);
    }

    /// Show the tab on screen as panes or as its canvas.
    pub(crate) fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        // A tab without an agent has neither.
        if self.pending_in_front() {
            return;
        }
        let Some(tab) = self
            .store
            .current()
            .and_then(|session| session.front())
            .map(|tab| tab.id.clone())
        else {
            return;
        };
        self.set_tab_layout(&tab, layout, cx);
    }

    /// Show a tab as panes or as its canvas.
    pub(crate) fn set_tab_layout(&mut self, tab: &str, layout: Layout, cx: &mut Context<Self>) {
        let Some(saved) = self.store.tab_mut(tab) else {
            return;
        };
        saved.layout = layout;
        self.close_pages();
        self.see_focused();
        self.follow_canvas(tab, cx);
        self.save(cx);
    }

    pub(crate) fn focus_pane(&mut self, pane: &str, cx: &mut Context<Self>) {
        let Some(session) = self.store.pane(pane).map(|pane| pane.session.clone()) else {
            return;
        };
        self.store.focus(pane);
        if let Some(saved) = self.store.session_mut(&session) {
            saved.archived = false;
        }
        // The tab waiting for its agent stays in the strip, behind this one.
        if let Some(picker) = self.picker.as_mut().filter(|picker| picker.pending()) {
            picker.front = false;
        }
        self.store.selected = Some(session);
        self.close_pages();
        self.see_focused();
        self.save(cx);
    }

    /// Whether the screen with the prompt field is what the window shows:
    /// a session with nothing in it yet, a new tab, or no space at all.
    pub(crate) fn asking(&self) -> bool {
        if self.page_open() || self.resuming.is_some() {
            return false;
        }
        match self.store.current() {
            Some(session) => session.tabs.is_empty() || self.pending_in_front(),
            None => self.store.spaces.is_empty(),
        }
    }

    /// The agent a prompt typed on an empty screen goes to, and how it
    /// opens: the one chosen in the field's list; otherwise the one the user
    /// started last, when it can be started; otherwise the first there is.
    /// Never a shell: a prompt is nothing to give one, so a shell is started
    /// at once, from the field's list or from the tab strip's plus.
    pub(crate) fn asked(&self) -> Option<(&HarnessInfo, PaneKind)> {
        let ready = |id: &str| {
            self.harness(id)
                .filter(|harness| harness.ready && harness.id != "shell")
        };
        if let Some((id, kind)) = &self.ask_choice
            && let Some(harness) = ready(id)
            && (*kind == PaneKind::Terminal || harness.native)
        {
            return Some((harness, *kind));
        }
        let harness = self
            .store
            .last_harness
            .as_deref()
            .and_then(ready)
            .or_else(|| {
                self.harnesses
                    .iter()
                    .find(|harness| harness.ready && harness.id != "shell")
            })?;
        Some((harness, self.default_kind(&harness.id)))
    }

    /// The account what is typed on an empty screen goes to, when one is
    /// named: the one chosen with the harness in the field's list. Without
    /// a choice it is the one the harness started last ran under, unless
    /// the accounts are one pool ([`crate::state::Store::balance`]): then
    /// none is named, and the start picks the one with the most left.
    pub(crate) fn asked_profile(&self) -> Option<&crate::state::Profile> {
        let (harness, _) = self.asked()?;
        let id = match &self.ask_choice {
            Some(_) => self.ask_profile.as_deref(),
            None if self.store.balance => None,
            None => self.store.last_profile.as_deref(),
        }?;
        self.store
            .profile(id)
            .filter(|profile| profile.harness == harness.id)
    }

    /// Choose, in the field's list, who gets what is typed there. The keys
    /// go back to the field.
    pub(crate) fn choose_asked(&mut self, harness: &str, kind: PaneKind, cx: &mut Context<Self>) {
        self.choose_account(harness, None, kind, cx);
    }

    /// The same, for one of the harness's accounts (`None`: its own).
    pub(crate) fn choose_account(
        &mut self,
        harness: &str,
        profile: Option<Id>,
        kind: PaneKind,
        cx: &mut Context<Self>,
    ) {
        self.ask_choice = Some((harness.to_owned(), kind));
        self.ask_access = None;
        self.prompt_models = None;
        self.ask_profile = profile;
        self.ask_menu = false;
        self.asked = false;
        cx.notify();
    }

    /// Forget an account (see [`Store::remove_profile`]).
    pub(crate) fn remove_account(&mut self, id: &str, cx: &mut Context<Self>) {
        self.store.remove_profile(id);
        // Its proxy's password goes with it; its folder stays.
        if let Err(error) = crate::accounts::set_secret(&self.home, id, None) {
            log::warn!("an account's proxy was not forgotten: {error}");
        }
        if self
            .account_form
            .as_ref()
            .is_some_and(|form| form.editing.as_deref() == Some(id))
        {
            self.account_form = None;
        }
        if self.ask_profile.as_deref() == Some(id) {
            self.ask_profile = None;
        }
        self.save(cx);
        self.usage_changed(cx);
    }

    /// Open or close the field's list of what can be started.
    pub(crate) fn toggle_ask_menu(&mut self, cx: &mut Context<Self>) {
        self.ask_menu = !self.ask_menu;
        cx.notify();
    }

    /// How many agents are at work or waiting for an answer that quitting
    /// would end: not those that go on without the app
    /// (`Shell::outlives_app`).
    pub(crate) fn at_work(&self) -> usize {
        self.live
            .iter()
            .filter(|(pane, live)| {
                matches!(live.mark, Mark::Working | Mark::Blocked) && !self.outlives_app(pane)
            })
            .count()
    }

    /// Whether to quit, asked at `now`: at once with nothing at work;
    /// otherwise only when asked a second time soon after the first, which
    /// says what would be lost.
    pub(crate) fn may_quit(&mut self, now: Instant) -> bool {
        let at_work = self.at_work();
        let again = self
            .quit_armed
            .take()
            .is_some_and(|at| now.saturating_duration_since(at) <= QUIT_TWICE);
        if at_work == 0 || again {
            return true;
        }
        self.quit_armed = Some(now);
        let who = if at_work == 1 {
            "1 agent is".to_owned()
        } else {
            format!("{at_work} agents are")
        };
        self.notice = Some(
            format!(
                "{who} at work and would be stopped. Press {}+Q again to quit.",
                crate::screens::APP_KEY
            )
            .into(),
        );
        false
    }

    /// Quit the app: the quit key, or Quit in the application's menu.
    pub(crate) fn quit(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        if self.may_quit(now) {
            self.save(cx);
            cx.quit();
            return;
        }
        // What it said holds only as long as a second press counts.
        let said = self.notice.clone();
        let timer = cx.background_executor().timer(QUIT_TWICE);
        cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |this, cx| {
                if this.notice == said {
                    this.notice = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Open or close the list under the tab strip's plus.
    pub(crate) fn toggle_new_menu(&mut self, cx: &mut Context<Self>) {
        self.new_menu = !self.new_menu;
        cx.notify();
    }

    /// Whether a shell can be opened now: there is a session on screen and
    /// a shell on this machine.
    pub(crate) fn shell_ready(&self) -> bool {
        self.store.current().is_some() && self.harness("shell").is_some_and(|shell| shell.ready)
    }

    /// Open a shell in a new tab of the session on screen, at once: there
    /// is nothing to ask before a shell starts. A tab that waits for its
    /// agent stays in the strip, behind the shell's.
    pub(crate) fn new_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_menu = false;
        if !self.shell_ready() {
            cx.notify();
            return;
        }
        // Not into what is being chosen (a waiting tab, the new half of a
        // split, a canvas's node): a tab of its own.
        let waiting = self.picker.take();
        self.start("shell", PaneKind::Terminal, window, cx);
        if let Some(mut waiting) = waiting.filter(|picker| picker.pending()) {
            waiting.front = false;
            self.picker = Some(waiting);
        }
        cx.notify();
    }

    /// The prompt field of an empty screen, made the first time it is asked for.
    pub(crate) fn prompt_field(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<kou_thread_view::PromptField> {
        if let Some(field) = &self.prompt {
            return field.clone();
        }
        let shell = cx.entity().downgrade();
        let face = self.field_face(cx);
        let lists = self.chat_face(cx);
        let field = cx.new(|cx| {
            let mut field = kou_thread_view::PromptField::new(window, cx);
            field.set_placeholder("What should be done?", cx);
            // Its frame is drawn as a chat's field's is: by the plugin.
            field.set_face(Some(face), cx);
            // And the lists over it, as a chat's are.
            field.set_chat_face(Some(lists), cx);
            // Who gets the prompt stands in the field's own foot.
            field.set_foot(
                Some(move |_: &mut Window, cx: &mut App| match shell.upgrade() {
                    Some(shell) => Shell::ask_chooser(&shell, cx),
                    None => gpui::IntoElement::into_any_element(gpui::Empty),
                }),
                cx,
            );
            field
        });
        cx.subscribe_in(&field, window, Self::prompt_event).detach();
        self.prompt = Some(field.clone());
        field
    }

    // Keep the prompt field's model control to what the harness it names
    // offers: asked of the harness the first time, and again when what it
    // said has grown old.
    fn offer_models(
        &mut self,
        field: &Entity<kou_thread_view::PromptField>,
        cx: &mut Context<Self>,
    ) {
        const FRESH: Duration = Duration::from_secs(600);
        let (Some((harness, kind)), Some(cwd)) = (
            self.asked()
                .map(|(harness, kind)| (harness.id.clone(), kind)),
            self.prompt_project.clone(),
        ) else {
            return;
        };
        let known = self.offered.get(&harness);
        let account = self.asked_profile().map(|p| p.id.clone());
        let stale = known.is_none_or(|known| {
            known.cwd != cwd
                || known.account != account
                || known.at.is_some_and(|at| at.elapsed() > FRESH)
        });
        if stale && self.probes {
            // Marked as asked for at once; what was known stays on show
            // until the answer comes.
            let mut asking = known.cloned().unwrap_or_default();
            if asking.cwd != cwd || asking.account != account {
                asking.access = Default::default();
                self.prompt_models = None;
            }
            asking.cwd = cwd.clone();
            asking.at = None;
            self.offered.insert(harness.clone(), asking);
            self.ask_for_models(&harness, cwd.clone(), cx);
        }
        let chosen = self
            .ask_model
            .as_ref()
            .filter(|(of, _)| *of == harness)
            .map(|(_, model)| model.clone());
        let access_choice = self
            .ask_access
            .as_ref()
            .filter(|(dir, _)| dir == &cwd)
            .map(|(_, id)| id.clone());
        let shown = PromptChoices {
            harness: harness.clone(),
            kind,
            revision: self.offered_changes,
            model: chosen.clone(),
            access: access_choice.clone(),
        };
        if self.prompt_models.as_ref() == Some(&shown) {
            return;
        }
        self.prompt_models = Some(shown);
        let offered = self.offered.get(&harness).cloned().unwrap_or_default();
        let model = chosen.or(offered.current);
        let mut access =
            if harness == "codex" && kind == PaneKind::Thread && self.store.codex_direct {
                offered.access
            } else {
                Default::default()
            };
        if !access.choices.is_empty() {
            access.choices.insert(
                0,
                kou_thread::Choice {
                    id: "configured".into(),
                    name: "Use Codex settings".into(),
                    description: None,
                },
            );
            access.current = access_choice;
        }
        field.update(cx, |field, cx| {
            field.set_models(offered.models, offered.providers, model, cx);
            field.set_access(access, cx);
        });
    }

    fn prompt_event(
        &mut self,
        _: &Entity<kou_thread_view::PromptField>,
        event: &kou_thread_view::PromptFieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use kou_thread_view::PromptFieldEvent as Event;
        match event {
            Event::Submit(prompt) => self.ask(Some(prompt.clone()), window, cx),
            // Enter on an empty field starts the agent with nothing to say
            // to it yet.
            Event::Empty => self.ask(None, window, cx),
            Event::Model(model) => {
                // For the harness the field names now.
                if let Some(harness) = self.asked().map(|(harness, _)| harness.id.clone()) {
                    self.ask_model = Some((harness, model.clone()));
                    cx.notify();
                }
            }
            Event::Height(_) => {}
            Event::Access(id) => {
                if self.store.codex_direct
                    && self
                        .asked()
                        .is_some_and(|(h, k)| h.id == "codex" && k == PaneKind::Thread)
                {
                    self.ask_access = self
                        .prompt_project
                        .clone()
                        .filter(|_| id != "configured")
                        .map(|cwd| (cwd, id.clone()));
                    self.prompt_models = None;
                    cx.notify();
                }
            }
        }
    }

    /// Start the agent the prompt field names, with `prompt` as the first
    /// thing it is sent. Without a folder to work in, one is asked for
    /// first, and the prompt waits for it.
    pub(crate) fn ask(
        &mut self,
        prompt: Option<kou_thread_view::Prompt>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.store.current().is_none() {
            self.ask_pending = prompt.map(|prompt| (prompt, Instant::now()));
            self.add_space(window, cx);
            return;
        }
        let Some((harness, kind)) = self
            .asked()
            .map(|(harness, kind)| (harness.id.clone(), kind))
        else {
            return;
        };
        self.first_prompt = prompt.map(|prompt| (prompt.text, prompt.blocks));
        self.launch_profile = self.asked_profile().map(|profile| profile.id.clone());
        self.start(&harness, kind, window, cx);
    }

    /// Whether the picker is of what the screen shows: the tab waiting in
    /// front of the session on screen, the half being chosen in its front
    /// tab, or a place asked for on a canvas of that session. A picker can
    /// be none of these and still be kept: a tab that waits in a session
    /// the user has left.
    pub(crate) fn picker_shown(&self) -> bool {
        let (Some(picker), Some(session)) = (self.picker.as_ref(), self.store.current()) else {
            return false;
        };
        if picker.session != session.id || !picker.front {
            return false;
        }
        match (&picker.beside, &picker.tab) {
            (Some((at, _)), _) => session.front().is_some_and(|tab| tab.root.contains(at)),
            // (Asked for on a canvas of this session: it joins that tab.)
            (None, Some(tab)) => session.tabs.iter().any(|own| own.id == *tab),
            (None, None) => true,
        }
    }

    /// Runs `add`, which adds a pane that is not of the user's doing (an
    /// agent's `pane spawn`), and leaves what the user has open as it was:
    /// the page in front, a tab waiting for its agent, a half being chosen.
    pub(crate) fn undisturbed<T>(&mut self, add: impl FnOnce(&mut Self) -> T) -> T {
        let picker = self.picker.take();
        let (settings, usage) = (self.settings_open, self.usage_open);
        let plugin = self.plugin_page.take();
        let tick = self.usage_tick.take();
        let added = add(self);
        self.picker = picker;
        self.settings_open = settings;
        self.usage_open = usage;
        self.plugin_page = plugin;
        self.usage_tick = tick;
        added
    }

    /// The tab waiting for its agent, when the session has one.
    pub(crate) fn pending_tab(&self, session: &str) -> Option<&Picker> {
        self.picker
            .as_ref()
            .filter(|picker| picker.pending() && picker.session == session)
    }

    /// Whether the session on screen shows the tab waiting for its agent.
    pub(crate) fn pending_in_front(&self) -> bool {
        self.store.current().is_some_and(|session| {
            self.pending_tab(&session.id)
                .is_some_and(|picker| picker.front)
        })
    }

    /// Bring the tab waiting for its agent to the front.
    pub(crate) fn show_pending_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.as_mut().filter(|picker| picker.pending()) {
            picker.front = true;
            self.close_pages();
            self.asked = false;
            cx.notify();
        }
    }

    /// Close the tab waiting for its agent.
    pub(crate) fn close_pending_tab(&mut self, cx: &mut Context<Self>) {
        if self.picker.take_if(|picker| picker.pending()).is_some() {
            self.see_focused();
            cx.notify();
        }
    }

    // Whatever is on screen has been looked at.
    pub(crate) fn see_focused(&mut self) {
        let Some(session) = self.store.current() else {
            return;
        };
        if self.pending_in_front() {
            return;
        }
        // As panes or as a canvas, what is on screen is the tab in front.
        let visible: Vec<Id> = session
            .front()
            .map(|tab| tab.root.panes().into_iter().cloned().collect())
            .unwrap_or_default();
        for pane in visible {
            if let Some(live) = self.live.get_mut(&pane) {
                live.seen = true;
            }
        }
    }

    pub(crate) fn see_everything(&mut self, cx: &mut Context<Self>) {
        for live in self.live.values_mut() {
            live.seen = true;
        }
        cx.notify();
    }

    // The next or previous pane of the tab on screen; with one pane in it,
    // the next or previous tab, so the keys always go somewhere.
    fn step_pane(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.pending_in_front() {
            return self.step_tab(forward, cx);
        }
        let Some(session) = self.store.current() else {
            return;
        };
        let Some(tab) = session.tab() else {
            return;
        };
        let panes: Vec<Id> = session.tabs[tab]
            .root
            .panes()
            .into_iter()
            .cloned()
            .collect();
        if panes.len() < 2 {
            return self.step_tab(forward, cx);
        }
        let at = panes
            .iter()
            .position(|pane| Some(pane) == session.focused.as_ref())
            .unwrap_or(0);
        let next = panes[step(at, panes.len(), forward)].clone();
        self.focus_pane(&next, cx);
    }

    /// Give the keys to the next or previous pane of the tab on screen, in
    /// reading order and round again. On the tab's canvas the node is
    /// entered, which brings it into view. With one pane there is nowhere to
    /// go, and the key is not ours.
    pub(crate) fn swap_pane(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((tab, layout, panes, at)) = self
            .store
            .current()
            .filter(|_| !self.page_open() && !self.pending_in_front())
            .and_then(|session| {
                let tab = session.front()?;
                let panes: Vec<Id> = tab.root.panes().into_iter().cloned().collect();
                let at = panes
                    .iter()
                    .position(|pane| Some(pane) == session.focused.as_ref())
                    .unwrap_or(0);
                Some((tab.id.clone(), tab.layout, panes, at))
            })
        else {
            return false;
        };
        if panes.len() < 2 {
            return false;
        }
        let next = panes[step(at, panes.len(), forward)].clone();
        self.focus_pane(&next, cx);
        if layout == Layout::Canvas
            && let Some(node) = self.store.pane(&next).map(|pane| pane.node)
            && let Some(canvas) = self.canvas(&tab, cx)
        {
            // A node with something running in it takes the keys; one with
            // nothing in it is only shown (entering it would open it as a pane).
            let running = matches!(
                self.live.get(&next).map(|live| &live.content),
                Some(Content::View(_))
            );
            canvas.update(cx, |canvas, cx| {
                if running {
                    canvas.enter(node, window, cx);
                } else {
                    canvas.focus_node(node, cx);
                }
            });
        }
        true
    }

    /// Show the tab at `place` in the strip, counted from one; the tab
    /// waiting for its agent is the last. A place past the end is no tab.
    pub(crate) fn show_tab(&mut self, place: usize, cx: &mut Context<Self>) {
        let Some(session) = self.store.current() else {
            return;
        };
        match session.tabs.get(place.wrapping_sub(1)) {
            Some(tab) => {
                let pane = tab.active.clone();
                self.focus_pane(&pane, cx);
            }
            None if place == session.tabs.len() + 1 && self.pending_tab(&session.id).is_some() => {
                self.show_pending_tab(cx);
            }
            None => {}
        }
    }

    pub(crate) fn step_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(session) = self.store.current() else {
            return;
        };
        let Some(tab) = session.tab() else {
            return;
        };
        // The tab waiting for its agent is the last in the strip.
        let pending = self.pending_tab(&session.id).map(|picker| picker.front);
        let count = session.tabs.len() + usize::from(pending.is_some());
        let at = if pending == Some(true) {
            session.tabs.len()
        } else {
            tab
        };
        match session.tabs.get(step(at, count, forward)) {
            Some(next) => {
                let next = next.active.clone();
                self.focus_pane(&next, cx);
            }
            None => self.show_pending_tab(cx),
        }
    }

    /// Divide the focused pane's room: the new half shows what can be
    /// started there, and becomes a pane when one is chosen (see
    /// `Shell::start`); Escape gives the room back. With nothing to divide
    /// it is a new tab. False when there are no panes on screen to split: on
    /// a canvas the same key is the canvas's own, and copies what is selected.
    pub(crate) fn split(&mut self, axis: Axis, _: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.store.current() else {
            return false;
        };
        if session.layout() != Layout::Panes || self.page_open() || self.pending_in_front() {
            return false;
        }
        let (session, beside) = (session.id.clone(), session.focused.clone());
        match beside {
            Some(at) => {
                self.picker = Some(Picker {
                    session,
                    parent: None,
                    tab: None,
                    front: true,
                    beside: Some((at, axis)),
                });
                cx.notify();
            }
            None => self.pick(None, cx),
        }
        true
    }

    /// The pane being divided and which way, while what goes into the new
    /// half of the tab on screen is being chosen.
    pub(crate) fn choosing_beside(&self) -> Option<(Id, Axis)> {
        let session = self.store.current()?;
        let (at, axis) = self
            .picker
            .as_ref()
            .filter(|picker| picker.session == session.id && picker.front)?
            .beside
            .clone()?;
        session.front()?.root.contains(&at).then_some((at, axis))
    }

    /// What a tab is called: the name it was given, else its pane's.
    pub(crate) fn tab_name(&self, tab: &crate::state::Tab) -> String {
        tab.name.clone().unwrap_or_else(|| {
            self.store
                .pane(&tab.active)
                .map_or_else(String::new, |pane| pane.title.clone())
        })
    }

    /// Start naming a tab: its name in a field, all of it selected, and its
    /// icon to choose. Enter keeps both, Escape neither.
    pub(crate) fn edit_tab(
        &mut self,
        session: &str,
        tab: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, name, icon)) = self
            .store
            .session(session)
            .and_then(|session| session.tabs.get(tab))
            .map(|saved| (saved.id.clone(), self.tab_name(saved), saved.icon.clone()))
        else {
            return;
        };
        self.commit_name_edit(cx);
        self.tab_edit = Some(TabEdit {
            session: session.to_owned(),
            id,
            field: kou_canvas::field::Field::new(&name, false),
            icon,
        });
        window.focus(&self.tab_focus, cx);
        cx.notify();
    }

    /// Keep what the tab was named and given.
    pub(crate) fn commit_tab_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.tab_edit.take() else {
            return;
        };
        let tab = edit.id.clone();
        if self.store.tab(&tab).is_some() {
            self.name_tab(&tab, edit.field.text(), cx);
            // An icon the set does not have is no icon.
            let icon = edit
                .icon
                .as_deref()
                .filter(|icon| kou_ui::tab_icon(icon).is_some())
                .unwrap_or("auto");
            let _ = self.set_tab_icon(&tab, icon, cx);
        }
        cx.notify();
    }

    /// Start renaming a space or a session where its name stands: the name
    /// in a field, all of it selected. Enter keeps it, and so does taking the
    /// keys elsewhere; Escape leaves it as it was.
    pub(crate) fn edit_name(&mut self, what: Named, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &what {
            Named::Space(id) => self.store.space(id).map(|space| space.name.clone()),
            Named::Session(id) => self.store.session(id).map(|session| session.name.clone()),
        };
        let Some(name) = name else {
            return;
        };
        self.commit_tab_edit(cx);
        self.name_edit = Some(NameEdit {
            what,
            field: kou_canvas::field::Field::new(&name, false),
        });
        if self.name_blur.is_none() {
            self.name_blur = Some(cx.on_blur(&self.tab_focus, window, |this, _, cx| {
                this.commit_name_edit(cx);
            }));
        }
        window.focus(&self.tab_focus, cx);
        cx.notify();
    }

    /// Keep the name typed in the rail. An empty one changes nothing.
    pub(crate) fn commit_name_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.name_edit.take() else {
            return;
        };
        let name = edit.field.text();
        match &edit.what {
            Named::Space(id) => self.name_space(id, name, cx),
            Named::Session(id) => self.name_session(id, name, cx),
        };
        cx.notify();
    }

    /// A key in the field a space or a session is renamed in.
    pub(crate) fn name_edit_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use kou_canvas::field::Outcome;
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(edit) = self.name_edit.as_mut() else {
            return;
        };
        match edit.field.key(&event.keystroke, clipboard) {
            Outcome::Commit => self.commit_name_edit(cx),
            Outcome::Cancel => self.name_edit = None,
            Outcome::Copy(text) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
            Outcome::Edited | Outcome::Moved => {}
            // Keys the field has no use for go on to the window.
            Outcome::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Name a tab. An empty name, or its pane's own, means it goes back to
    /// being called after its pane. False when there is no such tab.
    pub(crate) fn name_tab(&mut self, tab: &str, name: &str, cx: &mut Context<Self>) -> bool {
        let name = one_line(name);
        let own = self
            .store
            .tab(tab)
            .and_then(|tab| self.store.pane(&tab.active))
            .map(|pane| pane.title.clone());
        let Some(saved) = self.store.tab_mut(tab) else {
            return false;
        };
        saved.name = (!name.is_empty() && Some(&name) != own.as_ref()).then_some(name);
        self.save(cx);
        true
    }

    /// Give a tab one of the icons of the set, by name; `auto` or nothing
    /// shows what runs in it again. The error says which names there are.
    pub(crate) fn set_tab_icon(
        &mut self,
        tab: &str,
        icon: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let icon = icon.trim();
        let chosen = match icon {
            "" | "auto" => None,
            known if kou_ui::tab_icon(known).is_some() => Some(known.to_owned()),
            unknown => {
                let names: Vec<&str> = kou_ui::TAB_ICONS.iter().map(|(name, _)| *name).collect();
                return Err(format!(
                    "there is no icon {unknown:?}; the icons are auto, {}",
                    names.join(", ")
                ));
            }
        };
        let Some(saved) = self.store.tab_mut(tab) else {
            return Err(format!("there is no tab {tab}"));
        };
        saved.icon = chosen;
        self.save(cx);
        Ok(())
    }

    /// Rename a session. False when there is none, or the name is empty.
    pub(crate) fn name_session(
        &mut self,
        session: &str,
        name: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let name = one_line(name);
        let Some(saved) = self.store.session_mut(session).filter(|_| !name.is_empty()) else {
            return false;
        };
        saved.name = name;
        self.save(cx);
        true
    }

    /// Rename a space. False when there is none, or the name is empty.
    pub(crate) fn name_space(&mut self, space: &str, name: &str, cx: &mut Context<Self>) -> bool {
        let name = one_line(name);
        let Some(saved) = self.store.space_mut(space).filter(|_| !name.is_empty()) else {
            return false;
        };
        saved.name = name;
        self.save(cx);
        true
    }

    /// A key in the field a tab is named in.
    pub(crate) fn tab_edit_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use kou_canvas::field::Outcome;
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(edit) = self.tab_edit.as_mut() else {
            return;
        };
        match edit.field.key(&event.keystroke, clipboard) {
            Outcome::Commit => self.commit_tab_edit(cx),
            Outcome::Cancel => self.tab_edit = None,
            Outcome::Copy(text) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
            Outcome::Edited | Outcome::Moved => {}
            // Keys the field has no use for go on to the window.
            Outcome::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Set the share of a split: a divider was dragged.
    pub(crate) fn set_share(
        &mut self,
        session: &str,
        tab: usize,
        path: &[bool],
        share: f32,
        cx: &mut Context<Self>,
    ) {
        let share = share.clamp(crate::state::MIN_SHARE, 1. - crate::state::MIN_SHARE);
        if let Some(ratio) = self
            .store
            .session_mut(session)
            .and_then(|session| session.tabs.get_mut(tab))
            .and_then(|tab| tab.root.ratio_mut(path))
            && (*ratio - share).abs() > 0.0005
        {
            *ratio = share;
            cx.notify();
        }
    }

    /// Set the rail's width: its edge was dragged.
    pub(crate) fn set_rail_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = width.clamp(RAIL_WIDTHS.0, RAIL_WIDTHS.1).round();
        if (self.store.rail_width - width).abs() > 0.5 {
            self.store.rail_width = width;
            cx.notify();
        }
    }

    /// Close every pane of a tab, asking for each that is in the middle of something.
    pub(crate) fn close_tab(
        &mut self,
        session: &str,
        tab: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panes: Vec<Id> = self
            .store
            .session(session)
            .and_then(|session| session.tabs.get(tab))
            .map(|tab| tab.root.panes().into_iter().cloned().collect())
            .unwrap_or_default();
        for pane in panes {
            self.request_close(&pane, window, cx);
        }
    }

    /// Open a tab for an agent to be chosen in, in the session on screen.
    /// There is one such tab at most: asked for again, it comes to the front.
    pub(crate) fn pick(&mut self, parent: Option<Id>, cx: &mut Context<Self>) {
        let Some(session) = self.store.selected.clone() else {
            return;
        };
        match self.picker.as_mut() {
            Some(picker) if picker.pending() && picker.session == session => {
                picker.front = true;
                if parent.is_some() {
                    picker.parent = parent;
                }
            }
            _ => {
                self.picker = Some(Picker {
                    session,
                    parent,
                    tab: None,
                    front: true,
                    beside: None,
                });
            }
        }
        self.close_pages();
        // Asked for: its prompt field takes the keys, wherever they were.
        self.asked = false;
        cx.notify();
    }

    /// Add a pane for a harness to a session, with the keys. A child of a
    /// pane whose tab is shown as a canvas joins that canvas, beside its
    /// parent; anything else gets a tab of its own. Nothing runs in it yet:
    /// the caller installs its content.
    pub fn add_pane(
        &mut self,
        session: &str,
        harness: &str,
        parent: Option<Id>,
        kind: PaneKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Id> {
        let seat = self.seat_for(parent.as_deref());
        self.add_pane_at(
            session,
            harness,
            parent,
            kind,
            Place::Auto,
            seat,
            window,
            cx,
        )
    }

    /// Where a pane goes when nobody said: into its parent's tab when that
    /// tab is shown as a canvas, so the two are seen together with the line
    /// between them, and into a tab of its own otherwise.
    pub(crate) fn seat_for(&self, parent: Option<&str>) -> Seat {
        match parent.and_then(|parent| self.store.tab_of(parent)) {
            Some(tab) if tab.layout == Layout::Canvas => Seat::In(tab.id.clone()),
            _ => Seat::Tab(Board::new()),
        }
    }

    /// [`Shell::add_pane`], saying which tab it joins and where on that
    /// tab's canvas its node goes.
    #[allow(clippy::too_many_arguments)]
    pub fn add_pane_at(
        &mut self,
        session: &str,
        harness: &str,
        parent: Option<Id>,
        kind: PaneKind,
        place: Place,
        mut seat: Seat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Id> {
        let info = self.harness(harness)?.clone();
        let saved = self.store.session(session)?;
        let cwd = parent
            .as_deref()
            .and_then(|parent| self.store.pane(parent))
            .map_or_else(|| saved.cwd.clone(), |parent| parent.cwd.clone());
        let spec = NodeSpec {
            kind: if harness == "shell" {
                NodeKind::Terminal
            } else {
                NodeKind::Agent
            },
            title: info.name.clone(),
            tag: info.code.clone(),
            detail: abbreviate(&cwd),
            size: None,
        };
        // The tab it joins, if there is one yet, has the board its node goes on.
        let joined = match &seat {
            Seat::Tab(_) => None,
            Seat::Beside { at, .. } => self.store.tab_of(at).map(|tab| tab.id.clone()),
            Seat::In(tab) => self.store.tab(tab).map(|tab| tab.id.clone()),
        };
        let node = match (&mut seat, joined) {
            (Seat::Tab(board), _) => board.add_at(spec, 0., 0.),
            (_, None) => return None,
            (seat, Some(tab)) => {
                // A parent draws a line to its child only on a board both are on.
                let parent_node = parent
                    .as_deref()
                    .filter(|parent| {
                        self.store
                            .tab_of(parent)
                            .is_some_and(|theirs| theirs.id == tab)
                    })
                    .and_then(|parent| self.store.pane(parent))
                    .map(|parent| parent.node);
                // A split's new part goes beside the pane it divides.
                let beside = match seat {
                    Seat::Beside { at, axis } => self.store.pane(at).map(|pane| (pane.node, *axis)),
                    _ => None,
                };
                let canvas = self.canvas(&tab, cx)?;
                canvas.update(cx, |canvas, cx| match (place, parent_node) {
                    (Place::Node(node), _) => Some(node),
                    (Place::Asked(request), _) => {
                        Some(canvas.fulfil(&request, spec.clone(), window, cx))
                    }
                    (Place::At(x, y), _) => Some(canvas.create_at(x, y, spec.clone(), window, cx)),
                    (Place::Auto, Some(parent)) => {
                        canvas.spawn_child(parent, spec.clone(), window, cx)
                    }
                    (Place::Auto, None) => {
                        let board = canvas.board();
                        let (w, h) = spec.kind.default_size();
                        let at = beside
                            .and_then(|(node, axis)| Some((board.node(node)?.rect, axis)))
                            .map(|(rect, axis)| match axis {
                                Axis::Row => {
                                    (rect.right() + kou_canvas::GAP_X + w / 2., rect.y + h / 2.)
                                }
                                Axis::Column => (
                                    rect.x + w / 2.,
                                    rect.bottom() + kou_canvas::GAP_Y * 2. + h / 2.,
                                ),
                            })
                            .or_else(|| {
                                board.bounds().map(|bounds| {
                                    (bounds.x + bounds.w / 2., bounds.bottom() + 160.)
                                })
                            })
                            .unwrap_or((0., 0.));
                        Some(canvas.create_at(at.0, at.1, spec.clone(), window, cx))
                    }
                })?
            }
        };
        let own_tab = matches!(seat, Seat::Tab(_));
        let pane = self.store.add_pane(
            session,
            harness,
            &info.name,
            cwd,
            kind,
            parent.clone(),
            node,
            seat,
        );
        if own_tab
            && let Some(layout) = self.new_tabs
            && let Some(tab) = self.store.tab_of(&pane).map(|tab| tab.id.clone())
            && let Some(tab) = self.store.tab_mut(&tab)
        {
            tab.layout = layout;
        }
        let mut live = Live::stopped(None);
        live.born = Some(Instant::now());
        self.live.insert(pane.clone(), live);
        self.picker = None;
        self.close_pages();
        self.save(cx);
        Some(pane)
    }

    /// Put a live view in a pane.
    pub fn set_content(&mut self, pane: &str, view: AnyView, cx: &mut Context<Self>) {
        if let Some(live) = self.live.get_mut(pane) {
            live.content = Content::View(view);
            cx.notify();
        }
    }

    /// Record what a pane's agent is doing. Work that finishes out of sight is
    /// kept as "done" until the pane is looked at.
    pub fn set_mark(
        &mut self,
        pane: &str,
        mark: Mark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visible = self.is_visible(pane) && window.is_window_active();
        let Some(live) = self.live.get_mut(pane) else {
            return;
        };
        if live.mark == mark {
            return;
        }
        let was = live.mark;
        let finished = live.mark == Mark::Working && mark == Mark::Idle;
        live.mark = mark;
        live.changed = Instant::now();
        live.seen = !finished || visible;
        let shown = live.shown();
        if let (Some(saved), Some((_, canvas))) = (self.store.pane(pane), self.canvas_of(pane)) {
            let node = saved.node;
            canvas.update(cx, |canvas, cx| {
                canvas.set_mark(node, if finished { Mark::Done } else { shown }, window, cx)
            });
        }
        self.announce(pane, was, mark, visible);
        // The agent that started this one shows it in its conversation.
        if let Some(parent) = self.store.pane(pane).and_then(|saved| saved.parent.clone()) {
            self.sync_started(&parent, cx);
        }
        cx.notify();
    }

    // Tell the user, if they asked to be told and are not looking: an agent
    // of theirs finished, or one stopped to ask.
    fn announce(&mut self, pane: &str, was: Mark, now: Mark, watched: bool) {
        let Some(saved) = self.store.pane(pane) else {
            return;
        };
        let Some(occasion) = crate::notify::occasion(
            was,
            now,
            watched,
            saved.parent.is_some(),
            self.store.notify_done,
            self.store.notify_asks,
        ) else {
            return;
        };
        // Where it is: its session, and the space when that says more.
        let session = self.store.session(&saved.session);
        let space = session.and_then(|session| self.store.space(&session.space));
        let place = match (session, space) {
            (Some(session), Some(space)) if session.name != space.name => {
                format!("{} · {}", session.name, space.name)
            }
            (Some(session), _) => session.name.clone(),
            _ => String::new(),
        };
        let (title, body) = crate::notify::words(occasion, &saved.title, &place);
        if self.notifies {
            // Its id says which pane it is about: a click on it shows that
            // pane, and a later one about the same pane takes its place.
            crate::notify::post(&format!("pane:{pane}"), &title, &body);
        }
        self.announced.push((title, body));
    }

    /// The system's news about the app's notifications: one was clicked, so
    /// the app comes to the front on the pane it was about; or the system
    /// says whether the app may post.
    pub(crate) fn heard(
        &mut self,
        heard: crate::notify::Heard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match heard {
            crate::notify::Heard::Clicked(id) => {
                if let Some(pane) = id.strip_prefix("pane:")
                    && self.store.pane(pane).is_some()
                {
                    self.focus_pane(pane, cx);
                }
                cx.activate(true);
                window.activate_window();
            }
            crate::notify::Heard::Permission(permission) => {
                if self.notice_permission != permission {
                    self.notice_permission = permission;
                    cx.notify();
                }
            }
        }
    }

    /// Ask the system again whether the app may post: the user may have
    /// changed it there since.
    pub(crate) fn look_at_notice_permission(&self) {
        if let Some(tell) = &self.notice_tell {
            crate::notify::look(tell);
        }
    }

    /// Turn one of the two notifications on or off.
    pub(crate) fn toggle_notice(&mut self, asks: bool, cx: &mut Context<Self>) {
        if asks {
            self.store.notify_asks = !self.store.notify_asks;
        } else {
            self.store.notify_done = !self.store.notify_done;
        }
        self.save(cx);
    }

    fn is_visible(&self, pane: &str) -> bool {
        !self.pending_in_front()
            && self
                .store
                .current()
                .and_then(|session| session.front())
                .is_some_and(|tab| tab.root.contains(pane))
    }

    /// End what runs in a pane and say why it is empty.
    pub(crate) fn stop_pane(&mut self, pane: &str, reason: &str, cx: &mut Context<Self>) {
        self.forget(pane, cx);
        if let Some(live) = self.live.get_mut(pane) {
            live.content = Content::Stopped(Some(reason.to_owned().into()));
            live.mark = Mark::Idle;
            live.seen = true;
            live.changed = Instant::now();
            cx.notify();
        }
    }

    // The pane a node of a tab's canvas stands for.
    fn pane_of(&self, tab: &str, node: NodeId) -> Option<Id> {
        self.store
            .panes_in(tab)
            .into_iter()
            .find(|pane| pane.node == node)
            .map(|pane| pane.id.clone())
    }

    // What the user did on a canvas: started things, renamed them, wired them.
    fn board_event(
        &mut self,
        tab: &str,
        event: BoardEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self
            .store
            .session_of_tab(tab)
            .map(|session| session.id.clone())
        else {
            return;
        };
        let session = session.as_str();
        match event {
            BoardEvent::AddRequested(request) => {
                let harness = request.offer.clone();
                let kind = self.default_kind(&harness);
                let pane = self.add_pane_at(
                    session,
                    &harness,
                    None,
                    kind,
                    Place::Asked(request),
                    Seat::In(tab.to_owned()),
                    window,
                    cx,
                );
                if let Some(pane) = pane {
                    self.launch(&pane, None, window, cx);
                }
            }
            BoardEvent::TemplateRequested(request) => {
                let Some(canvas) = self.canvas(tab, cx) else {
                    return;
                };
                let cwd = self
                    .store
                    .session(session)
                    .map(|saved| abbreviate(&saved.cwd))
                    .unwrap_or_default();
                let specs: Vec<NodeSpec> = request
                    .template
                    .nodes
                    .iter()
                    .map(|node| {
                        let code = self
                            .harness(&node.offer)
                            .map_or_else(String::new, |harness| harness.code.clone());
                        NodeSpec {
                            kind: if node.offer == "shell" {
                                NodeKind::Terminal
                            } else {
                                NodeKind::Agent
                            },
                            title: node.title.clone(),
                            tag: code,
                            detail: cwd.clone(),
                            size: None,
                        }
                    })
                    .collect();
                let nodes = canvas.update(cx, |canvas, cx| {
                    canvas.build_template(&request, specs, window, cx)
                });
                for (node, wanted) in nodes.into_iter().zip(&request.template.nodes) {
                    let kind = self.default_kind(&wanted.offer);
                    let pane = self.add_pane_at(
                        session,
                        &wanted.offer,
                        None,
                        kind,
                        Place::Node(node),
                        Seat::In(tab.to_owned()),
                        window,
                        cx,
                    );
                    if let Some(pane) = pane {
                        if let Some(saved) = self.store.pane_mut(&pane) {
                            saved.title = wanted.title.clone();
                        }
                        self.launch(&pane, None, window, cx);
                    }
                }
                self.save(cx);
            }
            BoardEvent::DuplicateRequested { node, x, y } => {
                let Some(original) = self
                    .pane_of(tab, node)
                    .and_then(|pane| self.store.pane(&pane).cloned())
                else {
                    return;
                };
                let pane = self.add_pane_at(
                    session,
                    &original.harness,
                    None,
                    original.kind,
                    Place::At(x, y),
                    Seat::In(tab.to_owned()),
                    window,
                    cx,
                );
                if let Some(pane) = pane {
                    self.launch(&pane, None, window, cx);
                }
            }
            BoardEvent::Renamed { node, title } => {
                if let Some(pane) = self.pane_of(tab, node)
                    && let Some(saved) = self.store.pane_mut(&pane)
                {
                    saved.title = title;
                    // The user's own name for it: it stays.
                    saved.titled = true;
                    self.save(cx);
                }
            }
            BoardEvent::Entered(Some(node)) => {
                if let Some(pane) = self.pane_of(tab, node) {
                    self.store.focus(&pane);
                    if let Some(live) = self.live.get_mut(&pane) {
                        live.seen = true;
                    }
                    cx.notify();
                }
            }
            // A node's waiting messages are let through from its badge.
            BoardEvent::PendingOpened(node) => {
                if let Some(pane) = self.pane_of(tab, node) {
                    self.release_held(tab, &pane, window, cx);
                }
            }
            BoardEvent::EdgeRemoved(edge) => {
                self.held
                    .retain(|held| !(held.tab == tab && held.edge == edge.id));
                self.traffic.remove(&(tab.to_owned(), edge.id));
                self.show_pending(tab, cx);
            }
            BoardEvent::EdgeChanged { after, .. } => {
                // An edge switched from asking to open lets what waits on it through.
                if after.mode == EdgeMode::Auto {
                    let waiting: Vec<Id> = self
                        .held
                        .iter()
                        .filter(|held| held.tab == tab && held.edge == after.id)
                        .map(|held| held.to.clone())
                        .collect();
                    for pane in waiting {
                        self.release_held(tab, &pane, window, cx);
                    }
                }
            }
            BoardEvent::EdgeAdded(_)
            | BoardEvent::NodeRemoved(_)
            | BoardEvent::Entered(None)
            | BoardEvent::SelectionChanged { .. } => {}
        }
    }

    /// One pane said something to another: what its agent ended a turn with,
    /// or what it sent on purpose. The message follows the edge between them,
    /// which is made if they had none, and waits there if the edge asks first.
    /// Returns whether it was delivered now.
    pub(crate) fn pass_along(
        &mut self,
        from: &str,
        to: &str,
        text: &str,
        explicit: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(source), Some(target)) = (self.store.pane(from), self.store.pane(to)) else {
            return true;
        };
        // An edge joins two nodes of one board: panes of different tabs
        // simply talk, with nothing between them to hold a message back.
        let (from_node, to_node) = (source.node, target.node);
        let Some(tab) = self
            .store
            .tab_of(from)
            .filter(|tab| tab.root.contains(to))
            .map(|tab| tab.id.clone())
        else {
            return true;
        };
        let Some(canvas) = self.canvas(&tab, cx) else {
            return true;
        };
        let existing = canvas
            .read(cx)
            .board()
            .edges()
            .iter()
            .find(|edge| edge.from == from_node && edge.to == to_node)
            .cloned();
        let edge = match existing {
            Some(edge) => edge,
            None => {
                // Two panes that talk are connected: the board shows it. The
                // line carries nothing by itself: what the sender answers
                // from then on is not the receiver's unless someone wires it.
                let made = canvas.update(cx, |canvas, cx| {
                    let id = canvas.connect(
                        PortRef::new(from_node, "out"),
                        PortRef::new(to_node, "in"),
                        cx,
                    )?;
                    canvas.update_edge(
                        id,
                        |edge| {
                            edge.origin = kou_canvas::EdgeOrigin::Sent;
                            edge.by = Some(from_node);
                        },
                        cx,
                    );
                    Some(id)
                });
                let Some(edge) = made.and_then(|id| canvas.read(cx).board().edge(id).cloned())
                else {
                    return true;
                };
                edge
            }
        };
        let key = (tab.clone(), edge.id);
        // An edge that is off stops what would flow by itself. What an agent
        // sends on purpose still gets through: off is not a wall between panes.
        if edge.mode == EdgeMode::Off && !explicit {
            return false;
        }
        let traffic = self.traffic.entry(key).or_default();
        if edge.mode == EdgeMode::Approve {
            traffic.waiting += 1;
            let traffic = *traffic;
            self.held.push(Held {
                tab: tab.clone(),
                edge: edge.id,
                from: from.to_owned(),
                to: to.to_owned(),
                text: text.to_owned(),
                hops: self.hops_from(from),
            });
            canvas.update(cx, |canvas, cx| canvas.set_traffic(edge.id, traffic, cx));
            self.show_pending(&tab, cx);
            return false;
        }
        traffic.delivered += 1;
        let traffic = *traffic;
        canvas.update(cx, |canvas, cx| {
            canvas.set_traffic(edge.id, traffic, cx);
            canvas.flow(edge.id, cx);
        });
        true
    }

    // Show on each node how many messages wait for it.
    pub(crate) fn show_pending(&mut self, tab: &str, cx: &mut Context<Self>) {
        let Some((canvas, _)) = self.canvases.get(tab) else {
            return;
        };
        let mut waiting: HashMap<NodeId, Pending> = HashMap::new();
        for held in self.held.iter().filter(|held| held.tab == tab) {
            let (Some(to), from) = (self.store.pane(&held.to), self.store.pane(&held.from)) else {
                continue;
            };
            let pending = waiting.entry(to.node).or_default();
            pending.count += 1;
            pending.awaiting += 1;
            if pending.from.is_none() {
                pending.from = from.map(|from| from.title.clone());
            }
        }
        let nodes: Vec<NodeId> = self
            .store
            .panes_in(tab)
            .into_iter()
            .map(|pane| pane.node)
            .collect();
        canvas.clone().update(cx, |canvas, cx| {
            for node in nodes {
                canvas.set_pending(node, waiting.remove(&node).unwrap_or_default(), cx);
            }
        });
    }

    // Let through everything that waits for a pane.
    fn release_held(&mut self, tab: &str, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (ready, rest): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|held| held.tab == tab && held.to == pane);
        self.held = rest;
        for held in ready {
            if let Some(traffic) = self.traffic.get_mut(&(tab.to_owned(), held.edge)) {
                traffic.waiting = traffic.waiting.saturating_sub(1);
                traffic.delivered += 1;
                let traffic = *traffic;
                if let Some((canvas, _)) = self.canvases.get(tab) {
                    canvas.clone().update(cx, |canvas, cx| {
                        canvas.set_traffic(held.edge, traffic, cx);
                        canvas.flow(held.edge, cx);
                    });
                }
            }
            self.hops.insert(held.to.clone(), held.hops);
            self.deliver(&held.to, &held.text, window, cx);
        }
        self.show_pending(tab, cx);
    }

    /// Close a pane, asking first when its agent is in the middle of something.
    pub(crate) fn request_close(
        &mut self,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_close_of(vec![pane.to_owned()], window, cx);
    }

    /// Close several panes: the ones at rest at once, and the ones whose
    /// agents are in the middle of something after asking, once for them all.
    pub(crate) fn request_close_of(
        &mut self,
        panes: Vec<Id>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (busy, idle): (Vec<Id>, Vec<Id>) = panes.into_iter().partition(|pane| {
            self.live
                .get(pane)
                .is_some_and(|live| matches!(live.mark, Mark::Working | Mark::Blocked))
        });
        for pane in idle {
            self.close_pane(&pane, window, cx);
        }
        let (question, detail, keep) = match busy.as_slice() {
            [] => return,
            [pane] => {
                let title = self
                    .store
                    .pane(pane)
                    .map_or_else(String::new, |pane| pane.title.clone());
                (
                    format!("Stop {title}?"),
                    "It is still working. Closing the pane ends its process.",
                    "Keep it",
                )
            }
            several => (
                format!("Stop {} agents?", several.len()),
                "They are still working. Closing their panes ends their processes.",
                "Keep them",
            ),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &question,
            Some(detail),
            &["Stop and close", keep],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| {
                    for pane in &busy {
                        this.close_pane(pane, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Close a pane: its view is dropped, which ends its process.
    pub(crate) fn close_pane(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Whoever asked it something is told that no answer will come.
        self.forget_asks(pane, window, cx);
        self.forget(pane, cx);
        let canvas = self.canvas_of(pane);
        let Some(saved) = self.store.remove_pane(pane) else {
            return;
        };
        self.live.remove(pane);
        // Its starter keeps what it was, as an agent that is gone.
        if let Some(parent) = &saved.parent {
            self.sync_started(parent, cx);
        }
        self.held
            .retain(|held| held.from != pane && held.to != pane);
        if let Some((tab, canvas)) = canvas {
            if self.store.tab(&tab).is_some() {
                canvas.update(cx, |canvas, cx| canvas.remove(saved.node, window, cx));
            } else {
                // Its last pane took the tab with it, and the tab its canvas.
                self.canvases.remove(&tab);
                self.held.retain(|held| held.tab != tab);
                self.traffic.retain(|(of, _), _| *of != tab);
            }
        }
        self.save(cx);
    }

    pub(crate) fn toggle_rail(&mut self, cx: &mut Context<Self>) {
        self.store.rail_hidden = !self.store.rail_hidden;
        self.save(cx);
    }

    /// Choose how agents started from now on are given the means to work
    /// with the others. What already runs keeps what it was started with.
    pub(crate) fn set_orchestration(
        &mut self,
        way: crate::state::Orchestration,
        cx: &mut Context<Self>,
    ) {
        self.store.orchestration = way;
        self.save(cx);
    }

    pub(crate) fn toggle_cursor(&mut self, cx: &mut Context<Self>) {
        self.store.smooth_cursor = !self.store.smooth_cursor;
        self.retheme(cx);
        self.save(cx);
    }

    /// The theme for a ground, dark or light, with the accent the user chose.
    pub(crate) fn themed(&self, dark: bool) -> kou_ui::Theme {
        self.appearance_theme(dark)
    }

    /// Take `accent` for links, focus, what is selected and an agent at
    /// work, or the palette's own with `None`: everything on screen is drawn
    /// again in it.
    pub(crate) fn set_accent(&mut self, accent: Option<u32>, cx: &mut Context<Self>) {
        self.store.accent = accent.map(|rgb| format!("#{:06x}", rgb & 0xff_ffff));
        let dark = kou_ui::theme(cx).dark;
        cx.set_global(self.themed(dark));
        self.retheme(cx);
        cx.refresh_windows();
        self.save(cx);
    }

    /// Whether a page (the settings, usage) stands where the workspace
    /// would.
    pub(crate) fn page_open(&self) -> bool {
        self.settings_open || self.usage_open || self.plugin_page.is_some()
    }

    /// Back to the workspace from a page. Whether one was open.
    pub(crate) fn close_pages(&mut self) -> bool {
        self.usage_tick = None;
        // (Both are taken, not the first found.)
        let settings = std::mem::take(&mut self.settings_open);
        let usage = std::mem::take(&mut self.usage_open);
        let plugin = self.plugin_page.take().is_some();
        settings || usage || plugin
    }

    /// Show the settings where the workspace is.
    pub(crate) fn show_settings(&mut self, cx: &mut Context<Self>) {
        self.close_pages();
        self.settings_open = true;
        // (What is left to each account, and whether anybody is signed in.)
        if !self.store.profiles.is_empty() {
            self.refresh_usage(false, cx);
        }
        cx.notify();
    }

    /// The settings, or back from them.
    pub(crate) fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            self.close_pages();
            self.see_focused();
            cx.notify();
        } else {
            self.show_settings(cx);
        }
        self.picker = None;
    }

    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) {
        // A list closes before what it belongs to does, and the account
        // form before the settings it is in.
        if std::mem::take(&mut self.new_menu)
            || std::mem::take(&mut self.ask_menu)
            || self.account_form.take().is_some()
        {
            cx.notify();
            return;
        }
        // A picker behind another tab is not what Escape is about.
        let here = self
            .store
            .current()
            .map(|session| session.id.clone())
            .unwrap_or_default();
        // A page (the settings, usage) covers the workspace: it goes before
        // what it covers does.
        if self.tab_edit.take().is_some()
            || self.name_edit.take().is_some()
            || self.close_pages()
            || self.resuming.take().is_some()
            || self
                .picker
                .take_if(|picker| picker.front && picker.session == here)
                .is_some()
        {
            self.see_focused();
            cx.notify();
        } else {
            // Nothing of ours was open: the key belongs to whoever is under it.
            cx.propagate();
        }
    }

    /// Handle what could not be handled where it was asked for. Called at the
    /// start of each frame, where the window is at hand.
    pub(crate) fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.answer_deferred(window, cx);
        let closing = std::mem::take(&mut self.close_requested);
        if !closing.is_empty() {
            self.request_close_of(closing, window, cx);
        }
        for (session, event) in std::mem::take(&mut self.board_events) {
            self.board_event(&session, event, window, cx);
        }
        // An empty screen's prompt field takes the keys when it comes on
        // screen, once: the first thing to do there is to type.
        let asking = self.asking();
        if asking {
            let field = self.prompt_field(window, cx);
            // Its file links are looked up in the folder the agent will work in.
            let picker = self.picker.as_ref().filter(|_| self.picker_shown());
            let project = picker
                .and_then(|p| p.beside.as_ref())
                .and_then(|(pane, _)| self.store.pane(pane))
                .or_else(|| picker.and_then(|p| self.store.pane(p.parent.as_deref()?)))
                .map(|pane| pane.cwd.clone())
                .or_else(|| self.store.current().map(|session| session.cwd.clone()));
            if project != self.prompt_project {
                self.prompt_project = project.clone();
                field.update(cx, |field, cx| field.set_project(project, cx));
            }
            if !self.asked {
                field.update(cx, |field, cx| field.focus(window, cx));
            }
            self.offer_models(&field, cx);
            // A prompt that waited for a folder goes to the agent now that
            // there is one; one that waited long was given up.
            if self.store.current().is_some()
                && let Some((prompt, at)) = self.ask_pending.take()
                && at.elapsed() < Duration::from_secs(120)
            {
                self.ask(Some(prompt), window, cx);
            }
        }
        self.asked = asking;
        self.revive_ready(window, cx);
        self.give_keys(window, cx);
    }

    /// Start again the conversations a rebuilt window is to carry on with,
    /// and those whose Claude Code went on while the app was closed: each
    /// by its session, once it is known what is installed. One whose
    /// harness has not been looked for yet waits for that: started before,
    /// it would not be found.
    pub(crate) fn revive_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.revive.is_empty() && !self.scanning && !self.harnesses.is_empty() {
            let (now, later): (Vec<Id>, Vec<Id>) = std::mem::take(&mut self.revive)
                .into_iter()
                .partition(|pane| {
                    self.store.pane(pane).is_none_or(|saved| {
                        saved.harness == "kou"
                            || self
                                .detected
                                .iter()
                                .any(|detected| detected.spec.id == saved.harness)
                    })
                });
            self.revive = later;
            for pane in now {
                if self.store.pane(&pane).is_some() && !self.threads.contains_key(&pane) {
                    self.launch(&pane, None, window, cx);
                }
            }
        }
    }

    // The pane that should have the keys: the focused one of the tab on
    // screen, when panes are what is on screen.
    fn keys_wanted(&self) -> Option<Id> {
        let session = self.store.current()?;
        let covered = self.page_open()
            || self.tab_edit.is_some()
            || self.name_edit.is_some()
            || self.resuming.is_some()
            || self
                .picker
                .as_ref()
                .is_some_and(|picker| picker.session == session.id && picker.front);
        if covered || session.layout() != Layout::Panes {
            return None;
        }
        session.focused.clone()
    }

    // Marking a pane as the one in front is not giving it the keys. When the
    // pane in front changes for a reason other than a click in it (its
    // neighbour closed, a split was made, a tab was chosen, a key walked to
    // it), the keys go where the mark went: into its terminal or its
    // conversation, or to the window when nothing runs in it, so that the
    // window's own keys still work. A change does this, and so does finding
    // the keys nowhere at all (what had them is gone): then no key would
    // reach the window. Keys the user took elsewhere (the rail, a field) are
    // not taken back.
    fn give_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = self.keys_wanted();
        let nowhere = window.focused(cx).is_none();
        if wanted == self.keys_at && !nowhere {
            return;
        }
        self.keys_at = wanted.clone();
        let handle = wanted
            .and_then(|pane| self.body_focus(&pane, cx))
            .unwrap_or_else(|| self.focus.clone());
        if nowhere || self.keys_at.is_some() {
            window.focus(&handle, cx);
        }
    }

    pub(crate) fn on_close_pane(
        &mut self,
        _: &ClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The tab in front is what closes: the one waiting for its agent
        // when that is it, not the pane behind it. A half that is still
        // being chosen for closes before the pane it was taken from.
        // A page over the workspace is what is in front: it closes, not a
        // pane behind it that cannot be seen.
        if self.close_pages() {
            cx.notify();
            return;
        }
        if self.pending_in_front() {
            return self.close_pending_tab(cx);
        }
        if self.choosing_beside().is_some() {
            self.picker = None;
            cx.notify();
            return;
        }
        let focused = self
            .store
            .current()
            .and_then(|session| session.focused.clone());
        if let Some(pane) = focused {
            self.request_close(&pane, window, cx);
        }
    }

    pub(crate) fn on_next_pane(&mut self, _: &NextPane, _: &mut Window, cx: &mut Context<Self>) {
        self.step_pane(true, cx);
    }

    pub(crate) fn on_previous_pane(
        &mut self,
        _: &PreviousPane,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_pane(false, cx);
    }
}

// One step along a ring of `count` places.
fn step(at: usize, count: usize, forward: bool) -> usize {
    if forward {
        (at + 1) % count
    } else {
        (at + count - 1) % count
    }
}

/// A path as the interface shows it: the home directory as `~`.
pub fn abbreviate(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

/// A board holding one node per pane, for a store built by hand.
pub fn board_for(panes: &[(&str, &str, Option<usize>)]) -> (Board, Vec<kou_canvas::NodeId>) {
    let mut board = Board::new();
    let mut ids = Vec::new();
    for (title, tag, parent) in panes {
        let spec = NodeSpec {
            kind: NodeKind::Agent,
            title: (*title).to_owned(),
            tag: (*tag).to_owned(),
            detail: String::new(),
            size: None,
        };
        let id = match parent.and_then(|parent| ids.get(parent).copied()) {
            Some(parent) => board.add_child(parent, spec).expect("the parent exists"),
            None => {
                let below = board.bounds().map_or(0., |bounds| bounds.bottom() + 160.);
                board.add_at(spec, 0., below)
            }
        };
        ids.push(id);
    }
    (board, ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Layout, PaneKind, Store};
    use gpui::{Entity, TestAppContext, VisualTestContext};

    fn open(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext, Id) {
        cx.update(|cx| {
            cx.set_global(kou_ui::Theme::new(true));
            bind_keys(cx);
        });
        let mut store = Store::default();
        let space = store.add_space("/fixture/atlas".into());
        let first = store.add_session(&space).unwrap();
        store.add_session(&space).unwrap();
        store.selected = Some(first.clone());
        let (shell, cx) = cx.add_window_view(|_, cx| {
            let mut shell = Shell::new(store, None, cx);
            // What a pane's launch writes (settings, shell integration)
            // goes to a folder of the tests' own, never the app's.
            shell.home =
                std::env::temp_dir().join(format!("kou-desktop-tests-{}", std::process::id()));
            shell.harnesses = vec![HarnessInfo {
                id: "shell".into(),
                name: "Shell".into(),
                code: "SH".into(),
                ready: true,
                version: None,
                message: None,
                native: false,
                adapter: None,
            }];
            shell
        });
        shell.update_in(cx, |shell, window, cx| window.focus(&shell.focus, cx));
        cx.run_until_parked();
        (shell, cx, first)
    }

    #[gpui::test]
    fn an_accent_chosen_in_the_settings_is_the_theme_s_at_once_and_is_kept(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, _) = open(cx);
        shell.update(cx, |shell, _| {
            shell.store.theme = kou_ui::themes::Selection::System
        });
        let before = cx.update(|_, cx| kou_ui::theme(cx).palette);
        assert_eq!(before.accent, kou_ui::DARK.accent);
        shell.update(cx, |shell, cx| shell.set_accent(Some(0x4d9fe0), cx));
        let after = cx.update(|_, cx| kou_ui::theme(cx).palette);
        assert_eq!(after.accent, 0x4d9fe0);
        // An agent at work wears the accent, as it does without a choice.
        assert_eq!((before.working, after.working), (before.accent, 0x4d9fe0));
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.accent.clone()),
            Some("#4d9fe0".to_owned())
        );
        // A terminal started from now on selects in it.
        let style = cx.update(|_, cx| crate::panes::terminal_style(kou_ui::theme(cx), true));
        assert_eq!(
            style.selection,
            gpui::Hsla::from(gpui::rgb(0x4d9fe0)).opacity(0.3)
        );
        // The palette's own again.
        shell.update(cx, |shell, cx| shell.set_accent(None, cx));
        assert_eq!(cx.update(|_, cx| kou_ui::theme(cx).palette), before);
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.accent.clone()),
            None
        );
    }

    // Two frames: the first measures the pane area, the second lays out in it.
    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..2 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        cx.run_until_parked();
    }

    fn start(shell: &Entity<Shell>, session: &str, cx: &mut VisualTestContext) -> Id {
        shell.update_in(cx, |shell, window, cx| {
            shell
                .add_pane(session, "shell", None, PaneKind::Terminal, window, cx)
                .unwrap()
        })
    }

    // A pane in the tab `at` is in, to its right.
    fn start_beside(shell: &Entity<Shell>, at: &str, cx: &mut VisualTestContext) -> Id {
        shell.update_in(cx, |shell, window, cx| {
            let session = shell.store.pane(at).unwrap().session.clone();
            let seat = Seat::Beside {
                at: at.to_owned(),
                axis: Axis::Row,
            };
            shell
                .add_pane_at(
                    &session,
                    "shell",
                    None,
                    PaneKind::Terminal,
                    Place::Auto,
                    seat,
                    window,
                    cx,
                )
                .unwrap()
        })
    }

    // The canvas of the tab a pane is in, and that tab's id.
    fn canvas_with(
        shell: &Entity<Shell>,
        pane: &str,
        cx: &mut VisualTestContext,
    ) -> (Id, Entity<CanvasView>) {
        shell.update(cx, |shell, cx| {
            let tab = shell.store.tab_of(pane).unwrap().id.clone();
            let canvas = shell.canvas(&tab, cx).unwrap();
            (tab, canvas)
        })
    }

    #[gpui::test]
    fn only_a_tab_s_orchestrator_wires_it_and_what_it_wires_says_so(cx: &mut TestAppContext) {
        use crate::wires::EdgeChange;
        use kou_ipc::Response;
        let (shell, cx, session) = open(cx);
        let a = start(&shell, &session, cx);
        let b = start_beside(&shell, &a, cx);
        let c = start_beside(&shell, &b, cx);
        let (_, canvas) = canvas_with(&shell, &a, cx);
        let node = |pane: &str, cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| shell.store.pane(pane).unwrap().node)
        };
        let change =
            |by: &str, from: &str, to: &str, change: EdgeChange, cx: &mut VisualTestContext| {
                shell.update(cx, |shell, cx| shell.change_edge(by, from, to, change, cx))
            };
        let edges = |response: Response| match response {
            Response::Edges { edges, said } => (edges, said),
            other => panic!("{other:?}"),
        };
        let refusal = |response: Response| match response {
            Response::Error { message } => message,
            other => panic!("{other:?}"),
        };
        // Nobody wires a tab but the user, until the user says who else may.
        let said = refusal(change(&a, &b, &a, EdgeChange::Add(None), cx));
        assert!(said.contains("not this tab's orchestrator"), "{said}");
        assert!(canvas.read_with(cx, |canvas, _| canvas.board().edges().is_empty()));
        // Anyone may look.
        let (listed, _) = edges(shell.update(cx, |shell, cx| shell.list_edges(&b, cx)));
        assert!(listed.is_empty());
        assert!(!shell.update(cx, |shell, cx| shell.place(&a, cx).unwrap().orchestrator));
        // The user makes one node the orchestrator: its agent wires the
        // tab, any two nodes of it, and the wire says who drew it.
        let lead = node(&a, cx);
        canvas.update(cx, |canvas, cx| {
            canvas.update_node(lead, |node| node.orchestrator = true, cx)
        });
        assert!(shell.update(cx, |shell, cx| shell.place(&a, cx).unwrap().orchestrator));
        let (listed, said) = edges(change(&a, &b, &c, EdgeChange::Add(None), cx));
        assert!(said.is_empty(), "{said:?}");
        assert_eq!(
            listed,
            [kou_ipc::Edge {
                from: b.clone(),
                to: c.clone(),
                wired: true,
                by: Some(a.clone()),
                ..kou_ipc::Edge::default()
            }]
        );
        canvas.read_with(cx, |canvas, _| {
            let edge = &canvas.board().edges()[0];
            assert_eq!(
                (edge.origin, edge.by),
                (kou_canvas::EdgeOrigin::Wired, Some(lead))
            );
        });
        // Still nobody else's to change.
        assert!(refusal(change(&b, &b, &c, EdgeChange::Remove, cx)).contains("orchestrator"));
        // The wire back closes a ring: made, and said.
        let (listed, said) = edges(change(&a, &c, &b, EdgeChange::Add(None), cx));
        assert_eq!(listed.len(), 2);
        assert!(
            said.len() == 1
                && said[0].contains(&format!("{b} -> {c} -> {b}"))
                && said[0].contains("waits on its target for the user"),
            "{said:?}"
        );
        // Asked for again, it is the same wire, in the mode asked for now.
        let (listed, said) = edges(change(
            &a,
            &c,
            &b,
            EdgeChange::Add(Some(kou_ipc::EdgeMode::Approve)),
            cx,
        ));
        assert_eq!(listed.len(), 2);
        assert!(said[0].contains("was wired already"), "{said:?}");
        assert_eq!(listed[1].mode, kou_ipc::EdgeMode::Approve);
        // A mode is set on a wire that is there, and a wire taken away.
        assert!(
            refusal(change(
                &a,
                &a,
                &c,
                EdgeChange::Set(kou_ipc::EdgeMode::Off),
                cx
            ))
            .contains("is not wired")
        );
        let (listed, _) = edges(change(
            &a,
            &b,
            &c,
            EdgeChange::Set(kou_ipc::EdgeMode::Off),
            cx,
        ));
        assert_eq!(listed[0].mode, kou_ipc::EdgeMode::Off);
        let (listed, _) = edges(change(&a, &b, &c, EdgeChange::Remove, cx));
        assert_eq!(listed.len(), 1);
        assert_eq!(
            (listed[0].from.as_str(), listed[0].to.as_str()),
            (c.as_str(), b.as_str())
        );
        // What is not on this tab's canvas is not its to wire.
        assert!(refusal(change(&a, "p999", &b, EdgeChange::Add(None), cx)).contains("no p999"));
        assert!(refusal(change(&a, &b, &b, EdgeChange::Add(None), cx)).contains("itself"));
        // A line that shows one wrote to the other becomes the wire, and a
        // wire between a starter and what it started goes back to the line.
        assert!(shell.update(cx, |shell, cx| shell.pass_along(&a, &b, "hello", true, cx)));
        let (listed, _) = edges(shell.update(cx, |shell, cx| shell.list_edges(&a, cx)));
        assert!(
            listed
                .iter()
                .any(|edge| edge.from == a && edge.to == b && edge.sent)
        );
        let child = node(&b, cx);
        canvas.update(cx, |canvas, cx| {
            canvas.update_node(child, |node| node.parent = Some(lead), cx)
        });
        let (listed, _) = edges(change(&a, &a, &b, EdgeChange::Add(None), cx));
        let wire = listed
            .iter()
            .find(|edge| edge.from == a && edge.to == b)
            .unwrap();
        assert!(wire.wired && !wire.sent && wire.by.as_deref() == Some(a.as_str()));
        let (listed, _) = edges(change(&a, &a, &b, EdgeChange::Remove, cx));
        let line = listed
            .iter()
            .find(|edge| edge.from == a && edge.to == b)
            .unwrap();
        assert!(!line.wired && !line.sent && line.by.is_none(), "{line:?}");
        // The right is the user's to take back.
        canvas.update(cx, |canvas, cx| {
            canvas.update_node(lead, |node| node.orchestrator = false, cx)
        });
        assert!(refusal(change(&a, &b, &c, EdgeChange::Add(None), cx)).contains("orchestrator"));
    }

    #[gpui::test]
    fn a_question_is_answered_once_and_a_message_passed_on_too_often_waits_for_the_user(
        cx: &mut TestAppContext,
    ) {
        use kou_thread::{Block, Capabilities, Command, Stop, Update};
        let (shell, cx, session) = open(cx);
        shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: true,
                adapter: None,
            });
        });
        // Two chats in one tab, each with an agent the test plays.
        let chat = |beside: Option<&str>, cx: &mut VisualTestContext| {
            shell.update_in(cx, |shell, window, cx| {
                let pane = match beside {
                    None => shell
                        .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                        .unwrap(),
                    Some(at) => shell
                        .add_pane_at(
                            &session,
                            "claude",
                            None,
                            PaneKind::Thread,
                            Place::Auto,
                            Seat::Beside {
                                at: at.to_owned(),
                                axis: Axis::Row,
                            },
                            window,
                            cx,
                        )
                        .unwrap(),
                };
                let saved = shell.store.pane(&pane).unwrap().clone();
                let (backend, end) = kou_thread::channel();
                shell.install_thread(&saved, backend, "Claude Code".into(), window, cx);
                let _ = end.updates.try_send(Update::Ready {
                    session: format!("sess-{pane}"),
                    capabilities: Capabilities::default(),
                    modes: Vec::new(),
                    mode: None,
                    models: Vec::new(),
                    model: None,
                });
                (pane, end)
            })
        };
        let (p, p_end) = chat(None, cx);
        let (q, q_end) = chat(Some(&p), cx);
        cx.run_until_parked();
        let prompts = |end: &kou_thread::BackendEnd| {
            let mut texts = Vec::new();
            while let Ok(command) = end.commands.try_recv() {
                if let Command::Prompt(blocks) = command {
                    texts.extend(blocks.into_iter().filter_map(|block| match block {
                        Block::Text(text) => Some(text),
                        _ => None,
                    }));
                }
            }
            texts
        };
        // (A turn begins, and later ends: the app sees it at work between.)
        let answers = |end: &kou_thread::BackendEnd, text: &str, cx: &mut VisualTestContext| {
            let _ = end.updates.try_send(Update::TurnStarted);
            cx.run_until_parked();
            let _ = end.updates.try_send(Update::AssistantChunk(text.into()));
            let _ = end.updates.try_send(Update::TurnEnded(Stop::Finished));
            cx.run_until_parked();
        };
        // One asks the other: the question arrives saying whose it is and
        // that the answer goes back by itself; nothing is wired.
        let asked = shell.update_in(cx, |shell, window, cx| {
            shell.ask_pane(&p, &q, "How far are you?", window, cx)
        });
        assert!(matches!(asked, kou_ipc::Response::Text { .. }), "{asked:?}");
        cx.run_until_parked();
        let got = prompts(&q_end);
        assert_eq!(got.len(), 1);
        assert!(
            got[0].starts_with(&format!("[canvas] question from «Claude Code» ({p}):\n"))
                && got[0].ends_with("\nHow far are you?"),
            "{got:?}"
        );
        let (_, canvas) = canvas_with(&shell, &p, cx);
        assert!(canvas.read_with(cx, |canvas, _| canvas.board().edges().is_empty()));
        // Its answer comes back once, as a message; the next does not.
        answers(&q_end, "Half way.", cx);
        assert_eq!(
            prompts(&p_end),
            [format!(
                "[canvas] answer from «Claude Code» ({q}), hop 2 of 6:\nHalf way."
            )]
        );
        answers(&q_end, "Done now.", cx);
        assert!(prompts(&p_end).is_empty());
        assert!(shell.read_with(cx, |shell, _| shell.asks.is_empty()));
        // A pane that is gone before it answers says so to who asked.
        // (Asked of a terminal, there is no answer to hand back at all.)
        let terminal = start(&shell, &session, cx);
        let refused = shell.update_in(cx, |shell, window, cx| {
            shell.ask_pane(&p, &terminal, "?", window, cx)
        });
        assert!(
            matches!(refused, kou_ipc::Response::Error { .. }),
            "{refused:?}"
        );

        // The user wires the two into a ring. What one answers goes to the
        // other, counted from the second time on, and past the limit it
        // waits for the user instead of going round for ever.
        let (pn, qn) = shell.read_with(cx, |shell, _| {
            (
                shell.store.pane(&p).unwrap().node,
                shell.store.pane(&q).unwrap().node,
            )
        });
        canvas.update(cx, |canvas, cx| {
            canvas.connect(PortRef::new(pn, "out"), PortRef::new(qn, "in"), cx);
            canvas.connect(PortRef::new(qn, "out"), PortRef::new(pn, "in"), cx);
        });
        let _ = prompts(&p_end);
        let _ = prompts(&q_end);
        // (What the first one answers now, it answers to the user: the
        // answer it was sent above is behind it.)
        shell.update(cx, |shell, _| shell.hops.clear());
        answers(&p_end, "ping", cx);
        let first = prompts(&q_end);
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(
            first[0].ends_with("):\nping") && !first[0].contains("hop"),
            "{first:?}"
        );
        let mut said = Vec::new();
        for round in 0..8 {
            let (from, to) = if round % 2 == 0 {
                (&q_end, &p_end)
            } else {
                (&p_end, &q_end)
            };
            answers(from, "pong", cx);
            let got = prompts(to);
            if got.is_empty() {
                break;
            }
            said.extend(got);
        }
        // Hops 2 to 6 went round; the seventh was held.
        assert_eq!(said.len(), 5, "{said:?}");
        assert!(
            said[0].contains(", hop 2 of 6:") && said[4].contains(", hop 6 of 6:"),
            "{said:?}"
        );
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.held.len(), 1);
            assert!(shell.held[0].text.contains("hop 7 of 6"));
            assert!(
                shell
                    .notice
                    .as_ref()
                    .is_some_and(|notice| notice.contains("waits on")),
                "{:?}",
                shell.notice
            );
        });
        std::mem::forget((p_end, q_end));
    }

    #[gpui::test]
    fn a_title_a_pane_was_given_stays_when_its_agent_names_the_session(cx: &mut TestAppContext) {
        use kou_thread::{Capabilities, Update};
        let (shell, cx, session) = open(cx);
        shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: true,
                adapter: None,
            });
        });
        let (pane, end) = shell.update_in(cx, |shell, window, cx| {
            let pane = shell
                .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                .unwrap();
            let saved = shell.store.pane(&pane).unwrap().clone();
            let (backend, end) = kou_thread::channel();
            shell.install_thread(&saved, backend, "Claude Code".into(), window, cx);
            (pane, end)
        });
        let title = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.store.pane(&pane).unwrap().title.clone()
            })
        };
        let _ = end.updates.try_send(Update::Ready {
            session: "sess-1".into(),
            capabilities: Capabilities::default(),
            modes: Vec::new(),
            mode: None,
            models: Vec::new(),
            model: None,
        });
        // Left to itself, the pane is called what its agent calls the session.
        let _ = end
            .updates
            .try_send(Update::Title("Fix the login test".into()));
        cx.run_until_parked();
        assert_eq!(title(cx), "Fix the login test");
        // Given a name (by who started it, or by the user), it keeps it.
        shell.update(cx, |shell, _| {
            let saved = shell.store.pane_mut(&pane).unwrap();
            saved.title = "Reviewer".into();
            saved.titled = true;
        });
        let _ = end
            .updates
            .try_send(Update::Title("Branch agent-v2.1".into()));
        cx.run_until_parked();
        assert_eq!(title(cx), "Reviewer");
        std::mem::forget(end);
    }

    #[gpui::test]
    fn deleting_several_selected_nodes_closes_every_one_of_them(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        let first = start(&shell, &session, cx);
        shell.update(cx, |shell, cx| shell.set_layout(Layout::Canvas, cx));
        draw(cx);
        let (tab, canvas) = canvas_with(&shell, &first, cx);
        // Two more panes beside the first: three nodes on the tab's canvas.
        for x in [1200., 2400.] {
            canvas.update(cx, |_, cx| {
                cx.emit(BoardEvent::AddRequested(AddRequest {
                    offer: "shell".into(),
                    x,
                    y: 80.,
                    wire: None,
                }));
            });
            draw(cx);
        }
        let nodes: Vec<(Id, NodeId)> = shell.read_with(cx, |shell, _| {
            shell
                .store
                .panes_in(&tab)
                .into_iter()
                .map(|pane| (pane.id.clone(), pane.node))
                .collect()
        });
        assert_eq!(nodes.len(), 3);
        // The canvas asks about each node of a deleted selection in turn,
        // within one frame: both go, not the last one asked about alone.
        canvas.update(cx, |_, cx| {
            cx.emit(CanvasEvent::RemoveRequested(nodes[1].1));
            cx.emit(CanvasEvent::RemoveRequested(nodes[2].1));
        });
        draw(cx);
        shell.read_with(cx, |shell, _| {
            let left: Vec<Id> = shell
                .store
                .panes_in(&tab)
                .into_iter()
                .map(|pane| pane.id.clone())
                .collect();
            assert_eq!(left, [nodes[0].0.clone()]);
        });
        canvas.read_with(cx, |canvas, _| {
            assert_eq!(canvas.board().nodes().len(), 1);
        });
    }

    #[gpui::test]
    fn what_the_user_does_on_the_canvas_starts_renames_and_copies_panes(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        // A canvas is a tab's: the session's first pane makes the tab.
        let first = start(&shell, &session, cx);
        shell.update(cx, |shell, cx| shell.set_layout(Layout::Canvas, cx));
        draw(cx);
        let (tab, canvas) = canvas_with(&shell, &first, cx);

        // Added where the user asked, from what the canvas was offered.
        canvas.update(cx, |_, cx| {
            cx.emit(BoardEvent::AddRequested(AddRequest {
                offer: "shell".into(),
                x: 1200.,
                y: 80.,
                wire: None,
            }));
        });
        draw(cx);
        let (pane, node) = shell.read_with(cx, |shell, _| {
            // It is a pane of the same tab: a node of this canvas, a part of its split.
            let panes = shell.store.panes_in(&tab);
            assert_eq!(panes.len(), 2);
            assert_eq!(shell.store.session(&session).unwrap().tabs.len(), 1);
            assert_eq!(panes[1].harness, "shell");
            (panes[1].id.clone(), panes[1].node)
        });
        canvas.read_with(cx, |canvas, _| {
            let rect = canvas.board().node(node).unwrap().rect;
            assert!(
                rect.contains(1200., 80.),
                "{rect:?} is where it was asked for"
            );
        });

        // Renamed on the board, renamed everywhere.
        canvas.update(cx, |_, cx| {
            cx.emit(BoardEvent::Renamed {
                node,
                title: "reviewer".into(),
            });
        });
        draw(cx);
        assert_eq!(
            shell.read_with(cx, |shell, _| shell
                .store
                .pane(&pane)
                .unwrap()
                .title
                .clone()),
            "reviewer"
        );

        // A copy is another pane of the same kind, with its own node.
        canvas.update(cx, |_, cx| {
            cx.emit(BoardEvent::DuplicateRequested {
                node,
                x: 2400.,
                y: 80.,
            });
        });
        draw(cx);
        shell.read_with(cx, |shell, _| {
            let panes = shell.store.panes_in(&tab);
            assert_eq!(panes.len(), 3);
            assert_eq!(panes[2].harness, "shell");
            assert_ne!(panes[2].node, node);
            assert_eq!(shell.store.session(&session).unwrap().tabs.len(), 1);
        });
        assert_eq!(
            canvas.read_with(cx, |canvas, _| canvas.board().nodes().len()),
            3
        );

        // Another tab is another canvas, with its own pane and nothing of this one.
        let elsewhere = start(&shell, &session, cx);
        let (other, theirs) = canvas_with(&shell, &elsewhere, cx);
        assert_ne!(other, tab);
        assert_eq!(
            theirs.read_with(cx, |canvas, _| canvas.board().nodes().len()),
            1
        );
        // What is asked for on a tab's canvas through the picker joins that tab.
        canvas.update(cx, |_, cx| {
            cx.emit(CanvasEvent::CreateRequested { x: 0., y: 0. });
        });
        draw(cx);
        shell.update_in(cx, |shell, window, cx| {
            assert_eq!(
                shell.picker.as_ref().unwrap().tab.as_deref(),
                Some(tab.as_str())
            );
            shell.start("shell", PaneKind::Terminal, window, cx);
            assert_eq!(shell.store.panes_in(&tab).len(), 4);
            assert_eq!(shell.store.panes_in(&other).len(), 1);
        });
    }

    #[gpui::test]
    fn a_message_between_panes_follows_their_edge_and_waits_on_one_that_asks(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        let a = start(&shell, &session, cx);
        let b = start_beside(&shell, &a, cx);
        let (tab, canvas) = canvas_with(&shell, &a, cx);
        assert!(canvas.read_with(cx, |canvas, _| canvas.board().edges().is_empty()));

        // Panes of different tabs are on different canvases: what one says
        // to the other simply arrives, with no edge to follow or wait on.
        let apart = start(&shell, &session, cx);
        assert!(shell.update(cx, |shell, cx| {
            shell.pass_along(&a, &apart, "hello", false, cx)
        }));
        assert!(canvas.read_with(cx, |canvas, _| canvas.board().edges().is_empty()));
        assert!(shell.read_with(cx, |shell, _| shell.traffic.is_empty()));

        // Two panes of a tab that talk are connected: the board shows it and counts it.
        assert!(shell.update(cx, |shell, cx| shell.pass_along(&a, &b, "one", true, cx)));
        let edge = canvas.read_with(cx, |canvas, _| canvas.board().edges()[0].clone());
        let key = (tab.clone(), edge.id);
        let traffic =
            |cx: &mut VisualTestContext| shell.read_with(cx, |shell, _| shell.traffic[&key]);
        assert_eq!((traffic(cx).delivered, traffic(cx).waiting), (1, 0));

        // An edge set to ask first holds what comes, and says so on the node.
        canvas.update(cx, |canvas, cx| {
            canvas.update_edge(edge.id, |edge| edge.mode = EdgeMode::Approve, cx);
        });
        assert!(!shell.update(cx, |shell, cx| shell.pass_along(&a, &b, "two", true, cx)));
        assert_eq!((traffic(cx).delivered, traffic(cx).waiting), (1, 1));
        assert_eq!(shell.read_with(cx, |shell, _| shell.held.len()), 1);

        // Let through from the node's badge.
        let node = shell.read_with(cx, |shell, _| shell.store.pane(&b).unwrap().node);
        canvas.update(cx, |_, cx| cx.emit(BoardEvent::PendingOpened(node)));
        draw(cx);
        assert!(shell.read_with(cx, |shell, _| shell.held.is_empty()));
        assert_eq!((traffic(cx).delivered, traffic(cx).waiting), (2, 0));

        // Off stops what flows by itself, not what is sent on purpose.
        canvas.update(cx, |canvas, cx| {
            canvas.update_edge(edge.id, |edge| edge.mode = EdgeMode::Off, cx);
        });
        assert!(!shell.update(cx, |shell, cx| shell.pass_along(&a, &b, "three", false, cx)));
        assert!(shell.read_with(cx, |shell, _| shell.held.is_empty()));
        assert_eq!(traffic(cx).delivered, 2);
        assert!(shell.update(cx, |shell, cx| shell.pass_along(&a, &b, "four", true, cx)));
        assert_eq!(traffic(cx).delivered, 3);
    }

    #[gpui::test]
    fn an_agent_that_asks_is_told_its_tab_its_neighbours_and_the_canvas_s_edges(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        let a = start(&shell, &session, cx);
        let b = start_beside(&shell, &a, cx);
        let (tab, canvas) = canvas_with(&shell, &a, cx);
        let place = |pane: &str, cx: &mut VisualTestContext| {
            shell.update(cx, |shell, cx| shell.place(pane, cx))
        };
        // Its own pane, its tab with both panes, the other one beside it;
        // nothing joins them yet.
        let found = place(&a, cx).expect("a pane of the app's");
        assert_eq!(
            (found.pane.id.as_str(), found.tab.id.as_str()),
            (a.as_str(), tab.as_str())
        );
        assert_eq!(found.tab.panes, [a.clone(), b.clone()]);
        assert_eq!(
            found
                .others
                .iter()
                .map(|pane| pane.id.as_str())
                .collect::<Vec<_>>(),
            [b.as_str()]
        );
        assert_eq!(found.session, "atlas");
        assert!(found.edges.is_empty() && found.notes.is_empty());
        // One says something to the other: a line from it that shows they
        // talk, which it made and which carries nothing by itself.
        assert!(shell.update(cx, |shell, cx| shell.pass_along(&a, &b, "one", true, cx)));
        let edge = canvas.read_with(cx, |canvas, _| canvas.board().edges()[0].id);
        let made = place(&a, cx).unwrap().edges;
        assert_eq!(
            made,
            [kou_ipc::Edge {
                from: a.clone(),
                to: b.clone(),
                sent: true,
                by: Some(a.clone()),
                ..kou_ipc::Edge::default()
            }]
        );
        assert!(
            kou_ipc::cli::describe_place(&place(&a, cx).unwrap(), true)
                .contains(&format!("made when you wrote to {b} on purpose"))
        );
        // Wired by the user and set to ask first, what comes next waits on
        // it and is counted.
        canvas.update(cx, |canvas, cx| {
            canvas.update_edge(
                edge,
                |edge| {
                    edge.mode = EdgeMode::Approve;
                    edge.origin = kou_canvas::EdgeOrigin::Wired;
                    edge.by = None;
                },
                cx,
            );
        });
        assert!(!shell.update(cx, |shell, cx| shell.pass_along(&a, &b, "two", true, cx)));
        let wired = kou_ipc::Edge {
            from: a.clone(),
            to: b.clone(),
            mode: kou_ipc::EdgeMode::Approve,
            wired: true,
            waiting: 1,
            ..kou_ipc::Edge::default()
        };
        // Both ends are told of the same edge, each from where it stands.
        assert_eq!(place(&a, cx).unwrap().edges, std::slice::from_ref(&wired));
        let other = place(&b, cx).unwrap();
        assert_eq!(other.edges, [wired]);
        let said = kou_ipc::cli::describe_place(&other, true);
        assert!(
            said.contains(&format!("  {a} -> {b}\twired, approve: "))
                && said.contains(" waits on you until"),
            "{said}"
        );
        // A pane of another tab is on another canvas, with no edges of this one.
        let apart = start(&shell, &session, cx);
        let elsewhere = place(&apart, cx).unwrap();
        assert!(elsewhere.others.is_empty() && elsewhere.edges.is_empty());
        // A pane that is not there has no place.
        assert!(place("p999", cx).is_none());
    }

    #[gpui::test]
    fn the_keys_split_the_focused_pane_and_closing_gives_its_room_back(cx: &mut TestAppContext) {
        use crate::state::{Axis, Split};
        let (shell, cx, session) = open(cx);
        let first = start(&shell, &session, cx);
        draw(cx);
        let tabs = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.store.session(&session).unwrap().tabs.clone()
            })
        };
        let focused = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell
                    .store
                    .session(&session)
                    .unwrap()
                    .focused
                    .clone()
                    .unwrap()
            })
        };

        // Split right: the new half first shows what can be started in it,
        // and nothing is a pane yet. Escape gives the room back.
        cx.simulate_keystrokes(&format!("{APP}-d"));
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.choosing_beside(), Some((first.clone(), Axis::Row)));
            assert_eq!(shell.store.panes_of(&session).count(), 1);
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        assert!(shell.read_with(cx, |shell, _| shell.picker.is_none()));
        assert!(matches!(tabs(cx)[0].root, Split::Pane(_)));
        // Chosen: one tab, the new terminal to the right, with the keys.
        cx.simulate_keystrokes(&format!("{APP}-d"));
        shell.update_in(cx, |shell, window, cx| {
            shell.start("shell", PaneKind::Terminal, window, cx)
        });
        let after = tabs(cx);
        assert_eq!(after.len(), 1);
        let Split::Parts {
            axis,
            first: left,
            second: right,
            ..
        } = &after[0].root
        else {
            panic!("the tab was not split: {after:?}");
        };
        assert_eq!(*axis, Axis::Row);
        assert_eq!(**left, Split::Pane(first.clone()));
        let Split::Pane(right) = &**right else {
            panic!("the right part is not one pane");
        };
        assert_eq!(focused(cx), *right);
        shell.read_with(cx, |shell, _| {
            let made = shell.store.pane(right).unwrap();
            assert_eq!(made.harness, "shell");
            assert_eq!(made.cwd, shell.store.pane(&first).unwrap().cwd);
            assert!(shell.live[right].born.is_some(), "it comes in once");
        });

        // Split down divides the pane with the keys, not the whole tab; the
        // close key takes the half back before it would take a pane.
        cx.simulate_keystrokes(&format!("{APP}-shift-d"));
        cx.simulate_keystrokes(&format!("{APP}-w"));
        assert!(shell.read_with(cx, |shell, _| shell.picker.is_none()));
        assert_eq!(tabs(cx)[0].root.panes().len(), 2);
        cx.simulate_keystrokes(&format!("{APP}-shift-d"));
        shell.update_in(cx, |shell, window, cx| {
            shell.start("shell", PaneKind::Terminal, window, cx)
        });
        let after = tabs(cx);
        let panes: Vec<Id> = after[0].root.panes().into_iter().cloned().collect();
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[..2], [first.clone(), right.clone()]);
        let below = panes[2].clone();
        assert_eq!(focused(cx), below);
        let Split::Parts { second, .. } = &after[0].root else {
            unreachable!()
        };
        assert!(matches!(
            **second,
            Split::Parts {
                axis: Axis::Column,
                ..
            }
        ));

        // The pane keys walk the tab's panes in reading order, round and round.
        cx.simulate_keystrokes(&format!("{APP}-]"));
        assert_eq!(focused(cx), first);
        cx.simulate_keystrokes(&format!("{APP}-["));
        assert_eq!(focused(cx), below);

        // A divider dragged sets the share, within limits; closing a pane gives
        // its room, and the keys, to what was beside it.
        shell.update(cx, |shell, cx| shell.set_share(&session, 0, &[], 0.99, cx));
        let Split::Parts { ratio, .. } = tabs(cx)[0].root.clone() else {
            unreachable!()
        };
        assert_eq!(ratio, 1. - crate::state::MIN_SHARE);
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-w"));
        let after = tabs(cx);
        assert_eq!(after[0].root.panes(), [&first, right]);
        assert_eq!(focused(cx), *right);

        // The swap key walks the tab's panes and stays in the tab: with a
        // second tab there, the bracket keys would leave a tab of one pane,
        // and this one does not.
        let alone = start(&shell, &session, cx);
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-`"));
        assert_eq!(focused(cx), alone, "one pane: nowhere to go");
        shell.update(cx, |shell, cx| shell.focus_pane(&first, cx));
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-`"));
        assert_eq!(focused(cx), *right);
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-`"));
        assert_eq!(focused(cx), first, "round again");
        // Back, in both spellings of the key with Shift.
        cx.simulate_keystrokes(&format!("{APP}-~"));
        assert_eq!(focused(cx), *right);
        cx.simulate_keystrokes(&format!("{APP}-shift-`"));
        assert_eq!(focused(cx), first);
        // The tab keys have both spellings too.
        cx.simulate_keystrokes(&format!("{APP}-}}"));
        assert_eq!(focused(cx), alone);
        cx.simulate_keystrokes(&format!("{APP}-{{"));
        assert_eq!(focused(cx), first);

        // Each tab splits into its own board: the split made two nodes of
        // this tab's, the one to the right of the other.
        let (tab, canvas) = canvas_with(&shell, &first, cx);
        canvas.read_with(cx, |canvas, _| {
            let board = canvas.board();
            assert_eq!(board.nodes().len(), 2);
            assert!(board.nodes()[1].rect.x >= board.nodes()[0].rect.right());
        });
        // On the canvas there is nothing to split: the key is the canvas's.
        shell.update(cx, |shell, cx| shell.set_layout(Layout::Canvas, cx));
        cx.simulate_keystrokes(&format!("{APP}-d"));
        assert_eq!(tabs(cx)[0].root.panes().len(), 2);
        // The swap key enters the next node there, and the other tab stays as it was.
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-`"));
        assert_eq!(focused(cx), *right);
        let node = shell.read_with(cx, |shell, _| shell.store.pane(right).unwrap().node);
        assert_eq!(
            canvas.read_with(cx, |canvas, _| canvas.selected()),
            Some(node)
        );
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.store.tab(&tab).unwrap().layout, Layout::Canvas);
            assert_eq!(shell.store.tab_of(&alone).unwrap().layout, Layout::Panes);
        });
    }

    #[gpui::test]
    fn the_line_between_two_panes_and_the_rails_edge_follow_a_drag(cx: &mut TestAppContext) {
        use crate::state::Split;
        use gpui::{Modifiers, MouseButton, point, px};
        let (shell, cx, session) = open(cx);
        start(&shell, &session, cx);
        cx.simulate_keystrokes(&format!("{APP}-d"));
        shell.update_in(cx, |shell, window, cx| {
            shell.start("shell", PaneKind::Terminal, window, cx)
        });
        draw(cx);
        let share = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                match &shell.store.session(&session).unwrap().tabs[0].root {
                    Split::Parts { ratio, .. } => *ratio,
                    Split::Pane(_) => panic!("not split"),
                }
            })
        };
        assert_eq!(share(cx), 0.5);
        let area = shell.read_with(cx, |shell, _| shell.content_area.get());
        let (width, middle) = (
            f32::from(area.size.width),
            area.origin.y + area.size.height / 2.,
        );
        // Take the line in the middle and carry it a fifth of the width right.
        let from = point(area.origin.x + px(width / 2.), middle);
        let to = point(from.x + px(width / 5.), middle);
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            from + point(px(6.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let dragged = share(cx);
        assert!(
            (dragged - 0.7).abs() < 0.02,
            "the left pane has {dragged} of the width"
        );

        // The rail's edge the same way, as far as its limits.
        let rail = shell.read_with(cx, |shell, _| shell.store.rail_width);
        let edge = point(px(rail), middle);
        cx.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            edge + point(px(8.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(rail + 60.), middle),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.rail_width),
            rail + 60.
        );
        cx.simulate_mouse_move(
            point(px(5.), middle),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(5.), middle),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.rail_width),
            RAIL_WIDTHS.0
        );
        // The panes were laid out again in what the rail left them.
        draw(cx);
        let after = shell.read_with(cx, |shell, _| shell.content_area.get());
        assert!(after.size.width > area.size.width);
    }

    #[gpui::test]
    fn the_keys_go_to_the_pane_that_comes_to_the_front(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        let (a, b) = (start(&shell, &session, cx), start(&shell, &session, cx));
        draw(cx);
        let has_keys = |cx: &mut VisualTestContext| {
            shell.update_in(cx, |shell, window, _| shell.focus.is_focused(window))
        };
        // Nothing runs in these panes, so the keys a pane is given are the window's.
        assert!(has_keys(cx));
        // The keys are never left nowhere: whatever had them is gone, and the
        // pane in front, or the window, has them on the next frame.
        shell.update_in(cx, |_, window, cx| window.blur(cx));
        draw(cx);
        assert!(has_keys(cx));

        // The pane in front is closed: the one that takes its place gets the
        // keys, not only the mark. Cmd+W then reaches it.
        shell.update_in(cx, |shell, window, cx| shell.close_pane(&b, window, cx));
        draw(cx);
        assert!(has_keys(cx));
        assert_eq!(
            shell.read_with(cx, |shell, _| shell
                .store
                .session(&session)
                .unwrap()
                .focused
                .clone()),
            Some(a.clone())
        );
        cx.simulate_keystrokes(&format!("{APP}-w"));
        assert!(shell.read_with(cx, |shell, _| shell.store.pane(&a).is_none()));

        // A split made by a key gets them too, and so does the pane a key walks to.
        let c = start(&shell, &session, cx);
        draw(cx);
        shell.update_in(cx, |_, window, cx| window.blur(cx));
        cx.simulate_keystrokes(&format!("{APP}-d"));
        shell.update_in(cx, |shell, window, cx| {
            shell.start("shell", PaneKind::Terminal, window, cx)
        });
        draw(cx);
        assert!(has_keys(cx));
        shell.update_in(cx, |_, window, cx| window.blur(cx));
        shell.update(cx, |shell, cx| shell.focus_pane(&c, cx));
        draw(cx);
        assert!(has_keys(cx));
    }

    #[gpui::test]
    fn a_tab_is_named_and_given_an_icon_and_keeps_both(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        let a = start(&shell, &session, cx);
        draw(cx);
        let tab = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.store.session(&session).unwrap().tabs[0].clone()
            })
        };
        let name = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.tab_name(&shell.store.session(&session).unwrap().tabs[0])
            })
        };
        assert_eq!(name(cx), "Shell", "called after its pane until it is named");

        // The key opens the field with the name selected; typing replaces it.
        cx.simulate_keystrokes(&format!("{APP}-r"));
        draw(cx);
        shell.update_in(cx, |shell, window, _| {
            let edit = shell.tab_edit.as_ref().expect("the tab is being named");
            assert_eq!(edit.field.text(), "Shell");
            assert!(shell.tab_focus.is_focused(window));
        });
        cx.simulate_input("api server");
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.tab_edit.as_ref().unwrap().field.text(), "api server");
        });
        // The icons are a row of tiles under the field: the first is the
        // harness's own mark, the second the first of the set.
        draw(cx);
        let area = shell.read_with(cx, |shell, _| shell.content_area.get());
        let tile = |index: f32| {
            area.origin
                + gpui::point(
                    gpui::px(6. + kou_ui::metric::PAD + 15. + index * 32.),
                    gpui::px(4. + kou_ui::metric::PAD * 2. + kou_ui::metric::CONTROL + 15.),
                )
        };
        cx.simulate_click(tile(1.), gpui::Modifiers::default());
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.tab_edit.as_ref().unwrap().icon.clone()),
            Some(kou_ui::TAB_ICONS[0].0.to_owned()),
            "a press on a tile chooses its icon and keeps the editor open"
        );
        shell.update(cx, |shell, cx| {
            shell.tab_edit.as_mut().unwrap().icon = Some("server".into());
            cx.notify();
        });
        // Nothing is kept until Enter.
        assert_eq!(name(cx), "Shell");
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(name(cx), "api server");
        assert_eq!(tab(cx).icon.as_deref(), Some("server"));
        assert!(shell.read_with(cx, |shell, _| shell.tab_edit.is_none()));

        // Escape leaves it as it was; a split and a closed pane keep the name.
        cx.simulate_keystrokes(&format!("{APP}-r"));
        cx.simulate_input("x");
        cx.simulate_keystrokes("escape");
        assert_eq!(name(cx), "api server");
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-d"));
        shell.update_in(cx, |shell, window, cx| {
            shell.start("shell", PaneKind::Terminal, window, cx)
        });
        assert_eq!(name(cx), "api server");
        shell.update_in(cx, |shell, window, cx| shell.close_pane(&a, window, cx));
        assert_eq!(name(cx), "api server");
        assert_eq!(tab(cx).icon.as_deref(), Some("server"));

        // Cleared, it is called after its pane again; an icon nobody knows is dropped.
        shell.update_in(cx, |shell, window, cx| {
            shell.edit_tab(&session, 0, window, cx);
            let edit = shell.tab_edit.as_mut().unwrap();
            edit.field = kou_canvas::field::Field::new("", false);
            edit.icon = Some("no such icon".into());
            shell.commit_tab_edit(cx);
        });
        assert_eq!((tab(cx).name, tab(cx).icon), (None, None));
        assert_eq!(name(cx), "Shell");
    }

    #[gpui::test]
    fn a_prompt_starts_its_agent_in_the_session_on_screen_whatever_waits_in_another(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, first) = open(cx);
        let second = shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: false,
                adapter: None,
            });
            shell.store.sessions[1].id.clone()
        });
        // The first session has a pane, and a new tab that waits for its
        // agent, in front.
        shell.update_in(cx, |shell, window, cx| {
            shell.add_pane(&first, "shell", None, PaneKind::Terminal, window, cx);
        });
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-t"));
        shell.read_with(cx, |shell, _| {
            assert!(shell.pending_in_front());
            assert!(shell.picker_shown());
        });
        // The other session is shown: nothing in it, so its screen is the
        // field. The tab that waits is not what this screen shows.
        shell.update(cx, |shell, cx| {
            shell.select_session(&second, cx);
            shell.asked = false;
        });
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert!(shell.asking());
            assert!(
                !shell.picker_shown(),
                "it waits in the session that was left"
            );
        });
        cx.simulate_input("fix the flaky test");
        cx.simulate_keystrokes("enter");
        shell.read_with(cx, |shell, _| {
            // Its agent starts here, not where the tab waits.
            let here: Vec<_> = shell.store.panes_of(&second).collect();
            assert_eq!(here.len(), 1, "started in the session on screen");
            assert_eq!(here[0].harness, "claude");
            assert_eq!(shell.store.panes_of(&first).count(), 1);
            // And the tab goes on waiting where it was asked for.
            assert!(shell.pending_tab(&first).is_some());
        });
    }

    #[gpui::test]
    fn a_pane_an_agent_asks_for_leaves_what_the_user_has_open(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        shell.update_in(cx, |shell, window, cx| {
            shell.add_pane(&session, "shell", None, PaneKind::Terminal, window, cx);
        });
        // The settings are in front when an agent asks for a pane.
        shell.update(cx, |shell, cx| shell.show_settings(cx));
        shell.update_in(cx, |shell, window, cx| {
            let seat = shell.seat_for(None);
            let made = shell.undisturbed(|this| {
                this.add_pane_at(
                    &session,
                    "shell",
                    None,
                    PaneKind::Terminal,
                    Place::Auto,
                    seat,
                    window,
                    cx,
                )
            });
            assert!(made.is_some());
        });
        shell.read_with(cx, |shell, _| {
            assert!(shell.settings_open, "the page stays");
            assert_eq!(shell.store.panes_of(&session).count(), 2);
        });
        // Cmd+W closes what is in front, the page, and no pane behind it.
        cx.simulate_keystrokes(&format!("{APP}-w"));
        shell.read_with(cx, |shell, _| {
            assert!(!shell.settings_open);
            assert_eq!(shell.store.panes_of(&session).count(), 2);
        });
    }

    #[gpui::test]
    fn a_save_that_works_leaves_a_notice_of_something_else_alone(cx: &mut TestAppContext) {
        let (shell, cx, _) = open(cx);
        let folder = std::env::temp_dir().join(format!("kou-save-notice-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        shell.update(cx, |shell, cx| {
            shell.path = Some(folder.join("state.json"));
            shell.notice = Some("Agents cannot reach this window".into());
            shell.save(cx);
            assert_eq!(
                shell.notice.as_deref(),
                Some("Agents cannot reach this window"),
                "not a save's to clear"
            );
            // A save that fails says so, and one that works takes that back.
            shell.path = Some(folder.join("state.json").join("under-a-file"));
            shell.save(cx);
            assert!(
                shell
                    .notice
                    .as_deref()
                    .unwrap()
                    .starts_with("Could not save")
            );
            shell.path = Some(folder.join("state.json"));
            shell.save(cx);
            assert_eq!(shell.notice, None);
        });
        std::fs::remove_dir_all(&folder).ok();
    }

    #[gpui::test]
    fn an_empty_screen_takes_a_prompt_and_starts_an_agent_with_it(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: false,
                adapter: None,
            });
        });
        // A session with nothing in it: the field has the keys as soon as
        // it is drawn. (The helper that opened the window took them for the
        // window after its first frame; this is the first frame again.)
        shell.update(cx, |shell, _| shell.asked = false);
        draw(cx);
        let typed = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, cx| {
                shell.prompt.as_ref().unwrap().read(cx).text(cx)
            })
        };
        let has_keys = |cx: &mut VisualTestContext| {
            shell.update_in(cx, |shell, window, cx| {
                let field = shell.prompt.clone().unwrap();
                gpui::Focusable::focus_handle(field.read(cx), cx).contains_focused(window, cx)
            })
        };
        assert!(has_keys(cx));
        shell.read_with(cx, |shell, _| {
            assert!(shell.asking());
            // An agent before a shell, until the user has started something.
            assert_eq!(shell.asked().unwrap().0.id, "claude");
        });
        cx.simulate_input("fix the flaky test");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("in retry.rs");
        assert_eq!(typed(cx), "fix the flaky test\nin retry.rs");
        // Enter starts it: a pane of that harness with what was typed as
        // its first prompt, the field empty again.
        cx.simulate_keystrokes("enter");
        shell.read_with(cx, |shell, _| {
            let panes: Vec<_> = shell.store.panes_of(&session).collect();
            assert_eq!(panes.len(), 1);
            assert_eq!(panes[0].harness, "claude");
            assert_eq!(shell.store.last_harness.as_deref(), Some("claude"));
            assert!(shell.first_prompt.is_none(), "it was handed over");
            assert!(!shell.asking());
        });
        assert_eq!(typed(cx), "");

        // A new tab asks again, and what was started last is what Enter starts.
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-t"));
        draw(cx);
        assert!(has_keys(cx));
        shell.read_with(cx, |shell, _| {
            assert!(shell.asking());
            assert_eq!(shell.asked().unwrap().0.id, "claude");
        });
        // Who gets it is chosen in the field's own list. Escape closes the
        // list first, and only then the tab.
        shell.update(cx, |shell, cx| shell.toggle_ask_menu(cx));
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| {
            assert!(!shell.ask_menu);
            assert!(shell.pending_in_front(), "the tab is still there");
        });
        // A shell is nothing a prompt goes to: whatever was chosen, Enter
        // in the field starts an agent.
        shell.update(cx, |shell, cx| {
            shell.toggle_ask_menu(cx);
            shell.choose_asked("shell", PaneKind::Terminal, cx);
        });
        draw(cx);
        assert!(has_keys(cx), "the keys are the field's again");
        shell.read_with(cx, |shell, _| {
            assert!(!shell.ask_menu, "choosing closes the list");
            assert_eq!(shell.asked().unwrap().0.id, "claude");
        });
        // A shell opens at once, in a tab of its own, with nothing asked:
        // by its key, as from the list under the tab strip's plus. The tab
        // that waits for its agent stays in the strip, behind it.
        cx.simulate_keystrokes(&format!("{APP}-alt-t"));
        shell.read_with(cx, |shell, _| {
            let panes: Vec<_> = shell.store.panes_of(&session).collect();
            assert_eq!(panes.len(), 2);
            assert_eq!(panes[1].harness, "shell");
            assert_eq!(panes[1].kind, PaneKind::Terminal);
            assert_eq!(shell.store.last_harness.as_deref(), Some("claude"));
            assert!(shell.pending_tab(&session).is_some(), "the waiting tab");
            assert!(!shell.pending_in_front());
            assert!(!shell.asking());
            assert!(shell.first_prompt.is_none());
        });
        // The plus opens its list; Escape closes it and nothing else.
        shell.update(cx, |shell, cx| shell.toggle_new_menu(cx));
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| {
            assert!(!shell.new_menu);
            assert!(shell.pending_tab(&session).is_some());
            assert_eq!(shell.store.panes_of(&session).count(), 2);
        });
    }

    /// The helper's shell with Claude Code beside its shell, and a second
    /// account of it in `/accounts/work`.
    fn with_an_account(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext, Id, Id) {
        let (shell, cx, session) = open(cx);
        let account = shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: true,
                adapter: None,
            });
            // (Each test says whether the accounts are one pool.)
            shell.store.balance = false;
            shell.store.add_profile("claude", "/accounts/work".into())
        });
        (shell, cx, session, account)
    }

    #[gpui::test]
    fn an_account_chosen_for_a_prompt_is_the_pane_s_and_its_home_goes_with_every_launch(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session, account) = with_an_account(cx);
        let home = vec![("CLAUDE_CONFIG_DIR".to_owned(), "/accounts/work".to_owned())];
        shell.update(cx, |shell, cx| {
            // An account is its harness's: its home goes with that one only.
            assert_eq!(shell.account_env("claude", Some(&account)), home);
            assert!(shell.account_env("claude", None).is_empty());
            assert!(shell.account_env("codex", Some(&account)).is_empty());
            assert!(shell.account_env("claude", Some("gone")).is_empty());
            // Chosen in the field's list, with its harness.
            assert!(shell.asked_profile().is_none());
            shell.choose_account("claude", Some(account.clone()), PaneKind::Terminal, cx);
            assert_eq!(shell.asked().unwrap().0.id, "claude");
            assert_eq!(
                shell.asked_profile().map(|profile| profile.name.as_str()),
                Some("work")
            );
        });
        // Started, the pane runs under it, and whatever starts its harness
        // for it is given the account's home.
        shell.update_in(cx, |shell, window, cx| shell.ask(None, window, cx));
        shell.read_with(cx, |shell, _| {
            let panes: Vec<_> = shell.store.panes_of(&session).collect();
            assert_eq!(panes.len(), 1);
            assert_eq!(panes[0].profile.as_ref(), Some(&account));
            assert_eq!(shell.store.last_profile.as_ref(), Some(&account));
            let context = shell.pane_context(panes[0]).expect("a context");
            assert_eq!(context.env, home);
            assert!(shell.launch_profile.is_none(), "taken by the start");
        });
        // The next prompt goes to the same account, until another is chosen.
        shell.update(cx, |shell, cx| {
            shell.pick(None, cx);
            assert_eq!(
                shell.asked_profile().map(|profile| profile.id.clone()),
                Some(account.clone())
            );
            shell.choose_asked("claude", PaneKind::Terminal, cx);
            assert!(shell.asked_profile().is_none(), "the harness's own");
        });
        shell.update_in(cx, |shell, window, cx| shell.ask(None, window, cx));
        shell.read_with(cx, |shell, _| {
            let panes: Vec<_> = shell.store.panes_of(&session).collect();
            assert_eq!(panes.len(), 2);
            assert_eq!(panes[1].profile, None);
            assert_eq!(shell.store.last_profile, None);
            assert!(
                shell
                    .pane_context(panes[1])
                    .expect("a context")
                    .env
                    .is_empty()
            );
        });
        // Forgotten, its pane is the harness's own from its next start.
        shell.update(cx, |shell, cx| shell.remove_account(&account, cx));
        shell.read_with(cx, |shell, _| {
            let panes: Vec<_> = shell.store.panes_of(&session).collect();
            assert_eq!(panes[0].profile, None);
            assert!(shell.store.profiles.is_empty());
        });
    }

    // A folder of a test's own for the app's home and a harness's own home.
    fn account_homes(shell: &Entity<Shell>, cx: &mut VisualTestContext, name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("kou-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("own").join("projects")).unwrap();
        shell.update(cx, |shell, _| {
            shell.home = root.join("app");
            shell.own_homes.insert("claude".into(), root.join("own"));
        });
        root
    }

    #[gpui::test]
    fn an_account_is_added_with_a_name_and_a_proxy_and_runs_through_it(cx: &mut TestAppContext) {
        let (shell, cx, _, _) = with_an_account(cx);
        let root = account_homes(&shell, cx, "account-form");
        shell.update_in(cx, |shell, window, cx| {
            shell.show_settings(cx);
            shell.open_account_form("claude", None, window, cx);
        });
        draw(cx);
        shell.update_in(cx, |shell, window, _| {
            assert!(
                shell.account_focus.is_focused(window),
                "the form has the keys"
            );
        });
        // The name, Tab, the proxy as it was sold; Enter takes them.
        cx.simulate_input("work 2");
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.account_form.as_ref().unwrap().name.text(), "work 2");
        });
        cx.simulate_keystrokes("tab");
        cx.simulate_input("socks5://10.0.0.7:1080");
        cx.simulate_keystrokes("enter");
        shell.read_with(cx, |shell, _| {
            let form = shell.account_form.as_ref().expect("the form stays open");
            assert!(form.problem.as_deref().unwrap().contains("SOCKS"));
            assert_eq!(shell.store.profiles.len(), 1, "nothing is added");
        });
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("gate.example.com:3128:alice:p@ss");
        cx.simulate_keystrokes("enter");
        let account = shell.read_with(cx, |shell, _| {
            assert!(shell.account_form.is_none());
            let added = shell.store.profiles.last().unwrap().clone();
            assert_eq!(added.name, "work 2");
            // Its folder is the app's own, made for it, and shares the
            // conversations of the harness's own home.
            assert_eq!(
                added.home,
                root.join("app")
                    .join("accounts")
                    .join(format!("claude-{}", added.id))
            );
            assert!(added.home.join("projects").is_dir());
            // What is kept with the account has no password in it.
            assert_eq!(added.proxy.as_deref(), Some("http://gate.example.com:3128"));
            let saved = serde_json::to_string(&shell.store).unwrap();
            assert!(
                !saved.contains("p%40ss") && !saved.contains("alice"),
                "{saved}"
            );
            // What runs under it gets its home and goes out through the
            // proxy, password and all.
            let env = shell.account_env("claude", Some(&added.id));
            let of = |name: &str| {
                env.iter()
                    .find(|(known, _)| known == name)
                    .map(|(_, value)| value.clone())
            };
            assert_eq!(
                of("CLAUDE_CONFIG_DIR"),
                Some(added.home.to_string_lossy().into_owned())
            );
            assert_eq!(
                of("HTTPS_PROXY").as_deref(),
                Some("http://alice:p%40ss@gate.example.com:3128")
            );
            assert_eq!(of("http_proxy"), of("HTTPS_PROXY"));
            assert!(of("NO_PROXY").unwrap().contains("localhost"));
            // The harness's own account, and another account, do not.
            assert!(shell.account_env("claude", None).is_empty());
            added.id
        });
        // Changed: the proxy as it was given comes back to be edited, and
        // one without a password leaves none kept.
        shell.update_in(cx, |shell, window, cx| {
            shell.open_account_form("claude", Some(&account), window, cx);
            let form = shell.account_form.as_ref().unwrap();
            assert_eq!(
                form.proxy.text(),
                "http://alice:p%40ss@gate.example.com:3128"
            );
            assert_eq!(form.name.text(), "work 2");
        });
        draw(cx);
        cx.simulate_keystrokes("tab cmd-a");
        cx.simulate_input("10.0.0.9:8080");
        cx.simulate_keystrokes("enter");
        shell.read_with(cx, |shell, _| {
            assert_eq!(
                shell.store.profile(&account).unwrap().proxy.as_deref(),
                Some("http://10.0.0.9:8080")
            );
            assert_eq!(crate::accounts::secret(&shell.home, &account), None);
            let env = shell.account_env("claude", Some(&account));
            assert!(env.contains(&("HTTPS_PROXY".to_owned(), "http://10.0.0.9:8080".to_owned())));
        });
        // Escape closes the form, and only then the settings.
        shell.update_in(cx, |shell, window, cx| {
            shell.open_account_form("claude", None, window, cx)
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| {
            assert!(shell.account_form.is_none());
            assert!(shell.settings_open, "the page is still there");
        });
        // Forgotten, nothing of its proxy is left.
        shell.update(cx, |shell, cx| shell.remove_account(&account, cx));
        shell.read_with(cx, |shell, _| {
            assert!(shell.store.profile(&account).is_none());
            assert_eq!(crate::accounts::secret(&shell.home, &account), None);
        });
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn the_accounts_of_a_harness_are_one_pool_for_what_is_started(cx: &mut TestAppContext) {
        use crate::usage::{Account, Asked};
        use kou_usage::{Kind, Limits, Window as Limit};
        let (shell, cx, session, work) = with_an_account(cx);
        let left = |used: f32| Asked {
            answer: Some(Ok(Limits {
                plan: None,
                windows: vec![Limit {
                    kind: Kind::Session,
                    used,
                    resets: None,
                }],
            })),
            at: Some(Instant::now()),
            asking: false,
        };
        let of = |profile: Option<&Id>| Account {
            harness: "claude".into(),
            profile: profile.cloned(),
        };
        shell.update(cx, |shell, _| {
            shell.store.balance = true;
            // The harness's own account is nearly spent; the other is not.
            shell.usage.insert(of(None), left(90.));
            shell.usage.insert(of(Some(&work)), left(20.));
            assert_eq!(shell.balanced_account("claude"), Some(work.clone()));
            // Nothing is named on the empty page: the field says so.
            assert!(shell.asked_profile().is_none());
            assert_eq!(shell.asked_account_shown().as_deref(), Some("auto"));
        });
        // A prompt with no account named goes to the one with the most left.
        shell.update_in(cx, |shell, window, cx| shell.ask(None, window, cx));
        shell.read_with(cx, |shell, _| {
            let panes: Vec<_> = shell.store.panes_of(&session).collect();
            assert_eq!(panes.len(), 1);
            assert_eq!(panes[0].profile.as_ref(), Some(&work));
            assert_eq!(shell.account_shown(panes[0]).as_deref(), Some("work"));
        });
        // One named is used as named, whatever is left to it.
        shell.update_in(cx, |shell, window, cx| {
            shell.launch_profile = None;
            shell.usage.insert(of(None), left(99.));
            shell.usage.insert(of(Some(&work)), left(10.));
            shell.choose_account("claude", Some(work.clone()), PaneKind::Terminal, cx);
            shell.ask(None, window, cx);
        });
        shell.read_with(cx, |shell, _| {
            let panes: Vec<_> = shell.store.panes_of(&session).collect();
            assert_eq!(panes[1].profile.as_ref(), Some(&work));
        });
        // What an account already has at work counts against it: with the
        // same left to both, the next goes to the other.
        shell.update(cx, |shell, _| {
            shell.usage.insert(of(None), left(40.));
            shell.usage.insert(of(Some(&work)), left(40.));
            let busy = shell.store.panes_of(&session).next().unwrap().id.clone();
            shell.live.get_mut(&busy).unwrap().mark = Mark::Working;
            assert_eq!(shell.balanced_account("claude"), None, "the harness's own");
            // And with the pool off nothing is picked for anybody: with
            // no account named, none is shown.
            shell.store.balance = false;
            shell.ask_choice = None;
            shell.store.last_profile = None;
            assert_eq!(shell.asked_account_shown(), None);
        });
    }

    #[gpui::test]
    fn a_pane_goes_on_under_the_account_with_the_most_left(cx: &mut TestAppContext) {
        use crate::usage::{Account, Asked};
        use kou_usage::{Kind, Limits, Window as Limit};
        let (shell, cx, session, apart) = with_an_account(cx);
        let root = account_homes(&shell, cx, "account-move");
        let left = |used: f32| Asked {
            answer: Some(Ok(Limits {
                plan: None,
                windows: vec![Limit {
                    kind: Kind::Weekly,
                    used,
                    resets: None,
                }],
            })),
            at: Some(Instant::now()),
            asking: false,
        };
        // A second account, made by the app: it shares the conversations
        // of the harness's own home. (The first, a folder of the user's,
        // keeps its own.)
        let shared = shell.update_in(cx, |shell, window, cx| {
            shell.store.balance = true;
            shell.open_account_form("claude", None, window, cx);
            shell.commit_account_form(window, cx);
            shell.store.profiles.last().unwrap().id.clone()
        });
        let pane = shell.update_in(cx, |shell, window, cx| {
            let pane = shell
                .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                .unwrap();
            for (profile, used) in [(None, 97.), (Some(&shared), 30.), (Some(&apart), 5.)] {
                shell.usage.insert(
                    Account {
                        harness: "claude".into(),
                        profile: profile.cloned(),
                    },
                    left(used),
                );
            }
            pane
        });
        // From the harness's own account, nearly spent, to the one that
        // shares its conversations: not to the one with the most left of
        // all, where this conversation would not be found.
        shell.update_in(cx, |shell, window, cx| {
            assert!(shell.move_to_best_account(&pane, window, cx));
            assert_eq!(
                shell.store.pane(&pane).unwrap().profile.as_ref(),
                Some(&shared)
            );
            assert!(shell.notice.as_deref().unwrap().contains("goes on under"));
        });
        // A pane of the account that keeps to itself stays where it is.
        shell.update_in(cx, |shell, window, cx| {
            shell.store.pane_mut(&pane).unwrap().profile = Some(apart.clone());
            assert!(!shell.move_to_best_account(&pane, window, cx));
            assert_eq!(
                shell.store.pane(&pane).unwrap().profile.as_ref(),
                Some(&apart)
            );
        });
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn the_accounts_of_a_provider_are_one_block_of_the_usage_page(cx: &mut TestAppContext) {
        use crate::usage::{Account, Asked};
        use kou_thread::{Capabilities, Update, Usage};
        use kou_usage::{Kind, Limits, Reason, Window as Limit};
        let (shell, cx, session, account) = with_an_account(cx);
        let window = |kind: Kind, used: f32| Limit {
            kind,
            used,
            resets: None,
        };
        let pane = shell.update_in(cx, |shell, window_, cx| {
            // Only what is installed and reports limits is an account.
            let accounts = shell.usage_accounts();
            assert_eq!(
                accounts,
                [
                    Account {
                        harness: "claude".into(),
                        profile: None
                    },
                    Account {
                        harness: "claude".into(),
                        profile: Some(account.clone())
                    },
                ]
            );
            // No program is run for a test: asking is a no-op here.
            shell.refresh_usage(true, cx);
            assert!(shell.usage.is_empty());
            let answered = |answer| Asked {
                answer: Some(answer),
                at: Some(Instant::now()),
                asking: false,
            };
            shell.usage.insert(
                accounts[0].clone(),
                answered(Ok(Limits {
                    plan: Some("Max".into()),
                    windows: vec![
                        window(Kind::Weekly, 66.),
                        window(Kind::Model("Fable".into()), 84.),
                    ],
                })),
            );
            shell.usage.insert(
                accounts[1].clone(),
                answered(Ok(Limits {
                    plan: Some("Max".into()),
                    windows: vec![window(Kind::Session, 10.), window(Kind::Weekly, 12.)],
                })),
            );
            // A chat under the second account, which has spent something.
            let pane = shell
                .add_pane(&session, "claude", None, PaneKind::Thread, window_, cx)
                .unwrap();
            shell.store.pane_mut(&pane).unwrap().profile = Some(account.clone());
            shell.store.pane_mut(&pane).unwrap().title = "Fix it".into();
            let saved = shell.store.pane(&pane).unwrap().clone();
            let (backend, end) = kou_thread::channel();
            shell.install_thread(&saved, backend, "Claude Code".into(), window_, cx);
            for update in [
                Update::Ready {
                    session: "s".into(),
                    capabilities: Capabilities::default(),
                    modes: Vec::new(),
                    mode: None,
                    models: Vec::new(),
                    model: None,
                },
                Update::Usage(Usage {
                    context_used: Some(50_000),
                    context_limit: Some(200_000),
                    input_tokens: Some(183_400),
                    output_tokens: Some(9_800),
                    cost_usd: Some(1.37),
                }),
            ] {
                let _ = end.updates.try_send(update);
            }
            std::mem::forget(end);
            pane
        });
        cx.run_until_parked();
        shell.read_with(cx, |shell, cx| {
            let block = shell.usage_block("claude", cx);
            assert_eq!(block.name, "Claude Code");
            let names: Vec<&str> = block
                .accounts
                .iter()
                .map(|line| line.name.as_str())
                .collect();
            assert_eq!(names, ["main", "work"]);
            assert_eq!(block.plan(), Some("Max"));
            // Each window once: the short one is the second account's alone,
            // the week is both together.
            let shown: Vec<(String, f32, usize)> = block
                .windows
                .iter()
                .map(|window| (window.kind.label(), window.used, window.parts.len()))
                .collect();
            assert_eq!(
                shown,
                [
                    ("5h".to_owned(), 10., 1),
                    ("Weekly".to_owned(), 39., 2),
                    ("Fable weekly".to_owned(), 84., 1),
                ]
            );
            assert_eq!(block.chats.len(), 1);
            assert_eq!(block.chats[0].pane, pane);
            assert_eq!(block.chats[0].account.as_deref(), Some("work"));
            assert_eq!(block.spent(), (183_400, 9_800, 1.37));
            // The window has that block and no other: nothing else here
            // reports limits or has spent anything.
            let blocks = shell.usage_blocks(cx);
            assert_eq!(blocks.len(), 1);
        });
        // An account that could not be asked says why, in the block.
        shell.update(cx, |shell, _| {
            let second = Account {
                harness: "claude".into(),
                profile: Some(account.clone()),
            };
            shell.usage.insert(
                second,
                Asked {
                    answer: Some(Err(Reason::SignIn("sign in again".into()))),
                    at: Some(Instant::now()),
                    asking: false,
                },
            );
        });
        shell.read_with(cx, |shell, cx| {
            let block = shell.usage_block("claude", cx);
            assert_eq!(block.accounts[1].note.as_deref(), Some("sign in again"));
            assert_eq!(block.accounts[1].plan, None);
            let weekly = block
                .windows
                .iter()
                .find(|window| window.kind == Kind::Weekly)
                .unwrap();
            assert_eq!(
                (weekly.used, weekly.parts.len()),
                (66., 1),
                "the first account's alone"
            );
        });
        // Forgetting the account forgets what was known of it.
        shell.update(cx, |shell, cx| shell.remove_account(&account, cx));
        shell.read_with(cx, |shell, _| assert_eq!(shell.usage.len(), 1));
    }

    #[gpui::test]
    fn usage_is_a_page_of_the_window_that_its_key_shows_and_escape_leaves(cx: &mut TestAppContext) {
        use crate::usage::{Account, Asked};
        use kou_usage::{Kind, Limits, Window as Limit};
        let (shell, cx, session, account) = with_an_account(cx);
        let pane = start(&shell, &session, cx);
        let now = std::time::SystemTime::now();
        shell.update(cx, |shell, _| {
            // The harness's own account has used a quarter of its week, the
            // added one three quarters; both start over in half a week.
            for (profile, used) in [(None, 25.), (Some(account.clone()), 75.)] {
                shell.usage.insert(
                    Account {
                        harness: "claude".into(),
                        profile,
                    },
                    Asked {
                        answer: Some(Ok(Limits {
                            plan: Some("Max".into()),
                            windows: vec![Limit {
                                kind: Kind::Weekly,
                                used,
                                resets: Some(now + Duration::from_secs(84 * 3600)),
                            }],
                        })),
                        at: Some(Instant::now()),
                        asking: false,
                    },
                );
            }
        });
        // From the key, which is handled inside the shell's own update: the
        // page is drawn by the window that is there, and no other opens.
        cx.simulate_keystrokes(&format!("{APP}-u"));
        draw(cx);
        assert_eq!(cx.update(|_, cx| cx.windows().len()), 1);
        shell.read_with(cx, |shell, _| {
            assert!(shell.usage_open && !shell.settings_open);
            assert!(shell.page_open());
        });
        // Its bars fill as it opens: empty at first, full a moment later.
        let fill = cx.debug_bounds("usage-fill-0").expect("a bar's fill");
        assert_eq!(f32::from(fill.size.width), 0.);
        cx.executor().advance_clock(Duration::from_secs(1));
        draw(cx);
        // A bar for each account, side by side, each filled by what its
        // account has left.
        let bars = [
            ("usage-bar-0", "usage-fill-0"),
            ("usage-bar-1", "usage-fill-1"),
        ]
        .map(|(bar, fill)| {
            (
                cx.debug_bounds(bar).expect("an account's bar"),
                cx.debug_bounds(fill).expect("its fill"),
            )
        });
        assert_eq!(bars[0].0.top(), bars[1].0.top(), "in one row");
        assert!(bars[0].0.right() < bars[1].0.left());
        for ((bar, fill), left) in bars.iter().zip([0.75, 0.25]) {
            assert_eq!(f32::from(bar.size.height), 32.);
            let share = f32::from(fill.size.width) / f32::from(bar.size.width);
            assert!(
                (share - left).abs() < 0.02,
                "{share} of the bar, for {left}"
            );
        }
        // The settings take its place, and it takes theirs: one page at most.
        shell.update(cx, |shell, cx| shell.toggle_settings(cx));
        shell.read_with(cx, |shell, _| {
            assert!(shell.settings_open && !shell.usage_open)
        });
        cx.simulate_keystrokes(&format!("{APP}-u"));
        shell.read_with(cx, |shell, _| {
            assert!(shell.usage_open && !shell.settings_open)
        });
        // Escape goes back to the workspace; so does the key, pressed again.
        draw(cx);
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| {
            assert!(!shell.page_open());
            assert!(
                shell.usage_tick.is_none(),
                "nothing asks behind a closed page"
            );
        });
        cx.simulate_keystrokes(&format!("{APP}-u"));
        cx.simulate_keystrokes(&format!("{APP}-u"));
        assert!(!shell.read_with(cx, |shell, _| shell.page_open()));
        // A chat's row goes to its pane, which closes the page too.
        cx.simulate_keystrokes(&format!("{APP}-u"));
        assert!(shell.read_with(cx, |shell, _| shell.usage_open));
        shell.update(cx, |shell, cx| shell.focus_pane(&pane, cx));
        assert!(!shell.read_with(cx, |shell, _| shell.page_open()));
        assert_eq!(cx.update(|_, cx| cx.windows().len()), 1);
    }

    #[gpui::test]
    fn what_can_be_started_in_a_split_s_new_half_scrolls_where_it_does_not_fit(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        start(&shell, &session, cx);
        shell.update(cx, |shell, cx| {
            let one = shell.harnesses[0].clone();
            shell.harnesses = (0..16)
                .map(|at| HarnessInfo {
                    id: format!("agent-{at}"),
                    name: format!("Agent {at}"),
                    ..one.clone()
                })
                .collect();
            cx.notify();
        });
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(360.)));
        cx.simulate_keystrokes(&format!("{APP}-d"));
        draw(cx);
        let before = cx.debug_bounds("pick-list").expect("the list");
        assert!(
            f32::from(before.size.height) > 360.,
            "as tall as its rows, not as the half it is in: {before:?}"
        );
        // A wheel over it moves it (in pixels, as a trackpad says it: no glide).
        cx.simulate_event(gpui::ScrollEvent {
            position: before.origin + gpui::point(gpui::px(60.), gpui::px(60.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-120.))),
            ..Default::default()
        });
        draw(cx);
        let after = cx.debug_bounds("pick-list").expect("the list");
        assert_eq!(
            f32::from(before.top() - after.top()),
            120.,
            "scrolled by what the wheel said"
        );
    }

    #[gpui::test]
    fn a_surface_an_agent_shows_is_drawn_in_its_chat_follows_its_file_and_tells_the_agent(
        cx: &mut TestAppContext,
    ) {
        use kou_thread::{Block, Capabilities, Command, ToolCall, ToolContent, ToolStatus, Update};
        use serde_json::json;
        let (shell, cx, session) = open(cx);
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(1500.)));
        shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: true,
                adapter: None,
            });
        });
        // The agent's workspace: a file of runs, and the surface over it as
        // kou's `UI` tool writes one.
        let work = tempfile::tempdir().unwrap();
        let root = work.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".harness/ui/s-1/bench")).unwrap();
        std::fs::create_dir_all(root.join("bench")).unwrap();
        std::fs::write(root.join("bench/runs.jsonl"), "{\"p99_ms\": 50.0}\n").unwrap();
        let version = json!({
            "kou": "genui/v0", "surface": "bench", "version": 1, "session": "s-1",
            "title": "Benchmarks", "glance": "",
            "sources": { "runs": { "file": "bench/runs.jsonl", "format": "jsonl" } },
            "data": {},
            "components": [
                { "id": "root", "component": "Column", "children": ["p99", "comment", "apply"] },
                { "id": "p99", "component": "Metric", "label": "p99", "unit": "ms", "digits": 1,
                  "value": { "path": "/sources/runs/last/p99_ms" } },
                { "id": "comment", "component": "TextField", "label": "Comment",
                  "value": { "path": "/comment" } },
                { "id": "apply", "component": "Button", "label": "Apply", "once": true,
                  "action": { "event": { "name": "apply",
                      "data": { "comment": { "path": "/comment" } } } } },
            ],
            "snapshot": { "sources": {}, "sourceInfo": {}, "diffs": {} },
            "text": "◇ Benchmarks",
        });
        let path = ".harness/ui/s-1/bench/v1.json";
        std::fs::write(root.join(path), version.to_string()).unwrap();
        let (pane, end) = shell.update_in(cx, |shell, window, cx| {
            let pane = shell
                .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                .unwrap();
            let mut saved = shell.store.pane(&pane).unwrap().clone();
            saved.cwd = root.clone();
            let (backend, end) = kou_thread::channel();
            shell.install_thread(&saved, backend, "kou".into(), window, cx);
            (pane, end)
        });
        for update in [
            Update::Ready {
                session: "s-1".into(),
                capabilities: Capabilities::default(),
                modes: Vec::new(),
                mode: None,
                models: Vec::new(),
                model: None,
            },
            Update::UserChunk(Block::Text("how fast is it".into())),
            Update::TurnStarted,
            Update::Tool(ToolCall {
                id: "u1".into(),
                title: "UI".into(),
                status: ToolStatus::Done,
                content: vec![ToolContent::Text(format!(
                    "UI bench v1 ({path})\n\n◇ Benchmarks"
                ))],
                ..ToolCall::default()
            }),
            Update::AssistantChunk("Here it is.".into()),
            Update::TurnEnded(kou_thread::Stop::Finished),
        ] {
            let _ = end.updates.try_send(update);
        }
        cx.run_until_parked();
        // The call's card is the surface, drawn by the app's own plugin
        // from the file as it is now.
        assert!(cx.debug_bounds("gu-p99").is_some(), "the surface is drawn");
        let instance = format!("{pane}:{path}");
        let said = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                let hosted = shell.plugins.borrow();
                let tree = hosted.host.as_ref().unwrap().drawn("genui", &instance);
                tree.map(|tree| tree.said()).unwrap_or_default()
            })
        };
        assert!(said(cx).contains("P99 50.0 ms"), "{}", said(cx));
        // The file grows: looked at again, the surface shows its new end.
        std::fs::write(
            root.join("bench/runs.jsonl"),
            "{\"p99_ms\": 50.0}\n{\"p99_ms\": 41.6}\n",
        )
        .unwrap();
        shell.update(cx, |shell, cx| shell.look_at_surfaces(cx));
        cx.run_until_parked();
        assert!(said(cx).contains("P99 41.6 ms"), "{}", said(cx));
        // Its field is typed in where it stands; Enter keeps what was typed.
        let field = cx.debug_bounds("gu-comment").expect("the field");
        cx.simulate_click(field.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(shell.read_with(cx, |shell, _| shell.plugins.borrow().editing.is_some()));
        cx.simulate_input("tonight");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(shell.read_with(cx, |shell, _| shell.plugins.borrow().editing.is_none()));
        assert!(
            said(cx).contains("tonight") || {
                // (A field's text is its `value`, not what it says.)
                shell.read_with(cx, |shell, _| {
                    let hosted = shell.plugins.borrow();
                    let tree = hosted
                        .host
                        .as_ref()
                        .unwrap()
                        .drawn("genui", &instance)
                        .unwrap();
                    tree.find("gu-comment").unwrap().text("value") == Some("tonight")
                })
            }
        );
        // Its button tells the agent, as a message of the user's.
        let apply = cx.debug_bounds("gu-apply").expect("the button");
        cx.simulate_click(apply.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let mut prompts = Vec::new();
        while let Ok(command) = end.commands.try_recv() {
            if let Command::Prompt(blocks) = command {
                prompts.push(blocks);
            }
        }
        assert_eq!(
            prompts,
            [vec![Block::Text(
                "[UI bench v1] apply {\"comment\":\"tonight\"}".into()
            )]]
        );
        // It works once: a second press tells nothing.
        let apply = cx.debug_bounds("gu-apply").expect("the button");
        cx.simulate_click(apply.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(end.commands.try_recv().is_err());
        std::mem::forget(end);
    }

    #[gpui::test]
    fn the_prompt_field_s_frame_is_a_plugin_s_to_draw_and_the_view_s_when_it_cannot(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, _) = open(cx);
        cx.executor().allow_parking();
        let home = tempfile::tempdir().unwrap();
        shell.update(cx, |shell, cx| {
            shell.home = home.path().to_owned();
            shell.watch_plugins(cx);
            // The empty page, with the field a prompt is typed in.
            shell.pick(None, cx);
        });
        draw(cx);
        // The app's own `composer` plugin draws the frame and places the
        // field's parts in it: they are where they always were.
        assert!(shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            hosted.host.as_ref().unwrap().has("composer") && !hosted.fields.is_empty()
        }));
        let frame = cx.debug_bounds("frame").expect("the field's frame");
        let attach = cx.debug_bounds("attach").expect("the paperclip");
        assert!(cx.debug_bounds("field-foot").is_some() && cx.debug_bounds("send").is_some());
        assert!(frame.contains(&attach.center()));
        let write = |script: &str| {
            let folder = home.path().join("plugins/composer");
            std::fs::create_dir_all(folder.join("desktop")).unwrap();
            std::fs::write(
                folder.join("plugin.json"),
                r#"{"name": "composer", "desktop": {"script": "desktop/main.js"}}"#,
            )
            .unwrap();
            std::fs::write(folder.join("desktop/main.js"), script).unwrap();
        };
        let until = |cx: &mut VisualTestContext, seen: &dyn Fn(&mut VisualTestContext) -> bool| {
            for _ in 0..200 {
                std::thread::sleep(Duration::from_millis(25));
                cx.run_until_parked();
                draw(cx);
                if seen(cx) {
                    return true;
                }
            }
            false
        };
        // The user's own `composer` takes its place as it is saved: the
        // paperclip above what is typed, in a frame with more room.
        write(
            r#"kou.view({ id: "composer", render: (ctx) => ui.col({ id: "frame", p: 30, gap: 6, border: 1, borderColor: "accent", bg: "raised" }, [
                 ui.row({ gap: 8 }, [ui.slot("attach"), ctx.data.slots.includes("foot") ? ui.slot("foot") : null]),
                 ui.slot("editor"),
                 ui.row({ justify: "end" }, [ui.slot("controls")]),
               ]) });"#,
        );
        assert!(
            until(cx, &|cx| cx
                .debug_bounds("attach")
                .is_some_and(
                    |now| now.origin.y < attach.origin.y - gpui::px(20.)
                )),
            "the field was not drawn again from the saved script"
        );
        let moved = cx.debug_bounds("frame").unwrap();
        assert!(moved.size.height > frame.size.height + gpui::px(30.));
        // Its parts are the same ones, and work as they did.
        assert!(cx.debug_bounds("field-foot").is_some() && cx.debug_bounds("send").is_some());
        // A `composer` that draws no field leaves the field to the app,
        // which draws it as it always did.
        write(r#"kou.page({ id: "other", title: "Other", render: () => ui.text("x") });"#);
        assert!(
            until(cx, &|cx| cx
                .debug_bounds("frame")
                .is_some_and(|now| now.size == frame.size)),
            "the field was not given back to the app"
        );
        assert_eq!(cx.debug_bounds("attach").unwrap().origin, attach.origin);
        // Taken away, the app's own plugin draws it again, the same.
        std::fs::remove_dir_all(home.path().join("plugins/composer")).unwrap();
        assert!(until(cx, &|cx| {
            shell.read_with(cx, |shell, _| {
                shell
                    .plugins
                    .borrow()
                    .host
                    .as_ref()
                    .unwrap()
                    .has("composer")
            })
        }));
        draw(cx);
        assert_eq!(cx.debug_bounds("frame").unwrap().size, frame.size);
        assert_eq!(cx.debug_bounds("attach").unwrap().origin, attach.origin);
    }

    #[gpui::test]
    fn the_rail_is_a_plugin_s_to_draw_and_the_app_s_when_it_cannot(cx: &mut TestAppContext) {
        let (shell, cx, first) = open(cx);
        cx.executor().allow_parking();
        let home = tempfile::tempdir().unwrap();
        shell.update(cx, |shell, cx| {
            shell.home = home.path().to_owned();
            shell.watch_plugins(cx);
        });
        draw(cx);
        // The app's own `rail` plugin draws it.
        assert!(cx.debug_bounds("rail").is_some());
        assert!(cx.debug_bounds("rail-native").is_none());
        // Its rows do what the app's did. The space's plus starts a session
        // there, and the click goes no further: the space stays open.
        let space = shell.read_with(cx, |shell, _| shell.store.spaces[0].id.clone());
        let plus = cx
            .debug_bounds(format!("space-new-{space}").leak())
            .expect("the space's plus");
        cx.simulate_click(plus.center(), gpui::Modifiers::none());
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.store.sessions.len(), 3);
            assert_ne!(shell.store.selected.as_ref(), Some(&first));
            assert!(!shell.store.spaces[0].collapsed);
        });
        // A session's row selects it; twice, it opens its name.
        let row = cx
            .debug_bounds(format!("session-{first}").leak())
            .expect("the first session's row");
        cx.simulate_click(row.center(), gpui::Modifiers::none());
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.selected.clone()),
            Some(first.clone())
        );
        for count in [1, 2] {
            cx.simulate_event(gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: row.center(),
                modifiers: gpui::Modifiers::none(),
                click_count: count,
                first_mouse: false,
            });
            cx.simulate_event(gpui::MouseUpEvent {
                button: gpui::MouseButton::Left,
                position: row.center(),
                modifiers: gpui::Modifiers::none(),
                click_count: count,
            });
        }
        assert!(shell.read_with(cx, |shell, _| {
            shell
                .name_edit
                .as_ref()
                .is_some_and(|edit| edit.what == Named::Session(first.clone()))
        }));
        cx.simulate_keystrokes("escape");
        let write = |script: &str| {
            let folder = home.path().join("plugins/rail");
            std::fs::create_dir_all(folder.join("desktop")).unwrap();
            std::fs::write(
                folder.join("plugin.json"),
                r#"{"name": "rail", "desktop": {"script": "desktop/main.js"}}"#,
            )
            .unwrap();
            std::fs::write(folder.join("desktop/main.js"), script).unwrap();
        };
        let until = |cx: &mut VisualTestContext, seen: &dyn Fn(&mut VisualTestContext) -> bool| {
            for _ in 0..200 {
                std::thread::sleep(Duration::from_millis(25));
                cx.run_until_parked();
                draw(cx);
                if seen(cx) {
                    return true;
                }
            }
            false
        };
        // The user's own `rail` takes its place as it is saved.
        write(
            r#"kou.view({ id: "rail", render: (ctx) => ui.col({ id: "my-rail", p: 12, gap: 4 },
                 ctx.data.spaces.map((space) => ui.text(space.name + " (" + space.count + ")"))) });"#,
        );
        assert!(
            until(cx, &|cx| cx.debug_bounds("my-rail").is_some()),
            "the rail was not drawn from the saved script"
        );
        let said = shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            hosted
                .host
                .as_ref()
                .unwrap()
                .drawn("rail", "")
                .unwrap()
                .said()
        });
        assert_eq!(said, "atlas (3)");
        // One that fails as it draws leaves the rail to the app, which draws
        // it as it always did.
        write(r#"kou.view({ id: "rail", render: (ctx) => ctx.data.nobody.home });"#);
        assert!(
            until(cx, &|cx| cx.debug_bounds("rail-native").is_some()),
            "the rail was not given back to the app"
        );
        assert!(cx.debug_bounds("my-rail").is_none() && cx.debug_bounds("rail").is_none());
        // Taken away, the app's own plugin draws it again.
        std::fs::remove_dir_all(home.path().join("plugins/rail")).unwrap();
        assert!(until(cx, &|cx| cx.debug_bounds("rail").is_some()));
        assert!(cx.debug_bounds("rail-native").is_none());
    }

    #[gpui::test]
    fn the_tab_strip_and_the_pane_headers_are_a_plugin_s_to_draw_and_work_as_the_app_s(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        let (a, b) = (start(&shell, &session, cx), start(&shell, &session, cx));
        draw(cx);
        // The app's own `workspace` plugin draws them.
        let tab = cx
            .debug_bounds(format!("tab-{a}").leak())
            .expect("the first tab");
        assert!(cx.debug_bounds("tabs-native").is_none());
        let focused = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.store.session(&session).unwrap().focused.clone()
            })
        };
        assert_eq!(focused(cx), Some(b.clone()));
        // A tab's click brings its pane to the front; a second opens the
        // panel it is named in, where an icon is chosen.
        cx.simulate_click(tab.center(), gpui::Modifiers::none());
        assert_eq!(focused(cx), Some(a.clone()));
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position: tab.center(),
            modifiers: gpui::Modifiers::none(),
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent {
            button: gpui::MouseButton::Left,
            position: tab.center(),
            modifiers: gpui::Modifiers::none(),
            click_count: 2,
        });
        draw(cx);
        assert!(cx.debug_bounds("tab-editor").is_some());
        let bug = cx.debug_bounds("tab-icon-bug").expect("an icon to choose");
        cx.simulate_click(bug.center(), gpui::Modifiers::none());
        assert!(shell.read_with(cx, |shell, _| {
            shell.tab_edit.as_ref().unwrap().icon.as_deref() == Some("bug")
        }));
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(cx.debug_bounds("tab-editor").is_none());
        // The plus opens what a new tab can be; a press elsewhere shuts it.
        let plus = cx.debug_bounds("agent-new").expect("the plus");
        cx.simulate_click(plus.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(shell.read_with(cx, |shell, _| shell.new_menu));
        assert!(cx.debug_bounds("new-tab-menu").is_some());
        cx.simulate_click(tab.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(!shell.read_with(cx, |shell, _| shell.new_menu));
        // A pane's header closes it.
        let close = cx
            .debug_bounds(format!("pane-close-{a}").leak())
            .expect("the pane's close");
        cx.simulate_click(close.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(shell.read_with(cx, |shell, _| shell.store.pane(&a).is_none()));
        assert_eq!(focused(cx), Some(b.clone()));
        // What the window has to say is along its foot.
        shell.update(cx, |shell, _| {
            shell.notice = Some("Something to say".into())
        });
        draw(cx);
        let notice = cx.debug_bounds("notice").expect("the notice");
        // A `workspace` of the user's that fails as it draws leaves the
        // strip to the app, which draws it as it always did.
        cx.executor().allow_parking();
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join("plugins/workspace");
        std::fs::create_dir_all(folder.join("desktop")).unwrap();
        std::fs::write(
            folder.join("plugin.json"),
            r#"{"name": "workspace", "desktop": {"script": "desktop/main.js"}}"#,
        )
        .unwrap();
        std::fs::write(
            folder.join("desktop/main.js"),
            r#"kou.view({ id: "tabs", render: (ctx) => ctx.data.nobody.home });"#,
        )
        .unwrap();
        shell.update(cx, |shell, cx| {
            shell.home = home.path().to_owned();
            shell.watch_plugins(cx);
        });
        draw(cx);
        assert!(cx.debug_bounds("tabs-native").is_some());
        assert!(cx.debug_bounds(format!("tab-{b}").leak()).is_none());
        // (The notice the same, where it was.)
        assert_eq!(cx.debug_bounds("notice"), Some(notice));
    }

    #[gpui::test]
    fn the_settings_and_the_picker_are_a_plugin_s_to_draw_and_their_controls_work(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        shell.update(cx, |shell, cx| shell.show_settings(cx));
        draw(cx);
        let general = cx.debug_bounds("settings-page-general").unwrap();
        cx.simulate_click(general.center(), gpui::Modifiers::none());
        draw(cx);
        // The app's own `screens` plugin draws the settings.
        assert!(cx.debug_bounds("settings").is_some());
        let smooth = shell.read_with(cx, |shell, _| shell.store.smooth_cursor);
        let cursor = cx
            .debug_bounds("settings-cursor")
            .expect("the cursor's button");
        cx.simulate_click(cursor.center(), gpui::Modifiers::none());
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.smooth_cursor),
            !smooth
        );
        draw(cx);
        let mcp = cx.debug_bounds("settings-together-mcp").expect("a way");
        cx.simulate_click(mcp.center(), gpui::Modifiers::none());
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.orchestration),
            crate::state::Orchestration::Mcp
        );
        draw(cx);
        let appearance = cx.debug_bounds("settings-page-appearance").unwrap();
        cx.simulate_click(appearance.center(), gpui::Modifiers::none());
        draw(cx);
        let white = cx.debug_bounds("theme-preset-2").expect("a theme");
        cx.simulate_click(white.center(), gpui::Modifiers::none());
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.theme.clone()),
            kou_ui::themes::Selection::Preset(kou_ui::themes::Preset::White)
        );
        // The picker under the empty page's field: a shell takes no
        // prompt, so chosen there it starts.
        shell.update(cx, |shell, cx| {
            shell.toggle_settings(cx);
            shell.pick(None, cx);
            shell.toggle_ask_menu(cx);
        });
        draw(cx);
        // At the field's foot it says who gets the prompt: with a shell
        // alone, nobody.
        let said = shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            host.drawn("askChooser", "").map(|tree| tree.said())
        });
        assert_eq!(said.as_deref(), Some("No agent to start yet"));
        assert!(cx.debug_bounds("pick-list").is_some());
        let row = cx.debug_bounds("pick-shell").expect("the shell's row");
        cx.simulate_click(row.center(), gpui::Modifiers::none());
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.panes_of(&session).count()),
            1
        );
    }

    #[gpui::test]
    fn a_canvas_s_chrome_is_a_plugin_s_to_draw_and_does_what_the_canvas_s_did(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        let pane = start(&shell, &session, cx);
        shell.update(cx, |shell, cx| shell.set_layout(Layout::Canvas, cx));
        draw(cx);
        let (_, canvas) = canvas_with(&shell, &pane, cx);
        let zoom =
            |cx: &mut VisualTestContext| canvas.read_with(cx, |canvas, _| canvas.camera().zoom);
        // The bar's percentage opens the zooms to go to, in a menu the
        // `canvas` plugin draws.
        let percent = cx.debug_bounds("canvas-zoom").expect("the bar's zoom");
        cx.simulate_click(percent.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("canvas-menu").is_some());
        // Its first row zooms in, as it is pressed.
        let before = zoom(cx);
        let row = cx.debug_bounds("menu-row-0").expect("the first row");
        cx.simulate_mouse_down(
            row.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::none(),
        );
        cx.simulate_mouse_up(
            row.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::none(),
        );
        draw(cx);
        let target = canvas.read_with(cx, |canvas, _| canvas.camera().zoom);
        assert!(target >= before, "{before} -> {target}");
        assert!(canvas.read_with(cx, |canvas, _| canvas.board().nodes().len()) == 1);
        // A board with nothing on it offers a note, which is put there.
        let node = canvas.read_with(cx, |canvas, _| canvas.board().nodes()[0].id);
        canvas.update_in(cx, |canvas, window, cx| canvas.remove(node, window, cx));
        draw(cx);
        let note = (0..8)
            .rev()
            .find_map(|at| cx.debug_bounds(format!("canvas-card-{at}").leak()))
            .expect("the last of an empty board's offers, the note");
        cx.simulate_click(note.center(), gpui::Modifiers::none());
        draw(cx);
        assert_eq!(
            canvas.read_with(cx, |canvas, _| canvas
                .board()
                .nodes()
                .iter()
                .map(|node| node.kind)
                .collect::<Vec<_>>()),
            [kou_canvas::NodeKind::Note]
        );
    }

    #[gpui::test]
    fn the_inspector_is_a_plugin_s_to_draw_and_its_switch_and_notes_say_what_they_did(
        cx: &mut TestAppContext,
    ) {
        use crate::wires::EdgeChange;
        let (shell, cx, session) = open(cx);
        let a = start(&shell, &session, cx);
        let b = start_beside(&shell, &a, cx);
        shell.update(cx, |shell, cx| shell.set_layout(Layout::Canvas, cx));
        let (_, canvas) = canvas_with(&shell, &a, cx);
        let lead = shell.read_with(cx, |shell, _| shell.store.pane(&a).unwrap().node);
        canvas.update(cx, |canvas, cx| {
            // (An agent's node: a terminal's has no orchestrator to be.)
            canvas.update_node(lead, |node| node.kind = kou_canvas::NodeKind::Agent, cx);
            canvas.select(Some(lead), cx);
            canvas.inspect(cx);
        });
        draw(cx);
        // What the `canvas` plugin drew of it, as it says it.
        let instance = format!("canvas-{}", canvas.entity_id().as_u64());
        let said = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                let hosted = shell.plugins.borrow();
                let host = hosted.host.as_ref().unwrap();
                host.drawn("canvasInspector", &instance)
                    .map(|tree| tree.said())
                    .unwrap_or_default()
            })
        };
        assert!(said(cx).contains("Only you wire this tab"), "{}", said(cx));
        // Its switch makes the node the tab's orchestrator.
        let on = cx
            .debug_bounds("inspector-orchestrator-1")
            .expect("the switch's On");
        cx.simulate_click(on.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(canvas.read_with(cx, |canvas, _| {
            canvas.board().node(lead).unwrap().orchestrator
        }));
        assert!(said(cx).contains("Its agent may wire"), "{}", said(cx));
        // What its agent wires says who wired it.
        shell.update(cx, |shell, cx| {
            shell.change_edge(&a, &a, &b, EdgeChange::Add(None), cx)
        });
        let edge = canvas.read_with(cx, |canvas, _| canvas.board().edges()[0].id);
        canvas.update(cx, |canvas, cx| {
            canvas.select_edge(Some(edge), cx);
            canvas.inspect(cx);
        });
        draw(cx);
        let words = said(cx);
        assert!(
            words.contains("Wired by the agent in «") && words.contains("not by you"),
            "{words}"
        );
    }

    #[gpui::test]
    fn a_node_s_chrome_is_a_plugin_s_to_draw_and_a_zoom_only_looks_its_drawing_up(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        let a = start(&shell, &session, cx);
        start_beside(&shell, &a, cx);
        shell.update(cx, |shell, cx| shell.set_layout(Layout::Canvas, cx));
        draw(cx);
        let (_, canvas) = canvas_with(&shell, &a, cx);
        let node = shell.read_with(cx, |shell, _| shell.store.pane(&a).unwrap().node);
        // And a note, which has no live body: what it says is drawn.
        let note = canvas.update_in(cx, |canvas, window, cx| {
            let spec = kou_canvas::NodeSpec {
                kind: kou_canvas::NodeKind::Note,
                title: "plan".into(),
                tag: String::new(),
                detail: "Ask for the schema first.\nThen wire the two.".into(),
                size: None,
            };
            canvas.create_at(40., 400., spec, window, cx)
        });
        draw(cx);
        // The `canvas` plugin draws a node's header and its footer, and the
        // body of one that has no live view.
        let key = canvas.entity_id().as_u64();
        let drawn = |part: &str, node: u64, cx: &mut VisualTestContext| {
            let instance = format!("canvas-{key}/{node}");
            shell.read_with(cx, |shell, _| {
                let hosted = shell.plugins.borrow();
                let tree = hosted.host.as_ref().unwrap().drawn(part, &instance);
                tree.map(|tree| tree.said())
            })
        };
        assert!(drawn("canvasHead", node, cx).is_some());
        assert!(drawn("canvasFoot", node, cx).is_some());
        assert_eq!(
            drawn("canvasBody", note, cx).as_deref(),
            Some("Ask for the schema first. Then wire the two.")
        );
        let made = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| shell.plugins.borrow().faced_made)
        };
        let zoom_to = |zoom: f32, cx: &mut VisualTestContext| {
            canvas.update(cx, |canvas, cx| {
                canvas.set_camera(
                    kou_canvas::Camera {
                        x: 40.,
                        y: 40.,
                        zoom,
                    },
                    cx,
                )
            });
            draw(cx);
        };
        // Twenty frames of a zoom, the lines they shaped: once through
        // first, so that what any first pass costs is not counted.
        let zoom = |cx: &mut VisualTestContext| {
            zoom_to(0.9, cx);
            cx.update(|window, _| window.reset_layout_stats());
            for step in 1..=20 {
                zoom_to(0.9 + step as f32 * 0.004, cx);
            }
            cx.update(|window, _| window.layout_stats().lines_shaped)
        };
        zoom(cx);
        // With the canvas's own chrome, then with the plugin's: each node's
        // parts are looked up, never made from their data again, and their
        // text is shaped no more than the canvas's own is.
        canvas.update(cx, |canvas, cx| canvas.set_face(None, cx));
        let native = zoom(cx);
        canvas.update(cx, |canvas, cx| {
            let face = shell.update(cx, |shell, cx| shell.canvas_face(cx));
            canvas.set_face(Some(face), cx);
        });
        zoom_to(0.9, cx);
        let before = made(cx);
        let shaped = zoom(cx);
        assert_eq!(made(cx), before, "a part was made again during a zoom");
        assert!(
            shaped <= native,
            "{shaped} lines shaped, {native} by the canvas's own"
        );
        // A change in what a part shows makes it again, once.
        let at = made(cx);
        canvas.update(cx, |canvas, cx| {
            canvas.update_node(node, |node| node.place = "/fixture/elsewhere".into(), cx)
        });
        draw(cx);
        draw(cx);
        assert_eq!(made(cx), at + 1);
        canvas.update(cx, |canvas, cx| {
            canvas.update_node(note, |node| node.detail = "Done.".into(), cx)
        });
        draw(cx);
        assert_eq!(made(cx), at + 2);
        assert_eq!(drawn("canvasBody", note, cx).as_deref(), Some("Done."));
    }

    #[gpui::test]
    fn an_edge_s_label_and_a_tooltip_are_a_plugin_s_to_draw(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        let a = start(&shell, &session, cx);
        let b = start_beside(&shell, &a, cx);
        shell.update(cx, |shell, cx| shell.set_layout(Layout::Canvas, cx));
        draw(cx);
        let (_, canvas) = canvas_with(&shell, &a, cx);
        let node = |pane: &str, cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| shell.store.pane(pane).unwrap().node)
        };
        let (from, to) = (node(&a, cx), node(&b, cx));
        let edge = canvas.update(cx, |canvas, cx| {
            let edge = canvas
                .connect(
                    kou_canvas::PortRef::new(from, "out"),
                    kou_canvas::PortRef::new(to, "in"),
                    cx,
                )
                .unwrap();
            let traffic = kou_canvas::Traffic {
                delivered: 2,
                waiting: 0,
            };
            canvas.set_traffic(edge, traffic, cx);
            edge
        });
        // The two in sight, and the edge between them.
        let area = shell.read_with(cx, |shell, _| shell.content_area.get().size);
        let viewport = (f32::from(area.width), f32::from(area.height));
        canvas.update(cx, |canvas, cx| {
            let rect = canvas.board().node(from).unwrap().rect;
            let camera = kou_canvas::Camera::centre_on(&rect, viewport, 0.8);
            canvas.set_camera(camera, cx);
        });
        draw(cx);
        // The `canvas` plugin draws an edge's label, the canvas puts it on
        // its edge, and a press on it selects the edge.
        let instance = format!("canvas-{}/{edge}", canvas.entity_id().as_u64());
        let said = shell.read_with(cx, |shell, _| {
            let hosted = shell.plugins.borrow();
            let host = hosted.host.as_ref().unwrap();
            host.drawn("canvasLabel", &instance).map(|tree| tree.said())
        });
        assert_eq!(said.as_deref(), Some("2"));
        let label = cx
            .debug_bounds(format!("edge-label-{edge}").leak())
            .expect("the edge's label");
        cx.simulate_click(label.center(), gpui::Modifiers::none());
        assert_eq!(
            canvas.read_with(cx, |canvas, _| canvas.selection().1),
            Some(edge)
        );
        // A tooltip's body is the `workspace` plugin's `tip`; without it, the
        // app's own, in the same place and of the same size.
        let tip = |cx: &mut VisualTestContext, selector: &'static str| {
            let plus = cx.debug_bounds("agent-new").expect("the plus");
            cx.simulate_mouse_move(
                plus.center() + gpui::point(gpui::px(200.), gpui::px(200.)),
                None,
                gpui::Modifiers::none(),
            );
            draw(cx);
            cx.simulate_mouse_move(plus.center(), None, gpui::Modifiers::none());
            cx.executor().advance_clock(Duration::from_secs(2));
            draw(cx);
            cx.debug_bounds(selector)
        };
        let drawn = tip(cx, "tip").expect("the plugin's tip");
        cx.update(|_, cx| kou_ui::set_tip_face(None, cx));
        let own = tip(cx, "tip-native").expect("the app's own tip");
        assert_eq!(drawn, own);
    }

    #[gpui::test]
    fn the_rail_and_the_tab_strip_are_drawn_again_when_what_they_show_changes_and_only_then(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        start(&shell, &session, cx);
        draw(cx);
        let drawn = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, cx| {
                let (rail, tabs) = (shell.rail_part.read(cx), shell.tabs_part.read(cx));
                let said = rail.drawn.as_ref().map(|drawn| drawn.tree.said());
                (rail.draws, tabs.draws, said.unwrap_or_default())
            })
        };
        // A frame the window is told to draw, as each of a drag between
        // panes is, where nothing they show changed: they are kept.
        let frame = |cx: &mut VisualTestContext| {
            shell.update(cx, |_, cx| cx.notify());
            cx.update(|window, cx| window.draw(cx).clear(cx));
        };
        frame(cx);
        let (rail, tabs, said) = drawn(cx);
        for _ in 0..5 {
            frame(cx);
        }
        assert_eq!(drawn(cx), (rail, tabs, said));
        // A session renamed: the rail is drawn again, saying so; the strip
        // is kept.
        shell.update(cx, |shell, cx| {
            shell.store.session_mut(&session).unwrap().name = "lexer".into();
            cx.notify();
        });
        frame(cx);
        frame(cx);
        let (now, strip, said) = drawn(cx);
        assert!(now > rail && strip == tabs, "{now} {strip}");
        assert!(said.contains("lexer"), "{said}");
        // And kept again from then on.
        for _ in 0..5 {
            frame(cx);
        }
        assert_eq!(drawn(cx).0, now);
    }

    /// What a render of the window costs, its parts drawn by the plugins and
    /// by the app itself, on a store of a size a user has: four spaces,
    /// thirty sessions, agents in a third of them, and the session on screen
    /// with six tabs, four panes in the one in front. Debug numbers, in
    /// microseconds a render: `cargo test -p kou-desktop -- --ignored
    /// --nocapture what_a_render`.
    #[gpui::test]
    #[ignore = "a measurement, not a check: run it with --ignored --nocapture"]
    fn what_a_render_of_the_window_costs(cx: &mut TestAppContext) {
        use crate::state::{Axis, Seat};
        cx.update(|cx| {
            cx.set_global(kou_ui::Theme::new(true));
            bind_keys(cx);
        });
        let mut store = Store::default();
        let mut current = None;
        for (space, sessions) in [8, 8, 7, 7].into_iter().enumerate() {
            let id = store.add_space(format!("/fixture/space-{space}").into());
            for at in 0..sessions {
                let session = store.add_session(&id).unwrap();
                let saved = store.session_mut(&session).unwrap();
                saved.name = format!("session {space}.{at}");
                saved.archived = at == sessions - 1;
                if space == 0 && at == 0 {
                    current = Some(session.clone());
                } else if at % 3 == 1 {
                    let (board, nodes) = board_for(&[("Codex", "CX", None)]);
                    store.add_pane(
                        &session,
                        "codex",
                        "Codex",
                        format!("/fixture/space-{space}/worktree").into(),
                        PaneKind::Terminal,
                        None,
                        nodes[0],
                        Seat::Tab(board),
                    );
                }
            }
        }
        let current = current.unwrap();
        // The session on screen: a tab of four panes, then five of one.
        let (board, nodes) = board_for(&[
            ("lead", "CC", None),
            ("schema", "CX", Some(0)),
            ("callers", "OC", Some(0)),
            ("tests", "CC", Some(0)),
        ]);
        let mut front: Vec<Id> = Vec::new();
        let mut board = Some(board);
        for (at, node) in nodes.into_iter().enumerate() {
            let seat = match front.first() {
                None => Seat::Tab(board.take().unwrap_or_default()),
                Some(first) => Seat::Beside {
                    at: front.last().unwrap_or(first).clone(),
                    axis: if at % 2 == 1 { Axis::Row } else { Axis::Column },
                },
            };
            front.push(store.add_pane(
                &current,
                "claude",
                &format!("agent {at}"),
                "/fixture/space-0".into(),
                PaneKind::Terminal,
                front.first().cloned(),
                node,
                seat,
            ));
        }
        for at in 0..5 {
            let (board, nodes) = board_for(&[("shell", "SH", None)]);
            store.add_pane(
                &current,
                "shell",
                &format!("shell {at}"),
                "/fixture/space-0".into(),
                PaneKind::Terminal,
                None,
                nodes[0],
                Seat::Tab(board),
            );
        }
        store.session_mut(&current).unwrap().focused = Some(front[0].clone());
        store.selected = Some(current);
        let (shell, cx) = cx.add_window_view(|_, cx| {
            let mut shell = Shell::new(store, None, cx);
            shell.home =
                std::env::temp_dir().join(format!("kou-desktop-tests-{}", std::process::id()));
            shell.notice = Some("A plugin asked for something the app does not do.".into());
            shell
        });
        draw(cx);
        draw(cx);
        let theme = cx.update(|_, cx| kou_ui::theme(cx).clone());
        let (session, panes) = shell.read_with(cx, |shell, _| {
            let session = shell.store.current().cloned();
            let panes: Vec<_> = front
                .iter()
                .filter_map(|id| shell.store.pane(id).cloned())
                .collect();
            (session, panes)
        });
        assert_eq!(panes.len(), 4);
        const RENDERS: u32 = 300;
        // Microseconds a time, after a few times not counted.
        let time =
            |cx: &mut VisualTestContext,
             what: &mut dyn FnMut(&mut Shell, &mut Window, &mut Context<Shell>)| {
                shell.update_in(cx, |shell, window, cx| {
                    for _ in 0..20 {
                        what(shell, window, cx);
                    }
                    let began = Instant::now();
                    for _ in 0..RENDERS {
                        what(shell, window, cx);
                    }
                    began.elapsed().as_secs_f64() * 1e6 / f64::from(RENDERS)
                })
            };
        let measure = |cx: &mut VisualTestContext| -> Vec<(&'static str, f64)> {
            let mut parts = Vec::new();
            parts.push((
                "rail",
                time(cx, &mut |shell, _, cx| drop(shell.render_rail(&theme, cx))),
            ));
            parts.push((
                "tab strip",
                time(cx, &mut |shell, _, cx| {
                    drop(shell.tab_strip(session.as_ref(), 1000., &theme, cx))
                }),
            ));
            parts.push((
                "4 pane headers",
                time(cx, &mut |shell, _, cx| {
                    for pane in &panes {
                        drop(shell.pane_header(pane, true, &theme, cx));
                    }
                }),
            ));
            parts.push((
                "4 panes (frame, header, body)",
                time(cx, &mut |shell, _, cx| {
                    for pane in &panes {
                        drop(shell.pane(pane, true, Some(true), &theme, cx));
                    }
                }),
            ));
            parts.push((
                "the shell's render",
                time(cx, &mut |shell, window, cx| drop(shell.draw(window, cx))),
            ));
            // A whole frame: the render, then laying it out and painting it.
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            let began = Instant::now();
            for _ in 0..RENDERS {
                cx.update(|window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                });
            }
            parts.push((
                "a whole frame, all drawn anew",
                began.elapsed().as_secs_f64() * 1e6 / f64::from(RENDERS),
            ));
            // A frame of a drag between panes: the window told to draw
            // again, what did not change kept from the frame before.
            let parts_drawn = |cx: &mut VisualTestContext| {
                shell.read_with(cx, |shell, cx| {
                    shell.rail_part.read(cx).draws + shell.tabs_part.read(cx).draws
                })
            };
            let drawn = parts_drawn(cx);
            let began = Instant::now();
            for _ in 0..RENDERS {
                shell.update(cx, |_, cx| cx.notify());
                cx.update(|window, cx| window.draw(cx).clear(cx));
            }
            parts.push((
                "a frame of a drag",
                began.elapsed().as_secs_f64() * 1e6 / f64::from(RENDERS),
            ));
            eprintln!(
                "the rail and the tab strip drawn {} times in {RENDERS} frames of a drag",
                parts_drawn(cx) - drawn,
            );
            parts
        };
        let runs = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell
                    .plugins
                    .borrow()
                    .host
                    .as_ref()
                    .map_or(0, |host| host.runs())
            })
        };
        let before = runs(cx);
        let with = measure(cx);
        eprintln!("scripts run while measuring: {}", runs(cx) - before);
        // Where a part's time goes: its theme and data put into words and
        // looked up, and its drawing turned into elements.
        let input = serde_json::json!({
            "theme": crate::plugin_view::theme_json(&theme),
            "data": { "probe": true },
        });
        let began = Instant::now();
        for _ in 0..RENDERS {
            std::hint::black_box(input.to_string());
        }
        eprintln!(
            "the theme put into words: {:.1} µs",
            began.elapsed().as_secs_f64() * 1e6 / f64::from(RENDERS)
        );
        // The rail's time, part by part.
        let room = crate::rail::Room::at(kou_ui::metric::RAIL);
        let data = time(cx, &mut |shell, _, _| drop(shell.rail_data(room)));
        let themed = time(cx, &mut |_, _, _| {
            drop(crate::plugin_view::theme_json(&theme))
        });
        let looked = time(cx, &mut |shell, _, _| {
            let input = serde_json::json!({
                "theme": crate::plugin_view::theme_json(&theme),
                "data": shell.rail_data(room),
            });
            let mut hosted = shell.plugins.borrow_mut();
            drop(
                hosted
                    .host
                    .as_mut()
                    .unwrap()
                    .render_strict("rail", "", &input),
            );
        });
        eprintln!(
            "rail: its data {data:.1} µs, the theme's {themed:.1} µs, both and the lookup {looked:.1} µs"
        );
        let tree = shell.read_with(cx, |shell, _| {
            shell
                .plugins
                .borrow()
                .host
                .as_ref()
                .unwrap()
                .drawn("rail", "")
                .unwrap()
        });
        let converted = time(cx, &mut |_, _, _| {
            let drawing = crate::plugin_view::Drawing {
                page: "rail".into(),
                theme: &theme,
                made: std::cell::Cell::new(0),
                on: std::rc::Rc::new(|_, _, _, _| {}),
                aids: None,
                root: None,
                typing: None,
                slots: std::cell::RefCell::default(),
                dropped: None,
            };
            drop(crate::plugin_view::element(&tree, &drawing));
        });
        eprintln!(
            "rail: {} nodes turned into elements in {converted:.1} µs",
            tree.walk().len()
        );
        // The same with every part drawn by the app itself.
        let host = shell.update(cx, |shell, cx| {
            // (What the parts were handed goes too: drawn by the app, they
            // are drawn again with the window, as before there were parts.)
            shell.rail_shows = None;
            shell.tabs_shows = None;
            cx.notify();
            shell.plugins.borrow_mut().host.take()
        });
        draw(cx);
        let without = measure(cx);
        shell.read_with(cx, |shell, _| shell.plugins.borrow_mut().host = host);
        eprintln!(
            "{:<32} {:>10} {:>10} {:>8}",
            "µs a render (debug)", "plugins", "app's own", "added"
        );
        for ((part, plugins), (_, own)) in with.iter().zip(&without) {
            eprintln!(
                "{part:<32} {plugins:>10.1} {own:>10.1} {:>7.0}%",
                (plugins / own - 1.) * 100.
            );
        }
    }

    #[gpui::test]
    fn a_plugin_s_page_is_drawn_again_as_its_file_is_saved(cx: &mut TestAppContext) {
        let (shell, cx, _) = open(cx);
        // The files are watched from a thread of the watcher's own, which
        // wakes the window when one changes.
        cx.executor().allow_parking();
        let home = tempfile::tempdir().unwrap();
        let write = |plugin: &str, script: &str| {
            let folder = home.path().join("plugins").join(plugin);
            std::fs::create_dir_all(folder.join("desktop")).unwrap();
            std::fs::write(
                folder.join("plugin.json"),
                format!(r#"{{"name": "{plugin}", "desktop": {{"script": "desktop/main.js"}}}}"#),
            )
            .unwrap();
            std::fs::write(folder.join("desktop/main.js"), script).unwrap();
        };
        let page = |words: &str| {
            format!(
                r#"kou.page({{ id: "hello", title: "Hello", icon: "note",
                     render: () => ui.row({{ p: 24 }}, [ui.text("{words}", {{ id: "hello-words" }})]) }});"#
            )
        };
        write("hello", &page("Hi"));
        shell.update(cx, |shell, cx| {
            shell.home = home.path().to_owned();
            shell.watch_plugins(cx);
        });
        // The user's plugin adds a page, beside the app's own Usage; its
        // row in the rail shows it where the workspace is.
        shell.read_with(cx, |shell, _| {
            let pages: Vec<&str> = shell
                .plugin_pages
                .iter()
                .map(|page| page.id.as_str())
                .collect();
            assert!(
                pages.contains(&"usage") && pages.contains(&"hello"),
                "{pages:?}"
            );
        });
        shell.update(cx, |shell, cx| shell.show_plugin_page("hello", cx));
        draw(cx);
        assert!(shell.read_with(cx, |shell, _| shell.page_open()));
        let first = cx.debug_bounds("hello-words").expect("the page's words");
        // Drawn again until `seen` holds, for as long as a change takes to
        // be noticed on disk.
        let until = |cx: &mut VisualTestContext, seen: &dyn Fn(&mut VisualTestContext) -> bool| {
            for _ in 0..200 {
                std::thread::sleep(Duration::from_millis(25));
                cx.run_until_parked();
                draw(cx);
                if seen(cx) {
                    return true;
                }
            }
            false
        };
        // The script is saved with more to say: the open page says it,
        // with nothing built or started again.
        write("hello", &page("Hi, and a good deal more than before"));
        assert!(
            until(cx, &|cx| {
                cx.debug_bounds("hello-words")
                    .is_some_and(|now| now.size.width > first.size.width + gpui::px(60.))
            }),
            "the page was not drawn again from the saved script"
        );
        // Saved broken, the page stays as it was, and says what is wrong.
        let whole = cx.debug_bounds("hello-words").unwrap();
        write("hello", "kou.page({ id: \"hello\", render( {");
        assert!(until(cx, &|cx| cx
            .debug_bounds("plugin-problems")
            .is_some()));
        assert_eq!(cx.debug_bounds("hello-words").unwrap().size, whole.size);
        assert!(shell.read_with(cx, |shell, _| {
            shell
                .notice
                .as_ref()
                .is_some_and(|notice| notice.starts_with("Plugin hello: "))
        }));
        // Mended, the problem is gone from the page and from the window.
        write("hello", &page("Hi"));
        assert!(until(cx, &|cx| cx
            .debug_bounds("plugin-problems")
            .is_none()));
        assert!(shell.read_with(cx, |shell, _| shell.notice.is_none()));
        // Escape leaves the page; its plugin taken away, its row is gone.
        cx.simulate_keystrokes("escape");
        assert!(!shell.read_with(cx, |shell, _| shell.page_open()));
        std::fs::remove_dir_all(home.path().join("plugins/hello")).unwrap();
        assert!(until(cx, &|cx| shell.read_with(cx, |shell, _| {
            shell.plugin_pages.iter().all(|page| page.id != "hello")
        })));
        // A plugin of the user's called `usage` takes the app's own Usage
        // page's place, and gives it back when it goes.
        write(
            "usage",
            r#"kou.page({ id: "usage", title: "Usage", render: (ctx) =>
                 ui.text("mine: " + ctx.data.providers.length, { id: "my-usage" }) });"#,
        );
        cx.simulate_keystrokes(&format!("{APP}-u"));
        assert!(until(cx, &|cx| cx.debug_bounds("my-usage").is_some()));
        std::fs::remove_dir_all(home.path().join("plugins/usage")).unwrap();
        assert!(until(cx, &|cx| cx.debug_bounds("my-usage").is_none()));
        assert!(
            cx.debug_bounds("usage-refresh").is_some(),
            "the app's own page again"
        );
    }

    // By hand: `cargo test -p kou-desktop the_real_agents -- --ignored --nocapture`.
    // It asks the programs installed here for their accounts' limits, as the
    // usage page does when it opens. Nothing is sent to a model.
    #[gpui::test]
    #[ignore = "asks the agents' real programs for their accounts' limits; run by hand"]
    fn the_real_agents_are_asked_for_their_limits(cx: &mut TestAppContext) {
        let (shell, cx, _) = open(cx);
        cx.executor().allow_parking();
        shell.update(cx, |shell, cx| {
            let detected: Vec<_> = crate::usage::PROVIDERS
                .into_iter()
                .filter_map(kou_agents::detect_one)
                .collect();
            shell.detected = detected;
            shell.list_harnesses();
            shell.probes = true;
            shell.refresh_usage(true, cx);
            assert!(shell.usage.values().all(|asked| asked.asking));
        });
        // Each answers within its patience, or is given up on.
        for _ in 0..600 {
            cx.run_until_parked();
            if shell.read_with(cx, |shell, _| {
                shell.usage.values().all(|asked| !asked.asking)
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        shell.read_with(cx, |shell, cx| {
            assert!(!shell.usage.is_empty(), "no provider is installed here");
            for block in shell.usage_blocks(cx) {
                eprintln!(
                    "{} ({})",
                    block.name,
                    block.plan().unwrap_or("no plan said")
                );
                for window in &block.windows {
                    eprintln!("  {:<14} {:>5.1}%", window.kind.label(), window.used);
                }
                for account in &block.accounts {
                    if let Some(note) = &account.note {
                        eprintln!("  {}: {note}", account.name);
                    }
                    assert!(!account.pending, "{} was never answered", block.name);
                }
            }
        });
    }

    #[gpui::test]
    fn quitting_with_agents_at_work_takes_a_second_press(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        let start = Instant::now();
        // Nothing at work: asked once, it quits.
        shell.update(cx, |shell, _| {
            assert_eq!(shell.at_work(), 0);
            assert!(shell.may_quit(start));
            assert!(shell.notice.is_none());
        });
        // With an agent at work the first press says what would be lost.
        shell.update_in(cx, |shell, window, cx| {
            let pane = shell
                .add_pane(&session, "shell", None, PaneKind::Terminal, window, cx)
                .unwrap();
            shell.set_mark(&pane, Mark::Working, window, cx);
        });
        shell.update(cx, |shell, _| {
            assert_eq!(shell.at_work(), 1);
            assert!(!shell.may_quit(start));
            let said = shell.notice.clone().expect("it says why");
            assert!(said.contains("1 agent is at work"), "{said}");
            assert!(said.contains("Q again"), "{said}");
            // Pressed again soon, it quits.
            assert!(shell.may_quit(start + Duration::from_secs(1)));
            // Too late is a first press again, and its second counts from it.
            assert!(!shell.may_quit(start + Duration::from_secs(10)));
            let late = start + Duration::from_secs(10) + QUIT_TWICE + Duration::from_millis(1);
            assert!(!shell.may_quit(late));
            assert!(shell.may_quit(late + Duration::from_millis(500)));
        });
        // The key reaches it wherever the keys are, and what it said goes
        // when a second press no longer counts.
        cx.simulate_keystrokes(&format!("{APP}-q"));
        assert!(shell.read_with(cx, |shell, _| shell.notice.is_some()));
        cx.executor()
            .advance_clock(QUIT_TWICE + Duration::from_millis(100));
        cx.run_until_parked();
        assert!(shell.read_with(cx, |shell, _| shell.notice.is_none()));
    }

    #[gpui::test]
    fn with_only_a_shell_there_is_no_agent_for_a_prompt_and_the_shell_still_opens(
        cx: &mut TestAppContext,
    ) {
        // The helper's machine has a shell and no agent.
        let (shell, cx, session) = open(cx);
        shell.update(cx, |shell, _| shell.asked = false);
        draw(cx);
        shell.read_with(cx, |shell, _| {
            assert!(shell.asking());
            assert!(shell.asked().is_none(), "a shell is not asked anything");
            assert!(shell.shell_ready());
        });
        // Enter in the field starts nothing; the shell's own way does.
        cx.simulate_input("ls");
        cx.simulate_keystrokes("enter");
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.store.panes_of(&session).count(), 0);
        });
        shell.update_in(cx, |shell, window, cx| shell.new_shell(window, cx));
        shell.read_with(cx, |shell, _| {
            let panes: Vec<_> = shell.store.panes_of(&session).collect();
            assert_eq!(panes.len(), 1);
            assert_eq!(panes[0].harness, "shell");
            assert!(
                shell.first_prompt.is_none(),
                "what was typed is not its input"
            );
        });
    }

    #[gpui::test]
    fn the_model_chosen_in_a_conversation_is_the_pane_s_from_then_on(cx: &mut TestAppContext) {
        use kou_thread::{Capabilities, Update};
        let (shell, cx, session) = open(cx);
        shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: true,
                adapter: None,
            });
        });
        let (pane, end) = shell.update_in(cx, |shell, window, cx| {
            let pane = shell
                .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                .unwrap();
            let saved = shell.store.pane(&pane).unwrap().clone();
            let (backend, end) = kou_thread::channel();
            shell.install_thread(&saved, backend, "Claude Code".into(), window, cx);
            (pane, end)
        });
        let model = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.store.pane(&pane).unwrap().model.clone()
            })
        };
        // The model a session starts with is the agent's own: nothing kept.
        let _ = end.updates.try_send(Update::Ready {
            session: "sess-1".into(),
            capabilities: Capabilities::default(),
            modes: Vec::new(),
            mode: None,
            models: Vec::new(),
            model: Some("default".into()),
        });
        cx.run_until_parked();
        assert_eq!(model(cx), None);
        // Chosen in the conversation: kept, for the session opened again.
        let _ = end.updates.try_send(Update::Model("opus".into()));
        cx.run_until_parked();
        assert_eq!(model(cx), Some("opus".to_owned()));
        // Nothing said is nothing chosen.
        let _ = end.updates.try_send(Update::Model(String::new()));
        cx.run_until_parked();
        assert_eq!(model(cx), Some("opus".to_owned()));
        // Back to the agent's own choice: nothing to keep.
        let _ = end.updates.try_send(Update::Model("default".into()));
        cx.run_until_parked();
        assert_eq!(model(cx), None);
        std::mem::forget(end);
    }

    #[gpui::test]
    fn an_agent_remembers_the_agents_it_started_and_what_became_of_them(cx: &mut TestAppContext) {
        use crate::state::StartedState as State;
        use kou_thread::{Capabilities, Update};
        let (shell, cx, session) = open(cx);
        shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: true,
                adapter: None,
            });
        });
        // A conversation with a scripted agent, and a worker it starts.
        let (parent, worker) = shell.update_in(cx, |shell, window, cx| {
            let parent = shell
                .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                .unwrap();
            let saved = shell.store.pane(&parent).unwrap().clone();
            let (backend, end) = kou_thread::channel();
            shell.install_thread(&saved, backend, "Claude Code".into(), window, cx);
            let _ = end.updates.try_send(Update::Ready {
                session: "sess-1".into(),
                capabilities: Capabilities::default(),
                modes: Vec::new(),
                mode: None,
                models: Vec::new(),
                model: None,
            });
            std::mem::forget(end);
            let worker = shell
                .add_pane(
                    &session,
                    "shell",
                    Some(parent.clone()),
                    PaneKind::Terminal,
                    window,
                    cx,
                )
                .unwrap();
            shell.store.pane_mut(&worker).unwrap().title = "Run the tests".into();
            shell.note_started(&parent, &worker, cx);
            (parent, worker)
        });
        draw(cx);
        let started = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.store.pane(&parent).unwrap().started.clone()
            })
        };
        let mark = |mark: Mark, cx: &mut VisualTestContext| {
            shell.update_in(cx, |shell, window, cx| {
                shell.set_mark(&worker, mark, window, cx)
            });
            started(cx)[0].state
        };
        let known = started(cx);
        assert_eq!(known.len(), 1);
        assert_eq!(
            (
                known[0].id.as_str(),
                known[0].title.as_str(),
                known[0].harness.as_str()
            ),
            (worker.as_str(), "Run the tests", "shell")
        );
        assert!(known[0].after.is_some(), "its place in the conversation");
        // What it does is what its starter shows of it.
        assert_eq!(mark(Mark::Working, cx), State::Running);
        assert_eq!(mark(Mark::Blocked, cx), State::Asking);
        assert_eq!(mark(Mark::Idle, cx), State::Finished);
        assert_eq!(mark(Mark::Failed, cx), State::Failed);
        // Noted once, however often it is said.
        shell.update(cx, |shell, cx| shell.note_started(&parent, &worker, cx));
        assert_eq!(started(cx).len(), 1);
        // Its pane closed, it is still remembered, as gone.
        shell.update_in(cx, |shell, window, cx| {
            shell.close_pane(&worker, window, cx)
        });
        let known = started(cx);
        assert_eq!(
            (known.len(), known[0].state, known[0].title.as_str()),
            (1, State::Gone, "Run the tests")
        );
    }

    #[gpui::test]
    #[ignore = "requires the installed Codex CLI"]
    fn the_launcher_starts_codex_with_the_permissions_chosen_before_the_first_prompt(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        cx.executor().allow_parking();
        cx.update(|window, _| window.activate_window());
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap().join("project");
        let home = dir.path().join("profile");
        std::fs::create_dir(&cwd).unwrap();
        std::fs::create_dir(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            r#"model_provider = "fixture"
[model_providers.fixture]
name = "Local fixture"
base_url = "http://127.0.0.1:1/v1"
wire_api = "responses"
"#,
        )
        .unwrap();
        let path = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join(format!("codex{}", std::env::consts::EXE_SUFFIX)))
            .find(|path| path.is_file())
            .expect("installed Codex CLI");
        shell.update(cx, |shell, cx| {
            shell.store.session_mut(&session).unwrap().cwd = cwd.clone();
            let profile = shell.store.add_profile("codex", home);
            shell.detected.push(kou_agents::Detected {
                spec: kou_agents::spec("codex").unwrap(),
                path: Some(path),
                version: None,
                readiness: kou_agents::Readiness::Ready,
                message: None,
                native: false,
                acp: None,
                hooks: false,
                chat: kou_agents::Chat::None,
            });
            shell.probes = true;
            shell.set_direct("codex", true, cx);
            shell.choose_account("codex", Some(profile), PaneKind::Thread, cx);
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        let at = loop {
            draw(cx);
            if let Some(at) = shell.read_with(cx, |shell, _| {
                shell
                    .offered
                    .get("codex")
                    .and_then(|o| o.access.choices.iter().position(|c| c.id == "auto"))
            }) {
                break at + 1;
            }
            assert!(
                Instant::now() < deadline,
                "Codex did not offer access profiles"
            );
            std::thread::sleep(Duration::from_millis(15));
        };
        let button = cx
            .debug_bounds("access-picker")
            .expect("launcher permissions control");
        cx.simulate_click(button.center(), gpui::Modifiers::none());
        draw(cx);
        assert!(
            cx.debug_bounds("choice-0").is_some(),
            "launcher permissions menu"
        );
        for _ in 0..at {
            cx.simulate_keystrokes("down");
        }
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(
            shell.read_with(cx, |s, _| s.ask_access.clone()),
            Some((cwd, "auto".into()))
        );
        cx.simulate_keystrokes("enter");
        let pane = shell.read_with(cx, |s, _| {
            s.store.panes_of(&session).last().unwrap().id.clone()
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            draw(cx);
            let access = shell.read_with(cx, |s, cx| {
                let view = s.threads.get(&pane)?.view();
                let thread = view.read(cx).thread();
                assert!(
                    !matches!(thread.state(), kou_thread::ThreadState::Closed(_)),
                    "{:?}",
                    thread.state()
                );
                thread.access.current.clone()
            });
            if access.as_deref() == Some("auto") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the launched Codex did not confirm Auto"
            );
            std::thread::sleep(Duration::from_millis(15));
        }
        shell.update_in(cx, |shell, window, cx| shell.close_pane(&pane, window, cx));
    }

    #[gpui::test]
    fn a_model_chosen_on_the_empty_screen_is_the_started_agents(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        let model = |id: &str| kou_thread::Choice {
            id: id.into(),
            name: id.into(),
            description: None,
        };
        shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: false,
                adapter: None,
            });
            // What the harness said it offers (a test's window asks none).
            shell.offered.insert(
                "claude".into(),
                Offered {
                    models: vec![model("default"), model("opus"), model("sonnet")],
                    providers: Vec::new(),
                    current: Some("opus".into()),
                    cwd: "/fixture/atlas".into(),
                    at: Some(Instant::now()),
                    ..Default::default()
                },
            );
            shell.asked = false;
        });
        draw(cx);
        let choose = |id: &str, cx: &mut VisualTestContext| {
            shell.update_in(cx, |shell, window, cx| {
                let field = shell.prompt.clone().unwrap();
                let chosen = kou_thread_view::PromptFieldEvent::Model(id.into());
                shell.prompt_event(&field, &chosen, window, cx);
            });
            draw(cx);
        };
        let started = |cx: &mut VisualTestContext| {
            cx.simulate_keystrokes("enter");
            shell.read_with(cx, |shell, _| {
                assert_eq!(shell.ask_model, None, "the choice went with the agent");
                let pane = shell.store.panes_of(&session).last().unwrap();
                (pane.harness.clone(), pane.model.clone())
            })
        };
        // Chosen in the field's control, for the harness it names.
        choose("sonnet", cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(
                shell.ask_model,
                Some(("claude".to_owned(), "sonnet".to_owned()))
            );
        });
        assert_eq!(
            started(cx),
            ("claude".to_owned(), Some("sonnet".to_owned()))
        );
        // The harness's own default is nothing to tell it.
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-t"));
        draw(cx);
        choose("default", cx);
        assert_eq!(started(cx), ("claude".to_owned(), None));
        // Without a choice the pane has no model of its own either.
        draw(cx);
        cx.simulate_keystrokes(&format!("{APP}-t"));
        draw(cx);
        assert_eq!(started(cx), ("claude".to_owned(), None));
    }

    #[gpui::test]
    fn claude_code_talked_to_directly_is_a_chat_without_its_adapter(cx: &mut TestAppContext) {
        use serde_json::{Value, json};
        let (shell, cx, session) = open(cx);
        cx.executor().allow_parking();
        let home = tempfile::tempdir().unwrap();
        let pane = shell.update_in(cx, |shell, window, cx| {
            // Found as a program that only ever fails, and with no adapter:
            // no real harness is started here, or asked anything.
            for (harness, version) in [("claude", "2.1.325"), ("codex", "0.169.0")] {
                shell.detected.push(kou_agents::Detected {
                    spec: kou_agents::spec(harness).unwrap(),
                    path: Some("/usr/bin/false".into()),
                    version: Some(version.into()),
                    readiness: kou_agents::Readiness::Ready,
                    message: None,
                    native: false,
                    acp: None,
                    hooks: true,
                    chat: kou_agents::Chat::NeedsAdapter {
                        message: format!("{harness} needs its ACP adapter."),
                        install: None,
                    },
                });
            }
            shell.list_harnesses();
            // Through an adapter that is not here it is no chat, and says
            // what it waits for.
            let listed = shell.harness("claude").unwrap();
            assert!(!listed.native && listed.adapter.is_some());
            // Talked to directly it is one, and waits for nothing.
            shell.set_direct("claude", true, cx);
            let listed = shell.harness("claude").unwrap();
            assert!(listed.native && listed.adapter.is_none());
            let codex = shell.harness("codex").unwrap();
            assert!(!codex.native && codex.adapter.is_some());
            shell.set_direct("codex", true, cx);
            let codex = shell.harness("codex").unwrap();
            assert!(codex.native && codex.adapter.is_none());
            shell.set_direct("codex", false, cx);
            assert!(!shell.harness("codex").unwrap().native);
            assert!(shell.harness("claude").unwrap().native);

            // Its conversations are read from its records, under the
            // account the pane runs as.
            let account = shell.store.add_profile("claude", home.path().to_path_buf());
            let pane = shell
                .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                .unwrap();
            shell.store.pane_mut(&pane).unwrap().profile = Some(account);
            let cwd = shell.store.pane(&pane).unwrap().cwd.clone();
            let slug: String = cwd
                .to_string_lossy()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            let folder = home.path().join("projects").join(slug);
            std::fs::create_dir_all(&folder).unwrap();
            let id = "0b6f7f3e-2a51-4c0e-9d6b-6f1c2c0e8a11";
            let line = |kind: &str, uuid: &str, parent: Value, message: Value| {
                json!({
                    "parentUuid": parent,
                    "isSidechain": false,
                    "type": kind,
                    "message": message,
                    "uuid": uuid,
                    "timestamp": "2026-11-15T10:00:00.000Z",
                    "sessionId": id,
                    "cwd": cwd,
                    "version": "2.1.325",
                })
                .to_string()
            };
            let record = [
                line(
                    "user",
                    "11111111-1111-4111-8111-111111111111",
                    Value::Null,
                    json!({ "role": "user", "content": "Why is it slow?" }),
                ),
                line(
                    "assistant",
                    "22222222-2222-4222-8222-222222222222",
                    json!("11111111-1111-4111-8111-111111111111"),
                    json!({
                        "role": "assistant",
                        "content": [{ "type": "text", "text": "The index is missing." }],
                    }),
                ),
            ]
            .join("\n");
            std::fs::write(folder.join(format!("{id}.jsonl")), record + "\n").unwrap();
            shell.open_sessions(&pane, cx);
            pane
        });
        cx.run_until_parked();
        let listed = (0..200).find_map(|_| {
            let sessions = shell.read_with(cx, |shell, _| {
                shell
                    .resuming
                    .as_ref()
                    .filter(|open| open.pane == pane)
                    .and_then(|open| open.sessions.clone())
            });
            if sessions.is_none() {
                std::thread::sleep(Duration::from_millis(10));
                cx.run_until_parked();
            }
            sessions
        });
        let listed = listed.expect("the records were read");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "0b6f7f3e-2a51-4c0e-9d6b-6f1c2c0e8a11");
        assert_eq!(listed[0].title, "Why is it slow?");
    }

    #[cfg(unix)]
    #[gpui::test]
    fn a_claude_code_kept_apart_from_the_app_is_named_let_go_of_and_ended(cx: &mut TestAppContext) {
        use kou_thread::{Capabilities, Command, Update};
        let (shell, cx, session) = open(cx);
        // (Short: a socket's address holds about a hundred bytes.)
        let folder = tempfile::Builder::new()
            .prefix("kk")
            .tempdir_in("/tmp")
            .unwrap();
        let (pane, end) = shell.update_in(cx, |shell, window, cx| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: true,
                adapter: None,
            });
            let pane = shell
                .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                .unwrap();
            // A window that listens on no socket of its own keeps nothing.
            assert!(shell.keepers.is_none());
            assert_eq!(shell.pane_keeper(&pane, cx), None);
            assert_eq!(shell.store.pane(&pane).unwrap().keeper, None);

            // One that does names what it keeps for the pane, once: the
            // same name finds it again.
            shell.keepers = Some(folder.path().to_path_buf());
            let keeper = shell.pane_keeper(&pane, cx).expect("it is kept");
            let name = shell.store.pane(&pane).unwrap().keeper.clone().unwrap();
            assert!(name.starts_with(&format!("{pane}-")), "{name}");
            assert_eq!(keeper.stem, folder.path().join(&name));
            assert_eq!(keeper.host_args, ["claude-host"]);
            assert_eq!(shell.pane_keeper(&pane, cx), Some(keeper));

            // With a conversation in the pane, quitting would not end it.
            let saved = shell.store.pane(&pane).unwrap().clone();
            let (backend, end) = kou_thread::channel();
            shell.install_thread(&saved, backend, "Claude Code".into(), window, cx);
            let _ = end.updates.try_send(Update::Ready {
                session: "sess-7".into(),
                capabilities: Capabilities::default(),
                modes: Vec::new(),
                mode: None,
                models: Vec::new(),
                model: None,
            });
            let _ = end.updates.try_send(Update::TurnStarted);
            (pane, end)
        });
        cx.run_until_parked();
        shell.update_in(cx, |shell, _, cx| {
            assert!(shell.outlives_app(&pane));
            assert_eq!(
                shell.live.get(&pane).map(|live| live.mark),
                Some(Mark::Working)
            );
            assert_eq!(shell.at_work(), 0, "it works on without the app");
            // Without a keeper the same pane's work would be lost.
            let name = shell.store.pane_mut(&pane).unwrap().keeper.take();
            assert_eq!(shell.at_work(), 1);
            shell.store.pane_mut(&pane).unwrap().keeper = name;

            // The pane forgotten, its conversation is told to end (let go
            // of without a word, it would work on), and nothing is kept
            // under its name any more.
            shell.forget(&pane, cx);
            assert_eq!(shell.store.pane(&pane).unwrap().keeper, None);
            assert!(!shell.outlives_app(&pane));
        });
        let mut told = Vec::new();
        while let Ok(command) = end.commands.try_recv() {
            told.push(command);
        }
        assert_eq!(told.last(), Some(&Command::Close));

        // Switched off, nothing is kept: a name the pane had is dropped.
        shell.update_in(cx, |shell, _, cx| {
            shell.store.pane_mut(&pane).unwrap().keeper = Some(format!("{pane}-0000beef"));
            shell.store.claude_keep = false;
            assert_eq!(shell.pane_keeper(&pane, cx), None);
            assert_eq!(shell.store.pane(&pane).unwrap().keeper, None);
        });
    }

    #[cfg(unix)]
    #[gpui::test]
    fn a_kept_chat_that_lost_its_keeper_is_opened_again_once_a_minute_and_a_move_ends_its_keeper(
        cx: &mut TestAppContext,
    ) {
        use kou_thread::{Capabilities, Command, Update};
        let (shell, cx, session) = open(cx);
        let folder = tempfile::Builder::new()
            .prefix("kt")
            .tempdir_in("/tmp")
            .unwrap();
        let start = |shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>| {
            let pane = shell
                .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                .unwrap();
            shell.pane_keeper(&pane, cx).expect("it is kept");
            let saved = shell.store.pane(&pane).unwrap().clone();
            let (backend, end) = kou_thread::channel();
            shell.install_thread(&saved, backend, "Claude Code".into(), window, cx);
            let _ = end.updates.try_send(Update::Ready {
                session: "sess-8".into(),
                capabilities: Capabilities::default(),
                modes: Vec::new(),
                mode: None,
                models: Vec::new(),
                model: None,
            });
            (pane, end)
        };
        let (lost, lost_end, gone, gone_end) = shell.update_in(cx, |shell, window, cx| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: true,
                adapter: None,
            });
            shell.keepers = Some(folder.path().to_path_buf());
            let (lost, lost_end) = start(shell, window, cx);
            let (gone, gone_end) = start(shell, window, cx);
            // The first one's keeper is still there; the second one's ended
            // and took its socket with it.
            let name = shell.store.pane(&lost).unwrap().keeper.clone().unwrap();
            std::fs::write(folder.path().join(format!("{name}-1.sock")), "").unwrap();
            (lost, lost_end, gone, gone_end)
        });
        cx.run_until_parked();
        for end in [&lost_end, &gone_end] {
            let _ = end
                .updates
                .try_send(Update::Closed(Some("the connection was lost".into())));
        }
        cx.run_until_parked();
        let log = folder.path().join(kou_claude::host::LOG);
        shell.update_in(cx, |shell, _, _| {
            // Opened again: here nothing can be started (no Claude Code is
            // installed for the test), which its pane says.
            assert!(shell.taken_up.contains_key(&lost));
            assert!(!shell.taken_up.contains_key(&gone));
            assert!(!shell.threads.contains_key(&lost));
            assert!(shell.threads.contains_key(&gone));
            let said = std::fs::read_to_string(&log).unwrap();
            assert!(said.contains("app"), "{said}");
            assert!(
                said.trim_end().ends_with("the chat is opened again"),
                "{said}"
            );
        });

        // Once a minute at most.
        let again = shell.update_in(cx, |shell, window, cx| {
            let (backend, end) = kou_thread::channel();
            let saved = shell.store.pane(&lost).unwrap().clone();
            shell.install_thread(&saved, backend, "Claude Code".into(), window, cx);
            end
        });
        let _ = again
            .updates
            .try_send(Update::Closed(Some("lost again".into())));
        cx.run_until_parked();
        shell.update_in(cx, |shell, _, cx| {
            assert!(
                shell.threads.contains_key(&lost),
                "not opened again so soon"
            );
            assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);

            // Moved to another account, the pane's Claude Code is ended and
            // its conversation goes on under a new keeper's name.
            shell.end_kept(&gone);
            assert_eq!(shell.store.pane(&gone).unwrap().keeper, None);
            assert!(!shell.threads.contains_key(&gone));
            let keeper = shell.pane_keeper(&gone, cx).expect("kept anew");
            assert!(keeper.stem.starts_with(folder.path()));
        });
        let mut told = Vec::new();
        while let Ok(command) = gone_end.commands.try_recv() {
            told.push(command);
        }
        assert_eq!(told.last(), Some(&Command::Close));
        drop(lost_end);
    }

    #[gpui::test]
    fn a_chat_goes_to_its_terminal_with_its_conversation_or_anew_without_one(
        cx: &mut TestAppContext,
    ) {
        use kou_thread::{Block, Capabilities, Update};
        let (shell, cx, session) = open(cx);
        shell.update(cx, |shell, _| {
            shell.harnesses.push(HarnessInfo {
                id: "claude".into(),
                name: "Claude Code".into(),
                code: "CC".into(),
                ready: true,
                version: None,
                message: None,
                native: true,
                adapter: None,
            });
            // Found as a program that only ever fails: no real harness is
            // started here, or asked anything.
            shell.detected.push(kou_agents::Detected {
                spec: kou_agents::spec("claude").unwrap(),
                path: Some("/usr/bin/false".into()),
                version: Some("2.1.322".into()),
                readiness: kou_agents::Readiness::Ready,
                message: None,
                native: false,
                acp: None,
                hooks: true,
                chat: kou_agents::Chat::None,
            });
        });
        // A chat on a session of the harness's, with a scripted agent.
        let chat = |said: bool, cx: &mut VisualTestContext| {
            let pane = shell.update_in(cx, |shell, window, cx| {
                let pane = shell
                    .add_pane(&session, "claude", None, PaneKind::Thread, window, cx)
                    .unwrap();
                shell.store.pane_mut(&pane).unwrap().resume = Some("sess-7".into());
                let saved = shell.store.pane(&pane).unwrap().clone();
                let (backend, end) = kou_thread::channel();
                shell.install_thread(&saved, backend, "Claude Code".into(), window, cx);
                let mut updates = vec![Update::Ready {
                    session: "sess-7".into(),
                    capabilities: Capabilities::default(),
                    modes: Vec::new(),
                    mode: None,
                    models: Vec::new(),
                    model: None,
                }];
                if said {
                    updates.push(Update::UserChunk(Block::Text("Why is it slow?".into())));
                }
                for update in updates {
                    let _ = end.updates.try_send(update);
                }
                // The scripted agent never hangs up.
                std::mem::forget(end);
                pane
            });
            draw(cx);
            pane
        };
        let switched = |pane: &Id, cx: &mut VisualTestContext| {
            shell.update_in(cx, |shell, window, cx| {
                assert!(shell.switch_interface(pane, window, cx));
                let saved = shell.store.pane(pane).unwrap();
                (saved.kind, saved.resume.clone())
            })
        };
        // Nothing said yet: the harness has no such session to resume, and
        // its terminal starts anew.
        let silent = chat(false, cx);
        assert_eq!(switched(&silent, cx), (PaneKind::Terminal, None));
        // Something said: the terminal carries the conversation on.
        let spoken = chat(true, cx);
        assert_eq!(
            switched(&spoken, cx),
            (PaneKind::Terminal, Some("sess-7".to_owned()))
        );
    }

    #[gpui::test]
    fn the_user_is_told_when_an_agent_out_of_sight_finishes_or_asks(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        // Two tabs; the second is in front, so the first is out of sight.
        let (behind, front) = (start(&shell, &session, cx), start(&shell, &session, cx));
        // A worker of the one behind, in a tab of its own.
        let worker = shell.update_in(cx, |shell, window, cx| {
            let worker = shell
                .add_pane(
                    &session,
                    "shell",
                    Some(behind.clone()),
                    PaneKind::Terminal,
                    window,
                    cx,
                )
                .unwrap();
            shell.focus_pane(&front, cx);
            worker
        });
        cx.update(|window, _| window.activate_window());
        draw(cx);
        let told = |cx: &mut VisualTestContext| {
            shell.update(cx, |shell, _| std::mem::take(&mut shell.announced))
        };
        let mark = |pane: &Id, mark: Mark, cx: &mut VisualTestContext| {
            shell.update_in(cx, |shell, window, cx| {
                shell.set_mark(pane, mark, window, cx)
            });
        };

        // Finished behind the tab in front: told, with where it is.
        mark(&behind, Mark::Working, cx);
        assert!(told(cx).is_empty(), "starting to work is no news");
        mark(&behind, Mark::Idle, cx);
        assert_eq!(told(cx), [("Shell is done".to_owned(), "atlas".to_owned())]);
        // Stopped to ask: told, and once.
        mark(&behind, Mark::Working, cx);
        mark(&behind, Mark::Blocked, cx);
        mark(&behind, Mark::Blocked, cx);
        assert_eq!(
            told(cx),
            [("Shell needs you".to_owned(), "atlas".to_owned())]
        );

        // The pane being looked at needs no notification.
        mark(&front, Mark::Working, cx);
        mark(&front, Mark::Idle, cx);
        mark(&front, Mark::Blocked, cx);
        assert!(told(cx).is_empty());

        // A worker's end is its starter's to hear of; its question is the user's.
        mark(&worker, Mark::Working, cx);
        mark(&worker, Mark::Idle, cx);
        assert!(told(cx).is_empty());
        mark(&worker, Mark::Blocked, cx);
        assert_eq!(told(cx).len(), 1);

        // Each can be turned off by itself, and the choice is kept.
        shell.update(cx, |shell, cx| shell.toggle_notice(false, cx));
        mark(&behind, Mark::Working, cx);
        mark(&behind, Mark::Idle, cx);
        assert!(told(cx).is_empty());
        mark(&behind, Mark::Blocked, cx);
        assert_eq!(told(cx).len(), 1);
        shell.update(cx, |shell, cx| shell.toggle_notice(true, cx));
        mark(&behind, Mark::Idle, cx);
        mark(&behind, Mark::Blocked, cx);
        assert!(told(cx).is_empty());
        shell.read_with(cx, |shell, _| {
            assert!(!shell.store.notify_done && !shell.store.notify_asks);
            // A window made for a test posts nothing to the system.
            assert!(!shell.notifies);
        });

        // A click on a notification shows the pane it was about; one about
        // a pane that is gone only brings the app forward.
        use crate::notify::{Heard, Permission};
        let hear = |heard: Heard, cx: &mut VisualTestContext| {
            shell.update_in(cx, |shell, window, cx| shell.heard(heard, window, cx));
        };
        let in_front = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                let session = shell.store.session(&session).unwrap();
                session.front().map(|tab| tab.active.clone())
            })
        };
        assert_eq!(in_front(cx), Some(front.clone()));
        hear(Heard::Clicked(format!("pane:{behind}")), cx);
        assert_eq!(in_front(cx), Some(behind.clone()));
        hear(Heard::Clicked("pane:gone".into()), cx);
        assert_eq!(in_front(cx), Some(behind.clone()));
        // What the system allows is kept as it last said.
        hear(Heard::Permission(Permission::Refused), cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.notice_permission, Permission::Refused)
        });
        hear(Heard::Permission(Permission::Allowed), cx);
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.notice_permission, Permission::Allowed)
        });
    }

    #[gpui::test]
    fn an_agent_not_chosen_yet_is_one_tab_that_waits_in_the_strip(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        let (a, b) = (start(&shell, &session, cx), start(&shell, &session, cx));
        draw(cx);
        let waiting = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.pending_tab(&session).map(|picker| picker.front)
            })
        };
        let front = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                (
                    shell.pending_in_front(),
                    shell.store.session(&session).unwrap().focused.clone(),
                )
            })
        };
        let keys = |cx: &mut VisualTestContext| {
            draw(cx);
            shell.read_with(cx, |shell, _| shell.keys_at.clone())
        };
        assert_eq!(waiting(cx), None);
        assert_eq!(keys(cx), Some(b.clone()));

        // The key opens it in front; the panes behind it keep their places.
        cx.simulate_keystrokes(&format!("{APP}-t"));
        assert_eq!(waiting(cx), Some(true));
        assert_eq!(front(cx), (true, Some(b.clone())));
        assert_eq!(keys(cx), None, "no pane has the keys under it");
        // Asked for again, it is the same one.
        cx.simulate_keystrokes(&format!("{APP}-t"));
        shell.update(cx, |shell, cx| shell.pick(None, cx));
        assert_eq!(waiting(cx), Some(true));

        // Another tab chosen: it waits in the strip, and Escape is the pane's.
        shell.update(cx, |shell, cx| shell.focus_pane(&a, cx));
        assert_eq!(waiting(cx), Some(false));
        assert_eq!(front(cx), (false, Some(a.clone())));
        assert_eq!(keys(cx), Some(a.clone()));
        cx.simulate_keystrokes("escape");
        assert_eq!(
            waiting(cx),
            Some(false),
            "it is not what Escape closes from here"
        );
        // There is nothing to split or to lay out in it, and it is the last of the tabs.
        cx.simulate_keystrokes(&format!("{APP}-}}"));
        assert_eq!(front(cx), (false, Some(b.clone())));
        cx.simulate_keystrokes(&format!("{APP}-}}"));
        assert_eq!(front(cx), (true, Some(b.clone())));
        cx.simulate_keystrokes(&format!("{APP}-d"));
        cx.simulate_keystrokes(&format!("{APP}-alt-2"));
        shell.read_with(cx, |shell, _| {
            let saved = shell.store.session(&session).unwrap();
            assert_eq!(saved.tabs.len(), 2);
            assert!(saved.tabs.iter().all(|tab| tab.layout == Layout::Panes));
            assert_eq!(shell.store.panes_of(&session).count(), 2);
        });
        cx.simulate_keystrokes(&format!("{APP}-}}"));
        assert_eq!(front(cx), (false, Some(a.clone())), "round to the first");
        cx.simulate_keystrokes(&format!("{APP}-{{"));
        assert!(front(cx).0, "and back");

        // Cmd+W closes the tab in front: this one, not the pane behind it.
        cx.simulate_keystrokes(&format!("{APP}-w"));
        assert_eq!(waiting(cx), None);
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.panes_of(&session).count()),
            2
        );
        // Escape closes it too, from in front.
        cx.simulate_keystrokes(&format!("{APP}-t"));
        cx.simulate_keystrokes("escape");
        assert_eq!(waiting(cx), None);

        // An agent chosen in it: the tab is that agent's, at the end of the strip.
        cx.simulate_keystrokes(&format!("{APP}-t"));
        let made = shell.update_in(cx, |shell, window, cx| {
            shell.start("shell", PaneKind::Terminal, window, cx);
            shell
                .store
                .session(&session)
                .unwrap()
                .focused
                .clone()
                .unwrap()
        });
        assert_eq!(waiting(cx), None);
        shell.read_with(cx, |shell, _| {
            let tabs = &shell.store.session(&session).unwrap().tabs;
            assert_eq!(tabs.len(), 3);
            assert_eq!(tabs[2].active, made);
        });

        // It waits in its session while another is looked at, and moves to
        // the session it is next asked for in: there is only ever one.
        cx.simulate_keystrokes(&format!("{APP}-t"));
        let other = shell.read_with(cx, |shell, _| shell.store.sessions[1].id.clone());
        shell.update(cx, |shell, cx| shell.select_session(&other, cx));
        assert_eq!(waiting(cx), Some(true));
        assert!(!shell.read_with(cx, |shell, _| shell.pending_in_front()));
        shell.update(cx, |shell, cx| shell.pick(None, cx));
        assert_eq!(waiting(cx), None);
        assert!(shell.read_with(cx, |shell, _| {
            shell.pending_tab(&other).is_some() && shell.pending_in_front()
        }));
    }

    #[gpui::test]
    fn a_space_and_a_session_are_renamed_where_their_names_stand(cx: &mut TestAppContext) {
        let (shell, cx, session) = open(cx);
        let pane = start(&shell, &session, cx);
        draw(cx);
        let space = shell.read_with(cx, |shell, _| shell.store.spaces[0].id.clone());
        let names = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                (
                    shell.store.space(&space).unwrap().name.clone(),
                    shell.store.session(&session).unwrap().name.clone(),
                )
            })
        };
        assert_eq!(names(cx), ("atlas".to_owned(), "atlas".to_owned()));

        // The session: its name in a field, selected; typing replaces it, Enter keeps it.
        shell.update_in(cx, |shell, window, cx| {
            shell.edit_name(Named::Session(session.clone()), window, cx);
            assert!(shell.tab_focus.is_focused(window));
            assert_eq!(shell.name_edit.as_ref().unwrap().field.text(), "atlas");
        });
        draw(cx);
        cx.simulate_input("  parser   rewrite ");
        assert_eq!(names(cx).1, "atlas", "nothing is kept until Enter");
        cx.simulate_keystrokes("enter");
        assert_eq!(names(cx).1, "parser rewrite");
        assert!(shell.read_with(cx, |shell, _| shell.name_edit.is_none()));
        // The keys go back to the pane in front.
        draw(cx);
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.keys_at.clone()),
            Some(pane.clone())
        );

        // The space: Escape leaves it, an empty name changes nothing, and
        // taking the keys elsewhere keeps what was typed.
        shell.update_in(cx, |shell, window, cx| {
            shell.edit_name(Named::Space(space.clone()), window, cx);
        });
        draw(cx);
        cx.simulate_input("x");
        cx.simulate_keystrokes("escape");
        assert_eq!(names(cx).0, "atlas");
        shell.update_in(cx, |shell, window, cx| {
            shell.edit_name(Named::Space(space.clone()), window, cx);
            shell.name_edit.as_mut().unwrap().field = kou_canvas::field::Field::new("   ", false);
            shell.commit_name_edit(cx);
        });
        assert_eq!(names(cx).0, "atlas");
        // (A window that is not the active one is told of no focus changes.)
        cx.update(|window, _| window.activate_window());
        shell.update_in(cx, |shell, window, cx| {
            shell.edit_name(Named::Space(space.clone()), window, cx);
        });
        draw(cx);
        cx.simulate_input("client work");
        shell.update_in(cx, |shell, window, cx| {
            assert_eq!(
                shell.name_edit.as_ref().unwrap().field.text(),
                "client work"
            );
            assert!(shell.tab_focus.is_focused(window));
            window.focus(&shell.focus, cx)
        });
        // The window tells who lost the keys when it draws its next frame.
        draw(cx);
        assert_eq!(names(cx).0, "client work");
        assert!(shell.read_with(cx, |shell, _| shell.name_edit.is_none()));

        // One field at a time: opening a tab's name closes the rail's, keeping it.
        shell.update_in(cx, |shell, window, cx| {
            shell.edit_name(Named::Session(session.clone()), window, cx);
            shell.name_edit.as_mut().unwrap().field = kou_canvas::field::Field::new("lexer", false);
            shell.edit_tab(&session, 0, window, cx);
            assert!(shell.name_edit.is_none() && shell.tab_edit.is_some());
        });
        assert_eq!(names(cx).1, "lexer");
    }

    #[gpui::test]
    fn tabs_are_walked_by_their_own_keys_and_a_tab_closes_with_all_its_panes(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, session) = open(cx);
        let (a, b) = (start(&shell, &session, cx), start(&shell, &session, cx));
        draw(cx);
        let focused = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| {
                shell.store.session(&session).unwrap().focused.clone()
            })
        };
        assert_eq!(focused(cx), Some(b.clone()));
        cx.simulate_keystrokes(&format!("{APP}-shift-["));
        assert_eq!(focused(cx), Some(a.clone()));
        // With one pane in the tab, the pane keys go on to the next tab.
        cx.simulate_keystrokes(&format!("{APP}-]"));
        assert_eq!(focused(cx), Some(b.clone()));
        // A digit shows the tab at that place; one past the end is no tab.
        cx.simulate_keystrokes(&format!("{APP}-1"));
        assert_eq!(focused(cx), Some(a.clone()));
        cx.simulate_keystrokes(&format!("{APP}-2"));
        assert_eq!(focused(cx), Some(b.clone()));
        cx.simulate_keystrokes(&format!("{APP}-7"));
        assert_eq!(focused(cx), Some(b.clone()));
        // The arrows walk the tabs, round the end.
        #[cfg(target_os = "macos")]
        {
            cx.simulate_keystrokes("cmd-left");
            assert_eq!(focused(cx), Some(a.clone()));
            cx.simulate_keystrokes("cmd-left");
            assert_eq!(focused(cx), Some(b.clone()));
            cx.simulate_keystrokes("cmd-right");
            assert_eq!(focused(cx), Some(a.clone()));
            cx.simulate_keystrokes("cmd-right");
            assert_eq!(focused(cx), Some(b.clone()));
        }

        cx.simulate_keystrokes(&format!("{APP}-d"));
        shell.update_in(cx, |shell, window, cx| {
            shell.start("shell", PaneKind::Terminal, window, cx)
        });
        shell.update_in(cx, |shell, window, cx| {
            shell.close_tab(&session, 1, window, cx)
        });
        shell.read_with(cx, |shell, _| {
            let session = shell.store.session(&session).unwrap();
            assert_eq!(session.tabs.len(), 1);
            assert_eq!(session.tabs[0].root.panes(), [&a]);
            assert_eq!(session.focused.as_ref(), Some(&a));
            assert_eq!(shell.store.panes_of(&session.id).count(), 1);
        });

        // The rail's width is set by dragging its edge, within limits.
        shell.update(cx, |shell, cx| {
            shell.set_rail_width(10., cx);
            assert_eq!(shell.store.rail_width, RAIL_WIDTHS.0);
            shell.set_rail_width(5000., cx);
            assert_eq!(shell.store.rail_width, RAIL_WIDTHS.1);
            shell.set_rail_width(300.4, cx);
            assert_eq!(shell.store.rail_width, 300.);
        });
    }

    #[cfg(target_os = "macos")]
    const APP: &str = "cmd";
    #[cfg(not(target_os = "macos"))]
    const APP: &str = "ctrl-shift";

    // A control answers a key when it is let go, as a button does a pointer.
    fn press(key: &'static str, cx: &mut VisualTestContext) {
        cx.simulate_keystrokes(key);
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse(key).unwrap(),
        });
    }

    #[gpui::test]
    fn tab_reaches_the_rail_in_reading_order_and_enter_or_space_activates(cx: &mut TestAppContext) {
        let (shell, cx, first) = open(cx);
        // Mark seen, hide the rail, add a space, then the first space's row.
        cx.simulate_keystrokes("tab tab tab tab");
        press("enter", cx);
        assert!(shell.read_with(cx, |shell, _| shell.store.spaces[0].collapsed));
        press("space", cx);
        assert!(!shell.read_with(cx, |shell, _| shell.store.spaces[0].collapsed));

        // Next the space's plus, which starts a session there and shows it.
        cx.simulate_keystrokes("tab");
        press("enter", cx);
        let third = shell.read_with(cx, |shell, _| shell.store.selected.clone().unwrap());
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.sessions.len()),
            3
        );
        // Then the session rows, oldest first.
        cx.simulate_keystrokes("tab");
        press("enter", cx);
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.store.selected.clone()),
            Some(first)
        );
        assert_ne!(
            third,
            shell.read_with(cx, |shell, _| shell.store.selected.clone().unwrap())
        );

        // Back up the rail: three stops before the first space's row hides the rail.
        cx.simulate_keystrokes("shift-tab shift-tab shift-tab shift-tab");
        press("enter", cx);
        assert!(shell.read_with(cx, |shell, _| shell.store.rail_hidden));
    }

    #[gpui::test]
    fn the_keys_switch_layouts_open_the_picker_and_escape_closes_it(cx: &mut TestAppContext) {
        let (shell, cx, first) = open(cx);
        let layout = |cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, _| shell.store.session(&first).unwrap().layout())
        };
        // Without a tab there is nothing to show as a canvas.
        cx.simulate_keystrokes(&format!("{APP}-alt-2"));
        assert_eq!(layout(cx), Layout::Panes);
        // The layout is the tab's: one tab a canvas, the one beside it panes.
        let (a, b) = (start(&shell, &first, cx), start(&shell, &first, cx));
        shell.update(cx, |shell, cx| shell.focus_pane(&a, cx));
        cx.simulate_keystrokes(&format!("{APP}-alt-2"));
        assert_eq!(layout(cx), Layout::Canvas);
        shell.update(cx, |shell, cx| shell.focus_pane(&b, cx));
        assert_eq!(layout(cx), Layout::Panes);
        shell.update(cx, |shell, cx| shell.focus_pane(&a, cx));
        assert_eq!(layout(cx), Layout::Canvas);
        cx.simulate_keystrokes(&format!("{APP}-alt-1"));
        assert_eq!(layout(cx), Layout::Panes);
        // A digit alone is a tab's place in the strip, and changes no layout.
        cx.simulate_keystrokes(&format!("{APP}-2"));
        assert_eq!(layout(cx), Layout::Panes);

        cx.simulate_keystrokes(&format!("{APP}-b"));
        assert!(shell.read_with(cx, |shell, _| shell.store.rail_hidden));
        cx.simulate_keystrokes(&format!("{APP}-b"));
        assert!(!shell.read_with(cx, |shell, _| shell.store.rail_hidden));

        cx.simulate_keystrokes(&format!("{APP}-t"));
        assert!(shell.read_with(cx, |shell, _| shell.picker.is_some()));
        cx.simulate_keystrokes("escape");
        assert!(shell.read_with(cx, |shell, _| shell.picker.is_none()));
    }

    #[gpui::test]
    fn a_pane_gets_a_node_work_finished_out_of_sight_stays_done_until_seen(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, first) = open(cx);
        let (lead, child) = shell.update_in(cx, |shell, window, cx| {
            let lead = shell
                .add_pane(&first, "shell", None, PaneKind::Terminal, window, cx)
                .unwrap();
            let child = shell
                .add_pane(
                    &first,
                    "shell",
                    Some(lead.clone()),
                    PaneKind::Terminal,
                    window,
                    cx,
                )
                .unwrap();
            assert!(
                shell
                    .add_pane(&first, "nope", None, PaneKind::Terminal, window, cx)
                    .is_none()
            );
            (lead, child)
        });
        shell.update_in(cx, |shell, window, cx| {
            // Started from a pane shown as panes, the child is a tab of its
            // own, with a board of its own: one node each, no line between.
            let (lead_tab, child_tab) = (
                shell.store.tab_of(&lead).unwrap().id.clone(),
                shell.store.tab_of(&child).unwrap().id.clone(),
            );
            assert_ne!(lead_tab, child_tab);
            for tab in [&lead_tab, &child_tab] {
                let board = shell.canvas(tab, cx).unwrap().read(cx).board().clone();
                assert_eq!((board.nodes().len(), board.edges().len()), (1, 0));
            }
            assert_eq!(
                shell.store.pane(&child).unwrap().parent.as_deref(),
                Some(lead.as_str())
            );
            // The child has the focus; the lead works and finishes behind it.
            assert_eq!(
                shell.store.session(&first).unwrap().focused.as_deref(),
                Some(child.as_str())
            );
            shell.set_mark(&lead, Mark::Working, window, cx);
            assert_eq!(shell.live[&lead].shown(), Mark::Working);
            shell.set_mark(&lead, Mark::Idle, window, cx);
            assert_eq!(shell.live[&lead].shown(), Mark::Done);
            shell.focus_pane(&lead, cx);
            assert_eq!(shell.live[&lead].shown(), Mark::Idle);

            // Started from a pane shown on its canvas, the child joins that
            // canvas: beside its parent, with the line between them.
            shell.set_tab_layout(&lead_tab, Layout::Canvas, cx);
            let second = shell
                .add_pane(
                    &first,
                    "shell",
                    Some(lead.clone()),
                    PaneKind::Terminal,
                    window,
                    cx,
                )
                .unwrap();
            assert_eq!(shell.store.tab_of(&second).unwrap().id, lead_tab);
            let canvas = shell.canvas(&lead_tab, cx).unwrap();
            let board = canvas.read(cx).board().clone();
            assert_eq!((board.nodes().len(), board.edges().len()), (2, 1));

            // A pane that goes takes its node; the last one takes the tab,
            // and the tab its canvas.
            shell.close_pane(&lead, window, cx);
            assert!(shell.store.pane(&lead).is_none());
            assert_eq!(shell.store.pane(&second).unwrap().parent, None);
            assert_eq!(shell.store.pane(&child).unwrap().parent, None);
            assert_eq!(canvas.read(cx).board().nodes().len(), 1);
            assert!(shell.canvases.contains_key(&lead_tab));
            shell.close_pane(&second, window, cx);
            assert!(shell.store.tab(&lead_tab).is_none());
            assert!(!shell.canvases.contains_key(&lead_tab));
            assert!(shell.canvases.contains_key(&child_tab));
        });
    }
}
