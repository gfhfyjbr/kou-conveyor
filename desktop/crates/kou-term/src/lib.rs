//! A terminal pane: a pseudo-terminal, an emulator and the GPUI element that draws it.
//!
//! [`Terminal`] runs a program on a pseudo-terminal and keeps the screen it has drawn;
//! reading and parsing happen on a thread of their own, and the entity reports changes
//! as [`TerminalEvent`]s, at most one redraw request per frame. [`TerminalView`] draws a
//! terminal, pictures included, and feeds it the keyboard, text input and the mouse.
//!
//! Another agent drives a pane without a view through [`Terminal::text`] (read what is on
//! screen) and [`Terminal::submit`] (paste a message and press Enter).
//!
//! The emulator is libghostty-vt, Ghostty's terminal library (through `ghostty-vt-sys`):
//! it parses the program's output, keeps the screen and its history, answers the
//! program's queries and implements the Kitty graphics protocol. Everything on the GPUI
//! side is this crate's own.

// Without a pseudo-terminal backend the reader-thread half of the crate has no caller.
#![cfg_attr(not(unix), allow(dead_code))]

mod blink;
mod boxdraw;
mod files;
mod keys;
mod mouse;
mod osc;
#[cfg(unix)]
mod pty;
mod render;
mod smear;
mod style;
mod terminal;
mod view;
mod vt;

pub use blink::CursorBlink;
pub use smear::CursorMotion;
pub use style::TerminalStyle;
pub use terminal::{Exit, Released, Spawn, Terminal, TerminalEvent};
pub use view::TerminalView;
