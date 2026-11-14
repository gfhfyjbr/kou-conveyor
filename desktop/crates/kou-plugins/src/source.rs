//! Where plugins come from, and what is found there.

use crate::host::Problem;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// A file of a plugin that is part of the program: the plugin's name, the
/// file's path within it, and its text.
pub type File = (&'static str, &'static str, &'static str);

/// Where plugins are looked for. Of two plugins of one name, the one from
/// the later source is the one that runs: a user's `usage` takes the place
/// of the app's own.
#[derive(Clone, Debug)]
pub enum Source {
    /// Compiled into the program.
    Embedded(&'static [File]),
    /// A folder with a folder for each plugin, read as it is on disk now.
    Folder(PathBuf),
}

/// `plugin.json`, as far as the desktop app reads it.
#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    desktop: Option<Desktop>,
}

#[derive(Debug, Default, Deserialize)]
struct Desktop {
    #[serde(default)]
    script: String,
}

/// A plugin with something for the desktop app, as found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub name: String,
    /// Its script's text, and the name errors in it are reported under.
    pub script: String,
    pub script_name: String,
}

/// The plugins of `sources` that have a script for the desktop app, each
/// name once, and what could not be read.
pub(crate) fn find(sources: &[Source]) -> (Vec<Found>, Vec<Problem>) {
    let mut found: Vec<Found> = Vec::new();
    let mut problems = Vec::new();
    let mut take = |plugin: Result<Option<Found>, Problem>| match plugin {
        Ok(Some(plugin)) => {
            found.retain(|known| known.name != plugin.name);
            found.push(plugin);
        }
        Ok(None) => {}
        Err(problem) => problems.push(problem),
    };
    for source in sources {
        match source {
            Source::Embedded(files) => {
                let mut names: Vec<&str> = Vec::new();
                for (plugin, _, _) in files.iter() {
                    if !names.contains(plugin) {
                        names.push(plugin);
                    }
                }
                for name in names {
                    let read = |path: &str| {
                        files
                            .iter()
                            .find(|(plugin, file, _)| *plugin == name && *file == path)
                            .map(|(_, _, text)| (*text).to_owned())
                    };
                    take(one(name, &format!("{name}/"), read));
                }
            }
            Source::Folder(folder) => {
                let Ok(entries) = std::fs::read_dir(folder) else {
                    continue;
                };
                let mut folders: Vec<PathBuf> = entries
                    .filter_map(|entry| Some(entry.ok()?.path()))
                    .filter(|path| path.join("plugin.json").is_file())
                    .collect();
                folders.sort();
                for plugin in folders {
                    let name = plugin
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let read = |path: &str| read_inside(&plugin, path);
                    take(one(&name, &format!("{}/", plugin.display()), read));
                }
            }
        }
    }
    (found, problems)
}

/// A file of a plugin's folder, unless its path leads out of the folder.
fn read_inside(plugin: &Path, path: &str) -> Option<String> {
    let inside = Path::new(path)
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)));
    if !inside {
        return None;
    }
    std::fs::read_to_string(plugin.join(path)).ok()
}

/// The plugin in the folder called `folder`, whose files `read` gives:
/// `None` when it has nothing for the desktop app.
fn one(
    folder: &str,
    shown: &str,
    read: impl Fn(&str) -> Option<String>,
) -> Result<Option<Found>, Problem> {
    let problem = |message: String| Problem {
        plugin: folder.to_owned(),
        message,
    };
    let manifest = read("plugin.json")
        .ok_or_else(|| problem(format!("{shown}plugin.json could not be read")))?;
    let manifest: Manifest = serde_json::from_str(&manifest)
        .map_err(|error| problem(format!("{shown}plugin.json: {error}")))?;
    let Some(desktop) = manifest
        .desktop
        .filter(|desktop| !desktop.script.is_empty())
    else {
        return Ok(None);
    };
    let name = if manifest.name.trim().is_empty() {
        folder.to_owned()
    } else {
        manifest.name.trim().to_owned()
    };
    let script = read(&desktop.script).ok_or_else(|| Problem {
        plugin: name.clone(),
        message: format!("{shown}{} could not be read", desktop.script),
    })?;
    Ok(Some(Found {
        script_name: format!("{name}/{}", desktop.script),
        name,
        script,
    }))
}
