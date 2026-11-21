//! Finding out which harnesses are installed and usable.
//!
//! The same order of questions t3code asks of its providers: is the binary
//! there, does it answer `--version`, is that version new enough, is the user
//! signed in. Each harness is probed on its own thread and every command has
//! a deadline, so a hanging CLI costs seconds, not the app's start.
//!
//! Only version, help and status commands are ever run. The version probe
//! comes first because it also proves the binary is the expected tool: a
//! status subcommand given to a different program of the same name could be
//! taken for a prompt.
//!
//! ACP adapters are looked for first in the folder the app installs them
//! into (npm's layout: `node_modules/.bin/<name>`), then on `PATH`, so the
//! version the app installed is the one it runs.

use crate::chat::{Chat, adapter_install, adapter_package, adapter_path};
use crate::spec::{Acp, Evidence, Identity, Login, Spec, Wiring, spec, specs};
use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Whether a harness can be launched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Installed and, as far as can be told, signed in.
    Ready,
    /// Installed, and its status command says no one is signed in.
    NeedsLogin,
    /// Installed, but older than kou's launch line and hooks need.
    Outdated {
        /// The oldest version that works.
        minimum: String,
    },
    /// Not installed.
    Missing,
    /// Installed but not answering as expected; the text says how.
    Broken(String),
}

/// The command that starts a harness as an ACP agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpCommand {
    /// The harness itself, or its adapter.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Variables the mode needs: what its documentation sets, and for an
    /// adapter the path of the harness's own CLI when it is installed.
    pub env: Vec<(String, String)>,
}

/// What was found out about one harness on this machine.
#[derive(Clone, Debug)]
pub struct Detected {
    /// The harness.
    pub spec: &'static Spec,
    /// Its executable, when one was found.
    pub path: Option<PathBuf>,
    /// The version it reported.
    pub version: Option<String>,
    /// Whether it can be launched.
    pub readiness: Readiness,
    /// One line for the user: why it cannot be launched, or what to do first.
    pub message: Option<String>,
    /// Whether it can run as an ACP agent right now (see [`crate::acp_launch`]).
    pub native: bool,
    /// The ACP command found for it, which may be an adapter installed
    /// without the harness's own CLI.
    pub acp: Option<AcpCommand>,
    /// Whether this installed version can have lifecycle hooks wired for a
    /// launch. Codex needs its `hooks` feature for that, OpenCode version 2.
    pub hooks: bool,
    /// Whether it can open as a chat, and what to do when it cannot.
    pub chat: Chat,
}

/// Probe every known harness, in display order, missing ones included.
/// Probes run in parallel; each command is given a few seconds. Adapters are
/// looked for on `PATH` only; see [`detect_with`].
pub fn detect() -> Vec<Detected> {
    detect_on(&Host::from_process())
}

/// Probe one harness by id.
pub fn detect_one(id: &str) -> Option<Detected> {
    Some(detect_spec(spec(id)?, &Host::from_process()))
}

/// [`detect`], with ACP adapters looked for in `adapters` before `PATH`. That
/// is the folder the app installs them into, with npm's layout inside; a
/// missing adapter's [`Chat::NeedsAdapter`] says how to install it there.
pub fn detect_with(adapters: &Path) -> Vec<Detected> {
    detect_on(&Host::from_process().with_adapters(adapters))
}

/// [`detect_one`], with ACP adapters looked for in `adapters` before `PATH`.
pub fn detect_one_with(id: &str, adapters: &Path) -> Option<Detected> {
    Some(detect_spec(
        spec(id)?,
        &Host::from_process().with_adapters(adapters),
    ))
}

/// The parts of the machine detection looks at, so tests can stand in a
/// directory of fake CLIs.
#[derive(Clone, Debug)]
pub(crate) struct Host {
    /// The value of `PATH`.
    pub path: Option<OsString>,
    /// `$SHELL`.
    pub shell: Option<PathBuf>,
    /// Directories that may hold kou's own binaries: the repository's `bin/`.
    pub kou_dirs: Vec<PathBuf>,
    /// How long the version probe may take.
    pub version_timeout: Duration,
    /// How long each later probe may take.
    pub probe_timeout: Duration,
    /// The app's folder of adapters, searched before `PATH`.
    pub adapters: Option<PathBuf>,
}

impl Host {
    pub(crate) fn from_process() -> Self {
        // The repository's bin/ is found from where the app runs: next to a
        // development build's target directory, or above the working directory.
        let mut kou_dirs = Vec::new();
        let starts = [std::env::current_exe().ok(), std::env::current_dir().ok()];
        for start in starts.into_iter().flatten() {
            for ancestor in start.ancestors().skip(1) {
                let bin = ancestor.join("bin");
                if bin.join("kou-conveyor-tui").is_file() && !kou_dirs.contains(&bin) {
                    kou_dirs.push(bin);
                }
            }
        }
        Host {
            path: std::env::var_os("PATH"),
            shell: std::env::var_os("SHELL")
                .filter(|shell| !shell.is_empty())
                .map(PathBuf::from),
            kou_dirs,
            // Slow starters exist: `droid --version` takes three seconds here.
            version_timeout: Duration::from_secs(5),
            probe_timeout: Duration::from_secs(4),
            adapters: None,
        }
    }

    fn with_adapters(self, adapters: &Path) -> Self {
        Host {
            adapters: Some(adapters.to_path_buf()),
            ..self
        }
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        let cwd = std::env::current_dir().unwrap_or_default();
        which::which_in(name, self.path.as_ref(), cwd).ok()
    }

    /// An adapter: the app's own copy first, then one on `PATH`.
    fn adapter(&self, name: &str) -> Option<PathBuf> {
        self.adapters
            .as_deref()
            .map(|dir| adapter_path(dir, name))
            .filter(|path| path.is_file())
            .or_else(|| self.which(name))
    }

    /// A `PATH` entry of this name that is a link to nothing: an uninstalled
    /// app often leaves one behind.
    fn dangling(&self, name: &str) -> Option<PathBuf> {
        std::env::split_paths(self.path.as_ref()?)
            .map(|dir| dir.join(name))
            .find(|path| path.symlink_metadata().is_ok() && !path.exists())
    }
}

pub(crate) fn detect_on(host: &Host) -> Vec<Detected> {
    thread::scope(|scope| {
        let probes: Vec<_> = specs()
            .iter()
            .map(|spec| scope.spawn(move || detect_spec(spec, host)))
            .collect();
        probes
            .into_iter()
            .zip(specs())
            .map(|(probe, spec)| {
                probe.join().unwrap_or_else(|_| {
                    found(
                        spec,
                        None,
                        None,
                        Readiness::Broken("The probe crashed.".into()),
                    )
                })
            })
            .collect()
    })
}

fn found(
    spec: &'static Spec,
    path: Option<PathBuf>,
    version: Option<String>,
    readiness: Readiness,
) -> Detected {
    let message = match &readiness {
        Readiness::Ready => None,
        Readiness::NeedsLogin => Some(match &spec.login {
            Login::Status { hint, .. } => format!("{} is not signed in. {hint}", spec.name),
            Login::Unknown => format!("{} is not signed in.", spec.name),
        }),
        Readiness::Outdated { minimum } => Some(format!(
            "{} {} is older than {minimum}, the oldest version kou works with. Update it.",
            spec.name,
            version.as_deref().unwrap_or("(unknown version)"),
        )),
        Readiness::Missing => Some(match spec.binaries.first() {
            Some(binary) => format!("Not installed: `{}` was not found on PATH.", binary.name),
            None => "Not found: $SHELL is not set and /bin/sh does not exist.".into(),
        }),
        Readiness::Broken(why) => Some(why.clone()),
    };
    Detected {
        spec,
        path,
        version,
        readiness,
        message,
        native: false,
        acp: None,
        hooks: false,
        chat: Chat::None,
    }
}

fn detect_spec(spec: &'static Spec, host: &Host) -> Detected {
    let mut detected = detect_cli(spec, host);
    // An adapter is a program of its own and may be installed without the CLI.
    let usable = matches!(detected.readiness, Readiness::Ready | Readiness::Missing);
    let found = if usable || detected.readiness == Readiness::NeedsLogin {
        find_acp(spec, detected.path.as_deref(), host)
    } else {
        None
    };
    if usable {
        detected.acp = found.clone();
    }
    if detected.readiness == Readiness::Missing
        && let Some(acp) = &detected.acp
    {
        detected.message = Some(format!(
            "Only the ACP adapter `{}` is installed; the terminal interface is not.",
            file_name(&acp.program)
        ));
    }
    detected.native = detected.acp.is_some();
    detected.chat = chat_state(&detected, found.is_some(), host);
    detected
}

/// What stands between a harness and its chat. A missing adapter comes
/// before a missing login: it can be installed now, the login done later.
fn chat_state(detected: &Detected, found: bool, host: &Host) -> Chat {
    let spec = detected.spec;
    if spec.acp.is_empty() {
        return Chat::None;
    }
    if detected.acp.is_some() {
        return Chat::Ready;
    }
    let reason = || {
        detected
            .message
            .clone()
            .unwrap_or_else(|| format!("{} cannot open as a chat here.", spec.name))
    };
    if let Some(package) = adapter_package(spec).filter(|_| !found) {
        if matches!(
            detected.readiness,
            Readiness::Outdated { .. } | Readiness::Broken(_)
        ) {
            return Chat::Unavailable(reason());
        }
        let mut message = format!(
            "{}'s chat needs its ACP adapter `{}`, which is not installed.",
            spec.name, package.binary
        );
        let npm = host.which("npm");
        let install = match (&host.adapters, &npm) {
            (Some(dir), Some(npm)) => {
                adapter_install(spec, dir, npm, detected.path.as_deref()).map(Box::new)
            }
            (Some(_), None) => {
                message.push_str(" Installing it needs npm, which is not on PATH.");
                None
            }
            (None, _) => None,
        };
        return Chat::NeedsAdapter { message, install };
    }
    match &detected.readiness {
        Readiness::NeedsLogin => Chat::NeedsLogin(reason()),
        Readiness::Ready if detected.path.is_some() => Chat::Unavailable(format!(
            "{} {} shows no ACP mode; a newer version has one.",
            spec.name,
            detected.version.as_deref().unwrap_or("(unknown version)")
        )),
        _ => Chat::Unavailable(reason()),
    }
}

fn detect_cli(spec: &'static Spec, host: &Host) -> Detected {
    if spec.id == "shell" {
        let shell = host.shell.clone().unwrap_or_else(|| "/bin/sh".into());
        return if shell.is_file() {
            found(spec, Some(shell), None, Readiness::Ready)
        } else {
            found(spec, None, None, Readiness::Missing)
        };
    }

    let mut candidates: Vec<(PathBuf, &Identity)> = spec
        .binaries
        .iter()
        .filter_map(|binary| Some((host.which(binary.name)?, &binary.identity)))
        .collect();
    if spec.id == "kou" {
        for dir in &host.kou_dirs {
            for binary in spec.binaries {
                let path = dir.join(binary.name);
                if path.is_file() {
                    candidates.push((path, &binary.identity));
                }
            }
        }
    }

    let mut failure = None;
    for (path, identity) in candidates {
        if spec.version.is_empty() {
            return found(spec, Some(path), None, Readiness::Ready);
        }
        let command = format!("`{} {}`", file_name(&path), spec.version.join(" "));
        let output = match run(&path, spec.version, host.version_timeout) {
            Ok(output) => output,
            Err(error) => {
                failure.get_or_insert((path, format!("{command} could not be started: {error}")));
                continue;
            }
        };
        let is_this_tool = match identity {
            Identity::Any => true,
            Identity::Reports(word) => output.text.to_lowercase().contains(word),
            Identity::DoesNotReport(word) => !output.text.to_lowercase().contains(word),
        };
        if !is_this_tool {
            continue;
        }
        if output.timed_out {
            let seconds = host.version_timeout.as_secs_f32();
            let why = format!("{command} did not answer within {seconds:.0} s.");
            return found(spec, Some(path), None, Readiness::Broken(why));
        }
        let version = parse_version(&output.text);
        if output.status != Some(0) {
            let why = format!(
                "{command} failed{}: {}",
                output
                    .status
                    .map(|code| format!(" with status {code}"))
                    .unwrap_or_default(),
                first_line(&output.text)
            );
            return found(spec, Some(path), version, Readiness::Broken(why));
        }
        return finish_cli(spec, path, version, host);
    }

    if let Some((path, why)) = failure {
        return found(spec, Some(path), None, Readiness::Broken(why));
    }
    let dangling = spec
        .binaries
        .iter()
        .find_map(|binary| host.dangling(binary.name));
    if let Some(link) = dangling {
        let why = format!(
            "`{}` on PATH points to a file that no longer exists; reinstall {}.",
            link.display(),
            spec.name
        );
        return found(spec, None, None, Readiness::Broken(why));
    }
    found(spec, None, None, Readiness::Missing)
}

/// The binary is there and is the right tool: check its age, its login and
/// what its hooks can do.
fn finish_cli(
    spec: &'static Spec,
    path: PathBuf,
    version: Option<String>,
    host: &Host,
) -> Detected {
    if let (Some(minimum), Some(version)) = (spec.minimum, &version)
        && older(version, minimum)
    {
        let readiness = Readiness::Outdated {
            minimum: minimum.to_owned(),
        };
        return found(spec, Some(path), Some(version.clone()), readiness);
    }

    let (signed_out, hooks) = thread::scope(|scope| {
        let login = scope.spawn(|| signed_out(spec, &path, host));
        let hooks = scope.spawn(|| hooks_available(spec, &path, version.as_deref(), host));
        (login.join().unwrap_or(false), hooks.join().unwrap_or(false))
    });
    let readiness = if signed_out {
        Readiness::NeedsLogin
    } else {
        Readiness::Ready
    };
    let mut detected = found(spec, Some(path), version, readiness);
    detected.hooks = hooks;
    detected
}

/// Whether the status command says no one is signed in. A probe that fails
/// or says something unexpected is not proof: the launch itself will tell.
fn signed_out(spec: &Spec, path: &Path, host: &Host) -> bool {
    let Login::Status {
        args, signed_out, ..
    } = &spec.login
    else {
        return false;
    };
    let Ok(output) = run(path, args, host.probe_timeout) else {
        return false;
    };
    let squeezed: String = output.text.to_lowercase().split_whitespace().collect();
    signed_out.iter().any(|needle| squeezed.contains(needle))
}

fn hooks_available(spec: &Spec, path: &Path, version: Option<&str>, host: &Host) -> bool {
    match spec.hooks.wiring {
        Wiring::None => false,
        // `codex features list` prints one line per feature: name, stage, state.
        Wiring::CodexConfig => {
            run(path, &["features", "list"], host.probe_timeout).is_ok_and(|output| {
                output.text.lines().any(|line| {
                    let mut words = line.split_whitespace();
                    words.next() == Some("hooks") && words.next_back() == Some("true")
                })
            })
        }
        // The plugin interface this wiring uses arrived with OpenCode 2.
        Wiring::OpencodePlugin => version.is_some_and(|version| !older(version, "2.0.0")),
        _ => true,
    }
}

fn find_acp(spec: &Spec, cli: Option<&Path>, host: &Host) -> Option<AcpCommand> {
    let owned = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
    spec.acp.iter().find_map(|acp| match acp {
        Acp::Adapter {
            binaries,
            args,
            package,
        } => {
            let program = binaries.iter().find_map(|name| host.adapter(name))?;
            // The adapter runs the harness the terminal runs, when there is one.
            let env = match (package.and_then(|package| package.cli_env), cli) {
                (Some(variable), Some(cli)) => {
                    vec![(variable.to_owned(), cli.to_string_lossy().into_owned())]
                }
                _ => Vec::new(),
            };
            Some(AcpCommand {
                program,
                args: owned(args),
                env,
            })
        }
        Acp::Builtin {
            args,
            evidence,
            env,
        } => {
            let cli = cli?;
            let offered = match evidence {
                Evidence::Documented => true,
                Evidence::Help { args: help, needle } => run(cli, help, host.probe_timeout)
                    .is_ok_and(|output| output.text.to_lowercase().contains(needle)),
            };
            offered.then(|| AcpCommand {
                program: cli.to_path_buf(),
                args: owned(args),
                env: env
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
            })
        }
    })
}

/// What a probe command printed.
pub(crate) struct Output {
    /// The exit code; `None` when killed.
    pub(crate) status: Option<i32>,
    /// Standard output, then standard error.
    pub(crate) text: String,
    pub(crate) timed_out: bool,
}

/// Run a probe with no input and a deadline, and collect what it prints.
pub(crate) fn run(program: &Path, args: &[&str], timeout: Duration) -> std::io::Result<Output> {
    run_in(program, args, &[], timeout)
}

/// [`run`] with `env` on top of this process's environment: an account's
/// home, for a command that answers about that account.
pub(crate) fn run_in(
    program: &Path,
    args: &[&str],
    env: &[(String, String)],
    timeout: Duration,
) -> std::io::Result<Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Plain text is all that is parsed.
        .env("NO_COLOR", "1")
        .envs(env.iter().map(|(name, value)| (name, value)))
        .spawn()?;
    // Readers run apart so a full pipe cannot stall the child, and report
    // over a channel so a grandchild holding the pipe open cannot stall us.
    let (sender, receiver) = mpsc::channel();
    let mut streams: Vec<(usize, Box<dyn std::io::Read + Send>)> = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        streams.push((0, Box::new(stdout)));
    }
    if let Some(stderr) = child.stderr.take() {
        streams.push((1, Box::new(stderr)));
    }
    let readers = streams.len();
    for (index, mut stream) in streams {
        let sender = sender.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            drop(stream.read_to_end(&mut bytes));
            drop(sender.send((index, bytes)));
        });
    }
    drop(sender);

    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status.code(), false);
        }
        if Instant::now() >= deadline {
            drop(child.kill());
            drop(child.wait());
            break (None, true);
        }
        thread::sleep(Duration::from_millis(5));
    };

    let mut parts = [Vec::new(), Vec::new()];
    for _ in 0..readers {
        match receiver.recv_timeout(Duration::from_millis(300)) {
            Ok((index, bytes)) => parts[index] = bytes,
            Err(_) => break,
        }
    }
    let mut text = String::from_utf8_lossy(&parts[0]).into_owned();
    text.push_str(&String::from_utf8_lossy(&parts[1]));
    Ok(Output {
        status,
        text,
        timed_out,
    })
}

/// The first thing in `text` that looks like a version: digits, a dot, digits,
/// and whatever is attached (`2.1.322`, `2026.11.05-7ae6800`).
pub(crate) fn parse_version(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        let attached = start > 0
            && (bytes[start - 1].is_ascii_alphanumeric()
                && !matches!(bytes[start - 1], b'v' | b'V'));
        if !bytes[start].is_ascii_digit() || attached {
            start += 1;
            continue;
        }
        let end = bytes[start..]
            .iter()
            .position(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+')))
            .map_or(bytes.len(), |length| start + length);
        let token = text[start..end].trim_end_matches(['.', '-', '+']);
        let mut parts = token.split('.');
        let numeric = |part: Option<&str>| {
            part.is_some_and(|part| part.starts_with(|first: char| first.is_ascii_digit()))
        };
        if numeric(parts.next()) && numeric(parts.next()) {
            return Some(token.to_owned());
        }
        start = end.max(start + 1);
    }
    None
}

/// Whether `version` is older than `minimum`, comparing the leading numbers
/// of each dotted part.
pub(crate) fn older(version: &str, minimum: &str) -> bool {
    let numbers = |text: &str| -> Vec<u64> {
        text.split(['.', '-', '+'])
            .map_while(|part| {
                let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            })
            .collect()
    };
    let (version, minimum) = (numbers(version), numbers(minimum));
    for index in 0..version.len().max(minimum.len()) {
        let ours = version.get(index).copied().unwrap_or(0);
        let theirs = minimum.get(index).copied().unwrap_or(0);
        if ours != theirs {
            return ours < theirs;
        }
    }
    false
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no output");
    line.chars().take(160).collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn versions_are_found_in_what_clis_print() {
        // Each line is what the CLI printed on the machine this was written on,
        // except Cursor's, which is the format its documentation shows.
        for (output, version) in [
            ("2.1.322 (Claude Code)", "2.1.322"),
            ("codex-cli 0.160.0", "0.160.0"),
            ("opencode v2.2.5", "2.2.5"),
            ("grok 1.0.70 (f50904eaee6c) [stable]", "1.0.70"),
            ("devin 3000.14.30821 (a09cd7f323ee)", "3000.14.30821"),
            ("aider 0.85.2", "0.85.2"),
            ("0.89.0", "0.89.0"),
            ("0.61.1\n", "0.61.1"),
            ("herdr 0.7.5", "0.7.5"),
            ("2026.11.05-7ae6800", "2026.11.05-7ae6800"),
            ("goose 1.59.0-beta.2", "1.59.0-beta.2"),
            ("Warning: node 22 is old.\nqwen 0.27.2", "0.27.2"),
        ] {
            assert_eq!(parse_version(output).as_deref(), Some(version), "{output}");
        }
        for output in ["", "no version here", "build f50904eaee6c", "port 8080"] {
            assert_eq!(parse_version(output), None, "{output}");
        }
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(older("1.0.84", "1.0.85"));
        assert!(older("1.9.0", "1.10.0"));
        assert!(older("1.14", "1.14.19"));
        assert!(older("0.61.1", "2.0.0"));
        assert!(!older("1.0.85", "1.0.85"));
        assert!(!older("2.1.322", "1.0.85"));
        assert!(!older("2.2.5-beta.1", "2.0.0"));
        assert!(!older("2026.11.05-7ae6800", "2.0.0"));
    }

    /// A directory of fake CLIs: shell scripts that answer the probes.
    #[cfg(unix)]
    pub(crate) struct Fakes {
        pub dir: tempfile::TempDir,
    }

    #[cfg(unix)]
    impl Fakes {
        pub(crate) fn new() -> Self {
            Fakes {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        /// Install `name` as a script with this body.
        pub(crate) fn script(&self, name: &str, body: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt as _;
            let path = self.dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        pub(crate) fn host(&self) -> Host {
            Host {
                path: Some(self.dir.path().as_os_str().to_owned()),
                shell: Some("/bin/sh".into()),
                kou_dirs: Vec::new(),
                version_timeout: Duration::from_secs(5),
                probe_timeout: Duration::from_secs(5),
                adapters: None,
            }
        }

        pub(crate) fn detect(&self, id: &str) -> Detected {
            detect_spec(spec(id).unwrap(), &self.host())
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_signed_in_cli_is_ready() {
        let fakes = Fakes::new();
        let path = fakes.script(
            "claude",
            r#"case "$*" in
  --version) echo "2.1.322 (Claude Code)" ;;
  "auth status") printf '{\n  "loggedIn": true,\n  "authMethod": "claude.ai"\n}\n' ;;
  *) exit 64 ;;
esac"#,
        );
        let detected = fakes.detect("claude");
        assert_eq!(detected.readiness, Readiness::Ready);
        assert_eq!(detected.path.as_deref(), Some(path.as_path()));
        assert_eq!(detected.version.as_deref(), Some("2.1.322"));
        assert_eq!(detected.message, None);
        assert!(detected.hooks);
        // No adapter on PATH: the terminal interface is the only way to run it.
        assert!(!detected.native && detected.acp.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_signed_out_cli_needs_login() {
        let fakes = Fakes::new();
        fakes.script(
            "claude",
            r#"case "$*" in
  --version) echo "2.1.322 (Claude Code)" ;;
  "auth status") printf '{\n  "loggedIn": false,\n  "authMethod": "none"\n}\n'; exit 1 ;;
esac"#,
        );
        let detected = fakes.detect("claude");
        assert_eq!(detected.readiness, Readiness::NeedsLogin);
        assert_eq!(
            detected.message.as_deref(),
            Some("Claude Code is not signed in. Run `claude auth login`.")
        );
        assert!(!detected.native);

        // Codex says it on stderr, in words.
        fakes.script(
            "codex",
            r#"case "$*" in
  --version) echo "codex-cli 0.160.0" ;;
  "login status") echo "Not logged in" >&2; exit 1 ;;
  "features list") echo "hooks    stable    true" ;;
esac"#,
        );
        assert_eq!(fakes.detect("codex").readiness, Readiness::NeedsLogin);

        // Devin says it on stdout and still exits 0.
        fakes.script(
            "devin",
            r#"case "$*" in
  --version) echo "devin 3000.14.30821 (a09cd7f323ee)" ;;
  "auth status") printf 'Not logged in.\n  Credentials path: /x/devin/credentials.toml\nRun `devin auth login` to authenticate.\n' ;;
esac"#,
        );
        let detected = fakes.detect("devin");
        assert_eq!(detected.readiness, Readiness::NeedsLogin);
        assert_eq!(
            detected.message.as_deref(),
            Some("Devin is not signed in. Run `devin auth login`.")
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_old_and_broken_clis_say_so() {
        let fakes = Fakes::new();
        let missing = fakes.detect("gemini");
        assert_eq!(missing.readiness, Readiness::Missing);
        assert_eq!(
            missing.message.as_deref(),
            Some("Not installed: `gemini` was not found on PATH.")
        );
        assert!(missing.path.is_none() && !missing.native && !missing.hooks);

        fakes.script("claude", r#"echo "1.0.40 (Claude Code)""#);
        let old = fakes.detect("claude");
        assert_eq!(
            old.readiness,
            Readiness::Outdated {
                minimum: "1.0.85".into()
            }
        );
        assert_eq!(old.version.as_deref(), Some("1.0.40"));
        assert!(old.message.unwrap().contains("older than 1.0.85"));

        fakes.script("droid", r#"echo "dyld: Library not loaded" >&2; exit 134"#);
        let broken = fakes.detect("droid");
        assert_eq!(
            broken.readiness,
            Readiness::Broken(
                "`droid --version` failed with status 134: dyld: Library not loaded".into()
            )
        );

        // What an uninstalled app leaves on PATH (kiro-cli, on this machine).
        std::os::unix::fs::symlink(
            "/nonexistent/Kiro CLI.app/kiro-cli",
            fakes.dir.path().join("kiro-cli"),
        )
        .unwrap();
        let dangling = fakes.detect("kiro");
        assert!(
            matches!(&dangling.readiness, Readiness::Broken(why) if why.contains("no longer exists")),
            "{:?}",
            dangling.readiness
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_cli_that_hangs_is_given_up_on() {
        let fakes = Fakes::new();
        fakes.script("pi", "sleep 30");
        let host = Host {
            version_timeout: Duration::from_millis(200),
            ..fakes.host()
        };
        let started = Instant::now();
        let detected = detect_spec(spec("pi").unwrap(), &host);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            matches!(&detected.readiness, Readiness::Broken(why) if why.contains("did not answer")),
            "{:?}",
            detected.readiness
        );
    }

    #[cfg(unix)]
    #[test]
    fn two_tools_with_one_name_are_told_apart() {
        // On the machine this was written on, `agent` is Grok Build, not Cursor.
        let fakes = Fakes::new();
        fakes.script(
            "agent",
            r#"case "$*" in
  --version) echo "grok 1.0.70 (f50904eaee6c) [stable]" ;;
  "agent --help") echo "  stdio     Run the agent over stdio" ;;
  status) echo "this must never run: it would be taken for a prompt" >&2; exit 99 ;;
esac"#,
        );
        assert_eq!(fakes.detect("cursor").readiness, Readiness::Missing);
        let grok = fakes.detect("grok");
        assert_eq!(grok.readiness, Readiness::Ready);
        assert_eq!(grok.version.as_deref(), Some("1.0.70"));
        assert_eq!(
            grok.acp.as_ref().map(|acp| acp.args.clone()),
            Some(vec!["agent".to_owned(), "stdio".to_owned()])
        );
        assert!(grok.native);

        // With Cursor's own binary beside it, each is found.
        fakes.script(
            "cursor-agent",
            r#"case "$*" in
  --version) echo "2026.11.05-7ae6800" ;;
  status) echo "Logged in as someone" ;;
esac"#,
        );
        let cursor = fakes.detect("cursor");
        assert_eq!(cursor.readiness, Readiness::Ready);
        assert_eq!(file_name(cursor.path.as_ref().unwrap()), "cursor-agent");
        assert_eq!(fakes.detect("grok").readiness, Readiness::Ready);
    }

    #[cfg(unix)]
    #[test]
    fn acp_is_offered_only_when_it_is_there() {
        let fakes = Fakes::new();
        // A help text that lists the subcommand, as OpenCode 2.2.5 prints it.
        fakes.script(
            "opencode",
            r#"case "$*" in
  --version) echo "opencode v2.2.5" ;;
  --help) echo "  acp                Start an Agent Client Protocol server" ;;
esac"#,
        );
        let opencode = fakes.detect("opencode");
        assert!(opencode.native && opencode.hooks);
        assert_eq!(opencode.acp.unwrap().args, ["acp"]);

        // An older Gemini CLI knows only the experimental flag.
        fakes.script(
            "gemini",
            r#"case "$*" in
  --version) echo "0.20.0" ;;
  --help) echo "      --experimental-acp  Starts the agent in ACP mode" ;;
esac"#,
        );
        assert_eq!(
            fakes.detect("gemini").acp.unwrap().args,
            ["--experimental-acp"]
        );

        // A version without the mode is not native.
        fakes.script(
            "goose",
            r#"case "$*" in
  --version) echo "goose 1.0.0" ;;
  --help) echo "  session   Start or resume a session" ;;
esac"#,
        );
        let goose = fakes.detect("goose");
        assert_eq!(goose.readiness, Readiness::Ready);
        assert!(!goose.native);

        // An adapter is found by its own name, with or without the CLI.
        let adapter = fakes.script("claude-agent-acp", "exit 0");
        let claude = fakes.detect("claude");
        assert_eq!(claude.readiness, Readiness::Missing);
        assert!(claude.native);
        assert_eq!(claude.acp.unwrap().program, adapter);
        assert!(claude.message.unwrap().contains("Only the ACP adapter"));
        // Aider has no ACP at all.
        fakes.script("aider", r#"echo "aider 0.85.2""#);
        assert!(!fakes.detect("aider").native);

        // A signed-in Devin offers its own `devin acp`.
        let path = fakes.script(
            "devin",
            r#"case "$*" in
  --version) echo "devin 3000.14.30821 (a09cd7f323ee)" ;;
  --help) echo "  acp        Run as an ACP (Agent Client Protocol) server over stdio" ;;
  "auth status") echo "Logged in (via Devin)." ;;
esac"#,
        );
        let devin = fakes.detect("devin");
        assert_eq!(devin.readiness, Readiness::Ready);
        assert!(devin.native);
        let acp = devin.acp.unwrap();
        assert_eq!(acp.args, ["acp"]);
        assert_eq!(acp.program, path);
        assert!(!devin.hooks);
    }

    /// An adapter installed the way the app installs one: npm's layout.
    #[cfg(unix)]
    fn npm_installed(folder: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let bin = folder.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = bin.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn signed_in_claude(fakes: &Fakes) -> PathBuf {
        fakes.script(
            "claude",
            r#"case "$*" in
  --version) echo "2.1.322 (Claude Code)" ;;
  "auth status") echo '{"loggedIn": true}' ;;
esac"#,
        )
    }

    #[cfg(unix)]
    #[test]
    fn the_apps_own_adapter_comes_before_one_on_path() {
        let fakes = Fakes::new();
        let cli = signed_in_claude(&fakes);
        let on_path = fakes.script("claude-agent-acp", "exit 0");
        let folder = tempfile::tempdir().unwrap();
        let ours = npm_installed(folder.path(), "claude-agent-acp");
        let host = Host {
            adapters: Some(folder.path().to_path_buf()),
            ..fakes.host()
        };
        let detected = detect_spec(spec("claude").unwrap(), &host);
        let acp = detected.acp.clone().unwrap();
        assert_eq!(acp.program, ours);
        // It is pointed at the CLI the terminal runs.
        assert_eq!(
            acp.env,
            [(
                "CLAUDE_CODE_EXECUTABLE".to_owned(),
                cli.to_string_lossy().into_owned()
            )]
        );
        assert_eq!(detected.chat, Chat::Ready);
        assert!(detected.native);

        // Without the app's copy, the one on PATH; under its old name too.
        assert_eq!(fakes.detect("claude").acp.unwrap().program, on_path);
        std::fs::remove_file(&on_path).unwrap();
        let old = fakes.script("claude-code-acp", "exit 0");
        assert_eq!(fakes.detect("claude").acp.unwrap().program, old);
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_adapter_says_how_the_app_would_install_it() {
        let fakes = Fakes::new();
        fakes.script(
            "codex",
            r#"case "$*" in
  --version) echo "codex-cli 0.160.0" ;;
  "login status") echo "Logged in using ChatGPT" >&2 ;;
  "features list") echo "hooks    stable    true" ;;
esac"#,
        );
        let folder = tempfile::tempdir().unwrap();
        let host = Host {
            adapters: Some(folder.path().to_path_buf()),
            ..fakes.host()
        };
        let message = "Codex's chat needs its ACP adapter `codex-acp`, which is not installed.";

        // No npm: said, and nothing to run.
        let detected = detect_spec(spec("codex").unwrap(), &host);
        assert_eq!(
            detected.chat,
            Chat::NeedsAdapter {
                message: format!("{message} Installing it needs npm, which is not on PATH."),
                install: None,
            }
        );
        assert!(!detected.native);

        // With npm: the exact command, into the app's folder only.
        let npm = fakes.script("npm", "exit 0");
        let detected = detect_spec(spec("codex").unwrap(), &host);
        let Chat::NeedsAdapter {
            message: said,
            install: Some(install),
        } = detected.chat
        else {
            panic!("{:?}", detected.chat);
        };
        assert_eq!(said, message);
        assert_eq!(install.program, npm);
        assert_eq!(install.cwd, folder.path());
        assert_eq!(install.writes, folder.path());
        assert_eq!(
            install.args.last().unwrap(),
            "@agentclientprotocol/codex-acp@2.6.0"
        );
        assert!(install.args.contains(&"--omit=optional".to_owned()));
        assert!(!install.installed());

        // Once it is there, detection finds it where the install put it.
        npm_installed(folder.path(), "codex-acp");
        assert!(install.installed());
        let detected = detect_spec(spec("codex").unwrap(), &host);
        assert_eq!(detected.chat, Chat::Ready);
        assert_eq!(detected.acp.unwrap().program, install.binary);

        // Without a folder there is nowhere to install to.
        let Chat::NeedsAdapter { install, .. } = fakes.detect("claude").chat else {
            panic!("an adapter is missing");
        };
        assert_eq!(install, None);
    }

    #[cfg(unix)]
    #[test]
    fn the_chat_says_what_stands_in_its_way() {
        let fakes = Fakes::new();
        // Signed out, with the adapter there: a login is what is missing.
        fakes.script(
            "claude",
            r#"case "$*" in
  --version) echo "2.1.322 (Claude Code)" ;;
  "auth status") echo '{"loggedIn": false}'; exit 1 ;;
esac"#,
        );
        fakes.script("claude-agent-acp", "exit 0");
        let detected = fakes.detect("claude");
        assert_eq!(
            detected.chat,
            Chat::NeedsLogin("Claude Code is not signed in. Run `claude auth login`.".into())
        );
        // As before, a signed-out harness is not offered as native.
        assert!(!detected.native && detected.acp.is_none());

        // A harness with its own mode that is not installed.
        assert_eq!(
            fakes.detect("gemini").chat,
            Chat::Unavailable("Not installed: `gemini` was not found on PATH.".into())
        );
        // Installed, but its help shows no mode.
        fakes.script(
            "goose",
            r#"case "$*" in
  --version) echo "goose 1.0.0" ;;
  --help) echo "  session   Start or resume a session" ;;
esac"#,
        );
        assert_eq!(
            fakes.detect("goose").chat,
            Chat::Unavailable("Goose 1.0.0 shows no ACP mode; a newer version has one.".into())
        );
        // No ACP at all.
        fakes.script("aider", r#"echo "aider 0.85.2""#);
        assert_eq!(fakes.detect("aider").chat, Chat::None);
        // A mode with environment of its own.
        fakes.script("droid", r#"echo "0.89.0""#);
        let droid = fakes.detect("droid");
        assert_eq!(droid.chat, Chat::Ready);
        assert!(
            droid
                .acp
                .unwrap()
                .env
                .contains(&("DROID_DISABLE_AUTO_UPDATE".to_owned(), "true".to_owned()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn hooks_depend_on_the_installed_version() {
        let fakes = Fakes::new();
        fakes.script(
            "codex",
            r#"case "$*" in
  --version) echo "codex-cli 0.40.0" ;;
  "login status") echo "Logged in using ChatGPT" >&2 ;;
  "features list") echo "unknown subcommand" >&2; exit 2 ;;
esac"#,
        );
        let old = fakes.detect("codex");
        assert_eq!(old.readiness, Readiness::Ready);
        assert!(!old.hooks, "no hooks feature, only notify");

        fakes.script(
            "codex",
            r#"case "$*" in
  --version) echo "codex-cli 0.160.0" ;;
  "login status") echo "Logged in using ChatGPT" >&2 ;;
  "features list") printf 'goals    stable    true\nhooks    stable    true\nmemories stable false\n' ;;
esac"#,
        );
        assert!(fakes.detect("codex").hooks);

        fakes.script("opencode", r#"echo "1.14.19""#);
        assert!(!fakes.detect("opencode").hooks);
    }

    #[cfg(unix)]
    #[test]
    fn kou_and_the_shell_are_found_without_a_version() {
        let fakes = Fakes::new();
        assert_eq!(fakes.detect("kou").readiness, Readiness::Missing);
        let shell = fakes.detect("shell");
        assert_eq!(shell.readiness, Readiness::Ready);
        assert_eq!(shell.path.as_deref(), Some(Path::new("/bin/sh")));

        // The repository's bin/ counts when PATH has nothing.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join("bin")).unwrap();
        let tui = repo.path().join("bin").join("kou-conveyor-tui");
        std::fs::write(&tui, "").unwrap();
        let host = Host {
            kou_dirs: vec![repo.path().join("bin")],
            ..fakes.host()
        };
        let kou = detect_spec(spec("kou").unwrap(), &host);
        assert_eq!(kou.readiness, Readiness::Ready);
        assert_eq!(kou.path.as_deref(), Some(tui.as_path()));
        assert_eq!(kou.version, None);
    }

    #[cfg(unix)]
    #[test]
    fn every_spec_is_reported_in_order() {
        let fakes = Fakes::new();
        fakes.script("pi", r#"echo "0.61.1""#);
        let all = detect_on(&fakes.host());
        let ids: Vec<_> = all.iter().map(|detected| detected.spec.id).collect();
        let expected: Vec<_> = specs().iter().map(|spec| spec.id).collect();
        assert_eq!(ids, expected);
        for detected in &all {
            let expected = match detected.spec.id {
                "pi" | "shell" => Readiness::Ready,
                _ => Readiness::Missing,
            };
            assert_eq!(detected.readiness, expected, "{}", detected.spec.id);
        }
    }
}
