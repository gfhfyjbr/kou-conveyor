//! Starts a Claude Code under a keeper, lets go of it, finds it again and
//! closes it, saying nothing in it: no prompt is sent, so no model is asked
//! and no record is written. Prints what each session says of itself.
//!
//!     keep_check <claude> <cwd> <home> <stem>
//!
//! `<stem>` is where the keeper's socket goes (a short path: `/tmp/…`). The
//! keeper is the `claude_host` example beside this one.

use kou_thread::{Backend, Command, Update};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [program, cwd, home, stem] = &args[..] else {
        eprintln!("usage: keep_check <claude> <cwd> <home> <stem>");
        std::process::exit(2);
    };
    let mut host = std::env::current_exe().expect("this program's path");
    host.set_file_name("claude_host");
    let options = || {
        let mut options = kou_claude::Options::new(program, cwd);
        options
            .env
            .push(("CLAUDE_CONFIG_DIR".to_owned(), home.clone()));
        options.home = Some(home.into());
        options.keep = Some(kou_claude::Keeper {
            host: host.clone(),
            host_args: Vec::new(),
            stem: stem.into(),
            idle: Duration::from_secs(30),
            away: Duration::from_secs(60),
        });
        options
    };

    println!("-- started under a keeper");
    let first = kou_claude::connect(options()).expect("it starts");
    listen(&first, Duration::from_secs(3));
    println!("-- let go of, without a word");
    drop(first);
    std::thread::sleep(Duration::from_secs(2));

    println!("-- looked for again");
    let second = kou_claude::connect(options()).expect("it is found");
    listen(&second, Duration::from_secs(3));
    println!("-- closed");
    let _ = second.commands.send_blocking(Command::Close);
    listen(&second, Duration::from_secs(8));
}

/// Print what the session says for `long`, or until it closes.
fn listen(backend: &Backend, long: Duration) {
    let started = Instant::now();
    while started.elapsed() < long {
        let update = match backend.updates.try_recv() {
            Ok(update) => update,
            Err(async_channel::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(async_channel::TryRecvError::Closed) => return,
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
                "   {at} ms  ready: session {session:?}, {} modes (in force {mode:?}), {} models (in force {model:?})",
                modes.len(),
                models.len()
            ),
            Update::Commands(commands) => println!("   {at} ms  {} slash commands", commands.len()),
            Update::Efforts { levels, current } => {
                println!("   {at} ms  {} efforts, in force {current:?}", levels.len())
            }
            Update::Notice(level, text) => println!("   {at} ms  notice ({level:?}): {text}"),
            Update::Closed(reason) => {
                println!("   {at} ms  closed: {reason:?}");
                return;
            }
            other => {
                let shown = format!("{other:?}");
                let kind: String = shown
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                println!("   {at} ms  {kind}");
            }
        }
    }
}
