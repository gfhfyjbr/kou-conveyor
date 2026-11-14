//! Which agent harnesses are installed, how to launch them, and whether each is busy.
//!
//! - [`specs`] describes every harness kou knows: binaries, probes, launch
//!   lines, ACP modes, hook wiring and screen text.
//! - [`detect`] probes the machine for them, the way t3code checks its
//!   providers: binary, version, minimum version, login.
//! - [`tui_launch`] and [`acp_launch`] build the command for a pane, with the
//!   harness's hooks pointed at `"$KOU_BIN" hook <harness> <event>` for that
//!   launch only, and the app's orchestration server and guidance attached
//!   when [`PaneContext::orchestration`] asks; [`acp_setup`] is what the chat
//!   session opens with.
//! - [`Chat`] says whether a harness opens as a chat, and [`Install`] how to
//!   put a missing adapter into the app's folder; [`terminal_resume`] and
//!   [`chat_resume`] carry a conversation across the switch.
//! - [`parse_hook`] turns what such a hook delivers into a [`Signal`], and a
//!   [`Tracker`] per pane turns signals and terminal activity into an
//!   [`Activity`].
//! - [`usage`] asks a harness's own command line how much of its account's
//!   plan is used.

mod chat;
mod detect;
mod hooks;
mod launch;
mod spec;
mod tracker;
pub mod usage;

pub use chat::{
    Chat, Install, OpenSession, adapter_install, adapter_package, adapter_path, chat_resume,
    open_sessions, stop_background, terminal_resume,
};
pub use detect::{
    AcpCommand, Detected, Readiness, detect, detect_one, detect_one_with, detect_with,
};
pub use hooks::{Signal, parse_hook};
pub use launch::{
    AcpSetup, Launch, McpServer, Orchestration, PaneContext, TuiOptions, acp_launch, acp_setup,
    tui_launch, typed_prompt,
};
pub use spec::{
    ACP_REGISTRY, Acp, Arg, Attach, Binary, Coverage, Crossing, Evidence, Guidance, Hooks,
    Identity, Login, Notice, Orchestrate, Package, Screen, Sessions, Spec, Tui, Wiring, spec,
    specs,
};
pub use tracker::{Activity, HOOK_STALE, QUIET, READ_EVERY, READ_LINES, SUSTAIN, Tracker, UNSEEN};
