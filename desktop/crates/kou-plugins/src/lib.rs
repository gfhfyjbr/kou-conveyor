//! Plugins of the desktop app: scripts that build parts of its interface.
//!
//! A plugin is a folder with a `plugin.json`, as the browser cockpit's are;
//! what it gives the desktop app is named under `desktop`:
//!
//! ```json
//! { "name": "usage", "desktop": { "script": "desktop/usage.js" } }
//! ```
//!
//! The script is JavaScript, run by an interpreter that is part of the app
//! (QuickJS), so nothing is compiled when it changes: the app reads the
//! file again and the plugin's new version takes the old one's place, in a
//! release build as in any other. A script says what it adds with `kou`
//! (see `kernel.js`) and describes what it draws as a tree of elements
//! ([`Node`]) that the app turns into its own; it has no way to the disk,
//! the network or other programs, and is stopped when it runs for too long
//! or takes too much memory.
//!
//! [`Host`] finds the plugins of its [`Source`]s, runs them and keeps what
//! each remembers; [`watch`] says when their files change.

mod host;
mod node;
mod source;
mod watch;

pub use host::{Act, Host, PageInfo, Problem};
pub use node::Node;
pub use source::{File, Source};
pub use watch::watch;
