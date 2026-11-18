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
    /// The plugin's script.
    #[serde(default)]
    script: String,
    /// Or several, run in this order in one world: what the first defines
    /// the next can use.
    #[serde(default)]
    scripts: Vec<String>,
}

/// A plugin with something for the desktop app, as found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub name: String,
    /// Its scripts in the order they run: the name errors in each are
    /// reported under, and its text.
    pub scripts: Vec<(String, String)>,
}

/// A script as it is run. One written as a module of exports alone (a file
/// a plugin shares with the browser cockpit, say) runs with what it exports
/// as names of the plugin's own: there are no modules here, and the files
/// of a plugin are one program. One that imports is refused, with where.
pub(crate) fn runnable(name: &str, text: &str) -> Result<String, String> {
    const DECLARED: [&str; 7] = [
        "const ",
        "let ",
        "var ",
        "function ",
        "function* ",
        "async function ",
        "class ",
    ];
    let mut out = String::with_capacity(text.len());
    // (Line for line and byte for byte otherwise: an error is reported
    // where it is in the file.)
    for (at, line) in text.split_inclusive('\n').enumerate() {
        if line.starts_with("import ") || line.starts_with("import{") {
            return Err(format!(
                "{name}:{}: a plugin's script cannot import; list the file under \"scripts\" before this one",
                at + 1
            ));
        }
        let kept = line
            .strip_prefix("export ")
            .filter(|rest| DECLARED.iter().any(|keyword| rest.starts_with(keyword)))
            .unwrap_or(line);
        out.push_str(kept);
    }
    Ok(out)
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
    let Some(desktop) = manifest.desktop else {
        return Ok(None);
    };
    let files: Vec<String> = desktop
        .scripts
        .into_iter()
        .chain([desktop.script])
        .filter(|file| !file.trim().is_empty())
        .collect();
    if files.is_empty() {
        return Ok(None);
    }
    let name = if manifest.name.trim().is_empty() {
        folder.to_owned()
    } else {
        manifest.name.trim().to_owned()
    };
    let mut scripts = Vec::with_capacity(files.len());
    for file in files {
        let text = read(&file).ok_or_else(|| Problem {
            plugin: name.clone(),
            message: format!("{shown}{file} could not be read"),
        })?;
        scripts.push((format!("{name}/{file}"), text));
    }
    Ok(Some(Found { name, scripts }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_of_exports_runs_as_a_script_and_one_that_imports_is_refused() {
        let shared = "// shared\nexport const LIMIT = 3;\nexport function twice(x) {\n  return x * 2;\n}\nconst own = 1;";
        assert_eq!(
            runnable("p/web/data.js", shared).unwrap(),
            "// shared\nconst LIMIT = 3;\nfunction twice(x) {\n  return x * 2;\n}\nconst own = 1;"
        );
        // Only a declaration at the start of a line is an export.
        let quoted = "const said = `\n  export const no = 1;`;\nexport default 1;\n";
        assert_eq!(runnable("p/a.js", quoted).unwrap(), quoted);
        let refused =
            runnable("p/web/genui.js", "// x\nimport { get } from './data.js';\n").unwrap_err();
        assert!(refused.starts_with("p/web/genui.js:2: "), "{refused}");
    }
}
