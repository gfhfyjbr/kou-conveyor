//! A look at how the records reader fares on records that were not made up
//! for a test: how many conversations a folder lists and how long that
//! takes, and what a record replays as. It prints counts and times only,
//! never anything that was said.
//!
//!     records_check sessions <home> <cwd>
//!     records_check replay <record.jsonl>

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["sessions", home, cwd] => {
            let started = Instant::now();
            let sessions = kou_claude::sessions(Path::new(home), Path::new(cwd));
            let untitled = sessions
                .iter()
                .filter(|session| session.title.trim().is_empty())
                .count();
            let longest = sessions
                .iter()
                .map(|session| session.title.chars().count())
                .max()
                .unwrap_or(0);
            let ordered = sessions
                .windows(2)
                .all(|pair| pair[0].updated >= pair[1].updated);
            println!(
                "{} conversations in {:?}: {untitled} without a title, the longest title {longest} \
                 characters, newest first: {ordered}",
                sessions.len(),
                started.elapsed(),
            );
        }
        ["replay", record] => {
            let started = Instant::now();
            let updates = kou_claude::records::replay(Path::new(record));
            let took = started.elapsed();
            let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
            for update in &updates {
                let shown = format!("{update:?}");
                let kind: String = shown
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                *kinds.entry(kind).or_default() += 1;
            }
            // Folded as the chat folds them: what the thread ends up as.
            let mut thread = kou_thread::Thread::default();
            let folding = Instant::now();
            for update in updates {
                thread.apply(update);
            }
            println!("replayed in {took:?}, folded in {:?}", folding.elapsed());
            for (kind, count) in kinds {
                println!("  {kind}: {count}");
            }
            println!(
                "  the thread: {} entries, state {:?}",
                thread.entries().len(),
                thread.state()
            );
        }
        _ => {
            eprintln!("usage: records_check sessions <home> <cwd> | replay <record.jsonl>");
            std::process::exit(2);
        }
    }
}
