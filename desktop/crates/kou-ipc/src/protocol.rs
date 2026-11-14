//! What travels over the socket: one JSON object per line each way.
//!
//! A connection carries one request and one response, except that the server
//! may hold a [`Request::PaneWait`] open for as long as the wait lasts.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The protocol's version, sent in [`Response::Pong`]. Bump on an incompatible change.
pub const VERSION: u32 = 1;

/// The environment variables every pane's process is started with.
pub mod env {
    /// The socket to talk to.
    pub const SOCKET: &str = "KOU_SOCKET";
    /// The id of the pane the process runs in.
    pub const PANE: &str = "KOU_PANE";
    /// The id of the session the pane belongs to.
    pub const SESSION: &str = "KOU_SESSION";
    /// The desktop binary, which is also the command line client.
    pub const BIN: &str = "KOU_BIN";
}

/// Something asked of the running app.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Is anyone there?
    Ping,
    /// A harness's hook fired in a pane. `event` and `payload` are the
    /// harness's own; the app interprets them.
    Hook {
        /// The pane the harness runs in.
        pane: String,
        /// The harness id, as the app launched it.
        harness: String,
        /// The hook's name in the harness's vocabulary.
        event: String,
        /// The hook's input, or `null`.
        payload: serde_json::Value,
    },
    /// Describe a pane: the caller's own, by `$KOU_PANE`.
    PaneInfo {
        /// The pane.
        pane: String,
    },
    /// The harnesses the app found installed.
    Harnesses,
    /// The panes of a session, or of every session.
    PaneList {
        /// Limit to this session.
        session: Option<String>,
    },
    /// Start an agent in a new pane.
    PaneSpawn(Spawn),
    /// Give a pane input: typed into a terminal, or sent as a prompt to a thread.
    PaneSend {
        /// The pane.
        pane: String,
        /// The text.
        text: String,
        /// Press Enter after it (a terminal) or send it now (a thread).
        submit: bool,
        /// The pane asking, so the receiver can be told who wrote.
        from: Option<String>,
    },
    /// What a pane shows: a terminal's screen, or a thread's latest answer.
    PaneRead {
        /// The pane.
        pane: String,
        /// At most this many lines, counted from the end.
        lines: Option<u32>,
        /// Include what has scrolled off the screen.
        scrollback: bool,
    },
    /// Block until panes reach a state.
    PaneWait {
        /// The panes to watch.
        panes: Vec<String>,
        /// The state to wait for.
        until: Until,
        /// Return when the first pane gets there, rather than all of them.
        any: bool,
        /// Give up after this long.
        timeout_ms: Option<u64>,
    },
    /// Close a pane and end its process.
    PaneClose {
        /// The pane.
        pane: String,
    },
    /// Bring a pane to the front.
    PaneFocus {
        /// The pane.
        pane: String,
    },
    /// List tabs: one session's, or every session's when `session` is `None`.
    TabList {
        /// The session whose tabs to list.
        session: Option<String>,
    },
    /// Give a tab a name, an icon, or both.
    TabSet {
        /// The tab, by id. Without one, the tab `pane` is in.
        tab: Option<String>,
        /// The pane asking.
        pane: Option<String>,
        /// Its new name. Empty: it is called after its pane again.
        name: Option<String>,
        /// Its new icon, by one of the names [`Request::TabIcons`] lists.
        /// Empty or `auto`: it shows what runs in it again.
        icon: Option<String>,
    },
    /// The names of the icons a tab can be given, one per line.
    TabIcons,
    /// List sessions: one space's, or every space's when `space` is `None`.
    SessionList {
        /// The space whose sessions to list.
        space: Option<String>,
    },
    /// Rename a session.
    SessionRename {
        /// The session, by id. Without one, the session `pane` is in.
        session: Option<String>,
        /// The pane asking.
        pane: Option<String>,
        /// Its new name; not empty.
        name: String,
    },
    /// List spaces.
    SpaceList,
    /// Rename a space.
    SpaceRename {
        /// The space, by id. Without one, the space `pane`'s session is in.
        space: Option<String>,
        /// The pane asking.
        pane: Option<String>,
        /// Its new name; not empty.
        name: String,
    },
}

/// A new pane.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spawn {
    /// The pane asking; the new pane is drawn as its child.
    pub from: Option<String>,
    /// The harness to run, by id.
    pub harness: String,
    /// Where to run it; the parent's directory when absent.
    pub cwd: Option<PathBuf>,
    /// A name for the pane.
    pub title: Option<String>,
    /// A first prompt.
    pub prompt: Option<String>,
    /// A model, in the harness's own naming.
    pub model: Option<String>,
    /// Where the pane goes.
    #[serde(default)]
    pub placement: Placement,
    /// For a split: the new pane goes below its parent, not to its right.
    #[serde(default)]
    pub below: bool,
    /// For a split: how much of its parent's room the new pane takes, in
    /// percent (10 to 90). Half of it when absent.
    #[serde(default)]
    pub share: Option<u8>,
    /// Run it as a native thread rather than its terminal interface, when the harness can.
    #[serde(default)]
    pub native: bool,
}

/// Where a new pane appears.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// A tab of its own that does not take the keys from where the user is,
    /// and a child node on the canvas.
    #[default]
    Auto,
    /// Half of its parent's room: the parent's pane is split, the new one to its right.
    Split,
    /// A tab of its own, brought to the front.
    Tab,
}

/// A state [`Request::PaneWait`] waits for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Until {
    /// The agent is free: not working and not waiting on the user.
    #[default]
    Idle,
    /// The pane's process has exited.
    Exit,
}

/// What a pane's agent is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    /// Free.
    #[default]
    Idle,
    /// A turn is running.
    Working,
    /// Waiting for the user.
    Blocked,
    /// The process has exited.
    Exited,
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

/// A pane, as other panes see it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    /// Its id.
    pub id: String,
    /// The session it belongs to.
    pub session: String,
    /// The harness it runs.
    pub harness: String,
    /// Its name.
    pub title: String,
    /// Its working directory.
    pub cwd: PathBuf,
    /// See [`PaneKind`].
    pub kind: PaneKind,
    /// See [`Activity`].
    pub activity: Activity,
    /// The pane that spawned it.
    pub parent: Option<String>,
    /// Milliseconds since its activity last changed.
    pub since_ms: u64,
}

/// How a tab shows its panes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// Side by side and one over the other, as it was split.
    #[default]
    Panes,
    /// As nodes on the tab's own canvas.
    Canvas,
}

/// A tab as the app describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    /// Its id.
    pub id: String,
    /// The session it is a tab of.
    pub session: String,
    /// What it is called: the name it was given, else its pane's.
    pub name: String,
    /// Whether that name was given to it.
    pub named: bool,
    /// The icon it was given, by name.
    pub icon: Option<String>,
    /// See [`Layout`].
    pub layout: Layout,
    /// Its panes, in reading order.
    pub panes: Vec<String>,
    /// The pane of it that had the keys last.
    pub active: String,
    /// Whether it is the tab its session shows.
    pub front: bool,
}

/// A session as the app describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Its id.
    pub id: String,
    /// The space it belongs to.
    pub space: String,
    /// Its name.
    pub name: String,
    /// Where its agents start.
    pub cwd: PathBuf,
    /// Whether it is put away.
    pub archived: bool,
    /// How many tabs it has.
    pub tabs: u32,
    /// Whether it is the session on screen.
    pub selected: bool,
}

/// A space as the app describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    /// Its id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// The directory its sessions start in.
    pub path: PathBuf,
    /// How many sessions it has that are not put away.
    pub sessions: u32,
}

/// An installed harness, as agents see it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harness {
    /// The id to spawn it by.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its version, when it reported one.
    pub version: Option<String>,
    /// Whether it can be launched now.
    pub ready: bool,
    /// Why not, or a warning.
    pub message: Option<String>,
    /// Whether it can run as a native thread.
    pub native: bool,
}

/// The app's answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ok", rename_all = "snake_case")]
pub enum Response {
    /// Answer to [`Request::Ping`].
    Pong {
        /// [`VERSION`].
        version: u32,
    },
    /// Done; nothing to return.
    Done,
    /// One pane.
    Pane(Pane),
    /// Several panes.
    Panes {
        /// The panes.
        panes: Vec<Pane>,
    },
    /// What a pane shows.
    Text {
        /// The text.
        text: String,
    },
    /// Answer to [`Request::PaneWait`].
    Waited {
        /// The watched panes as they are now.
        panes: Vec<Pane>,
        /// The wait ran out before the state was reached.
        timed_out: bool,
    },
    /// One tab.
    Tab(Tab),
    /// Several tabs.
    Tabs {
        /// In strip order, session by session.
        tabs: Vec<Tab>,
    },
    /// One session.
    Session(Session),
    /// Several sessions.
    Sessions {
        /// Oldest first.
        sessions: Vec<Session>,
    },
    /// One space.
    Space(Space),
    /// Several spaces.
    Spaces {
        /// In rail order.
        spaces: Vec<Space>,
    },
    /// Answer to [`Request::Harnesses`].
    Harnesses {
        /// The harnesses.
        harnesses: Vec<Harness>,
    },
    /// The request failed.
    Error {
        /// Why, for the caller to print.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_responses_read_as_tagged_json() {
        let request = Request::PaneSpawn(Spawn {
            from: Some("p1".into()),
            harness: "codex".into(),
            prompt: Some("review the diff".into()),
            ..Spawn::default()
        });
        let line = serde_json::to_string(&request).unwrap();
        assert!(
            line.starts_with(r#"{"op":"pane_spawn","from":"p1","harness":"codex""#),
            "{line}"
        );
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);

        // A hand-written request may leave defaults out.
        let short: Request =
            serde_json::from_str(r#"{"op":"pane_spawn","harness":"claude"}"#).unwrap();
        assert_eq!(
            short,
            Request::PaneSpawn(Spawn {
                harness: "claude".into(),
                ..Spawn::default()
            })
        );

        let response = Response::Waited {
            panes: vec![Pane {
                id: "p2".into(),
                activity: Activity::Idle,
                ..Pane::default()
            }],
            timed_out: false,
        };
        let line = serde_json::to_string(&response).unwrap();
        assert!(
            line.starts_with(r#"{"ok":"waited","panes":[{"id":"p2""#),
            "{line}"
        );
        assert_eq!(serde_json::from_str::<Response>(&line).unwrap(), response);
        assert_eq!(
            serde_json::to_string(&Response::Done).unwrap(),
            r#"{"ok":"done"}"#
        );
    }
}
