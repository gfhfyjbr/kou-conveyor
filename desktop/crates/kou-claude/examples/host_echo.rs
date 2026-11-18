//! A stand-in program for the host's tests (`tests/host.rs`), where the
//! scripted Claude Code (`claude_fake.rs`) cannot do what a test needs. It
//! reads JSON lines and does what the `do` of each says:
//!
//! - `echo`: writes the line back as it is.
//! - `noise`, with `text`: writes the text on a line of its own.
//! - `stderr`, with `text`: writes the text to standard error.
//! - `flood`, with `lines` and `mark`: writes that many lines of a kilobyte,
//!   then writes the file `mark`.
//! - `exit`, with `code` and `after_ms`: waits that long, says `leaving` on
//!   standard error and exits with the code.

use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    process, thread,
    time::Duration,
};

fn main() {
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else {
            return;
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let text = message["text"].as_str().unwrap_or_default();
        match message["do"].as_str() {
            Some("echo") => {
                let _ = writeln!(out, "{line}").and_then(|()| out.flush());
            }
            Some("noise") => {
                let _ = writeln!(out, "{text}").and_then(|()| out.flush());
            }
            Some("stderr") => eprintln!("{text}"),
            Some("flood") => {
                let pad = "x".repeat(1000);
                for at in 0..message["lines"].as_u64().unwrap_or(0) {
                    let _ = writeln!(out, "{}", json!({"flood": at, "pad": pad}));
                }
                let _ = out.flush();
                if let Some(mark) = message["mark"].as_str() {
                    let _ = std::fs::write(mark, "done");
                }
            }
            Some("exit") => {
                let wait = message["after_ms"].as_u64().unwrap_or(0);
                thread::sleep(Duration::from_millis(wait));
                eprintln!("leaving");
                process::exit(message["code"].as_i64().unwrap_or(0) as i32);
            }
            _ => {}
        }
    }
}
