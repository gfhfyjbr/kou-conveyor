//! Says when the files of plugins change.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// How deep under a plugins folder files are looked at: a plugin's folder,
/// and a few folders inside it.
const DEPTH: usize = 5;

/// What is in a folder of plugins: each file with when it was written and
/// how long it is. Hidden files and `node_modules` are passed by.
fn stamp(folder: &Path, depth: usize, out: &mut Vec<(PathBuf, Option<SystemTime>, u64)>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let path = entry.path();
        let Ok(known) = std::fs::metadata(&path) else {
            continue;
        };
        if known.is_dir() {
            if depth > 0 {
                stamp(&path, depth - 1, out);
            }
        } else {
            out.push((path, known.modified().ok(), known.len()));
        }
    }
}

fn stamps(folders: &[PathBuf]) -> Vec<(PathBuf, Option<SystemTime>, u64)> {
    let mut out = Vec::new();
    for folder in folders {
        stamp(folder, DEPTH, &mut out);
    }
    out.sort();
    out
}

/// Watches `folders` of plugins and says, on what it returns, each time a
/// file in them was added, written or removed. It looks every `every`, on
/// a thread of its own that ends when what it returns is dropped; nothing
/// is said, and nobody woken, while nothing changes.
pub fn watch(folders: Vec<PathBuf>, every: Duration) -> async_channel::Receiver<()> {
    let (tell, told) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("kou-plugins-watch".into())
        .spawn(move || {
            let mut known = stamps(&folders);
            loop {
                std::thread::sleep(every);
                if tell.is_closed() {
                    break;
                }
                let now = stamps(&folders);
                if now != known {
                    known = now;
                    // One already waiting to be heard says it all.
                    if let Err(async_channel::TrySendError::Closed(())) = tell.try_send(()) {
                        break;
                    }
                }
            }
        });
    if let Err(error) = spawned {
        log::warn!("plugins are not watched for changes: {error}");
    }
    told
}
