//! Asks an agent's program for its account's limits and prints them.
//!
//! `cargo run -p kou-usage --example ask -- <claude|codex|grok> <program> [home]`
//!
//! `home` is the folder of another account (see `kou_usage::home_variable`).
//! This asks the real program, with the login it has: nothing is sent to a
//! model, but the agent's service is asked for the account's usage.

use std::time::{Duration, SystemTime};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(harness), Some(program)) = (args.next(), args.next()) else {
        eprintln!("usage: ask <claude|codex|grok> <program> [home]");
        std::process::exit(2);
    };
    let env = args
        .next()
        .and_then(|home| Some((kou_usage::home_variable(&harness)?.to_owned(), home)))
        .into_iter()
        .collect();
    let ask = kou_usage::Ask {
        program: program.into(),
        env,
        timeout: Duration::from_secs(20),
    };
    let started = std::time::Instant::now();
    let Some(answer) = kou_usage::limits(&harness, &ask) else {
        eprintln!("{harness} cannot be asked");
        std::process::exit(2);
    };
    let took = started.elapsed();
    match answer {
        Ok(limits) => {
            println!("plan: {}", limits.plan.as_deref().unwrap_or("not said"));
            let now = SystemTime::now();
            for window in &limits.windows {
                let resets = window.resets.map_or_else(
                    || "no reset said".to_owned(),
                    |at| format!("resets in {}", kou_usage::until(at, now)),
                );
                println!(
                    "{:<16} {:>5.1}%  {resets}",
                    window.kind.label(),
                    window.used
                );
            }
            if limits.windows.is_empty() {
                println!("no windows");
            }
        }
        Err(reason) => println!("unavailable: {}", reason.message()),
    }
    println!("took {took:.1?}");
}
