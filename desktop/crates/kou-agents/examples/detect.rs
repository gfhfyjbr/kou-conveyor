//! Probe this machine and print what was found, and the command line each
//! installed harness would be started with. Nothing is launched or installed.
//!
//! `cargo run -p kou-agents --example detect [-- <adapters folder>]`

use kou_agents::{
    Chat, PaneContext, Readiness, TuiOptions, acp_launch, detect, detect_with, tui_launch,
};
use std::time::Instant;

fn main() {
    let started = Instant::now();
    let adapters = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let found = match &adapters {
        Some(dir) => detect_with(dir),
        None => detect(),
    };
    println!(
        "probed {} harnesses in {:.1?}\n",
        found.len(),
        started.elapsed()
    );

    let state = std::env::temp_dir().join("kou-agents-example");
    let context = PaneContext::new(
        "p1",
        "s1",
        state.join("kou.sock"),
        "/usr/local/bin/kou-desktop",
        std::env::current_dir().unwrap_or_default(),
        state.clone(),
    );
    let options = TuiOptions {
        prompt: Some("say hello".into()),
        ..TuiOptions::default()
    };
    for detected in &found {
        let readiness = match &detected.readiness {
            Readiness::Ready => "ready".to_owned(),
            Readiness::NeedsLogin => "needs login".to_owned(),
            Readiness::Outdated { minimum } => format!("older than {minimum}"),
            Readiness::Missing => "missing".to_owned(),
            Readiness::Broken(_) => "broken".to_owned(),
        };
        println!(
            "{:<9} {:<12} version {:<20} hooks {:<5} native {}",
            detected.spec.id,
            readiness,
            detected.version.as_deref().unwrap_or("-"),
            detected.hooks,
            detected.native,
        );
        if let Some(path) = &detected.path {
            println!("          at {}", path.display());
        }
        if let Some(message) = &detected.message {
            println!("          {message}");
        }
        match tui_launch(detected, &context, &options) {
            Ok(launch) => println!(
                "          tui: {} {:?}",
                launch.program.display(),
                launch.args
            ),
            Err(error) if detected.path.is_some() => println!("          tui: {error}"),
            Err(_) => {}
        }
        if let Some(launch) = acp_launch(detected, &context) {
            println!(
                "          acp: {} {:?}",
                launch.program.display(),
                launch.args
            );
        }
        match &detected.chat {
            Chat::Ready | Chat::None => {}
            Chat::NeedsAdapter { message, install } => {
                println!("          chat: {message}");
                if let Some(install) = install {
                    println!(
                        "          install: {} {}",
                        install.program.display(),
                        install.args.join(" ")
                    );
                }
            }
            Chat::NeedsLogin(why) | Chat::Unavailable(why) => println!("          chat: {why}"),
        }
    }
    println!("\ngenerated files are under {}", state.display());
}
