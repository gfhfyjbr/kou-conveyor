//! A harness as a chat: whether it can open as one, what an adapter it lacks
//! would take to install, and whether a conversation keeps across a switch
//! between the chat and the terminal interface.
//!
//! Nothing here installs anything. [`Install`] describes a command for the
//! app to show and run, into a folder the app passes in, and how to tell
//! afterwards that it worked.

use crate::spec::{Acp, Arg, Crossing, Package, Spec};
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};

/// Whether a harness can open as a chat now, and if not, why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chat {
    /// It can: [`crate::Detected::acp`] holds the command.
    Ready,
    /// Its chat goes through an adapter that is not installed.
    NeedsAdapter {
        /// One sentence for the user.
        message: String,
        /// How the app can install it, when it can: `None` without a folder
        /// for adapters or without npm.
        install: Option<Box<Install>>,
    },
    /// It could open, but its status command says no one is signed in.
    NeedsLogin(String),
    /// It has an ACP route, but not on this machine as it is: the harness is
    /// missing, too old, or broken. The text says which.
    Unavailable(String),
    /// It has no ACP route at all, so it runs only in its terminal interface.
    None,
}

/// A command that installs an adapter into the app's folder, for the app to
/// show, run and check. Only that folder is written to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    /// The harness it is for.
    pub harness: &'static str,
    /// The package and its exact version, as npm takes it.
    pub package: String,
    /// The program to run: npm, as found on `PATH`.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// The directory to run it in, which is also the one it writes into.
    pub cwd: PathBuf,
    /// The only directory the install writes into.
    pub writes: PathBuf,
    /// Where it downloads from.
    pub source: &'static str,
    /// About how much it puts on disk, in megabytes.
    pub megabytes: u32,
    /// The program detection finds once it worked.
    pub binary: PathBuf,
    /// What it does, in a sentence or two the app can show before the user
    /// confirms.
    pub summary: String,
}

impl Install {
    /// Whether the adapter is there now, as detection will find it.
    pub fn installed(&self) -> bool {
        self.binary.is_file()
    }
}

/// Where an npm install into `dir` puts a package's program.
pub fn adapter_path(dir: &Path, binary: &str) -> PathBuf {
    dir.join("node_modules").join(".bin").join(binary)
}

/// The npm package of a harness's adapter, when its chat goes through one.
pub fn adapter_package(spec: &Spec) -> Option<Package> {
    spec.acp.iter().find_map(|acp| match acp {
        Acp::Adapter { package, .. } => *package,
        Acp::Builtin { .. } => None,
    })
}

/// How to install a harness's adapter into `dir`, with the npm found at
/// `npm`. `cli` is the harness's own command line when it is installed: the
/// adapter is then pointed at it and the copy the package would bring is left
/// out, so both interfaces run one version.
///
/// `None` when the harness has no adapter that npm installs.
pub fn adapter_install(
    spec: &'static Spec,
    dir: &Path,
    npm: &Path,
    cli: Option<&Path>,
) -> Option<Install> {
    let package = adapter_package(spec)?;
    let shares_cli = package.cli_env.is_some() && cli.is_some();
    let pinned = format!("{}@{}", package.npm, package.version);
    let mut args: Vec<String> = [
        "install",
        "--prefix",
        &dir.to_string_lossy(),
        "--save-exact",
        "--no-audit",
        "--no-fund",
        // No package in these trees has an install script (checked for the
        // Claude Code and Codex adapters); refusing them keeps it that way.
        "--ignore-scripts",
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect();
    if shares_cli {
        args.push("--omit=optional".to_owned());
    }
    args.push(pinned.clone());
    let megabytes = if shares_cli {
        package.megabytes.0
    } else {
        package.megabytes.1
    };
    let mut summary = format!(
        "Installs the ACP adapter for {}, {pinned}, and its dependencies from the npm registry into {} (about {megabytes} MB).",
        spec.name,
        dir.display()
    );
    if shares_cli {
        summary.push_str(&format!(
            " It runs the {} you have installed, so the chat and the terminal are one version.",
            spec.name
        ));
    } else if package.cli_env.is_some() {
        summary.push_str(&format!(
            " {} is not installed, so the adapter brings its own copy of it.",
            spec.name
        ));
    }
    summary.push_str(" Nothing outside that folder changes.");
    Some(Install {
        harness: spec.id,
        package: pinned,
        program: npm.to_path_buf(),
        args,
        cwd: dir.to_path_buf(),
        writes: dir.to_path_buf(),
        source: "the npm registry (registry.npmjs.org)",
        megabytes,
        binary: adapter_path(dir, package.binary),
        summary,
    })
}

/// The id to resume in the terminal interface a conversation begun as a chat
/// with this ACP session id. `None` when the terminal cannot resume it, and a
/// switch therefore starts a new conversation.
///
/// A session with no turn yet may not be saved at all (Claude Code saves none),
/// so even an id returned here can fail to resume; the app can tell from an
/// empty thread.
pub fn terminal_resume(spec: &Spec, acp_session: &str) -> Option<String> {
    if acp_session.is_empty() || spec.tui.resume == Arg::No || spec.acp.is_empty() {
        return None;
    }
    match spec.sessions.crossing {
        Crossing::SameId => Some(acp_session.to_owned()),
        Crossing::PiSessionFile => pi_session_file(&pi_sessions()?, acp_session)
            .map(|file| file.to_string_lossy().into_owned()),
        Crossing::None => None,
    }
}

/// The ACP session to reopen for a conversation begun in the terminal,
/// from the id the terminal resumes it by. `None` when the chat cannot
/// reopen it.
pub fn chat_resume(spec: &Spec, terminal_session: &str) -> Option<String> {
    if terminal_session.is_empty() || spec.acp.is_empty() {
        return None;
    }
    match spec.sessions.crossing {
        Crossing::SameId => Some(terminal_session.to_owned()),
        Crossing::PiSessionFile => pi_session_id(Path::new(terminal_session)),
        Crossing::None => None,
    }
}

/// A session a harness has open right now, outside this app or in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSession {
    /// The session's id, as the harness resumes it by.
    pub session: String,
    /// The name it is attached by, when the harness runs it in the
    /// background: such a session cannot be resumed anywhere, a chat
    /// included, while it runs there. It is opened in a terminal by
    /// attaching to it, or stopped there first ([`stop_background`]) and
    /// then resumed like any other.
    pub background: Option<String>,
    /// Whether it is in a turn right now.
    pub busy: bool,
}

/// Stop a session the harness runs in the background, by the name it is
/// attached by. Its conversation is kept, and can be resumed afterwards
/// (Claude Code: `claude stop <id>`). The turn it was in, if any, ends.
pub fn stop_background(
    detected: &crate::Detected,
    name: &str,
    env: &[(String, String)],
) -> Result<(), String> {
    let (true, Some(program)) = (detected.spec.id == "claude", &detected.path) else {
        return Err(format!(
            "{} has no background sessions to stop",
            detected.spec.name
        ));
    };
    // (`env`: the home of the account the session is of.)
    match crate::detect::run_in(
        program,
        &["stop", name],
        env,
        std::time::Duration::from_secs(15),
    ) {
        Ok(output) if output.status == Some(0) => Ok(()),
        Ok(output) => Err(output
            .text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("it did not say why")
            .trim()
            .to_owned()),
        Err(error) => Err(error.to_string()),
    }
}

/// The sessions the harness has open right now, where it can say: Claude
/// Code can (`claude agents --json`, which lists them and opens nothing).
/// Empty for the others, and when the harness does not answer.
pub fn open_sessions(detected: &crate::Detected, env: &[(String, String)]) -> Vec<OpenSession> {
    let (true, Some(program)) = (detected.spec.id == "claude", &detected.path) else {
        return Vec::new();
    };
    // (`env`: the home of the account asked about, when it is not the
    // program's own.)
    match crate::detect::run_in(
        program,
        &["agents", "--json"],
        env,
        std::time::Duration::from_secs(3),
    ) {
        Ok(output) if output.status == Some(0) => claude_agents(&output.text),
        _ => Vec::new(),
    }
}

/// What `claude agents --json` prints (2.1.322): an array of
/// `{sessionId, kind: "background" | "interactive", id, ...}`, `id` being
/// the short name `claude attach` takes.
pub(crate) fn claude_agents(json: &str) -> Vec<OpenSession> {
    // The array, whatever is printed around it.
    let array = json
        .find('[')
        .zip(json.rfind(']'))
        .filter(|(from, to)| from < to)
        .map_or("", |(from, to)| &json[from..=to]);
    let Ok(serde_json::Value::Array(listed)) = serde_json::from_str(array) else {
        return Vec::new();
    };
    listed
        .iter()
        .filter_map(|entry| {
            let session = entry.get("sessionId")?.as_str()?.to_owned();
            let background = (entry.get("kind").and_then(|kind| kind.as_str())
                == Some("background"))
            .then(|| {
                entry
                    .get("id")
                    .and_then(|id| id.as_str())
                    .map(str::to_owned)
            })
            .flatten();
            let said = |key: &str| entry.get(key).and_then(|value| value.as_str());
            Some(OpenSession {
                session,
                background,
                // An interactive session says `status`, one in the
                // background `state`: either way a turn that runs is one
                // the user is asked about before it is stopped. (Asking
                // once too often costs a click; not asking ends work.)
                busy: said("status") == Some("busy")
                    || matches!(said("state"), Some("working" | "busy" | "running")),
            })
        })
        .collect()
}

/// Where Pi keeps its sessions: `$PI_CODING_AGENT_DIR/sessions`, else
/// `~/.pi/agent/sessions`. A `sessionDir` in Pi's own settings is not read.
fn pi_sessions() -> Option<PathBuf> {
    let agent = std::env::var_os("PI_CODING_AGENT_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(dirs::home_dir()?.join(".pi").join("agent")))?;
    Some(agent.join("sessions"))
}

/// The session file under `root` whose first line names `id`.
pub(crate) fn pi_session_file(root: &Path, id: &str) -> Option<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
                && pi_session_id(&path).as_deref() == Some(id)
            {
                return Some(path);
            }
        }
    }
    None
}

/// The id in the first line of a Pi session file:
/// `{"type":"session","id":"...","cwd":"..."}`.
pub(crate) fn pi_session_id(file: &Path) -> Option<String> {
    let mut first = String::new();
    BufReader::new(std::fs::File::open(file).ok()?)
        .read_line(&mut first)
        .ok()?;
    let header: serde_json::Value = serde_json::from_str(first.trim()).ok()?;
    if header["type"] != "session" {
        return None;
    }
    header["id"].as_str().map(str::to_owned)
}

#[cfg(test)]
mod open_tests {
    use super::*;

    #[test]
    fn claudes_open_sessions_are_read_with_how_to_reach_each() {
        // As 2.1.322 printed it, cut down to what is read.
        let listed = claude_agents(
            r#"[
              {"id":"c255120d","cwd":"/work","kind":"background","sessionId":"c255120d-2731","state":"working","pid":18510},
              {"cwd":"/work","kind":"interactive","sessionId":"8f9526eb-1046","status":"busy","pid":20184},
              {"kind":"background","sessionId":"no-name-to-attach-by"},
              {"kind":"background"}
            ]"#,
        );
        assert_eq!(
            listed,
            [
                OpenSession {
                    session: "c255120d-2731".into(),
                    background: Some("c255120d".into()),
                    busy: true,
                },
                OpenSession {
                    session: "8f9526eb-1046".into(),
                    background: None,
                    busy: true,
                },
                OpenSession {
                    session: "no-name-to-attach-by".into(),
                    background: None,
                    busy: false,
                },
            ]
        );
        assert!(claude_agents("not json").is_empty());
        assert!(claude_agents("{}").is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::spec;

    #[test]
    fn an_install_names_the_package_the_folder_and_the_program_it_leaves() {
        let dir = Path::new("/Users/someone/Library/Application Support/kou/adapters");
        let install = adapter_install(
            spec("claude").unwrap(),
            dir,
            Path::new("/opt/homebrew/bin/npm"),
            Some(Path::new("/usr/local/bin/claude")),
        )
        .unwrap();
        assert_eq!(install.program, Path::new("/opt/homebrew/bin/npm"));
        assert_eq!(
            install.args,
            [
                "install",
                "--prefix",
                "/Users/someone/Library/Application Support/kou/adapters",
                "--save-exact",
                "--no-audit",
                "--no-fund",
                "--ignore-scripts",
                "--omit=optional",
                "@agentclientprotocol/claude-agent-acp@0.106.0",
            ]
        );
        assert_eq!(install.cwd, dir);
        assert_eq!(install.writes, dir);
        assert_eq!(
            install.binary,
            dir.join("node_modules/.bin/claude-agent-acp")
        );
        assert_eq!(install.megabytes, 70);
        assert!(
            install.summary.contains("one version"),
            "{}",
            install.summary
        );
        assert!(!install.installed());

        // Without the CLI the package's own copy comes along.
        let full = adapter_install(spec("codex").unwrap(), dir, Path::new("npm"), None).unwrap();
        assert!(!full.args.iter().any(|arg| arg == "--omit=optional"));
        assert_eq!(
            full.args.last().unwrap(),
            "@agentclientprotocol/codex-acp@2.6.0"
        );
        assert_eq!(full.megabytes, 400);
        assert!(
            full.summary.contains("brings its own copy"),
            "{}",
            full.summary
        );

        // Pi's adapter is pointed at pi, and Amp's at nothing.
        let pi = adapter_install(
            spec("pi").unwrap(),
            dir,
            Path::new("npm"),
            Some(Path::new("/bin/pi")),
        );
        assert_eq!(pi.unwrap().args.last().unwrap(), "pi-acp@0.0.34");
        let amp = adapter_install(
            spec("amp").unwrap(),
            dir,
            Path::new("npm"),
            Some(Path::new("/bin/amp")),
        );
        assert!(!amp.unwrap().args.iter().any(|arg| arg == "--omit=optional"));
        // A harness with its own ACP mode has nothing to install.
        assert!(adapter_install(spec("opencode").unwrap(), dir, Path::new("npm"), None).is_none());
    }

    #[test]
    fn a_conversation_crosses_where_the_ids_are_shared() {
        for id in ["claude", "codex", "gemini", "opencode", "grok"] {
            let spec = spec(id).unwrap();
            assert_eq!(
                terminal_resume(spec, "abc-1").as_deref(),
                Some("abc-1"),
                "{id}"
            );
            assert_eq!(chat_resume(spec, "abc-1").as_deref(), Some("abc-1"), "{id}");
        }
        for id in [
            "cursor", "copilot", "amp", "droid", "goose", "qwen", "kiro", "aider", "kou", "shell",
        ] {
            let spec = spec(id).unwrap();
            assert_eq!(terminal_resume(spec, "abc-1"), None, "{id}");
            assert_eq!(chat_resume(spec, "abc-1"), None, "{id}");
        }
        assert_eq!(terminal_resume(spec("claude").unwrap(), ""), None);
    }

    #[test]
    fn a_pi_session_is_found_by_the_id_in_its_file() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("--work-api--");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join("2026-11-12T10-00-00_abc.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"session\",\"version\":3,\"id\":\"0199-pi\",\"cwd\":\"/work/api\"}\n{\"type\":\"message\"}\n",
        )
        .unwrap();
        std::fs::write(project.join("other.jsonl"), "{\"type\":\"message\"}\n").unwrap();
        assert_eq!(pi_session_file(root.path(), "0199-pi"), Some(file.clone()));
        assert_eq!(pi_session_file(root.path(), "0199-nope"), None);
        assert_eq!(pi_session_id(&file).as_deref(), Some("0199-pi"));
        assert_eq!(
            chat_resume(spec("pi").unwrap(), &file.to_string_lossy()).as_deref(),
            Some("0199-pi")
        );
    }
}
