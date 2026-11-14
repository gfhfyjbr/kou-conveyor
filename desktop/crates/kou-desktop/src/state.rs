//! What the app remembers between runs: spaces, their sessions, and the panes
//! each session had. Processes are not part of it; a pane restored from disk
//! is a record of what ran there, to be started again.

use kou_canvas::{Board, Camera, NodeId, NodeKind, NodeSpec};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Identifies a space, a session or a pane. Short, so an agent can type it.
pub type Id = String;

/// A folder of sessions: usually a project directory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Space {
    /// Its id.
    pub id: Id,
    /// Its name: the directory's, unless renamed.
    pub name: String,
    /// The directory new sessions start in.
    pub path: PathBuf,
    /// Whether its sessions are folded away in the rail.
    #[serde(default)]
    pub collapsed: bool,
    /// Whether its archived sessions are listed.
    #[serde(default)]
    pub archive_open: bool,
}

/// How a tab's panes are shown in the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// Side by side and one over the other, as the tab was split. What
    /// earlier builds saved as "single" and "tiles".
    #[default]
    #[serde(alias = "single", alias = "tiles")]
    Panes,
    /// The tab's panes as nodes on its own canvas, linked by who started whom.
    Canvas,
}

/// Which way a split divides its room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Side by side: the second part is to the right.
    Row,
    /// One over the other: the second part is below.
    Column,
}

/// The smallest share of a split either part can be dragged to.
pub const MIN_SHARE: f32 = 0.12;

/// The panes of a tab: one, or two parts divided along an axis, each of
/// which may be divided again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    /// One pane, by id.
    Pane(Id),
    /// Two parts.
    Parts {
        /// See [`Axis`].
        axis: Axis,
        /// The first part's share of the room, 0..1.
        ratio: f32,
        /// The left or upper part.
        first: Box<Split>,
        /// The right or lower part.
        second: Box<Split>,
    },
}

/// Left, top, width and height of something in the pane area.
pub type Place = [f32; 4];

/// A divider between the two parts of a split, as laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    /// Which split it divides: `false` for first, `true` for second, from the root.
    pub path: Vec<bool>,
    /// The split's axis.
    pub axis: Axis,
    /// The whole split's room, which a drag measures against.
    pub room: Place,
    /// The divider's own line.
    pub line: Place,
}

impl Split {
    /// Its panes, in reading order.
    pub fn panes(&self) -> Vec<&Id> {
        match self {
            Self::Pane(id) => vec![id],
            Self::Parts { first, second, .. } => {
                let mut panes = first.panes();
                panes.extend(second.panes());
                panes
            }
        }
    }

    /// Whether a pane is in it.
    pub fn contains(&self, pane: &str) -> bool {
        match self {
            Self::Pane(id) => id == pane,
            Self::Parts { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    /// Divide the room `at` has in two, with `new` after it. False if `at` is not here.
    pub fn divide(&mut self, at: &str, axis: Axis, new: Id) -> bool {
        match self {
            Self::Pane(id) if id == at => {
                *self = Self::Parts {
                    axis,
                    ratio: 0.5,
                    first: Box::new(Self::Pane(id.clone())),
                    second: Box::new(Self::Pane(new)),
                };
                true
            }
            Self::Pane(_) => false,
            Self::Parts { first, second, .. } => {
                if first.contains(at) {
                    first.divide(at, axis, new)
                } else {
                    second.divide(at, axis, new)
                }
            }
        }
    }

    /// Give `pane` this share (0..1) of the room it divides with what is
    /// beside it: the nearest line between panes that has `pane` alone on
    /// one side is moved. False if there is no such line here (a pane alone
    /// in its tab has all of it).
    pub fn set_share(&mut self, pane: &str, share: f32) -> bool {
        let share = share.clamp(0.1, 0.9);
        let Self::Parts {
            ratio,
            first,
            second,
            ..
        } = self
        else {
            return false;
        };
        match (first.as_ref(), second.as_ref()) {
            (Self::Pane(id), _) if id == pane => {
                *ratio = share;
                true
            }
            (_, Self::Pane(id)) if id == pane => {
                *ratio = 1. - share;
                true
            }
            _ => first.set_share(pane, share) || second.set_share(pane, share),
        }
    }

    /// Panes shared out evenly: halves, each halved the other way, and so
    /// on. `None` for no panes.
    pub fn tiled(panes: &[Id], axis: Axis) -> Option<Self> {
        match panes {
            [] => None,
            [one] => Some(Self::Pane(one.clone())),
            _ => {
                let (first, second) = panes.split_at(panes.len().div_ceil(2));
                let other = match axis {
                    Axis::Row => Axis::Column,
                    Axis::Column => Axis::Row,
                };
                Some(Self::Parts {
                    axis,
                    ratio: first.len() as f32 / panes.len() as f32,
                    first: Box::new(Self::tiled(first, other)?),
                    second: Box::new(Self::tiled(second, other)?),
                })
            }
        }
    }

    /// Make room for `new` where there is most of it: the pane with the
    /// largest room is divided along its longer side.
    pub fn divide_widest(&mut self, new: Id) {
        // The shape of a window's pane area, near enough to choose by.
        let (panes, _) = self.lay_out([0., 0., 1600., 1000.], 0.);
        let Some((at, [_, _, w, h])) = panes
            .into_iter()
            .max_by(|(_, a), (_, b)| (a[2] * a[3]).total_cmp(&(b[2] * b[3])))
        else {
            return;
        };
        let axis = if w >= h { Axis::Row } else { Axis::Column };
        self.divide(&at, axis, new);
    }

    /// The pane that takes a pane's room when it goes: the nearest one of the
    /// part beside it.
    pub fn neighbour(&self, pane: &str) -> Option<&Id> {
        let Self::Parts { first, second, .. } = self else {
            return None;
        };
        match (&**first, &**second) {
            (Self::Pane(id), beside) if id == pane => beside.panes().first().copied(),
            (beside, Self::Pane(id)) if id == pane => beside.panes().last().copied(),
            _ => first.neighbour(pane).or_else(|| second.neighbour(pane)),
        }
    }

    /// The same without a pane: the part beside it takes its room. `None`
    /// when the pane was all there was.
    pub fn without(self, pane: &str) -> Option<Self> {
        match self {
            Self::Pane(id) if id == pane => None,
            Self::Pane(_) => Some(self),
            Self::Parts {
                axis,
                ratio,
                first,
                second,
            } => match (first.without(pane), second.without(pane)) {
                (Some(first), Some(second)) => Some(Self::Parts {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }

    /// The share of the split at `path`, to change it.
    pub fn ratio_mut(&mut self, path: &[bool]) -> Option<&mut f32> {
        match (self, path) {
            (Self::Parts { ratio, .. }, []) => Some(ratio),
            (Self::Parts { first, second, .. }, [second_part, rest @ ..]) => {
                if *second_part { second } else { first }.ratio_mut(rest)
            }
            (Self::Pane(_), _) => None,
        }
    }

    /// Where each pane and each divider is when the whole has `room`, with
    /// `gap` between the two parts of a split.
    pub fn lay_out(&self, room: Place, gap: f32) -> (Vec<(Id, Place)>, Vec<Divider>) {
        let (mut panes, mut dividers) = (Vec::new(), Vec::new());
        self.place(room, gap, &mut Vec::new(), &mut panes, &mut dividers);
        (panes, dividers)
    }

    fn place(
        &self,
        room: Place,
        gap: f32,
        path: &mut Vec<bool>,
        panes: &mut Vec<(Id, Place)>,
        dividers: &mut Vec<Divider>,
    ) {
        let [x, y, w, h] = room;
        match self {
            Self::Pane(id) => panes.push((id.clone(), room)),
            Self::Parts {
                axis,
                ratio,
                first,
                second,
            } => {
                let ratio = ratio.clamp(MIN_SHARE, 1. - MIN_SHARE);
                let (one, line, two) = match axis {
                    Axis::Row => {
                        let left = ((w - gap) * ratio).round();
                        (
                            [x, y, left, h],
                            [x + left, y, gap, h],
                            [x + left + gap, y, w - gap - left, h],
                        )
                    }
                    Axis::Column => {
                        let top = ((h - gap) * ratio).round();
                        (
                            [x, y, w, top],
                            [x, y + top, w, gap],
                            [x, y + top + gap, w, h - gap - top],
                        )
                    }
                };
                dividers.push(Divider {
                    path: path.clone(),
                    axis: *axis,
                    room,
                    line,
                });
                path.push(false);
                first.place(one, gap, path, panes, dividers);
                path.pop();
                path.push(true);
                second.place(two, gap, path, panes, dividers);
                path.pop();
            }
        }
    }
}

/// One tab of a session: a pane, or panes it was split into. The same panes
/// are the nodes of the tab's own canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    /// Its id. Empty in a file from an earlier build until it is settled.
    #[serde(default)]
    pub id: Id,
    /// Its panes.
    pub root: Split,
    /// The pane of it that had the keys last.
    pub active: Id,
    /// The name the user gave it. Without one it is called after its pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The icon the user gave it, by the name `kou_ui::TAB_ICONS` saves it
    /// under. Without one it shows its pane's harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// See [`Layout`].
    #[serde(default)]
    pub layout: Layout,
    /// Where its panes sit on its canvas.
    #[serde(default)]
    pub board: Board,
    /// Its canvas's camera, once the user has moved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<Camera>,
}

impl Tab {
    /// A tab of one pane, named after it, with the board its node is on.
    pub fn new(id: Id, pane: &str, board: Board) -> Self {
        Self {
            id,
            root: Split::Pane(pane.to_owned()),
            active: pane.to_owned(),
            name: None,
            icon: None,
            layout: Layout::default(),
            board,
            camera: None,
        }
    }
}

/// Where a new pane goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Seat {
    /// A tab of its own, with the board its node is already on.
    Tab(Board),
    /// The tab `at` is in, dividing that pane's room along the axis.
    Beside {
        /// The pane whose room is divided.
        at: Id,
        /// Which way.
        axis: Axis,
    },
    /// A tab, by id, wherever it has the most room.
    In(Id),
}

/// A piece of work: a directory and the agents working in it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Its id.
    pub id: Id,
    /// The space it belongs to.
    pub space: Id,
    /// Its name.
    pub name: String,
    /// Where its agents run.
    pub cwd: PathBuf,
    /// Put away, out of the rail's main list.
    #[serde(default)]
    pub archived: bool,
    /// When it was created, in milliseconds since the epoch.
    pub created_ms: u64,
    /// What earlier builds kept for the whole session: one layout, one
    /// board, one camera. Each tab has its own now; these are read from an
    /// old file, shared out among the tabs when it is settled, never written.
    #[serde(default, rename = "layout", skip_serializing)]
    old_layout: Option<Layout>,
    #[serde(default, rename = "board", skip_serializing)]
    old_board: Option<Board>,
    #[serde(default, rename = "camera", skip_serializing)]
    old_camera: Option<Camera>,
    /// The pane last looked at. The tab on screen is the one it is in.
    #[serde(default)]
    pub focused: Option<Id>,
    /// Its tabs, in strip order. Every pane of the session is in exactly one.
    #[serde(default)]
    pub tabs: Vec<Tab>,
}

impl Session {
    /// Which tab a pane is in.
    pub fn tab_of(&self, pane: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.root.contains(pane))
    }

    /// The tab on screen: the focused pane's, else the first.
    pub fn tab(&self) -> Option<usize> {
        self.focused
            .as_deref()
            .and_then(|pane| self.tab_of(pane))
            .or((!self.tabs.is_empty()).then_some(0))
    }

    /// The tab on screen itself.
    pub fn front(&self) -> Option<&Tab> {
        self.tab().and_then(|tab| self.tabs.get(tab))
    }

    /// How the tab on screen shows its panes.
    pub fn layout(&self) -> Layout {
        self.front().map_or(Layout::Panes, |tab| tab.layout)
    }

    /// A tab by id.
    pub fn tab_by(&self, id: &str) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }
}

/// How a pane presents its agent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    /// The harness's own terminal interface.
    #[default]
    Terminal,
    /// A native conversation view.
    Thread,
}

/// One agent in a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    /// Its id.
    pub id: Id,
    /// The session it belongs to.
    pub session: Id,
    /// The harness it runs, by id.
    pub harness: String,
    /// Its name; the harness's until something better is known.
    pub title: String,
    /// Where it runs.
    pub cwd: PathBuf,
    /// See [`PaneKind`].
    #[serde(default)]
    pub kind: PaneKind,
    /// The pane that started it.
    #[serde(default)]
    pub parent: Option<Id>,
    /// Its node on its tab's board.
    pub node: NodeId,
    /// The harness's own session id, to resume it after a restart.
    #[serde(default)]
    pub resume: Option<String>,
    /// The model it was asked to run, in the harness's own naming. Without
    /// one the harness uses its default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The account it runs under (see [`Profile`]), by id; `None` is the
    /// harness's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Id>,
    /// The agents its agent started, in the order started: what its
    /// conversation shows of them, also once their own panes are gone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub started: Vec<Started>,
}

/// Another account of a harness: the same program with its home in another
/// folder (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`), so with a login, sessions and
/// settings of its own. The app writes nothing there; signing in is done
/// once, by the user, in a pane of that account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: Id,
    /// The harness it is an account of, by id.
    pub harness: String,
    /// What it is called beside the harness's name.
    pub name: String,
    /// The folder its home variable is set to.
    pub home: PathBuf,
}

/// An agent a pane's agent started, as that pane remembers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    /// The pane it runs in, or ran in.
    pub id: Id,
    /// Its name and its harness, as last seen.
    pub title: String,
    pub harness: String,
    /// How many entries the starter's conversation had when it was started:
    /// where its card stands there.
    #[serde(default)]
    pub after: Option<usize>,
    /// What it was last seen doing, and since when (seconds of the epoch).
    #[serde(default)]
    pub state: StartedState,
    pub since: u64,
    /// Its latest words, on one line.
    #[serde(default)]
    pub detail: String,
}

/// What an agent another one started is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartedState {
    /// At work.
    #[default]
    Running,
    /// Stopped for an answer or a permission.
    Asking,
    /// Its turn is over.
    Finished,
    /// It ended badly.
    Failed,
    /// Its pane was closed.
    Gone,
}

/// How an agent started in a pane is given the means to work with the app's
/// other agents: starting them, sending to them, waiting, reading. Whatever
/// is given is attached to that one launch; nothing is written into the
/// harness's own configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orchestration {
    /// A skill and instructions. The agent does the work through the app's
    /// command line, which costs its context nothing until it is used.
    #[default]
    Skills,
    /// The app's tools through an MCP server, with the server's
    /// instructions. The tools are in the agent's context in every session.
    Mcp,
    /// Nothing: the harness is started as it is.
    Off,
}

/// Everything saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Store {
    /// Spaces in rail order.
    #[serde(default)]
    pub spaces: Vec<Space>,
    /// Sessions, oldest first.
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// Panes, oldest first.
    #[serde(default)]
    pub panes: Vec<Pane>,
    /// The session on screen.
    #[serde(default)]
    pub selected: Option<Id>,
    /// Whether the rail is hidden.
    #[serde(default)]
    pub rail_hidden: bool,
    /// How wide the rail is, in pixels, within [`RAIL_WIDTHS`].
    #[serde(default = "rail_width")]
    pub rail_width: f32,
    /// Whether a terminal's cursor glides between cells, stretched and sprung.
    #[serde(default = "yes")]
    pub smooth_cursor: bool,
    /// See [`Orchestration`].
    #[serde(default)]
    pub orchestration: Orchestration,
    /// The harness the user started last, by id: what a prompt typed on an
    /// empty screen goes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_harness: Option<String>,
    /// Whether the user is told, by a notification, that an agent they
    /// started has finished while they were not looking at it.
    #[serde(default = "yes")]
    pub notify_done: bool,
    /// Whether the user is told that an agent stopped to ask something.
    #[serde(default = "yes")]
    pub notify_asks: bool,
    /// The accent the user chose in place of the palette's, as `#rrggbb`: one
    /// of the settings', or any colour written here by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// The other accounts of harnesses, in the order added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<Profile>,
    /// The account the harness started last was started under, when it was
    /// not the harness's own: what a prompt on an empty screen goes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_profile: Option<Id>,
    #[serde(default)]
    next: u64,
}

/// What an account is called, from its home's folder: the folder's name
/// without a leading dot and without the harness's own name before it, so
/// `~/.claude-work` is `work`.
pub fn profile_name(harness: &str, home: &Path) -> String {
    let folder = home
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = folder.trim_start_matches('.');
    let short = name
        .strip_prefix(harness)
        .map(|rest| rest.trim_start_matches(['-', '_', '.', ' ']))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(name);
    if short.is_empty() {
        "account".to_owned()
    } else {
        short.to_owned()
    }
}

/// The narrowest and the widest the rail can be dragged to.
pub const RAIL_WIDTHS: (f32, f32) = (208., 440.);

fn rail_width() -> f32 {
    264.
}

impl Default for Store {
    fn default() -> Self {
        Self {
            spaces: Vec::new(),
            sessions: Vec::new(),
            panes: Vec::new(),
            selected: None,
            rail_hidden: false,
            rail_width: rail_width(),
            smooth_cursor: true,
            orchestration: Orchestration::default(),
            last_harness: None,
            notify_done: true,
            notify_asks: true,
            accent: None,
            profiles: Vec::new(),
            last_profile: None,
            next: 0,
        }
    }
}

fn yes() -> bool {
    true
}

// The node a pane has on a board: what the board needs to know of it. Its
// harness's letters and where it works are filled in when it is shown.
fn node_for(pane: &Pane) -> NodeSpec {
    NodeSpec {
        kind: if pane.harness == "shell" {
            NodeKind::Terminal
        } else {
            NodeKind::Agent
        },
        title: pane.title.clone(),
        ..NodeSpec::default()
    }
}

/// Milliseconds since the epoch, now.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// The name a directory gives a space or a session: its last component.
pub fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

impl Store {
    /// The chosen accent as `0xRRGGBB`, when there is one and it reads as a
    /// colour.
    pub fn accent_rgb(&self) -> Option<u32> {
        let hex = self.accent.as_deref()?.trim().trim_start_matches('#');
        (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| u32::from_str_radix(hex, 16).ok())
            .flatten()
    }

    /// The account with this id.
    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// The accounts of a harness beside its own, in the order added.
    pub fn profiles_of<'a>(&'a self, harness: &'a str) -> impl Iterator<Item = &'a Profile> {
        self.profiles
            .iter()
            .filter(move |profile| profile.harness == harness)
    }

    /// Add an account of `harness` whose home is `home`, named after the
    /// folder. The same folder again is the account there already.
    pub fn add_profile(&mut self, harness: &str, home: PathBuf) -> Id {
        if let Some(known) = self
            .profiles_of(harness)
            .find(|profile| profile.home == home)
        {
            return known.id.clone();
        }
        let wanted = profile_name(harness, &home);
        // No two accounts of a harness under one name.
        let mut name = wanted.clone();
        let mut nth = 2;
        while self
            .profiles_of(harness)
            .any(|profile| profile.name == name)
        {
            name = format!("{wanted} {nth}");
            nth += 1;
        }
        let id = self.id('a');
        self.profiles.push(Profile {
            id: id.clone(),
            harness: harness.to_owned(),
            name,
            home,
        });
        id
    }

    /// Forget an account. Panes that ran under it are the harness's own
    /// from their next start; nothing in its folder is touched.
    pub fn remove_profile(&mut self, id: &str) {
        self.profiles.retain(|profile| profile.id != id);
        for pane in &mut self.panes {
            if pane.profile.as_deref() == Some(id) {
                pane.profile = None;
            }
        }
        if self.last_profile.as_deref() == Some(id) {
            self.last_profile = None;
        }
    }

    /// A fresh id with a one-letter prefix: `s` space, `w` session, `p` pane,
    /// `t` tab, `a` account.
    fn id(&mut self, prefix: char) -> Id {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    /// Add a space for a directory, or return the one that already has it.
    pub fn add_space(&mut self, path: PathBuf) -> Id {
        if let Some(space) = self.spaces.iter().find(|space| space.path == path) {
            return space.id.clone();
        }
        let id = self.id('s');
        self.spaces.push(Space {
            id: id.clone(),
            name: name_of(&path),
            path,
            collapsed: false,
            archive_open: false,
        });
        id
    }

    /// Add a session to a space, named after the space and how many it has had.
    pub fn add_session(&mut self, space: &str) -> Option<Id> {
        let (cwd, base) = {
            let space = self.space(space)?;
            (space.path.clone(), space.name.clone())
        };
        let ordinal = self
            .sessions
            .iter()
            .filter(|session| session.space == space)
            .count()
            + 1;
        let id = self.id('w');
        self.sessions.push(Session {
            id: id.clone(),
            space: space.to_owned(),
            name: if ordinal == 1 {
                base
            } else {
                format!("{base} {ordinal}")
            },
            cwd,
            archived: false,
            created_ms: now_ms(),
            old_layout: None,
            old_board: None,
            old_camera: None,
            focused: None,
            tabs: Vec::new(),
        });
        self.selected = Some(id.clone());
        Some(id)
    }

    /// Record a new pane where `seat` says, with the keys. Its node must be
    /// on that tab's board already; a seat that is not there (the pane or
    /// the tab it names is gone) becomes a tab of its own.
    #[allow(clippy::too_many_arguments)]
    pub fn add_pane(
        &mut self,
        session: &str,
        harness: &str,
        title: &str,
        cwd: PathBuf,
        kind: PaneKind,
        parent: Option<Id>,
        node: NodeId,
        seat: Seat,
    ) -> Id {
        let id = self.id('p');
        // The id a tab of its own would take; taken only if there is one.
        let fresh = format!("t{}", self.next + 1);
        let mut made_tab = false;
        let mut pane = Pane {
            id: id.clone(),
            session: session.to_owned(),
            harness: harness.to_owned(),
            title: title.to_owned(),
            cwd,
            kind,
            parent,
            node,
            resume: None,
            model: None,
            profile: None,
            started: Vec::new(),
        };
        if let Some(session) = self.session_mut(session) {
            let seated = match &seat {
                Seat::Tab(_) => None,
                Seat::Beside { at, axis } => session.tab_of(at).inspect(|&tab| {
                    session.tabs[tab].root.divide(at, *axis, id.clone());
                }),
                Seat::In(tab) => session
                    .tabs
                    .iter()
                    .position(|saved| saved.id == *tab)
                    .inspect(|tab| session.tabs[*tab].root.divide_widest(id.clone())),
            };
            match seated {
                Some(tab) => session.tabs[tab].active = id.clone(),
                None => {
                    let board = match seat {
                        Seat::Tab(board) => board,
                        // Its node was put on a board that is not this one.
                        _ => {
                            let mut board = Board::new();
                            pane.node = board.add_at(node_for(&pane), 0., 0.);
                            board
                        }
                    };
                    session.tabs.push(Tab::new(fresh, &id, board));
                    made_tab = true;
                }
            }
            session.focused = Some(id.clone());
        }
        if made_tab {
            self.next += 1;
        }
        self.panes.push(pane);
        id
    }

    /// Give a pane the keys: its session's focus, and its tab's.
    pub fn focus(&mut self, pane: &str) {
        let Some(session) = self.pane(pane).map(|pane| pane.session.clone()) else {
            return;
        };
        if let Some(session) = self.session_mut(&session) {
            session.focused = Some(pane.to_owned());
            if let Some(tab) = session.tab_of(pane) {
                session.tabs[tab].active = pane.to_owned();
            }
        }
    }

    /// Forget a pane. Its children stay, without a parent. What was beside
    /// it in its tab takes its room, and the keys.
    pub fn remove_pane(&mut self, id: &str) -> Option<Pane> {
        let index = self.panes.iter().position(|pane| pane.id == id)?;
        let pane = self.panes.remove(index);
        for other in &mut self.panes {
            if other.parent.as_deref() == Some(id) {
                other.parent = None;
            }
        }
        if let Some(session) = self.session_mut(&pane.session) {
            let had_keys = session.focused.as_deref() == Some(id);
            let mut heir = None;
            if let Some(at) = session.tab_of(id) {
                let mut tab = session.tabs.remove(at);
                let beside = tab.root.neighbour(id).cloned();
                tab.board.remove(pane.node);
                if let Some(root) = tab.root.clone().without(id) {
                    let active = if tab.active == id {
                        beside.unwrap_or_else(|| root.panes()[0].clone())
                    } else {
                        tab.active.clone()
                    };
                    heir = Some(active.clone());
                    session.tabs.insert(
                        at,
                        Tab {
                            root,
                            active,
                            ..tab
                        },
                    );
                } else {
                    // The tab is gone: the one that took its place, else the last.
                    heir = session
                        .tabs
                        .get(at)
                        .or(session.tabs.last())
                        .map(|tab| tab.active.clone());
                }
            }
            if had_keys {
                session.focused = heir;
            }
        }
        Some(pane)
    }

    /// Make every session's tabs agree with its panes: a pane in no tab gets
    /// one, a tab's pane that is gone leaves it, every tab has an id and
    /// every pane a node on its tab's board. What a file from an earlier
    /// build needs, and what keeps a damaged one usable.
    pub fn settle_tabs(&mut self) {
        let Self {
            sessions,
            panes,
            next,
            ..
        } = self;
        let mut taken: Vec<Id> = Vec::new();
        for session in sessions.iter_mut() {
            let own: Vec<Id> = panes
                .iter()
                .filter(|pane| pane.session == session.id)
                .map(|pane| pane.id.clone())
                .collect();
            let mut seen: Vec<Id> = Vec::new();
            let mut tabs = Vec::new();
            for tab in std::mem::take(&mut session.tabs) {
                let mut root = Some(tab.root.clone());
                // Out go panes that are gone, and second mentions of one.
                let mut kept: Vec<Id> = Vec::new();
                for pane in tab.root.panes().into_iter().cloned() {
                    if own.contains(&pane) && !seen.contains(&pane) && !kept.contains(&pane) {
                        kept.push(pane);
                    } else if !kept.contains(&pane) {
                        root = root.and_then(|root| root.without(&pane));
                    }
                }
                if let Some(root) = root {
                    let active = if kept.contains(&tab.active) {
                        tab.active.clone()
                    } else {
                        kept[0].clone()
                    };
                    seen.extend(kept);
                    tabs.push(Tab {
                        root,
                        active,
                        ..tab
                    });
                }
            }
            for pane in &own {
                if !seen.contains(pane) {
                    tabs.push(Tab::new(Id::new(), pane, Board::new()));
                }
            }
            session.tabs = tabs;
            if session
                .focused
                .as_deref()
                .is_none_or(|pane| session.tab_of(pane).is_none())
            {
                session.focused = session.tabs.last().map(|tab| tab.active.clone());
            }

            // What an earlier build kept for the whole session goes to its tabs.
            let (layout, camera) = (session.old_layout.take(), session.old_camera.take());
            if let Some(board) = session.old_board.take().filter(|board| !board.is_empty()) {
                if layout == Some(Layout::Canvas) && !session.tabs.is_empty() {
                    // It was one canvas on screen: it stays one, as one tab.
                    let all: Vec<Id> = session
                        .tabs
                        .iter()
                        .flat_map(|tab| tab.root.panes().into_iter().cloned())
                        .collect();
                    let first = session.tabs.swap_remove(0);
                    let active = session
                        .focused
                        .clone()
                        .filter(|pane| all.contains(pane))
                        .unwrap_or_else(|| first.active.clone());
                    session.tabs = vec![Tab {
                        root: Split::tiled(&all, Axis::Row).unwrap_or_else(|| first.root.clone()),
                        active,
                        layout: Layout::Canvas,
                        board,
                        camera,
                        ..first
                    }];
                } else {
                    // Each tab takes its own panes' nodes, and what is wired
                    // between them; notes stay with the first.
                    let node_of = |pane: &Id| {
                        panes
                            .iter()
                            .find(|saved| saved.id == *pane)
                            .map(|saved| saved.node)
                    };
                    for (index, tab) in session.tabs.iter_mut().enumerate() {
                        let mine: Vec<NodeId> =
                            tab.root.panes().into_iter().filter_map(node_of).collect();
                        let mut part = board.clone();
                        for node in board.nodes() {
                            let stays =
                                mine.contains(&node.id) || (index == 0 && node.kind.canvas_owned());
                            if !stays {
                                part.remove(node.id);
                            }
                        }
                        tab.board = part;
                    }
                }
            }

            for tab in &mut session.tabs {
                if tab.id.is_empty() || taken.contains(&tab.id) {
                    *next += 1;
                    tab.id = format!("t{next}");
                }
                taken.push(tab.id.clone());
                // A pane without a node of its own gets one under the others.
                let mut used: Vec<NodeId> = Vec::new();
                for id in tab.root.panes().into_iter().cloned().collect::<Vec<_>>() {
                    let Some(pane) = panes.iter_mut().find(|pane| pane.id == id) else {
                        continue;
                    };
                    if tab.board.node(pane.node).is_none() || used.contains(&pane.node) {
                        let (x, y) = tab.board.bounds().map_or((0., 0.), |bounds| {
                            (bounds.x + bounds.w / 2., bounds.bottom() + 160.)
                        });
                        pane.node = tab.board.add_at(node_for(pane), x, y);
                    }
                    used.push(pane.node);
                }
            }
        }
        self.rail_width = self.rail_width.clamp(RAIL_WIDTHS.0, RAIL_WIDTHS.1);
    }

    /// A tab by id, in whichever session has it.
    pub fn tab(&self, id: &str) -> Option<&Tab> {
        self.sessions.iter().find_map(|session| session.tab_by(id))
    }

    /// A tab by id, to change it.
    pub fn tab_mut(&mut self, id: &str) -> Option<&mut Tab> {
        self.sessions
            .iter_mut()
            .find_map(|session| session.tabs.iter_mut().find(|tab| tab.id == id))
    }

    /// The tab a pane is in.
    pub fn tab_of(&self, pane: &str) -> Option<&Tab> {
        let session = self.session(&self.pane(pane)?.session)?;
        session.tabs.get(session.tab_of(pane)?)
    }

    /// The session a tab belongs to.
    pub fn session_of_tab(&self, tab: &str) -> Option<&Session> {
        self.sessions
            .iter()
            .find(|session| session.tab_by(tab).is_some())
    }

    /// A tab's panes, in reading order.
    pub fn panes_in<'a>(&'a self, tab: &str) -> Vec<&'a Pane> {
        self.tab(tab)
            .map(|tab| {
                tab.root
                    .panes()
                    .into_iter()
                    .filter_map(|pane| self.pane(pane))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A space by id.
    pub fn space(&self, id: &str) -> Option<&Space> {
        self.spaces.iter().find(|space| space.id == id)
    }

    /// A space by id, to change it.
    pub fn space_mut(&mut self, id: &str) -> Option<&mut Space> {
        self.spaces.iter_mut().find(|space| space.id == id)
    }

    /// A session by id.
    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|session| session.id == id)
    }

    /// A session by id, to change it.
    pub fn session_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|session| session.id == id)
    }

    /// A pane by id.
    pub fn pane(&self, id: &str) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    /// A pane by id, to change it.
    pub fn pane_mut(&mut self, id: &str) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|pane| pane.id == id)
    }

    /// The session on screen.
    pub fn current(&self) -> Option<&Session> {
        self.session(self.selected.as_deref()?)
    }

    /// A session's panes, oldest first.
    pub fn panes_of<'a>(&'a self, session: &'a str) -> impl Iterator<Item = &'a Pane> + 'a {
        self.panes
            .iter()
            .filter(move |pane| pane.session == session)
    }

    /// A space's sessions, live or archived, oldest first.
    pub fn sessions_of<'a>(
        &'a self,
        space: &'a str,
        archived: bool,
    ) -> impl Iterator<Item = &'a Session> + 'a {
        self.sessions
            .iter()
            .filter(move |session| session.space == space && session.archived == archived)
    }

    /// Read a store. A missing file is an empty store; a damaged one is an error,
    /// so it is not overwritten with nothing.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut store: Self = serde_json::from_slice(&bytes)?;
                store.settle_tabs();
                Ok(store)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Write a store: to a sibling file first, then moved into place, so a
    /// crash mid-write leaves the previous one.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let draft = path.with_extension("json.tmp");
        std::fs::write(&draft, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&draft, path)?;
        Ok(())
    }
}

/// Where the store lives: `$KOU_DESKTOP_HOME`, else the platform's data directory.
pub fn home() -> PathBuf {
    std::env::var_os("KOU_DESKTOP_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|data| data.join("kou-desktop")))
        .unwrap_or_else(|| PathBuf::from(".kou-desktop"))
}

/// The current git branch of a directory, read from its `.git/HEAD`: no process.
pub fn branch_of(directory: &Path) -> Option<String> {
    let mut at = Some(directory);
    while let Some(directory) = at {
        let git = directory.join(".git");
        // A worktree's `.git` is a file pointing at the real directory.
        let head = if git.is_file() {
            let pointer = std::fs::read_to_string(&git).ok()?;
            let target = pointer.trim().strip_prefix("gitdir:")?.trim();
            directory.join(target).join("HEAD")
        } else {
            git.join("HEAD")
        };
        if let Ok(head) = std::fs::read_to_string(&head) {
            let head = head.trim();
            return Some(match head.strip_prefix("ref: refs/heads/") {
                Some(branch) => branch.to_owned(),
                // Detached: the commit, shortened.
                None => head.chars().take(7).collect(),
            });
        }
        at = directory.parent();
    }
    None
}

/// An age for the rail: `<1m`, `5m`, `3h`, `2d`.
pub fn age(milliseconds: u64) -> String {
    let minutes = milliseconds / 60_000;
    match minutes {
        0 => "<1m".to_owned(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_sessions_and_panes_get_short_ids_and_sensible_names() {
        let mut store = Store::default();
        let space = store.add_space("/work/herdr".into());
        assert_eq!(
            store.add_space("/work/herdr".into()),
            space,
            "one space per directory"
        );
        let first = store.add_session(&space).unwrap();
        let second = store.add_session(&space).unwrap();
        assert_eq!(store.session(&first).unwrap().name, "herdr");
        assert_eq!(store.session(&second).unwrap().name, "herdr 2");
        assert_eq!(store.selected.as_deref(), Some(second.as_str()));
        assert!(store.add_session("nope").is_none());

        let parent = store.add_pane(
            &first,
            "claude",
            "Claude Code",
            "/work/herdr".into(),
            PaneKind::Terminal,
            None,
            1,
            Seat::Tab(Board::new()),
        );
        let child = store.add_pane(
            &first,
            "codex",
            "Codex",
            "/work/herdr".into(),
            PaneKind::Terminal,
            Some(parent.clone()),
            2,
            Seat::Tab(Board::new()),
        );
        // A pane and its tab each take an id.
        assert_eq!(
            (
                space.as_str(),
                first.as_str(),
                parent.as_str(),
                child.as_str()
            ),
            ("s1", "w2", "p4", "p6")
        );
        let ids: Vec<&str> = store
            .session(&first)
            .unwrap()
            .tabs
            .iter()
            .map(|tab| tab.id.as_str())
            .collect();
        assert_eq!(ids, ["t5", "t7"]);
        assert_eq!(store.tab_of(&child).unwrap().id, "t7");
        assert_eq!(store.session_of_tab("t7").unwrap().id, first);
        assert_eq!(
            store.session(&first).unwrap().focused.as_deref(),
            Some("p6")
        );
        assert_eq!(store.panes_of(&first).count(), 2);

        // Closing the focused pane focuses the tab that is left; closing a parent orphans the child.
        store.remove_pane(&child);
        assert_eq!(
            store.session(&first).unwrap().focused.as_deref(),
            Some("p4")
        );
        let again = store.add_pane(
            &first,
            "codex",
            "Codex",
            "/work/herdr".into(),
            PaneKind::Terminal,
            Some(parent.clone()),
            3,
            Seat::Tab(Board::new()),
        );
        store.remove_pane(&parent);
        assert_eq!(store.pane(&again).unwrap().parent, None);
        assert!(store.remove_pane(&parent).is_none());
    }

    #[test]
    fn a_store_survives_a_restart_and_a_damaged_file_is_not_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("state.json");
        assert_eq!(Store::load(&path).unwrap(), Store::default());

        let mut store = Store::default();
        let space = store.add_space("/work/a".into());
        let session = store.add_session(&space).unwrap();
        let pane = pane(&mut store, &session);
        let tab = store.tab_of(&pane).unwrap().id.clone();
        let tab = store.tab_mut(&tab).unwrap();
        tab.layout = Layout::Canvas;
        tab.name = Some("api".into());
        tab.camera = Some(Camera::default());
        store.save(&path).unwrap();
        let back = Store::load(&path).unwrap();
        assert_eq!(back, store);
        // Ids keep counting from where they were.
        let mut back = back;
        assert_eq!(back.add_space("/work/b".into()), "s5");

        std::fs::write(&path, b"{ not json").unwrap();
        assert!(Store::load(&path).is_err());
        // An older file without newer fields still loads.
        std::fs::write(
            &path,
            br#"{"spaces":[{"id":"s1","name":"a","path":"/work/a"}]}"#,
        )
        .unwrap();
        let old = Store::load(&path).unwrap();
        assert!(old.smooth_cursor && !old.spaces[0].collapsed);
        assert_eq!(old.rail_width, 264.);
        // A first run starts as a file without the fields does.
        assert_eq!(Store::default().smooth_cursor, old.smooth_cursor);
        assert_eq!(old.accent_rgb(), None);
    }

    #[test]
    fn accounts_are_added_named_after_their_folders_kept_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut store = Store::default();
        let space = store.add_space("/work/api".into());
        let session = store.add_session(&space).unwrap();
        let work = store.add_profile("claude", "/Users/me/.claude-work".into());
        // The same folder again is the same account.
        assert_eq!(
            store.add_profile("claude", "/Users/me/.claude-work".into()),
            work
        );
        let second = store.add_profile("claude", "/accounts/work".into());
        let codex = store.add_profile("codex", "/accounts/work".into());
        let names: Vec<(&str, &str)> = store
            .profiles
            .iter()
            .map(|profile| (profile.harness.as_str(), profile.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [("claude", "work"), ("claude", "work 2"), ("codex", "work")],
            "no two of a harness under one name"
        );
        assert_eq!(store.profiles_of("claude").count(), 2);
        assert_eq!(profile_name("claude", Path::new("/a/.claude")), "claude");
        assert_eq!(profile_name("codex", Path::new("/a/codex_team")), "team");
        assert_eq!(profile_name("grok", Path::new("/")), "account");
        // A pane runs under one, and what is saved says so.
        let pane = seated(&mut store, &session, Seat::Tab(Board::new()));
        store.pane_mut(&pane).unwrap().profile = Some(second.clone());
        store.last_profile = Some(second.clone());
        store.save(&path).unwrap();
        let loaded = Store::load(&path).unwrap();
        assert_eq!(loaded.profiles, store.profiles);
        assert_eq!(loaded.pane(&pane).unwrap().profile.as_ref(), Some(&second));
        assert_eq!(
            loaded.profile(&codex).unwrap().home,
            Path::new("/accounts/work")
        );
        // Forgotten, its panes are the harness's own again.
        store.remove_profile(&second);
        assert_eq!(store.pane(&pane).unwrap().profile, None);
        assert_eq!(store.last_profile, None);
        assert_eq!(store.profiles.len(), 2);
        // A file from before there were accounts has none.
        std::fs::write(
            &path,
            br#"{"spaces":[{"id":"s1","name":"a","path":"/work/a"}]}"#,
        )
        .unwrap();
        assert!(Store::load(&path).unwrap().profiles.is_empty());
    }

    #[test]
    fn a_chosen_accent_is_kept_and_read_back_as_a_colour() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut store = Store::default();
        // Without one the file does not mention it.
        store.save(&path).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("accent"));
        store.accent = Some("#4d9fe0".into());
        store.save(&path).unwrap();
        assert_eq!(Store::load(&path).unwrap().accent_rgb(), Some(0x4d9fe0));
        // Written by hand: with or without the sign, in either case; what
        // is not a colour is no choice.
        for (written, read) in [
            ("A58BF0", Some(0xa58bf0)),
            (" #009f8b ", Some(0x009f8b)),
            ("teal", None),
            ("#fff", None),
            ("+12345", None),
            ("", None),
        ] {
            store.accent = Some(written.into());
            assert_eq!(store.accent_rgb(), read, "{written:?}");
        }
    }

    // A pane seated somewhere, with a node made for it on that tab's board.
    fn seated(store: &mut Store, session: &str, seat: Seat) -> Id {
        let spec = || NodeSpec {
            kind: NodeKind::Terminal,
            title: "Shell".into(),
            ..NodeSpec::default()
        };
        let (node, seat) = match seat {
            Seat::Tab(mut board) => (board.add_at(spec(), 0., 0.), Seat::Tab(board)),
            Seat::Beside { at, axis } => {
                let tab = store.tab_of(&at).unwrap().id.clone();
                let node = store.tab_mut(&tab).unwrap().board.add_at(spec(), 0., 0.);
                (node, Seat::Beside { at, axis })
            }
            Seat::In(tab) => {
                let node = store.tab_mut(&tab).unwrap().board.add_at(spec(), 0., 0.);
                (node, Seat::In(tab))
            }
        };
        store.add_pane(
            session,
            "shell",
            "Shell",
            "/work/a".into(),
            PaneKind::Terminal,
            None,
            node,
            seat,
        )
    }

    fn pane(store: &mut Store, session: &str) -> Id {
        seated(store, session, Seat::Tab(Board::new()))
    }

    #[test]
    fn a_tab_splits_into_panes_and_closes_back_into_one() {
        let mut store = Store::default();
        let space = store.add_space("/work/a".into());
        let session = store.add_session(&space).unwrap();
        let a = pane(&mut store, &session);
        let other = pane(&mut store, &session);
        let tabs = |store: &Store| -> Vec<Vec<Id>> {
            store
                .session(&session)
                .unwrap()
                .tabs
                .iter()
                .map(|tab| tab.root.panes().into_iter().cloned().collect())
                .collect()
        };
        // A pane seated as a tab is a tab; the last one made has the keys.
        assert_eq!(tabs(&store), [vec![a.clone()], vec![other.clone()]]);
        assert_eq!(store.session(&session).unwrap().tab(), Some(1));
        store.remove_pane(&other);

        // b goes beside a, c under b: one tab of three panes, in reading
        // order, and one board with a node for each.
        let seat = |at: &Id, axis| Seat::Beside {
            at: at.clone(),
            axis,
        };
        let b = seated(&mut store, &session, seat(&a, Axis::Row));
        let c = seated(&mut store, &session, seat(&b, Axis::Column));
        assert_eq!(tabs(&store), [vec![a.clone(), b.clone(), c.clone()]]);
        let tab = store.tab_of(&a).unwrap();
        assert_eq!(tab.active, c);
        assert_eq!(tab.board.nodes().len(), 3);
        assert_eq!(store.panes_in(&tab.id).len(), 3);
        // A seat that is gone is a tab of its own, with a node of its own.
        let stray = seated(&mut store, &session, seat(&a, Axis::Row));
        store.remove_pane(&stray);
        let stray = store.add_pane(
            &session,
            "shell",
            "Shell",
            "/work/a".into(),
            PaneKind::Terminal,
            None,
            99,
            Seat::In("no such tab".into()),
        );
        assert_eq!(tabs(&store).len(), 2);
        let alone = store.tab_of(&stray).unwrap();
        assert!(alone.board.node(store.pane(&stray).unwrap().node).is_some());
        store.remove_pane(&stray);

        // Laid out in 1000 by 600 with a 2 pixel gap: a on the left half, b
        // over c on the right.
        let root = store.session(&session).unwrap().tabs[0].root.clone();
        let (panes, dividers) = root.lay_out([0., 0., 1000., 600.], 2.);
        assert_eq!(
            panes,
            [
                (a.clone(), [0., 0., 499., 600.]),
                (b.clone(), [501., 0., 499., 299.]),
                (c.clone(), [501., 301., 499., 299.]),
            ]
        );
        assert_eq!(dividers.len(), 2);
        assert_eq!(
            (dividers[0].path.clone(), dividers[0].axis),
            (vec![], Axis::Row)
        );
        assert_eq!(
            (dividers[1].path.clone(), dividers[1].axis),
            (vec![true], Axis::Column)
        );
        assert_eq!(dividers[1].room, [501., 0., 499., 600.]);

        // A divider dragged: the share changes, within limits when laid out.
        let tab = &mut store.session_mut(&session).unwrap().tabs[0];
        *tab.root.ratio_mut(&[]).unwrap() = 0.25;
        *tab.root.ratio_mut(&[true]).unwrap() = 0.99;
        assert!(
            tab.root.ratio_mut(&[false]).is_none(),
            "a pane has no share to set"
        );
        let (panes, _) = tab.root.lay_out([0., 0., 1000., 600.], 2.);
        assert_eq!(panes[0].1[2], 250.);
        assert!(panes[2].1[3] >= 600. * MIN_SHARE - 2.);

        // Focus follows into the tab; closing the pane with the keys gives
        // them to what takes its room.
        store.focus(&b);
        assert_eq!(store.session(&session).unwrap().tabs[0].active, b);
        store.remove_pane(&b);
        assert_eq!(tabs(&store), [vec![a.clone(), c.clone()]]);
        assert_eq!(store.tab_of(&a).unwrap().board.nodes().len(), 2);
        // c was under b and has its room now.
        assert_eq!(store.session(&session).unwrap().focused.as_ref(), Some(&c));
        store.remove_pane(&a);
        store.remove_pane(&c);
        assert!(store.session(&session).unwrap().tabs.is_empty());
        assert_eq!(store.session(&session).unwrap().focused, None);
    }

    #[test]
    fn a_file_from_an_earlier_build_gets_a_tab_for_each_pane() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        // "tiles" was a layout once, and panes had no tabs.
        std::fs::write(
            &path,
            br#"{
              "spaces":[{"id":"s1","name":"a","path":"/work/a"}],
              "sessions":[{"id":"w2","space":"s1","name":"a","cwd":"/work/a","created_ms":1,
                           "layout":"tiles","focused":"p3"}],
              "panes":[{"id":"p3","session":"w2","harness":"shell","title":"Shell","cwd":"/work/a","node":1},
                       {"id":"p4","session":"w2","harness":"shell","title":"Shell","cwd":"/work/a","node":2}],
              "sound":false,"distortion":true,"rail_width":9000,"next":4}"#,
        )
        .unwrap();
        let store = Store::load(&path).unwrap();
        let session = store.session("w2").unwrap();
        assert_eq!(session.layout(), Layout::Panes);
        assert_eq!(session.tabs.len(), 2);
        assert_eq!(
            session.tab(),
            Some(0),
            "the focused pane's tab is on screen"
        );
        assert_eq!(store.rail_width, RAIL_WIDTHS.1);
        // Each tab has an id past those taken, and a node for its pane.
        let ids: Vec<&str> = session.tabs.iter().map(|tab| tab.id.as_str()).collect();
        assert_eq!(ids, ["t5", "t6"]);
        for tab in &session.tabs {
            let pane = store.pane(&tab.active).unwrap();
            assert!(tab.board.node(pane.node).is_some());
        }

        // A tab that names a pane that is gone, or one twice, is put right.
        let mut store = store;
        let tabs = &mut store.session_mut("w2").unwrap().tabs;
        tabs[0].root = Split::Parts {
            axis: Axis::Row,
            ratio: 0.5,
            first: Box::new(Split::Pane("p3".into())),
            second: Box::new(Split::Pane("gone".into())),
        };
        tabs[1].root = Split::Pane("p3".into());
        store.settle_tabs();
        let session = store.session("w2").unwrap();
        let listed: Vec<Vec<&Id>> = session.tabs.iter().map(|tab| tab.root.panes()).collect();
        assert_eq!(listed, [vec!["p3"], vec!["p4"]]);
    }

    // A board as an earlier build saved it for a whole session: three nodes
    // in a row, the first wired to the second and the second to the third.
    fn old_board() -> (Board, [NodeId; 3]) {
        let mut board = Board::new();
        let spec = |title: &str| NodeSpec {
            kind: NodeKind::Agent,
            title: title.into(),
            ..NodeSpec::default()
        };
        let a = board.add_at(spec("a"), 0., 0.);
        let b = board.add_at(spec("b"), 900., 0.);
        let c = board.add_at(spec("c"), 1800., 0.);
        assert!(board.link(a, b) && board.link(b, c));
        (board, [a, b, c])
    }

    fn old_file(layout: &str) -> String {
        let (board, [a, b, c]) = old_board();
        format!(
            r#"{{
              "spaces":[{{"id":"s1","name":"a","path":"/work/a"}}],
              "sessions":[{{"id":"w2","space":"s1","name":"a","cwd":"/work/a","created_ms":1,
                           "layout":"{layout}","focused":"p4","board":{},
                           "camera":{},
                           "tabs":[{{"root":{{"parts":{{"axis":"row","ratio":0.5,
                                     "first":{{"pane":"p3"}},"second":{{"pane":"p4"}}}}}},
                                    "active":"p4","name":"pair"}},
                                   {{"root":{{"pane":"p5"}},"active":"p5"}}]}}],
              "panes":[{{"id":"p3","session":"w2","harness":"claude","title":"a","cwd":"/work/a","node":{a}}},
                       {{"id":"p4","session":"w2","harness":"codex","title":"b","cwd":"/work/a","node":{b}}},
                       {{"id":"p5","session":"w2","harness":"shell","title":"c","cwd":"/work/a","node":{c}}}],
              "next":5}}"#,
            serde_json::to_string(&board).unwrap(),
            serde_json::to_string(&Camera::default()).unwrap(),
        )
    }

    #[test]
    fn a_session_that_had_one_canvas_gives_each_tab_its_own() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");

        // Shown as tabs: each tab keeps its panes' nodes where they were, and
        // the wire between two panes of one tab.
        std::fs::write(&path, old_file("single")).unwrap();
        let store = Store::load(&path).unwrap();
        let session = store.session("w2").unwrap();
        let (pair, alone) = (&session.tabs[0], &session.tabs[1]);
        assert_eq!((pair.layout, alone.layout), (Layout::Panes, Layout::Panes));
        assert_eq!(pair.name.as_deref(), Some("pair"));
        assert_eq!((pair.board.nodes().len(), pair.board.edges().len()), (2, 1));
        assert_eq!(
            (alone.board.nodes().len(), alone.board.edges().len()),
            (1, 0)
        );
        let node = |pane: &str| store.pane(pane).unwrap().node;
        let (was, _) = old_board();
        assert_eq!(
            pair.board.node(node("p4")).unwrap().rect,
            was.node(node("p4")).unwrap().rect
        );
        assert!(alone.board.node(node("p5")).is_some());
        // Saved again, nothing of the old shape is written, and it reads the same.
        store.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
        let written = saved["sessions"][0].as_object().unwrap();
        assert!(!written.contains_key("board") && !written.contains_key("layout"));
        assert_eq!(Store::load(&path).unwrap(), store);

        // Shown as a canvas: it stays one canvas, as one tab with every pane
        // in it, every wire, the camera, and the first tab's name.
        std::fs::write(&path, old_file("canvas")).unwrap();
        let store = Store::load(&path).unwrap();
        let session = store.session("w2").unwrap();
        assert_eq!(session.tabs.len(), 1);
        let tab = &session.tabs[0];
        assert_eq!(tab.layout, Layout::Canvas);
        assert_eq!(tab.root.panes(), ["p3", "p4", "p5"]);
        assert_eq!((tab.board.nodes().len(), tab.board.edges().len()), (3, 2));
        assert_eq!(tab.camera, Some(Camera::default()));
        assert_eq!(
            (tab.name.as_deref(), tab.active.as_str()),
            (Some("pair"), "p4")
        );
        assert_eq!(session.layout(), Layout::Canvas);
    }

    #[test]
    fn panes_share_a_tab_evenly_and_a_new_one_takes_the_most_room() {
        let ids = |count: usize| -> Vec<Id> { (1..=count).map(|n| format!("p{n}")).collect() };
        assert_eq!(Split::tiled(&[], Axis::Row), None);
        assert_eq!(
            Split::tiled(&ids(1), Axis::Row),
            Some(Split::Pane("p1".into()))
        );
        // Four: two columns of two.
        let four = Split::tiled(&ids(4), Axis::Row).unwrap();
        let (panes, _) = four.lay_out([0., 0., 1000., 600.], 0.);
        let rooms: Vec<Place> = panes.into_iter().map(|(_, room)| room).collect();
        assert_eq!(
            rooms,
            [
                [0., 0., 500., 300.],
                [0., 300., 500., 300.],
                [500., 0., 500., 300.],
                [500., 300., 500., 300.],
            ]
        );
        // Three: the first part holds two, and gets two thirds.
        let three = Split::tiled(&ids(3), Axis::Row).unwrap();
        let (panes, _) = three.lay_out([0., 0., 900., 600.], 0.);
        assert_eq!(panes[2].1, [600., 0., 300., 600.]);

        // A new pane halves whichever has the most room, along its longer
        // side; of two with as much, the later one.
        let mut root = Split::Pane("a".into());
        root.divide_widest("b".into());
        root.divide_widest("c".into());
        root.divide_widest("d".into());
        let (panes, _) = root.lay_out([0., 0., 1600., 1000.], 0.);
        let rooms: Vec<(Id, Place)> = panes;
        assert_eq!(
            rooms,
            [
                ("a".to_owned(), [0., 0., 800., 500.]),
                ("d".to_owned(), [0., 500., 800., 500.]),
                ("b".to_owned(), [800., 0., 800., 500.]),
                ("c".to_owned(), [800., 500., 800., 500.]),
            ]
        );

        // A pane is given a share of the room it divides with its
        // neighbour: the line between them moves, wherever in the tab it is.
        let mut root = Split::Pane("a".into());
        root.divide("a", Axis::Row, "b".into());
        assert!(root.set_share("b", 0.3));
        let (panes, _) = root.lay_out([0., 0., 1000., 600.], 0.);
        assert_eq!(
            panes,
            [
                ("a".to_owned(), [0., 0., 700., 600.]),
                ("b".to_owned(), [700., 0., 300., 600.]),
            ]
        );
        // Below it, and the first of two takes its share as well.
        root.divide("b", Axis::Column, "c".into());
        assert!(root.set_share("c", 0.25));
        assert!(root.set_share("b", 0.75));
        let (panes, _) = root.lay_out([0., 0., 1000., 600.], 0.);
        assert_eq!(panes[1], ("b".to_owned(), [700., 0., 300., 450.]));
        assert_eq!(panes[2], ("c".to_owned(), [700., 450., 300., 150.]));
        // Never all of it or none, and a pane alone has nothing to share.
        assert!(root.set_share("c", 0.99));
        let (panes, _) = root.lay_out([0., 0., 1000., 600.], 0.);
        let (above, below) = (panes[1].1[3], panes[2].1[3]);
        assert!(
            above >= 60. && below > 500. && above + below == 600.,
            "{above} {below}"
        );
        assert!(!root.set_share("nobody", 0.5));
        assert!(!Split::Pane("a".into()).set_share("a", 0.5));
    }

    #[test]
    fn a_branch_is_read_from_the_checkout_or_from_a_worktree_pointer() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_eq!(branch_of(root), None);
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::write(
            root.join("repo/.git/HEAD"),
            "ref: refs/heads/chore/review\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("repo/src/deep")).unwrap();
        assert_eq!(
            branch_of(&root.join("repo/src/deep")).as_deref(),
            Some("chore/review")
        );

        std::fs::write(
            root.join("repo/.git/HEAD"),
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904\n",
        )
        .unwrap();
        assert_eq!(branch_of(&root.join("repo")).as_deref(), Some("4b825dc"));

        std::fs::create_dir_all(root.join("repo/.git/worktrees/wt")).unwrap();
        std::fs::write(
            root.join("repo/.git/worktrees/wt/HEAD"),
            "ref: refs/heads/tasks\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: ../repo/.git/worktrees/wt\n").unwrap();
        assert_eq!(branch_of(&root.join("wt")).as_deref(), Some("tasks"));
    }

    #[test]
    fn ages_read_like_the_rail() {
        assert_eq!(age(0), "<1m");
        assert_eq!(age(59_999), "<1m");
        assert_eq!(age(60_000), "1m");
        assert_eq!(age(59 * 60_000), "59m");
        assert_eq!(age(3 * 3_600_000 + 5), "3h");
        assert_eq!(age(49 * 3_600_000), "2d");
        assert_eq!(name_of(Path::new("/work/herdr")), "herdr");
        assert_eq!(name_of(Path::new("/")), "/");
    }
}
