//! Greets a Claude Code and prints the models it offers: the one exchange
//! with it that sends nothing to a model and writes no record.
//!
//!     models_check <claude> <cwd> [<home>]
//!
//! With `<home>` Claude Code runs under that folder (`CLAUDE_CONFIG_DIR`),
//! not its own.

use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(program), Some(cwd)) = (args.first(), args.get(1)) else {
        eprintln!("usage: models_check <claude> <cwd> [<home>]");
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
    match kou_claude::models(&options, Duration::from_secs(30)) {
        Ok(offered) => {
            println!(
                "{} models in {:?}, a new conversation starts with {:?}",
                offered.models.len(),
                started.elapsed(),
                offered.current
            );
            for model in offered.models {
                println!("  {} ({})", model.id, model.name);
            }
        }
        Err(error) => {
            println!("no answer in {:?}: {error:#}", started.elapsed());
            std::process::exit(1);
        }
    }
}
