//! Opens a session with a Claude Code, says nothing in it, and prints what
//! the session says of itself for a few seconds: no prompt is sent, so no
//! model is asked and no record is written.
//!
//!     connect_check <claude> <cwd> [<home>]

use kou_thread::{Command, Update};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(program), Some(cwd)) = (args.first(), args.get(1)) else {
        eprintln!("usage: connect_check <claude> <cwd> [<home>]");
        std::process::exit(2);
    };
    let mut options = kou_claude::Options::new(program, cwd);
    if let Some(home) = args.get(2) {
        options
            .env
            .push(("CLAUDE_CONFIG_DIR".to_owned(), home.clone()));
        options.home = Some(home.into());
    }
    let started = Instant::now();
    let backend = match kou_claude::connect(options) {
        Ok(backend) => backend,
        Err(error) => {
            println!("it did not start: {error:#}");
            std::process::exit(1);
        }
    };
    let mut closing = false;
    loop {
        let waited = started.elapsed();
        if !closing && waited > Duration::from_secs(4) {
            closing = true;
            let _ = backend.commands.send_blocking(Command::Close);
        }
        if waited > Duration::from_secs(10) {
            println!("it did not close");
            std::process::exit(1);
        }
        let update = match backend.updates.try_recv() {
            Ok(update) => update,
            Err(async_channel::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(async_channel::TryRecvError::Closed) => break,
        };
        let at = started.elapsed().as_millis();
        match update {
            Update::Ready {
                session,
                modes,
                mode,
                models,
                model,
                ..
            } => println!(
                "{at} ms  ready: session {:?}, modes {:?} (in force {mode:?}), {} models (in force {model:?})",
                session,
                modes
                    .iter()
                    .map(|mode| mode.id.as_str())
                    .collect::<Vec<_>>(),
                models.len()
            ),
            Update::Commands(commands) => println!("{at} ms  {} slash commands", commands.len()),
            Update::Offers(offers) => println!(
                "{at} ms  offers: steer {}, rewind {}, listening {}",
                offers.steer, offers.rewind, offers.listening
            ),
            Update::Efforts { levels, current } => println!(
                "{at} ms  efforts: {:?}, in force {current:?}",
                levels
                    .iter()
                    .map(|level| level.id.as_str())
                    .collect::<Vec<_>>()
            ),
            Update::Notice(level, text) => println!("{at} ms  notice ({level:?}): {text}"),
            Update::Closed(reason) => {
                println!("{at} ms  closed: {reason:?}");
                break;
            }
            other => {
                let shown = format!("{other:?}");
                let kind: String = shown
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                println!("{at} ms  {kind}");
            }
        }
    }
}
