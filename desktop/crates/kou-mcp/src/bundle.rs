//! The guidance as files, for harnesses that read it from disk: for each
//! flavour, the short text, one plain Markdown file with the whole guide, and
//! a Claude Code plugin holding the skill.
//!
//! Everything goes in a directory named after a hash of its content. A newer
//! app writes a new directory beside the old one rather than changing files a
//! running harness may be reading, and writing the same bundle again finds its
//! directory there and writes nothing.

use crate::guidance::{self, Flavour, Pointer};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The plugin's name: Claude Code namespaces the skill under it
/// (`/kou-desktop:orchestrate`). Both flavours use it: a launch is given one.
pub const PLUGIN_NAME: &str = "kou-desktop";

/// The skill's name, the same in both flavours.
pub const SKILL_NAME: &str = "orchestrate";

/// Where [`write_bundle`] put both flavours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    /// For a launch with the MCP server attached.
    pub tools: Guidance,
    /// For a launch that works through the command line.
    pub commands: Guidance,
}

impl Bundle {
    /// The guidance of one flavour.
    pub fn get(&self, flavour: Flavour) -> &Guidance {
        match flavour {
            Flavour::Tools => &self.tools,
            Flavour::Commands => &self.commands,
        }
    }
}

/// The files of one flavour. Attach one flavour per launch: both plugins hold
/// a skill of the same name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guidance {
    /// The short instructions, naming the file [`Guidance::guide`]: what to
    /// put into the context at launch for a harness that takes instructions
    /// as text or as a file.
    pub brief: PathBuf,
    /// The short instructions and the long guide as one Markdown file.
    pub guide: PathBuf,
    /// A plugin directory for `claude --plugin-dir`: `.claude-plugin/plugin.json`
    /// and `skills/orchestrate/SKILL.md`.
    pub claude_plugin: PathBuf,
    /// Where the agent is, in a few lines ([`Flavour::place`]): what to put
    /// into the context at launch for a harness that is given the plugin,
    /// whose skill it reads only when its description fits the task.
    pub place: PathBuf,
}

const BRIEF_FILE: &str = "brief.md";
const PLACE_FILE: &str = "place.md";
const GUIDE_FILE: &str = "kou-desktop.md";
const PLUGIN_DIR: &str = "claude-plugin";

/// Write the guidance of both flavours under `dir`, unless a bundle with the
/// same content is already there, and say where each is.
pub fn write_bundle(dir: &Path) -> io::Result<Bundle> {
    // The hash is taken over the content with the briefs naming no real
    // place, so it does not depend on where the bundle goes.
    let hash = hash(&files(Path::new("")));
    let root = dir.join(format!("kou-mcp-{hash:016x}"));
    let guidance = |flavour: Flavour| {
        let base = root.join(flavour.word());
        Guidance {
            brief: base.join(BRIEF_FILE),
            guide: base.join(GUIDE_FILE),
            claude_plugin: base.join(PLUGIN_DIR),
            place: base.join(PLACE_FILE),
        }
    };
    let bundle = Bundle {
        tools: guidance(Flavour::Tools),
        commands: guidance(Flavour::Commands),
    };
    if root.is_dir() {
        return Ok(bundle);
    }
    fs::create_dir_all(dir)?;
    // Written aside and renamed into place, so no one ever sees half a bundle.
    let staging = dir.join(format!(".kou-mcp-{hash:016x}.{}.tmp", std::process::id()));
    drop(fs::remove_dir_all(&staging));
    let written = files(&root).iter().try_for_each(|(path, content)| {
        let path = staging.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)
    });
    let placed = written.and_then(|()| fs::rename(&staging, &root));
    if placed.is_err() {
        drop(fs::remove_dir_all(&staging));
    }
    match placed {
        Ok(()) => Ok(bundle),
        // Another process put the same bundle there first.
        Err(_) if root.is_dir() => Ok(bundle),
        Err(error) => Err(error),
    }
}

/// The short instructions of `flavour`, naming `guide` as the full guide.
pub(crate) fn brief(flavour: Flavour, guide: &Path) -> String {
    guidance::render(guidance::INSTRUCTIONS, flavour, Pointer::File(guide)).text
}

/// Where the agent is, in a few lines, naming `guide` as the full guide
/// for a harness without skills.
pub(crate) fn place(flavour: Flavour, guide: &Path) -> String {
    guidance::render(guidance::PLACE, flavour, Pointer::File(guide)).text
}

/// The plain file: the rules, then the how-to.
pub(crate) fn guide_file(flavour: Flavour) -> String {
    let rules = guidance::render(guidance::INSTRUCTIONS, flavour, Pointer::Omit).text;
    format!(
        "# Working with other agents in kou desktop\n\n{}\n\n{}",
        rules.trim_end(),
        flavour.guide().trim_end()
    ) + "\n"
}

/// The skill: frontmatter, then the how-to. The rules reach the model in the
/// short instructions.
pub(crate) fn skill_file(flavour: Flavour) -> String {
    let rules = match flavour {
        Flavour::Tools => "the kou server's instructions",
        Flavour::Commands => "the kou desktop instructions, if you were started with them,",
    };
    format!(
        "---\nname: {}\ndescription: {}\n---\n\n# Orchestrating agents in kou desktop\n\n\
         This is the how-to; the rules in {rules} still apply.\n\n{}\n",
        yaml_string(SKILL_NAME),
        yaml_string(flavour.skill_description()),
        flavour.guide().trim_end()
    )
}

pub(crate) fn plugin_manifest(flavour: Flavour) -> String {
    let what = match flavour {
        Flavour::Tools => "with its MCP tools",
        Flavour::Commands => "with its command line",
    };
    let manifest = serde_json::json!({
        "name": PLUGIN_NAME,
        "description": format!("Guidance for orchestrating other coding agents from inside kou desktop, {what}."),
        "version": env!("CARGO_PKG_VERSION"),
    });
    serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n"
}

/// A YAML double-quoted scalar. A JSON string is one, YAML 1.2 being a superset of JSON.
fn yaml_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// Every file of the bundle at `root`, by path relative to it.
fn files(root: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for flavour in Flavour::ALL {
        let base = flavour.word();
        let guide = root.join(base).join(GUIDE_FILE);
        files.push((format!("{base}/{BRIEF_FILE}"), brief(flavour, &guide)));
        files.push((format!("{base}/{PLACE_FILE}"), place(flavour, &guide)));
        files.push((format!("{base}/{GUIDE_FILE}"), guide_file(flavour)));
        files.push((
            format!("{base}/{PLUGIN_DIR}/.claude-plugin/plugin.json"),
            plugin_manifest(flavour),
        ));
        files.push((
            format!("{base}/{PLUGIN_DIR}/skills/{SKILL_NAME}/SKILL.md"),
            skill_file(flavour),
        ));
    }
    files
}

/// FNV-1a over every path and content, each followed by a zero byte. It only
/// has to change when the content does and stay the same across builds and
/// Rust versions, which the standard library's hasher does not promise.
fn hash(files: &[(String, String)]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (path, content) in files {
        for byte in path.bytes().chain([0]).chain(content.bytes()).chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}
