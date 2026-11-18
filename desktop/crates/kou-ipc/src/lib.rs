//! The desktop app's local socket: orchestration requests from agents and hook
//! events from harnesses.
//!
//! The app binds a [`Server`] and answers each [`Incoming`] request through its
//! [`Reply`]. Everything else is a client: [`call`] sends one request, and
//! [`cli::run`] is the command line built on it, which is also what harness
//! hooks execute.

pub mod cli;
mod protocol;
mod transport;

pub use protocol::*;
pub use transport::{
    Incoming, MAX_REQUEST_BYTES, NotRunning, Reply, Server, call, socket_path, tell,
};
