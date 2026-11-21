//! The command line that starts a harness in a pane, with its hooks pointed
//! at the app for that launch only.
//!
//! Nothing here writes outside [`PaneContext::state_dir`]: hooks reach the
//! harness through its own flags, through environment variables, or through
//! a settings file generated there. The user's configuration is at most read,
//! to carry entries over that a generated file would otherwise replace.
//!
//! [`PaneContext::orchestration`], when set, attaches the app's MCP server
//! and guidance the same way, to this launch only: see [`crate::Attach`] for
//! the mechanism per harness, and [`acp_setup`] for the chat.

use crate::detect::{Detected, Readiness};
use crate::spec::{Acp, Arg, Attach, Login, Spec, Wiring};
use anyhow::{Context as _, bail};
use kou_ipc::env;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The pane a harness is started in.
#[derive(Clone, Debug)]
pub struct PaneContext {
    /// The pane's id.
    pub pane: String,
    /// The session the pane belongs to.
    pub session: String,
    /// The app's socket.
    pub socket: PathBuf,
    /// The `kou-desktop` binary, which hooks call.
    pub bin: PathBuf,
    /// The directory to start in.
    pub cwd: PathBuf,
    /// App-owned scratch for generated settings files. Files in it are shared
    /// by panes and named after their content, so one is never rewritten
    /// under a running harness.
    pub state_dir: PathBuf,
    /// What lets the agent drive the app, attached to this launch only.
    /// `None` attaches nothing.
    pub orchestration: Option<Orchestration>,
    /// What the harness's environment gets beside the pane's own: the home
    /// of another account of the same program (`CLAUDE_CONFIG_DIR`,
    /// `CODEX_HOME`), which moves its login, sessions and settings there.
    pub env: Vec<(String, String)>,
}

impl PaneContext {
    /// A pane with nothing attached for orchestration.
    pub fn new(
        pane: impl Into<String>,
        session: impl Into<String>,
        socket: impl Into<PathBuf>,
        bin: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
    ) -> Self {
        PaneContext {
            pane: pane.into(),
            session: session.into(),
            socket: socket.into(),
            bin: bin.into(),
            cwd: cwd.into(),
            state_dir: state_dir.into(),
            orchestration: None,
            env: Vec::new(),
        }
    }
}

/// What lets an agent started inside the app work with the others: guidance,
/// and in one mode an MCP server. With `mcp: None` no server is attached
/// anywhere (no `session/new` entry, no flag, no generated config) and the
/// guidance, which then speaks of the app's command line, attaches alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Orchestration {
    /// The MCP server, a stdio program (`<KOU_BIN> mcp`), or `None` for
    /// guidance only. The pane's four variables are added to its environment
    /// wherever the harness does not pass its own on, and wherever that is
    /// not known.
    pub mcp: Option<McpServer>,
    /// The guidance as a Claude Code plugin directory with a skill in it, for
    /// the harnesses that load such a plugin.
    pub claude_plugin: Option<PathBuf>,
    /// The same guidance as one Markdown file, for the others.
    pub instructions: Option<PathBuf>,
    /// A few lines on where the agent is (the app, its panes, the canvas,
    /// how to look), as a file: put into the context of a harness that is
    /// given the plugin, which otherwise learns nothing of the app until
    /// its skill's description happens to fit a task.
    pub place: Option<PathBuf>,
}

/// A stdio MCP server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServer {
    /// The name the agent knows it by; its tools are prefixed with it.
    pub name: String,
    /// The program.
    pub command: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Variables set for it.
    pub env: Vec<(String, String)>,
}

/// What a chat session is opened with, for `kou_acp::Options`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AcpSetup {
    /// MCP servers for `session/new`, and again for `session/load` or
    /// `session/resume`.
    pub mcp_servers: Vec<McpServer>,
    /// Guidance to add to the agent's instructions, sent only to agents that
    /// advertise `sessionCapabilities.systemPrompt`. `None` where the guidance
    /// goes another way (a plugin, the launch's environment) or there is none.
    pub instructions: Option<String>,
    /// `_meta` for opening the session.
    pub meta: Option<Map<String, Value>>,
    /// A few lines on where the agent is, for an agent that takes no added
    /// instructions (`kou_acp::Options::preface`): they go with its first
    /// prompt.
    pub preface: Option<String>,
    /// The harness's own login command, said when the agent needs a login.
    pub login_hint: Option<String>,
    /// How `session/fork` is told the message a fork ends at, for
    /// `kou_acp::Options::fork_meta`, which a chat needs to edit a prompt
    /// already sent: `_meta` with `"{messageId}"` (`kou_acp::FORK_MESSAGE_ID`)
    /// where the message's id goes. `None` for an agent not known to fork up
    /// to a message it is told of.
    pub fork_meta: Option<Map<String, Value>>,
    /// The agent is to be asked to show the commands it runs itself as
    /// terminals (`kou_acp::Options::display_terminals`): only for an
    /// adapter known to say then which commands it left running in the
    /// background, and when each of them ends.
    pub display_terminals: bool,
    /// The adapter keeps a turn open while what the agent started in the
    /// background runs, and takes the next prompt meanwhile
    /// (`kou_acp::Options::holds_turns`).
    pub holds_turns: bool,
}

/// The adapters that fork a session up to one of their messages when
/// `session/fork` names it in JetBrains AIR's `_meta.jetbrains.air.fork`
/// (version 1, the message's ACP `messageId`), by the binaries their
/// packages install: claude-agent-acp 0.106.0 (`dist/fork-session.js`) and
/// codex-acp 2.6.0 (`SessionFork.ts`), both read for it. Others fork whole
/// sessions or not at all: OpenCode 2.2.5 forks a whole session whatever
/// `_meta` says, so an edit there would run after everything.
const AIR_FORK_ADAPTERS: [&str; 2] = ["claude-agent-acp", "codex-acp"];

/// What to start a terminal interface with.
#[derive(Clone, Debug, Default)]
pub struct TuiOptions {
    /// A first prompt.
    pub prompt: Option<String>,
    /// A model, in the harness's own naming.
    pub model: Option<String>,
    /// A session to resume, by the harness's own id.
    pub resume: Option<String>,
    /// A session the harness runs in the background, to attach to in place
    /// of resuming one, by the name it attaches by (see
    /// [`crate::OpenSession::background`]). The session is as its own start
    /// made it: nothing else here applies to it.
    pub attach: Option<String>,
}

/// A process to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The executable.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Variables to add to the app's own environment.
    pub env: Vec<(String, String)>,
    /// The directory to start it in.
    pub cwd: PathBuf,
}

/// The command that runs a harness's terminal interface in a pane, with its
/// hooks reporting to the app.
///
/// Fails when the harness is not installed, is too old, or cannot take an
/// option that was asked for (a model, for a CLI with no model flag): running
/// with something other than what was asked would be worse than not running.
pub fn tui_launch(
    detected: &Detected,
    context: &PaneContext,
    options: &TuiOptions,
) -> anyhow::Result<Launch> {
    let theirs = match context.orchestration {
        Some(_) => Theirs::read(context),
        None => Theirs::default(),
    };
    tui_launch_with(detected, context, options, &theirs)
}

fn tui_launch_with(
    detected: &Detected,
    context: &PaneContext,
    options: &TuiOptions,
    theirs: &Theirs,
) -> anyhow::Result<Launch> {
    let spec = detected.spec;
    let program = match (&detected.readiness, &detected.path) {
        (Readiness::Outdated { .. } | Readiness::Broken(_), _) | (_, None) => bail!(
            "{}",
            detected
                .message
                .clone()
                .unwrap_or_else(|| format!("{} is not installed.", spec.name))
        ),
        // Signed out is no obstacle: its own interface is where one signs in.
        (_, Some(path)) => path.clone(),
    };

    let mut launch = Launch {
        program,
        args: spec.tui.args.iter().map(|arg| (*arg).to_owned()).collect(),
        env: pane_env(context),
        cwd: context.cwd.clone(),
    };
    if let Some(name) = &options.attach {
        // Claude Code's way (`claude attach <id|name>`); no other harness
        // runs sessions in the background.
        if spec.id != "claude" {
            bail!("{} has no background session to attach to.", spec.name);
        }
        launch.args.extend(["attach".to_owned(), name.clone()]);
        return Ok(launch);
    }
    for (name, value) in spec.tui.env {
        launch.env.push(((*name).to_owned(), (*value).to_owned()));
    }
    // Keys for the settings file a hook wiring may generate anyway.
    let mut settings = Map::new();
    let mut last = None;
    let mut place = |launch: &mut Launch, arg: Arg, value: Option<&str>, what: &str| {
        let Some(value) = value else {
            return Ok(());
        };
        match arg {
            Arg::No => bail!(
                "{} takes no {what} when it starts; set it inside the session.",
                spec.name
            ),
            Arg::Typed => {}
            Arg::Positional => last = Some(value.to_owned()),
            Arg::Words(words) => {
                launch
                    .args
                    .extend(words.iter().map(|word| (*word).to_owned()));
                launch.args.push(value.to_owned());
            }
            Arg::Joined(flag) => launch.args.push(format!("{flag}={value}")),
            Arg::Leading(words) => {
                let at = spec.tui.args.len();
                let leading = words.iter().map(|word| (*word).to_owned());
                launch
                    .args
                    .splice(at..at, leading.chain([value.to_owned()]));
            }
            Arg::Env(name) => launch.env.push((name.to_owned(), value.to_owned())),
            Arg::Setting(key) => {
                settings.insert(key.to_owned(), Value::String(value.to_owned()));
            }
        }
        Ok(())
    };
    let cwd = context.cwd.to_string_lossy();
    place(
        &mut launch,
        spec.tui.resume,
        options.resume.as_deref(),
        "session to resume",
    )?;
    place(
        &mut launch,
        spec.tui.model,
        options.model.as_deref(),
        "model",
    )?;
    // Most CLIs just run where they are started.
    if spec.tui.cwd != Arg::No {
        place(&mut launch, spec.tui.cwd, Some(&cwd), "directory")?;
    }
    let prompt = options
        .prompt
        .as_deref()
        .filter(|_| !prompt_is_typed(spec, options));
    place(&mut launch, spec.tui.prompt, prompt, "prompt")?;

    wire_hooks(detected, context, settings, &mut launch)?;
    if let Some(orchestration) = &context.orchestration {
        attach(detected, context, orchestration, theirs, &mut launch)?;
    }
    launch.args.extend(last);
    Ok(launch)
}

/// What of the user's own setup an orchestration launch carries over, or must
/// not hide. Read only when orchestration is attached.
#[derive(Debug, Default)]
struct Theirs {
    /// Their `OPENCODE_CONFIG_CONTENT`, which the generated one replaces.
    opencode_config: Option<String>,
    /// Their `COPILOT_CUSTOM_INSTRUCTIONS_DIRS`, which the generated one replaces.
    copilot_dirs: Option<String>,
    /// Whether their Codex config.toml sets `developer_instructions`, which an
    /// override would hide.
    codex_instructions: bool,
}

impl Theirs {
    fn read(context: &PaneContext) -> Self {
        let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Theirs {
            opencode_config: variable("OPENCODE_CONFIG_CONTENT"),
            copilot_dirs: variable("COPILOT_CUSTOM_INSTRUCTIONS_DIRS"),
            codex_instructions: codex_sets(context, "developer_instructions"),
        }
    }
}

/// The first prompt, when the harness cannot take it on its command line and
/// the app has to type it into the terminal once the harness is up.
///
/// That is every harness whose spec says [`Arg::Typed`], and any prompt that
/// starts with a dash, which a command line would read as an option.
pub fn typed_prompt(detected: &Detected, options: &TuiOptions) -> Option<String> {
    options
        .prompt
        .clone()
        .filter(|_| prompt_is_typed(detected.spec, options))
}

fn prompt_is_typed(spec: &Spec, options: &TuiOptions) -> bool {
    match (&options.prompt, spec.tui.prompt) {
        (None, _) => false,
        (Some(_), Arg::Typed) => true,
        (Some(prompt), Arg::Positional) => prompt.starts_with('-'),
        (Some(_), _) => false,
    }
}

/// The command that speaks the Agent Client Protocol on stdio for this
/// harness, when detection found one: the harness's own ACP mode, or an
/// adapter that is installed. Nothing is ever installed to make one.
///
/// With orchestration, what the harness takes at launch rather than in
/// `session/new` is added here: Grok Build's `--plugin-dir`, OpenCode's and
/// Copilot's guidance by environment. That part is best effort: a guidance
/// file that cannot be read or copied leaves it out rather than failing.
pub fn acp_launch(detected: &Detected, context: &PaneContext) -> Option<Launch> {
    let theirs = match context.orchestration {
        Some(_) => Theirs::read(context),
        None => Theirs::default(),
    };
    acp_launch_with(detected, context, &theirs)
}

fn acp_launch_with(detected: &Detected, context: &PaneContext, theirs: &Theirs) -> Option<Launch> {
    let acp = detected.acp.as_ref()?;
    let mut launch = Launch {
        program: acp.program.clone(),
        args: acp.args.clone(),
        env: pane_env(context),
        cwd: context.cwd.clone(),
    };
    launch.env.extend(acp.env.iter().cloned());
    if let Some(orchestration) = &context.orchestration {
        let dir = context.state_dir.join(detected.spec.id);
        let guidance = instructions_text(orchestration);
        match detected.spec.id {
            "grok" => {
                if let Some(plugin) = &orchestration.claude_plugin {
                    // `--plugin-dir` belongs to `grok agent`, before its subcommand.
                    let at = launch.args.len().saturating_sub(1);
                    launch
                        .args
                        .splice(at..at, ["--plugin-dir".to_owned(), text(plugin)]);
                }
            }
            "opencode" => {
                if let Some(file) = &orchestration.instructions {
                    let config = opencode_config(theirs, None, Some(file));
                    launch
                        .env
                        .push(("OPENCODE_CONFIG_CONTENT".to_owned(), config.to_string()));
                }
            }
            "copilot" => {
                if let Some(guidance) = &guidance
                    && let Ok(folder) = copilot_instructions(theirs, &dir, guidance)
                {
                    launch.env.push(folder);
                }
            }
            _ => {}
        }
    }
    Some(launch)
}

/// What to open a chat session with: the orchestration server, the guidance
/// where it goes into the session, the `_meta` an agent takes options in, the
/// harness's own login command, and how its adapter is told where a fork ends.
pub fn acp_setup(detected: &Detected, context: &PaneContext) -> AcpSetup {
    let spec = detected.spec;
    let mut setup = AcpSetup {
        login_hint: match &spec.login {
            Login::Status { hint, .. } => Some((*hint).to_owned()),
            Login::Unknown => None,
        },
        ..AcpSetup::default()
    };
    let forks_to_a_message = detected.acp.as_ref().is_some_and(|acp| {
        acp.program
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| AIR_FORK_ADAPTERS.contains(&stem))
    });
    if forks_to_a_message {
        let fork = json!({ "version": 1, "messageId": "{messageId}" });
        let mut meta = Map::new();
        meta.insert("jetbrains".to_owned(), json!({ "air": { "fork": fork } }));
        setup.fork_meta = Some(meta);
    }
    let adapter = spec
        .acp
        .iter()
        .any(|acp| matches!(acp, Acp::Adapter { .. }));
    // Claude's adapter takes the SDK's options under `claudeCode.options`.
    // Asked to, it passes on what the agents Claude Code starts itself say,
    // each piece marked with the call that started its agent: the chat shows
    // those agents at work. (Unasked, only their tool calls come.)
    let mut claude_options = Map::new();
    if spec.id == "claude" && adapter {
        claude_options.insert("forwardSubagentText".to_owned(), json!(true));
        setup.meta = Some(claude_meta(&claude_options));
        // It tells a client that shows its commands as terminals which of
        // them went on in the background and when they ended; another
        // client sees such a command as one that is over.
        setup.display_terminals = true;
        // It holds a turn open until the agents started in it are done
        // (`Turn.deferredSettle` in its source), hands that turn off to a
        // prompt sent meanwhile, and ends every one of them at a stop.
        setup.holds_turns = true;
    }
    let Some(orchestration) = &context.orchestration else {
        return setup;
    };
    // Whatever the harness, its agent is to know where it runs: one that
    // takes no instructions when its session opens is told with its first
    // prompt.
    setup.preface = place_text(orchestration);
    // The agent starts the server; how much of its own environment it passes
    // on differs, so the pane's variables are always spelled out.
    if let Some(server) = &orchestration.mcp {
        setup.mcp_servers.push(McpServer {
            env: server_env(server, context),
            ..server.clone()
        });
    }
    match (spec.id, &orchestration.claude_plugin) {
        // Claude's adapter loads plugins through the SDK option of that name.
        ("claude", Some(plugin)) if adapter => {
            claude_options.insert(
                "plugins".to_owned(),
                json!([{ "type": "local", "path": text(plugin) }]),
            );
            setup.meta = Some(claude_meta(&claude_options));
            // The skill is read when a task fits it; where the agent is,
            // it is told from the start (its adapter adds this to Claude
            // Code's own instructions).
            setup.instructions = place_text(orchestration);
        }
        // Loaded at launch by `acp_launch`: a skill, read when a task fits
        // it. Where the agent is, it is told from the start.
        ("grok", Some(_)) => setup.instructions = place_text(orchestration),
        // Given at launch by `acp_launch`.
        ("opencode" | "copilot", _) => {}
        _ => setup.instructions = instructions_text(orchestration),
    }
    setup
}

/// What Claude Code is started with when the app talks to it itself, over
/// its own stream of JSON lines, with no adapter between (`kou-claude`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaudeDirect {
    /// The `claude` executable.
    pub program: PathBuf,
    /// Variables to add to the app's own environment: the pane's, and its
    /// account's.
    pub env: Vec<(String, String)>,
    /// The account's home, when it is not Claude Code's own: where its
    /// records are.
    pub home: Option<PathBuf>,
    /// The orchestration server, when one is attached.
    pub mcp_servers: Vec<McpServer>,
    /// What is added to Claude Code's own instructions: where the agent is
    /// when the guidance comes as a plugin, else the guidance itself.
    pub instructions: Option<String>,
    /// The guidance as a plugin folder.
    pub plugins: Vec<PathBuf>,
}

/// Whether the app can hold a chat with this harness itself, with no
/// adapter: an installed, ready Claude Code or Codex CLI.
pub fn talks_directly(detected: &Detected) -> bool {
    matches!(detected.spec.id, "claude" | "codex")
        && detected.readiness == Readiness::Ready
        && detected.path.is_some()
}

/// How to start Claude Code for a chat the app holds with it directly, with
/// the same things attached as its terminal launch gets by flags
/// ([`Attach::ClaudeFlags`]). `None` for another harness, and for a Claude
/// Code that is not installed or cannot be started.
pub fn claude_direct(detected: &Detected, context: &PaneContext) -> Option<ClaudeDirect> {
    if detected.spec.id != "claude" || !talks_directly(detected) {
        return None;
    }
    let mut direct = ClaudeDirect {
        program: detected.path.clone()?,
        env: pane_env(context),
        home: context
            .env
            .iter()
            .find(|(name, _)| name == "CLAUDE_CONFIG_DIR")
            .map(|(_, home)| PathBuf::from(home)),
        ..ClaudeDirect::default()
    };
    let Some(orchestration) = &context.orchestration else {
        return Some(direct);
    };
    if let Some(server) = &orchestration.mcp {
        direct.mcp_servers.push(McpServer {
            env: server_env(server, context),
            ..server.clone()
        });
    }
    match &orchestration.claude_plugin {
        Some(plugin) => {
            direct.plugins.push(plugin.clone());
            direct.instructions = place_text(orchestration);
        }
        None => direct.instructions = instructions_text(orchestration),
    }
    Some(direct)
}

/// A private app server for this pane, with the same account and orchestration
/// as its Codex terminal. The chat gets its events over stdio.
pub fn codex_direct(detected: &Detected, context: &PaneContext) -> Option<Launch> {
    if detected.spec.id != "codex" || !talks_directly(detected) {
        return None;
    }
    let mut launch = Launch {
        program: detected.path.clone()?,
        args: vec!["app-server".into()],
        env: pane_env(context),
        cwd: context.cwd.clone(),
    };
    if let Some(orchestration) = &context.orchestration {
        let server = orchestration
            .mcp
            .as_ref()
            .map(|s| (s, server_env(s, context)));
        codex_config(
            &mut launch,
            server.as_ref(),
            instructions_text(orchestration),
            codex_sets(context, "developer_instructions"),
        );
    }
    Some(launch)
}

/// The `_meta` Claude's adapter reads the SDK's `options` from.
fn claude_meta(options: &Map<String, Value>) -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert("claudeCode".to_owned(), json!({ "options": options }));
    meta
}

/// The few lines on where the agent is, when there are any to read.
fn place_text(orchestration: &Orchestration) -> Option<String> {
    let file = orchestration.place.as_ref()?;
    std::fs::read_to_string(file)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// The guidance file's text, when there is one and it can be read.
fn instructions_text(orchestration: &Orchestration) -> Option<String> {
    let file = orchestration.instructions.as_ref()?;
    std::fs::read_to_string(file)
        .ok()
        .filter(|text| !text.trim().is_empty())
}

/// The server's own variables, then whichever of the pane's it does not set.
fn server_env(server: &McpServer, context: &PaneContext) -> Vec<(String, String)> {
    let mut env = server.env.clone();
    for (name, value) in pane_env(context) {
        if !env.iter().any(|(known, _)| *known == name) {
            env.push((name, value));
        }
    }
    env
}

fn env_object(env: &[(String, String)]) -> Value {
    Value::Object(
        env.iter()
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// Attach the orchestration server, when there is one, and the guidance to a
/// terminal launch. Neither depends on the other.
fn attach(
    detected: &Detected,
    context: &PaneContext,
    orchestration: &Orchestration,
    theirs: &Theirs,
    launch: &mut Launch,
) -> anyhow::Result<()> {
    let spec = detected.spec;
    let dir = context.state_dir.join(spec.id);
    let server = orchestration
        .mcp
        .as_ref()
        .map(|server| (server, server_env(server, context)));
    let guidance = instructions_text(orchestration);
    match spec.orchestration.attach {
        Attach::None => {}
        // Gemini CLI and Qwen Code: added to the system-defaults file in `wire_hooks`.
        Attach::SystemDefaults => {}
        Attach::ClaudeFlags => {
            if let Some((server, env)) = &server {
                let config = json!({ "mcpServers": { server.name.clone(): {
                    "type": "stdio",
                    "command": text(&server.command),
                    "args": server.args,
                    "env": env_object(env),
                } } });
                let file = write_named(&dir, "kou-mcp", "json", &pretty(&config))?;
                // Joined: `--mcp-config` takes several values, and a prompt after
                // a separate value would be read as one more.
                launch.args.push(format!("--mcp-config={}", text(&file)));
            }
            if let Some(plugin) = &orchestration.claude_plugin {
                launch
                    .args
                    .extend(["--plugin-dir".to_owned(), text(plugin)]);
                // The skill is read when a task fits it; where the agent
                // is, it is told from the start.
                if let Some(place) = place_text(orchestration) {
                    launch
                        .args
                        .extend(["--append-system-prompt".to_owned(), place]);
                }
            } else if let Some(guidance) = guidance {
                launch
                    .args
                    .extend(["--append-system-prompt".to_owned(), guidance]);
            }
        }
        Attach::CodexConfig => {
            codex_config(launch, server.as_ref(), guidance, theirs.codex_instructions)
        }
        Attach::OpencodeConfig => {
            let instructions = orchestration
                .instructions
                .as_deref()
                .filter(|_| guidance.is_some());
            if server.is_some() || instructions.is_some() {
                let config = opencode_config(
                    theirs,
                    server
                        .as_ref()
                        .map(|(server, env)| (*server, env.as_slice())),
                    instructions,
                );
                launch
                    .env
                    .push(("OPENCODE_CONFIG_CONTENT".to_owned(), config.to_string()));
                // A private server reads the variable; the shared background one never saw it.
                let two = detected
                    .version
                    .as_deref()
                    .is_some_and(|version| !crate::detect::older(version, "2.0.0"));
                if two && !launch.args.iter().any(|arg| arg == "--standalone") {
                    launch
                        .args
                        .insert(spec.tui.args.len(), "--standalone".to_owned());
                }
            }
        }
        Attach::CursorPlugin => {
            let mut files = Vec::new();
            if let Some((server, env)) = &server {
                files.push((
                    "mcp.json",
                    pretty(&json!({ "mcpServers": { server.name.clone(): {
                        "command": text(&server.command),
                        "args": server.args,
                        "env": env_object(env),
                    } } })),
                ));
            }
            if let Some(guidance) = guidance {
                files.push((
                    "rules/kou-orchestration.mdc",
                    format!(
                        "---\ndescription: How to work with the other panes of kou-desktop\nalwaysApply: true\n---\n\n{guidance}"
                    ),
                ));
            }
            if !files.is_empty() {
                files.push((
                    ".cursor-plugin/plugin.json",
                    pretty(&json!({
                        "name": "kou-orchestration",
                        "version": "1.0.0",
                        "description": "Lets this agent work with the other panes of kou-desktop.",
                    })),
                ));
                let plugin = write_tree(&dir, "kou-orchestration", &files)?;
                launch
                    .args
                    .extend(["--plugin-dir".to_owned(), text(&plugin)]);
            }
        }
        Attach::CopilotFlags => {
            if let Some((server, env)) = &server {
                let config = json!({ "mcpServers": { server.name.clone(): {
                    "type": "local",
                    "command": text(&server.command),
                    "args": server.args,
                    "env": env_object(env),
                    "tools": ["*"],
                } } });
                let file = write_named(&dir, "kou-mcp", "json", &pretty(&config))?;
                launch
                    .args
                    .push(format!("--additional-mcp-config=@{}", text(&file)));
            }
            if let Some(guidance) = guidance {
                launch
                    .env
                    .push(copilot_instructions(theirs, &dir, &guidance)?);
            }
        }
        Attach::GooseExtension => {
            if let Some((server, env)) = &server {
                // `[name:]VAR=value ... command args`, split by goose like a shell line.
                let mut words: Vec<String> = env
                    .iter()
                    .map(|(name, value)| shell_quote(&format!("{name}={value}")))
                    .collect();
                words.push(shell_quote(&text(&server.command)));
                words.extend(server.args.iter().map(|arg| shell_quote(arg)));
                launch.args.extend([
                    "--with-extension".to_owned(),
                    format!("{}:{}", server.name, words.join(" ")),
                ]);
            }
        }
        Attach::PiPrompt => {
            if let Some(file) = orchestration
                .instructions
                .as_ref()
                .filter(|_| guidance.is_some())
            {
                launch
                    .args
                    .extend(["--append-system-prompt".to_owned(), text(file)]);
            }
        }
        Attach::AiderRead => {
            if let Some(file) = orchestration
                .instructions
                .as_ref()
                .filter(|_| guidance.is_some())
            {
                launch.args.extend(["--read".to_owned(), text(file)]);
            }
        }
        Attach::GrokRules => {
            if let Some(guidance) = guidance {
                launch.args.extend(["--rules".to_owned(), guidance]);
            }
        }
    }
    Ok(())
}

/// How long Codex lets one call of the orchestration server run, in seconds.
const CODEX_TOOL_TIMEOUT: u32 = 3600;

/// OpenCode's inline configuration: the user's own `OPENCODE_CONFIG_CONTENT`,
/// if they set one, with the server and the guidance added. The server is in
/// the version 1 shape, which version 2 reads too.
fn opencode_config(
    theirs: &Theirs,
    server: Option<(&McpServer, &[(String, String)])>,
    instructions: Option<&Path>,
) -> Value {
    let mut config = theirs
        .opencode_config
        .as_deref()
        .and_then(|content| serde_json::from_str::<Value>(content).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some((server, env)) = server {
        if !config["mcp"].is_object() {
            config["mcp"] = json!({});
        }
        let mut command = vec![text(&server.command)];
        command.extend(server.args.iter().cloned());
        config["mcp"][server.name.as_str()] = json!({
            "type": "local",
            "command": command,
            "environment": env_object(env),
            "enabled": true,
        });
    }
    if let Some(file) = instructions {
        if !config["instructions"].is_array() {
            config["instructions"] = json!([]);
        }
        if let Some(list) = config["instructions"].as_array_mut() {
            list.push(Value::String(text(file)));
        }
    }
    config
}

/// A generated folder whose AGENTS.md holds the guidance, added to the
/// folders Copilot CLI reads instructions from, after the user's own.
fn copilot_instructions(
    theirs: &Theirs,
    dir: &Path,
    guidance: &str,
) -> anyhow::Result<(String, String)> {
    let folder = write_tree(
        dir,
        "kou-instructions",
        &[("AGENTS.md", guidance.to_owned())],
    )?;
    let value = match &theirs.copilot_dirs {
        Some(dirs) => format!("{dirs},{}", text(&folder)),
        None => text(&folder),
    };
    Ok(("COPILOT_CUSTOM_INSTRUCTIONS_DIRS".to_owned(), value))
}

/// Whether the user's Codex configuration sets this top-level key, which an
/// override would hide.
fn codex_sets(context: &PaneContext, key: &str) -> bool {
    let home = context
        .env
        .iter()
        .find(|(name, _)| name == "CODEX_HOME")
        .map(|(_, home)| std::ffi::OsString::from(home))
        .or_else(|| std::env::var_os("CODEX_HOME"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(dirs::home_dir()?.join(".codex")));
    let Some(config) = home.and_then(|home| std::fs::read_to_string(home.join("config.toml")).ok())
    else {
        return false;
    };
    config
        .lines()
        .take_while(|line| !line.trim_start().starts_with('['))
        .any(|line| {
            line.trim_start()
                .strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
}

fn codex_config(
    launch: &mut Launch,
    server: Option<&(&McpServer, Vec<(String, String)>)>,
    guidance: Option<String>,
    has_instructions: bool,
) {
    let mut settings = Vec::new();
    if let Some((server, env)) = server {
        let key = format!("mcp_servers.{}", toml_key(&server.name));
        let args: Vec<_> = server.args.iter().map(|arg| toml_string(arg)).collect();
        settings.push(format!(
            "{key}.command={}",
            toml_string(&text(&server.command))
        ));
        settings.push(format!("{key}.args=[{}]", args.join(", ")));
        // Pass the pane's environment by name. Account proxy passwords belong
        // in the environment, not in the program's command line.
        let (inherited, explicit): (Vec<_>, Vec<_>) =
            env.iter().partition(|pair| launch.env.contains(pair));
        let names: Vec<_> = inherited
            .iter()
            .map(|(name, _)| toml_string(name))
            .collect();
        settings.push(format!("{key}.env_vars=[{}]", names.join(", ")));
        if !explicit.is_empty() {
            let table: Vec<_> = explicit
                .iter()
                .map(|(name, value)| format!("{}={}", toml_key(name), toml_string(value)))
                .collect();
            settings.push(format!("{key}.env={{{}}}", table.join(", ")));
        }
        settings.push(format!("{key}.tool_timeout_sec={CODEX_TOOL_TIMEOUT}"));
    }
    if let Some(guidance) = guidance.filter(|_| !has_instructions) {
        settings.push(format!("developer_instructions={}", toml_string(&guidance)));
    }
    for value in settings {
        launch.args.extend(["-c".into(), value]);
    }
}

/// A TOML key: bare when it can be, quoted otherwise.
fn toml_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'));
    if bare {
        key.to_owned()
    } else {
        toml_string(key)
    }
}

/// The variables every pane's process gets, so the command line client and
/// the hooks know where they run.
fn pane_env(context: &PaneContext) -> Vec<(String, String)> {
    let path = |path: &Path| path.to_string_lossy().into_owned();
    let mut env = vec![
        (env::SOCKET.to_owned(), path(&context.socket)),
        (env::PANE.to_owned(), context.pane.clone()),
        (env::SESSION.to_owned(), context.session.clone()),
        (env::BIN.to_owned(), path(&context.bin)),
    ];
    // An account's home, after the pane's own: never one of the four above.
    env.extend(
        context
            .env
            .iter()
            .filter(|(name, _)| !name.starts_with("KOU_"))
            .cloned(),
    );
    env
}

/// The shell command a hook runs. The binary comes from the pane's
/// environment, so the text is the same for every pane and every launch; a
/// harness that asks the user to trust a hook asks once.
fn hook_command(harness: &str, event: &str) -> String {
    // A hook only tells the app what the harness is doing. If the app's
    // binary cannot be run (moved, being rebuilt), that is no business of
    // the harness or of the agent in it: the command still succeeds.
    format!("\"${}\" hook {harness} {event} || true", env::BIN)
}

fn wire_hooks(
    detected: &Detected,
    context: &PaneContext,
    mut settings: Map<String, Value>,
    launch: &mut Launch,
) -> anyhow::Result<()> {
    let spec = detected.spec;
    let dir = context.state_dir.join(spec.id);
    let version = detected.version.as_deref();
    match spec.hooks.wiring {
        Wiring::None => {}
        Wiring::ClaudeSettings => {
            settings.insert(
                "hooks".into(),
                claude_hooks(spec.id, &claude_events(version)),
            );
            let file = write_named(&dir, "settings", "json", &pretty(&Value::Object(settings)))?;
            launch.args.extend(["--settings".to_owned(), text(&file)]);
        }
        Wiring::DroidSettings => {
            settings.insert("hooks".into(), claude_hooks(spec.id, DROID_EVENTS));
            let file = write_named(&dir, "settings", "json", &pretty(&Value::Object(settings)))?;
            launch.args.extend(["--settings".to_owned(), text(&file)]);
        }
        Wiring::CodexConfig => {
            // The notify program is started directly, with no shell to expand
            // a variable, and is handed its payload as one more argument.
            let notify = format!(
                "notify=[{}, \"hook\", \"codex\", \"notify\"]",
                toml_string(&context.bin.to_string_lossy())
            );
            launch.args.extend(["-c".to_owned(), notify]);
            if detected.hooks {
                for event in codex_events(version) {
                    // A TOML literal string: the command holds double quotes.
                    let hook = format!(
                        "hooks.{event}=[{{hooks=[{{type=\"command\",command='{}',timeout=5}}]}}]",
                        hook_command(spec.id, event)
                    );
                    launch.args.extend(["-c".to_owned(), hook]);
                }
            }
        }
        Wiring::SystemDefaults {
            env: variable,
            mac_dir,
            etc_dir,
            gemini_events,
        } => {
            let events = if gemini_events {
                GEMINI_EVENTS
            } else {
                QWEN_EVENTS
            };
            let base = system_defaults(variable, mac_dir, etc_dir);
            let mut merged = merge_hooks(base, spec.id, events);
            if let Some(server) = context
                .orchestration
                .as_ref()
                .and_then(|orchestration| orchestration.mcp.as_ref())
                && spec.orchestration.attach == Attach::SystemDefaults
            {
                if !merged["mcpServers"].is_object() {
                    merged["mcpServers"] = json!({});
                }
                merged["mcpServers"][server.name.as_str()] = json!({
                    "command": text(&server.command),
                    "args": server.args,
                    "env": env_object(&server_env(server, context)),
                });
            }
            let file = write_named(&dir, "system-defaults", "json", &pretty(&merged))?;
            launch.env.push((variable.to_owned(), text(&file)));
        }
        Wiring::PiExtension => {
            let file = write_named(&dir, "kou-status", "ts", PI_EXTENSION)?;
            launch.args.extend(["--extension".to_owned(), text(&file)]);
        }
        Wiring::OpencodePlugin => {
            if detected.hooks {
                let plugin = write_tree(
                    &dir,
                    "kou-status",
                    &[("tui.ts", OPENCODE_PLUGIN.to_owned())],
                )?;
                // The inline list replaces the one in the user's cli.json, so theirs goes first.
                let mut plugins = opencode_cli_plugins();
                plugins.push(Value::String(text(&plugin)));
                launch.env.push((
                    "OPENCODE_CLI_CONFIG_CONTENT".to_owned(),
                    json!({ "plugins": plugins }).to_string(),
                ));
                // The shared background server streams every client's
                // sessions; a private one streams only this pane's.
                launch
                    .args
                    .insert(spec.tui.args.len(), "--standalone".to_owned());
            }
        }
        Wiring::CursorPlugin => {
            let hooks: Map<String, Value> = CURSOR_EVENTS
                .iter()
                .map(|event| {
                    let entry = json!([{ "command": hook_command(spec.id, event) }]);
                    ((*event).to_owned(), entry)
                })
                .collect();
            let plugin = write_tree(
                &dir,
                "kou-status",
                &[
                    (".cursor-plugin/plugin.json", pretty(&plugin_manifest())),
                    ("hooks/hooks.json", pretty(&json!({ "hooks": hooks }))),
                ],
            )?;
            launch
                .args
                .extend(["--plugin-dir".to_owned(), text(&plugin)]);
        }
        Wiring::CopilotPlugin => {
            let hooks: Map<String, Value> = COPILOT_EVENTS
                .iter()
                .map(|event| {
                    let entry = json!([{
                        "type": "command",
                        "bash": hook_command(spec.id, event),
                        "timeoutSec": 5,
                    }]);
                    ((*event).to_owned(), entry)
                })
                .collect();
            let plugin = write_tree(
                &dir,
                "kou-status",
                &[
                    ("plugin.json", pretty(&plugin_manifest())),
                    (
                        "hooks.json",
                        pretty(&json!({ "version": 1, "hooks": hooks })),
                    ),
                ],
            )?;
            launch.args.push(format!("--plugin-dir={}", text(&plugin)));
        }
        Wiring::AiderNotify => {
            // Aider hands the string to a shell, or splits it like one; a
            // quoted absolute path survives both.
            let command = format!(
                "{} hook aider notification",
                shell_quote(&context.bin.to_string_lossy())
            );
            launch.args.extend([
                "--notifications".to_owned(),
                "--notifications-command".to_owned(),
                command,
            ]);
        }
    }
    Ok(())
}

/// Claude Code's events, by the version each arrived in (its changelog), so
/// an older build is never handed a settings file it would reject.
fn claude_events(version: Option<&str>) -> Vec<&'static str> {
    let mut events = vec![
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Notification",
        "Stop",
        "SessionEnd",
    ];
    let has = |since: &str| version.is_some_and(|version| !crate::detect::older(version, since));
    // Tells of a permission question at once; the notification waits six seconds.
    if has("2.0.45") {
        events.push("PermissionRequest");
    }
    if has("2.1.78") {
        events.push("StopFailure");
    }
    // Present by 2.1.119 at the latest; the changelog does not say since when.
    if has("2.1.119") {
        events.push("PostToolUseFailure");
    }
    events
}

/// Droid's events (docs.factory.ai/reference/hooks-reference).
const DROID_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "SessionEnd",
];

/// Codex's events. The five oldest go to any version with the hooks feature.
/// `PermissionRequest` and `Interrupt` are offered from 0.160.0, the version
/// they were checked against: it accepts the overrides, and ignores an event
/// name it does not know, so a name the documentation lists ahead of a
/// release costs nothing there. Older versions were not tested with them.
fn codex_events(version: Option<&str>) -> Vec<&'static str> {
    let mut events = vec![
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
    ];
    if version.is_some_and(|version| !crate::detect::older(version, "0.160.0")) {
        events.extend(["PermissionRequest", "Interrupt"]);
    }
    events
}

/// Gemini CLI's events (docs/hooks/reference.md).
const GEMINI_EVENTS: &[&str] = &[
    "SessionStart",
    "BeforeAgent",
    "BeforeTool",
    "AfterTool",
    "AfterAgent",
    "Notification",
    "SessionEnd",
];

/// Qwen Code's events (docs/users/features/hooks.md).
const QWEN_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// Cursor's events that a plugin may hook (cursor.com/docs/reference/plugins).
const CURSOR_EVENTS: &[&str] = &[
    "sessionStart",
    "beforeSubmitPrompt",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "stop",
    "sessionEnd",
];

/// Copilot CLI's events, in the camelCase form.
const COPILOT_EVENTS: &[&str] = &[
    "sessionStart",
    "userPromptSubmitted",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "agentStop",
    "notification",
    "sessionEnd",
];

/// One matcher group with one command, in the shape Claude Code, Droid,
/// Gemini CLI and Qwen Code share. No matcher: every tool, every notification.
fn hook_group(harness: &str, event: &str) -> Value {
    json!({ "hooks": [{ "type": "command", "command": hook_command(harness, event) }] })
}

fn claude_hooks(harness: &str, events: &[&str]) -> Value {
    let hooks: Map<String, Value> = events
        .iter()
        .map(|event| ((*event).to_owned(), json!([hook_group(harness, event)])))
        .collect();
    Value::Object(hooks)
}

/// Add kou's hook groups to a settings object, keeping what is there.
fn merge_hooks(mut settings: Value, harness: &str, events: &[&str]) -> Value {
    if !settings.is_object() {
        settings = json!({});
    }
    let hooks = &mut settings["hooks"];
    if !hooks.is_object() {
        *hooks = json!({});
    }
    for event in events {
        let groups = &mut hooks[*event];
        if !groups.is_array() {
            *groups = json!([]);
        }
        if let Some(groups) = groups.as_array_mut() {
            groups.push(hook_group(harness, event));
        }
    }
    settings
}

/// The system-defaults file the harness would have read had the variable not
/// been pointed elsewhere: the one the user's own variable names, or the
/// platform's. Its content is carried into the generated file.
fn system_defaults(variable: &str, mac_dir: &str, etc_dir: &str) -> Value {
    let path = std::env::var_os(variable)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                Path::new("/Library/Application Support").join(mac_dir)
            } else if cfg!(windows) {
                Path::new("C:\\ProgramData").join(etc_dir)
            } else {
                Path::new("/etc").join(etc_dir)
            }
            .join("system-defaults.json")
        });
    read_json(&path).unwrap_or(Value::Null)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The plugins of the user's own OpenCode `cli.json`, with relative paths
/// made absolute, since they are about to be named from somewhere else.
fn opencode_cli_plugins() -> Vec<Value> {
    let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(dirs::home_dir()?.join(".config")))
        .map(|home| home.join("opencode"))
    else {
        return Vec::new();
    };
    let plugins = read_json(&config.join("cli.json"))
        .and_then(|cli| cli.get("plugins")?.as_array().cloned())
        .unwrap_or_default();
    let absolute = |entry: &str| {
        if entry.starts_with("./") || entry.starts_with("../") {
            text(&config.join(entry))
        } else {
            entry.to_owned()
        }
    };
    plugins
        .into_iter()
        .map(|plugin| match plugin {
            Value::String(entry) => Value::String(absolute(&entry)),
            Value::Object(mut entry) => {
                if let Some(Value::String(package)) = entry.get("package").cloned() {
                    entry.insert("package".into(), Value::String(absolute(&package)));
                }
                Value::Object(entry)
            }
            other => other,
        })
        .collect()
}

fn plugin_manifest() -> Value {
    json!({
        "name": "kou-status",
        "version": "1.0.0",
        "description": "Tells kou-desktop whether this agent is busy or free.",
    })
}

/// Forwards Pi's agent and tool events. Event names and fields are from
/// `docs/extensions.md` of the pi-coding-agent package; Pi loads TypeScript
/// as it is.
const PI_EXTENSION: &str = r#"// Generated by kou-desktop. Tells the app whether Pi is busy or free.
import { spawn } from "node:child_process";

export default function (pi: any) {
  const report = (event: string, data: Record<string, unknown> = {}) => {
    const bin = process.env.KOU_BIN;
    if (!bin || !process.env.KOU_PANE) return;
    try {
      const child = spawn(bin, ["hook", "pi", event], { stdio: ["pipe", "ignore", "ignore"] });
      child.on("error", () => {});
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(data));
    } catch {
      // The app is optional; Pi must not notice it is gone.
    }
  };
  pi.on("session_start", async () => report("session_start"));
  pi.on("agent_start", async () => report("agent_start"));
  pi.on("tool_execution_start", async (event: any) =>
    report("tool_execution_start", { toolName: event?.toolName }));
  pi.on("tool_execution_end", async (event: any) =>
    report("tool_execution_end", { toolName: event?.toolName, isError: event?.isError }));
  pi.on("agent_end", async () => report("agent_end"));
  pi.on("session_shutdown", async () => report("session_shutdown"));
}
"#;

/// Forwards OpenCode's session events from the terminal client, which runs in
/// the pane and so has its environment. API from
/// opencode.ai/docs/build/plugins/cli; event types from the 2.2.5 binary.
const OPENCODE_PLUGIN: &str = r#"// Generated by kou-desktop. Tells the app whether OpenCode is busy or free.
import { Plugin } from "@opencode/plugin/tui"
import { spawn } from "node:child_process"

const EVENTS = [
  "session.execution.started",
  "session.execution.succeeded",
  "session.execution.failed",
  "session.execution.interrupted",
  "permission.asked",
  "permission.replied",
  "form.created",
  "form.replied",
  "form.cancelled",
]

export default Plugin.define({
  id: "kou.status",
  setup(context: any) {
    const report = (type: string, event: any) => {
      const bin = process.env.KOU_BIN
      if (!bin || !process.env.KOU_PANE) return
      try {
        // A subagent's session ending is not the turn ending.
        const session = event?.data?.sessionID
        if (session && type.startsWith("session.execution.")) {
          const root = context.data?.session?.root?.(session)
          if (root && root !== session) return
        }
        const child = spawn(bin, ["hook", "opencode", type], { stdio: ["pipe", "ignore", "ignore"] })
        child.on("error", () => {})
        child.stdin.on("error", () => {})
        child.stdin.end(JSON.stringify(event?.data ?? {}))
      } catch {
        // The app is optional; OpenCode must not notice it is gone.
      }
    }
    const stops = EVENTS.map((type) => context.data.on(type, (event: any) => report(type, event)))
    return () => {
      for (const stop of stops) stop()
    }
  },
})
"#;

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    text
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A TOML basic string.
fn toml_string(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            control if control.is_control() => {
                quoted.push_str(&format!("\\u{:04X}", control as u32));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// One word for a POSIX shell.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// FNV-1a: enough to name a generated file after its content.
fn content_hash(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in parts.iter().flat_map(|part| part.bytes().chain([0])) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Write `content` under `dir` in a file named after it, and return the path.
/// A file that exists already has that content.
fn write_named(dir: &Path, stem: &str, extension: &str, content: &str) -> anyhow::Result<PathBuf> {
    let path = dir.join(format!(
        "{stem}-{}.{extension}",
        &content_hash(&[content])[..12]
    ));
    write_once(&path, content)?;
    Ok(path)
}

/// Write several files under a directory named after their content, and
/// return the directory.
fn write_tree(dir: &Path, stem: &str, files: &[(&str, String)]) -> anyhow::Result<PathBuf> {
    let parts: Vec<&str> = files
        .iter()
        .flat_map(|(name, content)| [*name, content.as_str()])
        .collect();
    let root = dir.join(format!("{stem}-{}", &content_hash(&parts)[..12]));
    for (name, content) in files {
        write_once(&root.join(name), content)?;
    }
    Ok(root)
}

fn write_once(path: &Path, content: &str) -> anyhow::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == content) {
        return Ok(());
    }
    let parent = path
        .parent()
        .context("a generated file needs a directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    // Whole or absent: a harness starting at this moment must not read half a file.
    let partial = path.with_extension(format!("{}.partial", std::process::id()));
    std::fs::write(&partial, content).with_context(|| format!("writing {}", partial.display()))?;
    std::fs::rename(&partial, path).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Chat;
    use crate::detect::AcpCommand;
    use crate::spec::spec;

    fn context(state: &Path) -> PaneContext {
        PaneContext::new(
            "p3",
            "s1",
            "/run/kou/kou.sock",
            "/Applications/kou desktop/kou-desktop",
            "/work/api",
            state,
        )
    }

    fn installed(id: &str, version: Option<&str>) -> Detected {
        Detected {
            spec: spec(id).unwrap(),
            path: Some(PathBuf::from(format!("/usr/local/bin/{id}"))),
            version: version.map(str::to_owned),
            readiness: Readiness::Ready,
            message: None,
            native: false,
            acp: None,
            hooks: true,
            chat: Chat::None,
        }
    }

    fn options(prompt: Option<&str>, model: Option<&str>, resume: Option<&str>) -> TuiOptions {
        TuiOptions {
            prompt: prompt.map(str::to_owned),
            model: model.map(str::to_owned),
            resume: resume.map(str::to_owned),
            attach: None,
        }
    }

    #[test]
    fn a_session_claude_runs_in_the_background_is_attached_to() {
        let state = tempfile::tempdir().unwrap();
        let attach = |id: &str| {
            let options = TuiOptions {
                attach: Some("c255120d".into()),
                ..options(Some("not for it"), Some("opus"), None)
            };
            tui_launch(&installed(id, None), &context(state.path()), &options)
        };
        // Nothing else goes with it: the session is as its own start made it.
        let launch = attach("claude").unwrap();
        assert_eq!(launch.args, ["attach", "c255120d"]);
        assert!(launch.env.iter().any(|(name, _)| name == "KOU_PANE"));
        // No other harness has such sessions.
        let error = attach("codex").unwrap_err().to_string();
        assert!(error.contains("no background session"), "{error}");
    }

    fn value_after<'a>(launch: &'a Launch, flag: &str) -> &'a str {
        let at = launch
            .args
            .iter()
            .position(|arg| arg == flag)
            .unwrap_or_else(|| panic!("{flag} is not in {:?}", launch.args));
        &launch.args[at + 1]
    }

    fn env_of<'a>(launch: &'a Launch, name: &str) -> Option<&'a str> {
        launch
            .env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn every_launch_carries_the_pane() {
        let state = tempfile::tempdir().unwrap();
        for spec in crate::specs() {
            let detected = installed(spec.id, Some("99.0.0"));
            let launch = tui_launch(&detected, &context(state.path()), &TuiOptions::default())
                .unwrap_or_else(|error| panic!("{}: {error}", spec.id));
            assert_eq!(
                env_of(&launch, "KOU_SOCKET"),
                Some("/run/kou/kou.sock"),
                "{}",
                spec.id
            );
            assert_eq!(env_of(&launch, "KOU_PANE"), Some("p3"));
            assert_eq!(env_of(&launch, "KOU_SESSION"), Some("s1"));
            assert_eq!(
                env_of(&launch, "KOU_BIN"),
                Some("/Applications/kou desktop/kou-desktop")
            );
            assert_eq!(launch.cwd, Path::new("/work/api"));
            assert_eq!(launch.program, detected.path.unwrap());
            // Nothing generated may land outside the app's scratch directory.
            for arg in &launch.args {
                assert!(
                    !arg.contains("/.claude/") && !arg.contains("/.codex/"),
                    "{arg}"
                );
            }
        }
    }

    #[test]
    fn claude_code_gets_a_settings_file_with_its_hooks() {
        let state = tempfile::tempdir().unwrap();
        let launch = tui_launch(
            &installed("claude", Some("2.1.322")),
            &context(state.path()),
            &options(Some("fix the build"), Some("opus"), Some("0199-abcd")),
        )
        .unwrap();
        let file = PathBuf::from(value_after(&launch, "--settings"));
        assert!(file.starts_with(state.path()));
        assert_eq!(
            launch.args,
            [
                "--resume",
                "0199-abcd",
                "--model",
                "opus",
                "--settings",
                file.to_str().unwrap(),
                "fix the build"
            ]
        );
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let hooks = settings["hooks"].as_object().unwrap();
        let mut events: Vec<_> = hooks.keys().map(String::as_str).collect();
        events.sort_unstable();
        assert_eq!(
            events,
            [
                "Notification",
                "PermissionRequest",
                "PostToolUse",
                "PostToolUseFailure",
                "PreToolUse",
                "SessionEnd",
                "SessionStart",
                "Stop",
                "StopFailure",
                "UserPromptSubmit"
            ]
        );
        assert_eq!(
            hooks["Stop"],
            json!([{ "hooks": [{ "type": "command", "command": "\"$KOU_BIN\" hook claude Stop || true" }] }])
        );
        // Only the hooks: nothing else of the user's settings is overridden.
        assert_eq!(settings.as_object().unwrap().len(), 1);

        // An older build is offered only the events it knows.
        let old = tui_launch(
            &installed("claude", Some("2.0.30")),
            &context(state.path()),
            &TuiOptions::default(),
        )
        .unwrap();
        let old_file = PathBuf::from(value_after(&old, "--settings"));
        assert_ne!(old_file, file, "a different content is a different file");
        let old: Value = serde_json::from_str(&std::fs::read_to_string(old_file).unwrap()).unwrap();
        assert_eq!(old["hooks"].as_object().unwrap().len(), 7);
        assert!(old["hooks"].get("PermissionRequest").is_none());

        // The same launch again reuses the file.
        let again = tui_launch(
            &installed("claude", Some("2.1.322")),
            &context(state.path()),
            &TuiOptions::default(),
        )
        .unwrap();
        assert_eq!(value_after(&again, "--settings"), file.to_str().unwrap());
    }

    #[test]
    fn codex_gets_notify_and_hooks_as_config_overrides() {
        let state = tempfile::tempdir().unwrap();
        let launch = tui_launch(
            &installed("codex", Some("0.160.0")),
            &context(state.path()),
            &options(Some("review the diff"), Some("gpt-5.2-codex"), None),
        )
        .unwrap();
        let overrides: Vec<&str> = launch
            .args
            .iter()
            .zip(launch.args.iter().skip(1))
            .filter(|(flag, _)| *flag == "-c")
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(
            overrides[0],
            r#"notify=["/Applications/kou desktop/kou-desktop", "hook", "codex", "notify"]"#
        );
        assert_eq!(
            overrides[1],
            r#"hooks.SessionStart=[{hooks=[{type="command",command='"$KOU_BIN" hook codex SessionStart || true',timeout=5}]}]"#
        );
        assert_eq!(overrides.len(), 8, "{overrides:?}");
        assert!(
            overrides
                .iter()
                .any(|value| value.starts_with("hooks.PermissionRequest="))
        );
        assert_eq!(&launch.args[..2], ["--model", "gpt-5.2-codex"]);
        assert_eq!(launch.args.last().unwrap(), "review the diff");
        // Trust is the user's to give: the bypass flag is never passed.
        assert!(!launch.args.iter().any(|arg| arg.contains("bypass")));

        // Without the hooks feature only notify is wired.
        let plain = Detected {
            hooks: false,
            ..installed("codex", Some("0.40.0"))
        };
        let launch = tui_launch(&plain, &context(state.path()), &TuiOptions::default()).unwrap();
        assert_eq!(launch.args.len(), 2);
        assert!(launch.args[1].starts_with("notify="));

        // Resuming is a subcommand, and it comes first.
        let launch = tui_launch(
            &plain,
            &context(state.path()),
            &options(Some("carry on"), None, Some("019a-7f")),
        )
        .unwrap();
        assert_eq!(&launch.args[..2], ["resume", "019a-7f"]);
        assert_eq!(launch.args.last().unwrap(), "carry on");
    }

    #[test]
    fn generated_settings_keep_what_the_real_file_holds() {
        let existing = json!({
            "ui": { "theme": "dark" },
            "hooks": { "BeforeTool": [{ "matcher": "write_file", "hooks": [{ "type": "command", "command": "/usr/bin/audit" }] }] }
        });
        let merged = merge_hooks(existing, "gemini", GEMINI_EVENTS);
        assert_eq!(merged["ui"]["theme"], "dark");
        let before_tool = merged["hooks"]["BeforeTool"].as_array().unwrap();
        assert_eq!(before_tool.len(), 2);
        assert_eq!(before_tool[0]["hooks"][0]["command"], "/usr/bin/audit");
        assert_eq!(
            before_tool[1]["hooks"][0]["command"],
            "\"$KOU_BIN\" hook gemini BeforeTool || true"
        );
        assert_eq!(merged["hooks"]["AfterAgent"].as_array().unwrap().len(), 1);
        // A file that is not an object is not a settings file.
        let fresh = merge_hooks(json!("oops"), "qwen", QWEN_EVENTS);
        assert_eq!(fresh["hooks"].as_object().unwrap().len(), QWEN_EVENTS.len());

        let state = tempfile::tempdir().unwrap();
        let launch = tui_launch(
            &installed("gemini", Some("0.68.0")),
            &context(state.path()),
            &options(Some("explain this"), Some("gemini-3-pro"), None),
        )
        .unwrap();
        let file = env_of(&launch, "GEMINI_CLI_SYSTEM_DEFAULTS_PATH").unwrap();
        assert!(Path::new(file).starts_with(state.path()));
        assert_eq!(
            launch.args,
            [
                "--model",
                "gemini-3-pro",
                "--prompt-interactive",
                "explain this"
            ]
        );
    }

    #[test]
    fn each_wiring_lands_where_its_harness_looks() {
        let state = tempfile::tempdir().unwrap();
        let context = context(state.path());
        let launch = |id: &str, options: &TuiOptions| {
            tui_launch(&installed(id, Some("9.9.9")), &context, options).unwrap()
        };

        let droid = launch("droid", &options(Some("add tests"), Some("gpt-5.2"), None));
        let settings = read_json(Path::new(value_after(&droid, "--settings"))).unwrap();
        assert_eq!(settings["model"], "gpt-5.2");
        assert_eq!(
            settings["hooks"]["Notification"][0]["hooks"][0]["command"],
            "\"$KOU_BIN\" hook droid Notification || true"
        );
        assert_eq!(droid.args.last().unwrap(), "add tests");

        let pi = launch("pi", &options(Some("hello"), Some("sonnet:high"), None));
        let extension = std::fs::read_to_string(value_after(&pi, "--extension")).unwrap();
        assert!(extension.contains(r#"pi.on("agent_end""#));
        assert_eq!(&pi.args[..2], ["--model", "sonnet:high"]);
        assert_eq!(pi.args.last().unwrap(), "hello");

        let opencode = launch("opencode", &options(Some("hi"), None, Some("ses_1")));
        assert_eq!(opencode.args[0], "--standalone");
        assert_eq!(value_after(&opencode, "--session"), "ses_1");
        assert_eq!(value_after(&opencode, "--prompt"), "hi");
        assert_eq!(opencode.args.last().unwrap(), "/work/api");
        let inline: Value =
            serde_json::from_str(env_of(&opencode, "OPENCODE_CLI_CONFIG_CONTENT").unwrap())
                .unwrap();
        let plugin = inline["plugins"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .as_str()
            .unwrap();
        assert!(Path::new(plugin).join("tui.ts").is_file());

        let cursor = launch("cursor", &options(Some("go"), None, None));
        let plugin = Path::new(value_after(&cursor, "--plugin-dir"));
        assert!(plugin.join(".cursor-plugin/plugin.json").is_file());
        let hooks = read_json(&plugin.join("hooks/hooks.json")).unwrap();
        assert_eq!(
            hooks["hooks"]["stop"][0]["command"],
            "\"$KOU_BIN\" hook cursor stop || true"
        );
        assert_eq!(value_after(&cursor, "--workspace"), "/work/api");

        let copilot = launch("copilot", &options(Some("go"), Some("auto"), None));
        let plugin = copilot
            .args
            .iter()
            .find_map(|arg| arg.strip_prefix("--plugin-dir="))
            .unwrap();
        let hooks = read_json(&Path::new(plugin).join("hooks.json")).unwrap();
        assert_eq!(hooks["version"], 1);
        assert_eq!(
            hooks["hooks"]["agentStop"][0]["bash"],
            "\"$KOU_BIN\" hook copilot agentStop || true"
        );
        assert!(copilot.args.contains(&"--model=auto".to_owned()));
        assert!(copilot.args.contains(&"--interactive=go".to_owned()));

        let aider = launch("aider", &options(None, Some("sonnet"), None));
        assert_eq!(
            value_after(&aider, "--notifications-command"),
            "'/Applications/kou desktop/kou-desktop' hook aider notification"
        );
        assert!(aider.args.contains(&"--notifications".to_owned()));

        let goose = launch("goose", &options(None, Some("gpt-5"), Some("20251108_2")));
        assert_eq!(
            goose.args,
            ["session", "--resume", "--session-id", "20251108_2"]
        );
        assert_eq!(env_of(&goose, "GOOSE_MODEL"), Some("gpt-5"));

        let kiro = launch("kiro", &options(Some("why is it slow"), None, Some("abc")));
        assert_eq!(kiro.args, ["chat", "--resume-id", "abc", "why is it slow"]);

        let kou = launch("kou", &options(Some("plan it"), Some("m"), None));
        assert_eq!(
            kou.args,
            ["-model", "m", "-workspace", "/work/api", "-p", "plan it"]
        );

        let devin = launch(
            "devin",
            &options(Some("-v means verbose?"), Some("opus"), Some("rare-margin")),
        );
        assert_eq!(
            devin.args,
            [
                "--resume",
                "rare-margin",
                "--model",
                "opus",
                "--",
                "-v means verbose?"
            ]
        );
        assert!(!state.path().join("devin").exists());
        // The prompt goes after `--` on the command line, never typed in.
        assert_eq!(
            typed_prompt(
                &installed("devin", Some("3000.14.30821")),
                &options(Some("-v means verbose?"), None, None)
            ),
            None
        );

        assert_eq!(
            env_of(&launch("amp", &TuiOptions::default()), "AMP_FORCE_BEL"),
            Some("1")
        );
        assert!(launch("shell", &TuiOptions::default()).args.is_empty());
    }

    #[test]
    fn a_prompt_the_command_line_cannot_carry_is_typed() {
        let state = tempfile::tempdir().unwrap();
        let context = context(state.path());
        // Aider would answer once and exit; the prompt is left for the app.
        let aider = installed("aider", Some("0.85.2"));
        let asked = options(Some("add a test"), None, None);
        let launch = tui_launch(&aider, &context, &asked).unwrap();
        assert!(!launch.args.iter().any(|arg| arg == "add a test"));
        assert_eq!(typed_prompt(&aider, &asked).as_deref(), Some("add a test"));
        // A harness that takes it needs no typing.
        let claude = installed("claude", Some("2.1.322"));
        assert_eq!(typed_prompt(&claude, &asked), None);
        // Except when the prompt would be read as an option.
        let dashed = options(Some("--help me understand this"), None, None);
        let launch = tui_launch(&claude, &context, &dashed).unwrap();
        assert!(!launch.args.iter().any(|arg| arg.starts_with("--help")));
        assert_eq!(
            typed_prompt(&claude, &dashed).as_deref(),
            Some("--help me understand this")
        );
        assert_eq!(typed_prompt(&claude, &TuiOptions::default()), None);
    }

    #[test]
    fn what_cannot_be_honoured_is_refused() {
        let state = tempfile::tempdir().unwrap();
        let context = context(state.path());
        let refused = |detected: &Detected, options: &TuiOptions| {
            tui_launch(detected, &context, options)
                .unwrap_err()
                .to_string()
        };
        // OpenCode 2's full-screen interface has no model flag.
        let error = refused(
            &installed("opencode", Some("2.2.5")),
            &options(None, Some("x/y"), None),
        );
        assert!(error.contains("takes no model"), "{error}");
        let error = refused(&installed("aider", None), &options(None, None, Some("id")));
        assert!(error.contains("takes no session to resume"), "{error}");

        let missing = Detected {
            path: None,
            readiness: Readiness::Missing,
            message: Some("Not installed: `gemini` was not found on PATH.".into()),
            ..installed("gemini", None)
        };
        assert!(refused(&missing, &TuiOptions::default()).contains("Not installed"));
        let outdated = Detected {
            readiness: Readiness::Outdated {
                minimum: "1.0.85".into(),
            },
            message: Some("Claude Code 1.0.40 is older than 1.0.85.".into()),
            ..installed("claude", Some("1.0.40"))
        };
        assert!(refused(&outdated, &TuiOptions::default()).contains("older than"));
        // Signed out still launches: the interface is where one signs in.
        let signed_out = Detected {
            readiness: Readiness::NeedsLogin,
            ..installed("claude", Some("2.1.322"))
        };
        assert!(tui_launch(&signed_out, &context, &TuiOptions::default()).is_ok());
    }

    #[test]
    fn acp_launches_what_detection_found() {
        let state = tempfile::tempdir().unwrap();
        let context = context(state.path());
        assert_eq!(acp_launch(&installed("aider", None), &context), None);
        let opencode = Detected {
            native: true,
            acp: Some(AcpCommand {
                program: "/usr/local/bin/opencode".into(),
                args: vec!["acp".into()],
                env: Vec::new(),
            }),
            ..installed("opencode", Some("2.2.5"))
        };
        let launch = acp_launch(&opencode, &context).unwrap();
        assert_eq!(launch.program, Path::new("/usr/local/bin/opencode"));
        assert_eq!(launch.args, ["acp"]);
        assert_eq!(launch.cwd, Path::new("/work/api"));
        assert_eq!(env_of(&launch, "KOU_PANE"), Some("p3"));
    }

    /// The app's orchestration as `kou-desktop` will attach it: its MCP
    /// server, a plugin directory and a guidance file in `folder`.
    fn orchestrated(state: &Path, folder: &Path, plugin: bool) -> PaneContext {
        let guidance = folder.join("guidance.md");
        std::fs::write(&guidance, "Use kou to start agents in other panes.\n").unwrap();
        PaneContext {
            orchestration: Some(Orchestration {
                mcp: Some(McpServer {
                    name: "kou".into(),
                    command: "/Applications/kou desktop/kou-desktop".into(),
                    args: vec!["mcp".into()],
                    env: Vec::new(),
                }),
                claude_plugin: plugin.then(|| folder.join("kou-plugin")),
                instructions: Some(guidance),
                place: None,
            }),
            ..context(state)
        }
    }

    #[test]
    fn a_harness_given_the_skill_is_also_told_where_it_is() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let mut context = orchestrated(state.path(), folder.path(), true);
        let place = folder.path().join("place.md");
        std::fs::write(&place, "You run in a pane of kou desktop.\n").unwrap();
        if let Some(orchestration) = &mut context.orchestration {
            orchestration.place = Some(place);
        }
        // Claude Code in its terminal: the plugin, and the few lines with it.
        let launch = launch_with(
            &installed("claude", Some("2.1.322")),
            &context,
            &options(Some("fix the build"), None, None),
            &Theirs::default(),
        );
        let plugin = folder.path().join("kou-plugin");
        let at = launch
            .args
            .iter()
            .position(|arg| arg == "--plugin-dir")
            .expect("the plugin");
        assert_eq!(
            launch.args[at..],
            [
                "--plugin-dir",
                plugin.to_str().unwrap(),
                "--append-system-prompt",
                "You run in a pane of kou desktop.",
                "fix the build",
            ]
        );
        // As a chat: added to its instructions when the session opens.
        let claude = Detected {
            native: true,
            acp: Some(AcpCommand {
                program: "/x/claude-agent-acp".into(),
                args: Vec::new(),
                env: Vec::new(),
            }),
            ..installed("claude", Some("2.1.322"))
        };
        let setup = acp_setup(&claude, &context);
        assert_eq!(
            setup.instructions.as_deref(),
            Some("You run in a pane of kou desktop.")
        );
        // Any harness's chat has the lines at hand: an agent that takes no
        // instructions when its session opens is told with its first prompt.
        assert_eq!(setup.preface, setup.instructions);
        for (id, program) in [
            ("droid", "/x/droid"),
            ("codex", "/x/codex-acp"),
            ("grok", "/x/grok"),
            ("devin", "/x/devin"),
        ] {
            let other = Detected {
                native: true,
                acp: Some(AcpCommand {
                    program: program.into(),
                    args: Vec::new(),
                    env: Vec::new(),
                }),
                ..installed(id, Some("9.9.9"))
            };
            let setup = acp_setup(&other, &context);
            assert_eq!(
                setup.preface.as_deref(),
                Some("You run in a pane of kou desktop."),
                "{id}"
            );
            // Grok Build is given the skill, and so the lines; the others
            // the whole guidance.
            assert_eq!(
                setup.instructions.as_deref(),
                Some(if id == "grok" {
                    "You run in a pane of kou desktop."
                } else {
                    "Use kou to start agents in other panes.\n"
                }),
                "{id}"
            );
        }
        // Without the plugin a harness has the whole guidance, which says
        // where it is itself: nothing is added twice.
        let mut plain = orchestrated(state.path(), folder.path(), false);
        if let Some(orchestration) = &mut plain.orchestration {
            orchestration.place = Some(folder.path().join("place.md"));
        }
        let launch = launch_with(
            &installed("claude", Some("2.1.322")),
            &plain,
            &options(None, None, None),
            &Theirs::default(),
        );
        assert_eq!(
            value_after(&launch, "--append-system-prompt"),
            "Use kou to start agents in other panes.\n"
        );
    }

    /// The pane's variables as an MCP server is given them.
    fn pane_vars() -> Value {
        json!({
            "KOU_SOCKET": "/run/kou/kou.sock",
            "KOU_PANE": "p3",
            "KOU_SESSION": "s1",
            "KOU_BIN": "/Applications/kou desktop/kou-desktop",
        })
    }

    fn launch_with(
        detected: &Detected,
        context: &PaneContext,
        options: &TuiOptions,
        theirs: &Theirs,
    ) -> Launch {
        tui_launch_with(detected, context, options, theirs)
            .unwrap_or_else(|error| panic!("{}: {error}", detected.spec.id))
    }

    #[test]
    fn without_orchestration_nothing_of_it_is_attached() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let plain = context(state.path());
        let with = orchestrated(state.path(), folder.path(), true);
        for spec in crate::specs() {
            let detected = installed(spec.id, Some("99.0.0"));
            let bare = launch_with(
                &detected,
                &plain,
                &TuiOptions::default(),
                &Theirs::default(),
            );
            for arg in &bare.args {
                for marker in [
                    "kou-mcp",
                    "kou-orchestration",
                    "mcp_servers",
                    "developer_instructions",
                    "--rules",
                    "--read",
                    "--append-system-prompt",
                    "--with-extension",
                    "--additional-mcp-config",
                ] {
                    assert!(!arg.contains(marker), "{}: {arg}", spec.id);
                }
            }
            for name in [
                "OPENCODE_CONFIG_CONTENT",
                "COPILOT_CUSTOM_INSTRUCTIONS_DIRS",
            ] {
                assert_eq!(env_of(&bare, name), None, "{}", spec.id);
            }
            // A harness with no launch-scoped way gets exactly the plain launch.
            if spec.orchestration.attach == Attach::None {
                let attached =
                    launch_with(&detected, &with, &TuiOptions::default(), &Theirs::default());
                assert_eq!(attached, bare, "{}", spec.id);
            }
        }
    }

    #[test]
    fn claude_code_gets_its_server_by_flag_and_its_skill_by_plugin() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let context = orchestrated(state.path(), folder.path(), true);
        let launch = launch_with(
            &installed("claude", Some("2.1.322")),
            &context,
            &options(Some("fix the build"), None, None),
            &Theirs::default(),
        );
        let settings = value_after(&launch, "--settings").to_owned();
        let mcp = launch.args[2]
            .strip_prefix("--mcp-config=")
            .unwrap()
            .to_owned();
        assert!(Path::new(&mcp).starts_with(state.path().join("claude")));
        let plugin = folder.path().join("kou-plugin");
        assert_eq!(
            launch.args,
            [
                "--settings",
                settings.as_str(),
                &format!("--mcp-config={mcp}"),
                "--plugin-dir",
                plugin.to_str().unwrap(),
                "fix the build",
            ]
        );
        assert_eq!(
            read_json(Path::new(&mcp)).unwrap(),
            json!({ "mcpServers": { "kou": {
                "type": "stdio",
                "command": "/Applications/kou desktop/kou-desktop",
                "args": ["mcp"],
                "env": pane_vars(),
            } } })
        );
        // The hooks file is untouched by it.
        let hooks = read_json(Path::new(&settings)).unwrap();
        assert_eq!(hooks.as_object().unwrap().len(), 1);

        // Without a plugin, the guidance file's text is added to the prompt.
        let context = orchestrated(state.path(), folder.path(), false);
        let launch = launch_with(
            &installed("claude", Some("2.1.322")),
            &context,
            &TuiOptions::default(),
            &Theirs::default(),
        );
        assert_eq!(
            value_after(&launch, "--append-system-prompt"),
            "Use kou to start agents in other panes.\n"
        );
        assert!(!launch.args.iter().any(|arg| arg == "--plugin-dir"));
    }

    #[test]
    fn codex_gets_its_server_and_guidance_as_overrides() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let context = orchestrated(state.path(), folder.path(), true);
        let detected = Detected {
            hooks: false,
            ..installed("codex", Some("0.160.0"))
        };
        let launch = launch_with(
            &detected,
            &context,
            &options(Some("review"), None, None),
            &Theirs::default(),
        );
        assert_eq!(
            launch.args,
            [
                "-c",
                r#"notify=["/Applications/kou desktop/kou-desktop", "hook", "codex", "notify"]"#,
                "-c",
                r#"mcp_servers.kou.command="/Applications/kou desktop/kou-desktop""#,
                "-c",
                r#"mcp_servers.kou.args=["mcp"]"#,
                "-c",
                r#"mcp_servers.kou.env_vars=["KOU_SOCKET", "KOU_PANE", "KOU_SESSION", "KOU_BIN"]"#,
                "-c",
                "mcp_servers.kou.tool_timeout_sec=3600",
                "-c",
                r#"developer_instructions="Use kou to start agents in other panes.\u000A""#,
                "review",
            ]
        );
        // The user's own developer instructions are not hidden.
        let theirs = Theirs {
            codex_instructions: true,
            ..Theirs::default()
        };
        let launch = launch_with(&detected, &context, &TuiOptions::default(), &theirs);
        assert!(
            !launch
                .args
                .iter()
                .any(|arg| arg.starts_with("developer_instructions"))
        );
        assert_eq!(toml_key("odd name"), r#""odd name""#);
    }

    #[test]
    fn the_direct_codex_server_uses_the_panes_account_and_mcp_environment() {
        let folder = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let mut context = orchestrated(folder.path(), folder.path(), false);
        let profile_path = profile.path().to_string_lossy().into_owned();
        context.env = vec![
            ("CODEX_HOME".into(), profile_path.clone()),
            ("HTTPS_PROXY".into(), "http://proxy.invalid:8181".into()),
        ];
        let detected = installed("codex", None);
        let launch = codex_direct(&detected, &context).unwrap();
        assert_eq!(launch.program, detected.path.clone().unwrap());
        assert_eq!(launch.cwd, context.cwd);
        assert_eq!(env_of(&launch, "CODEX_HOME"), Some(profile_path.as_str()));
        assert_eq!(
            env_of(&launch, "HTTPS_PROXY"),
            Some("http://proxy.invalid:8181")
        );
        assert_eq!(launch.args[0], "app-server");
        let args = launch.args[1..].join(" ");
        assert!(args.contains("mcp_servers.kou.env_vars=") && args.contains("HTTPS_PROXY"));
        assert!(!args.contains("proxy.invalid") && !args.contains("notify="));
        assert!(args.contains("developer_instructions="));

        std::fs::write(
            profile.path().join("config.toml"),
            "developer_instructions = 'Account guidance'",
        )
        .unwrap();
        let launch = codex_direct(&detected, &context).unwrap();
        assert!(
            !launch
                .args
                .iter()
                .any(|arg| arg.starts_with("developer_instructions="))
        );
        assert!(codex_direct(&installed("claude", None), &context).is_none());
        let missing = Detected {
            path: None,
            ..detected
        };
        assert!(codex_direct(&missing, &context).is_none());
    }

    #[test]
    fn opencode_gets_an_inline_config_and_a_private_server() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let context = orchestrated(state.path(), folder.path(), true);
        let guidance = folder.path().join("guidance.md");
        let theirs = Theirs {
            opencode_config: Some(r#"{"model":"anthropic/claude-sonnet-5"}"#.into()),
            ..Theirs::default()
        };
        // With hooks, `--standalone` is there once.
        let launch = launch_with(
            &installed("opencode", Some("2.2.5")),
            &context,
            &TuiOptions::default(),
            &theirs,
        );
        assert_eq!(launch.args, ["--standalone", "/work/api"]);
        let config: Value =
            serde_json::from_str(env_of(&launch, "OPENCODE_CONFIG_CONTENT").unwrap()).unwrap();
        assert_eq!(
            config,
            json!({
                "model": "anthropic/claude-sonnet-5",
                "mcp": { "kou": {
                    "type": "local",
                    "command": ["/Applications/kou desktop/kou-desktop", "mcp"],
                    "environment": pane_vars(),
                    "enabled": true,
                } },
                "instructions": [guidance.to_str().unwrap()],
            })
        );
        // Without hooks it still needs its private server.
        let quiet = Detected {
            hooks: false,
            ..installed("opencode", Some("2.2.5"))
        };
        let launch = launch_with(&quiet, &context, &TuiOptions::default(), &Theirs::default());
        assert_eq!(launch.args, ["--standalone", "/work/api"]);
        // OpenCode 1 has no background service and no such flag.
        let one = Detected {
            hooks: false,
            ..installed("opencode", Some("1.14.19"))
        };
        let launch = launch_with(&one, &context, &TuiOptions::default(), &Theirs::default());
        assert_eq!(launch.args, ["/work/api"]);
        assert!(env_of(&launch, "OPENCODE_CONFIG_CONTENT").is_some());
    }

    #[test]
    fn gemini_and_qwen_get_the_server_in_their_system_defaults() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let context = orchestrated(state.path(), folder.path(), true);
        for (id, variable) in [
            ("gemini", "GEMINI_CLI_SYSTEM_DEFAULTS_PATH"),
            ("qwen", "QWEN_CODE_SYSTEM_DEFAULTS_PATH"),
        ] {
            let launch = launch_with(
                &installed(id, Some("0.68.0")),
                &context,
                &TuiOptions::default(),
                &Theirs::default(),
            );
            assert!(launch.args.is_empty(), "{id}: {:?}", launch.args);
            let file = read_json(Path::new(env_of(&launch, variable).unwrap())).unwrap();
            assert_eq!(
                file["mcpServers"]["kou"],
                json!({
                    "command": "/Applications/kou desktop/kou-desktop",
                    "args": ["mcp"],
                    "env": pane_vars(),
                })
            );
            // The hooks are in the same file.
            assert!(file["hooks"]["SessionStart"].is_array(), "{id}");
        }
    }

    #[test]
    fn cursor_and_copilot_get_generated_plugins_and_files() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let context = orchestrated(state.path(), folder.path(), true);

        let cursor = launch_with(
            &installed("cursor", Some("2026.11.05")),
            &context,
            &options(Some("go"), None, None),
            &Theirs::default(),
        );
        let dirs: Vec<&String> = cursor
            .args
            .iter()
            .zip(cursor.args.iter().skip(1))
            .filter(|(flag, _)| *flag == "--plugin-dir")
            .map(|(_, dir)| dir)
            .collect();
        assert_eq!(dirs.len(), 2, "hooks and orchestration: {:?}", cursor.args);
        let plugin = Path::new(dirs[1]);
        assert!(plugin.starts_with(state.path().join("cursor")));
        let name = plugin.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("kou-orchestration-"), "{name}");
        assert_eq!(
            read_json(&plugin.join("mcp.json")).unwrap(),
            json!({ "mcpServers": { "kou": {
                "command": "/Applications/kou desktop/kou-desktop",
                "args": ["mcp"],
                "env": pane_vars(),
            } } })
        );
        let rule = std::fs::read_to_string(plugin.join("rules/kou-orchestration.mdc")).unwrap();
        assert!(rule.starts_with("---\ndescription: "), "{rule}");
        assert!(rule.contains("alwaysApply: true\n---\n\nUse kou"), "{rule}");
        assert!(plugin.join(".cursor-plugin/plugin.json").is_file());
        assert_eq!(cursor.args.last().unwrap(), "go");
        assert!(!cursor.args.iter().any(|arg| arg == "--approve-mcps"));

        let theirs = Theirs {
            copilot_dirs: Some("/Users/someone/notes".into()),
            ..Theirs::default()
        };
        let copilot = launch_with(
            &installed("copilot", Some("1.0.109")),
            &context,
            &TuiOptions::default(),
            &theirs,
        );
        let file = copilot
            .args
            .iter()
            .find_map(|arg| arg.strip_prefix("--additional-mcp-config=@"))
            .unwrap();
        assert_eq!(
            read_json(Path::new(file)).unwrap(),
            json!({ "mcpServers": { "kou": {
                "type": "local",
                "command": "/Applications/kou desktop/kou-desktop",
                "args": ["mcp"],
                "env": pane_vars(),
                "tools": ["*"],
            } } })
        );
        let dirs = env_of(&copilot, "COPILOT_CUSTOM_INSTRUCTIONS_DIRS").unwrap();
        let (mine, ours) = dirs.split_once(',').unwrap();
        assert_eq!(mine, "/Users/someone/notes");
        assert_eq!(
            std::fs::read_to_string(Path::new(ours).join("AGENTS.md")).unwrap(),
            "Use kou to start agents in other panes.\n"
        );
    }

    #[test]
    fn harnesses_without_mcp_get_the_guidance_alone() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let context = orchestrated(state.path(), folder.path(), true);
        let guidance = folder.path().join("guidance.md");
        let guidance = guidance.to_str().unwrap();
        let launch = |id: &str, options: &TuiOptions| {
            launch_with(
                &installed(id, Some("9.9.9")),
                &context,
                options,
                &Theirs::default(),
            )
        };

        let pi = launch("pi", &options(Some("hello"), None, None));
        let extension = value_after(&pi, "--extension").to_owned();
        assert_eq!(
            pi.args,
            [
                "--extension",
                extension.as_str(),
                "--append-system-prompt",
                guidance,
                "hello"
            ]
        );

        let aider = launch("aider", &TuiOptions::default());
        assert_eq!(&aider.args[3..], ["--read", guidance]);

        let grok = launch("grok", &options(Some("hi"), None, None));
        assert_eq!(
            grok.args,
            [
                "--cwd",
                "/work/api",
                "--rules",
                "Use kou to start agents in other panes.\n",
                "hi"
            ]
        );

        let goose = launch("goose", &TuiOptions::default());
        assert_eq!(
            goose.args,
            [
                "session",
                "--with-extension",
                "kou:'KOU_SOCKET=/run/kou/kou.sock' 'KOU_PANE=p3' 'KOU_SESSION=s1' \
                 'KOU_BIN=/Applications/kou desktop/kou-desktop' \
                 '/Applications/kou desktop/kou-desktop' 'mcp'"
            ]
        );
    }

    #[test]
    fn guidance_alone_attaches_without_any_server() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let mut context = orchestrated(state.path(), folder.path(), true);
        if let Some(orchestration) = &mut context.orchestration {
            orchestration.mcp = None;
        }
        let guidance = folder.path().join("guidance.md");
        let text = "Use kou to start agents in other panes.\n";
        let launch = |id: &str| {
            launch_with(
                &installed(id, Some("9.9.9")),
                &context,
                &TuiOptions::default(),
                &Theirs::default(),
            )
        };
        for spec in crate::specs() {
            let launch = launch(spec.id);
            for arg in &launch.args {
                for marker in ["mcp", "--with-extension"] {
                    assert!(!arg.contains(marker), "{}: {arg}", spec.id);
                }
            }
            if let Some(config) = env_of(&launch, "OPENCODE_CONFIG_CONTENT") {
                assert!(!config.contains("\"mcp\""), "{config}");
            }
        }
        // What reaches the model instead.
        let plugin = folder.path().join("kou-plugin");
        assert_eq!(
            launch("claude").args[2..],
            ["--plugin-dir", plugin.to_str().unwrap()]
        );
        let codex = launch("codex");
        assert!(
            codex
                .args
                .iter()
                .any(|arg| arg.starts_with("developer_instructions="))
        );
        let opencode = launch("opencode");
        assert_eq!(opencode.args, ["--standalone", "/work/api"]);
        let config: Value =
            serde_json::from_str(env_of(&opencode, "OPENCODE_CONFIG_CONTENT").unwrap()).unwrap();
        assert_eq!(
            config,
            json!({ "instructions": [guidance.to_str().unwrap()] })
        );
        let cursor = launch("cursor");
        let plugin = Path::new(cursor.args.last().unwrap());
        assert!(plugin.join("rules/kou-orchestration.mdc").is_file());
        assert!(!plugin.join("mcp.json").exists());
        assert!(env_of(&launch("copilot"), "COPILOT_CUSTOM_INSTRUCTIONS_DIRS").is_some());
        assert_eq!(value_after(&launch("grok"), "--rules"), text);
        assert_eq!(
            value_after(&launch("pi"), "--append-system-prompt"),
            guidance.to_str().unwrap()
        );
        assert_eq!(
            value_after(&launch("aider"), "--read"),
            guidance.to_str().unwrap()
        );
        // Gemini CLI's system defaults get no server.
        let gemini = launch("gemini");
        let file = read_json(Path::new(
            env_of(&gemini, "GEMINI_CLI_SYSTEM_DEFAULTS_PATH").unwrap(),
        ))
        .unwrap();
        assert!(file.get("mcpServers").is_none());

        // Over ACP: no server, and the guidance where the agent takes it.
        let claude = Detected {
            native: true,
            acp: Some(AcpCommand {
                program: "/x/claude-agent-acp".into(),
                args: Vec::new(),
                env: Vec::new(),
            }),
            ..installed("claude", Some("2.1.322"))
        };
        let setup = acp_setup(&claude, &context);
        assert!(setup.mcp_servers.is_empty());
        assert!(setup.meta.is_some());
        let codex = Detected {
            native: true,
            acp: Some(AcpCommand {
                program: "/x/codex-acp".into(),
                args: Vec::new(),
                env: Vec::new(),
            }),
            ..installed("codex", Some("0.160.0"))
        };
        let setup = acp_setup(&codex, &context);
        assert!(setup.mcp_servers.is_empty());
        assert_eq!(setup.instructions.as_deref(), Some(text));
    }

    #[test]
    fn a_chat_is_opened_with_the_server_and_what_the_harness_takes_at_launch() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let context = orchestrated(state.path(), folder.path(), true);
        let guidance = folder.path().join("guidance.md");
        let server = McpServer {
            name: "kou".into(),
            command: "/Applications/kou desktop/kou-desktop".into(),
            args: vec!["mcp".into()],
            env: vec![
                ("KOU_SOCKET".into(), "/run/kou/kou.sock".into()),
                ("KOU_PANE".into(), "p3".into()),
                ("KOU_SESSION".into(), "s1".into()),
                (
                    "KOU_BIN".into(),
                    "/Applications/kou desktop/kou-desktop".into(),
                ),
            ],
        };
        let with_acp = |id: &str, program: &str, args: &[&str], env: &[(&str, &str)]| Detected {
            native: true,
            acp: Some(AcpCommand {
                program: program.into(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                env: env
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
            }),
            ..installed(id, Some("9.9.9"))
        };

        // Claude: the server, the plugin through `_meta`, its CLI by variable.
        let claude = with_acp(
            "claude",
            "/apps/kou/adapters/node_modules/.bin/claude-agent-acp",
            &[],
            &[("CLAUDE_CODE_EXECUTABLE", "/usr/local/bin/claude")],
        );
        let setup = acp_setup(&claude, &context);
        assert_eq!(setup.mcp_servers, std::slice::from_ref(&server));
        assert_eq!(setup.instructions, None);
        assert_eq!(
            Value::Object(setup.meta.unwrap()),
            json!({ "claudeCode": { "options": {
                "forwardSubagentText": true,
                "plugins": [{
                    "type": "local",
                    "path": folder.path().join("kou-plugin").to_str().unwrap(),
                }],
            } } })
        );
        assert_eq!(
            setup.login_hint.as_deref(),
            Some("Run `claude auth login`.")
        );
        let launch = acp_launch_with(&claude, &context, &Theirs::default()).unwrap();
        assert_eq!(
            env_of(&launch, "CLAUDE_CODE_EXECUTABLE"),
            Some("/usr/local/bin/claude")
        );
        assert!(launch.args.is_empty());

        // Codex: the guidance as added instructions, which its adapter takes.
        let codex = with_acp("codex", "/x/codex-acp", &[], &[]);
        let setup = acp_setup(&codex, &context);
        assert_eq!(
            setup.instructions.as_deref(),
            Some("Use kou to start agents in other panes.\n")
        );
        assert_eq!(setup.meta, None);
        assert_eq!(setup.login_hint.as_deref(), Some("Run `codex login`."));

        // Grok Build loads the plugin at launch, before its subcommand.
        let grok = with_acp("grok", "/x/grok", &["agent", "stdio"], &[]);
        let launch = acp_launch_with(&grok, &context, &Theirs::default()).unwrap();
        assert_eq!(
            launch.args,
            [
                "agent",
                "--plugin-dir",
                folder.path().join("kou-plugin").to_str().unwrap(),
                "stdio"
            ]
        );
        assert_eq!(acp_setup(&grok, &context).instructions, None);

        // OpenCode takes the guidance by its inline config, Copilot by folder.
        let opencode = with_acp("opencode", "/x/opencode", &["acp"], &[]);
        let launch = acp_launch_with(&opencode, &context, &Theirs::default()).unwrap();
        let config: Value =
            serde_json::from_str(env_of(&launch, "OPENCODE_CONFIG_CONTENT").unwrap()).unwrap();
        assert_eq!(
            config,
            json!({ "instructions": [guidance.to_str().unwrap()] })
        );
        assert_eq!(acp_setup(&opencode, &context).instructions, None);
        let copilot = with_acp("copilot", "/x/copilot", &["--acp"], &[]);
        let launch = acp_launch_with(&copilot, &context, &Theirs::default()).unwrap();
        assert!(env_of(&launch, "COPILOT_CUSTOM_INSTRUCTIONS_DIRS").is_some());

        // Devin takes nothing at launch: the server goes in `session/new`,
        // and the guidance with the first prompt.
        let devin = with_acp("devin", "/x/devin", &["acp"], &[]);
        let launch = acp_launch_with(&devin, &context, &Theirs::default()).unwrap();
        assert_eq!(launch.program, Path::new("/x/devin"));
        assert_eq!(launch.args, ["acp"]);
        assert_eq!(launch.env, pane_env(&context));
        let setup = acp_setup(&devin, &context);
        assert_eq!(setup.mcp_servers, std::slice::from_ref(&server));
        assert_eq!(setup.login_hint.as_deref(), Some("Run `devin auth login`."));
        assert_eq!((setup.fork_meta, setup.meta), (None, None));
        assert!(!setup.display_terminals && !setup.holds_turns);

        // Droid's mode runs with the variables its registry entry sets.
        let droid = with_acp(
            "droid",
            "/x/droid",
            &["exec", "--output-format", "acp-daemon"],
            &[("DROID_DISABLE_AUTO_UPDATE", "true")],
        );
        let launch = acp_launch_with(&droid, &context, &Theirs::default()).unwrap();
        assert_eq!(env_of(&launch, "DROID_DISABLE_AUTO_UPDATE"), Some("true"));
        assert_eq!(acp_setup(&droid, &context).mcp_servers, [server]);

        // Without orchestration, a chat gets the login hint and the way its
        // adapter is told where a fork ends; and Claude's adapter is still
        // asked for what the agents Claude Code starts itself say.
        let plain = acp_setup(&claude, &self::context(state.path()));
        let Value::Object(words) =
            json!({ "claudeCode": { "options": { "forwardSubagentText": true } } })
        else {
            unreachable!("an object");
        };
        assert_eq!(
            plain,
            AcpSetup {
                login_hint: Some("Run `claude auth login`.".into()),
                fork_meta: acp_setup(&claude, &context).fork_meta,
                meta: Some(words),
                display_terminals: true,
                holds_turns: true,
                ..AcpSetup::default()
            }
        );
        // Another harness's adapter is asked nothing of the kind.
        let other = acp_setup(&codex, &self::context(state.path()));
        assert_eq!((other.meta, other.display_terminals), (None, false));
    }

    #[test]
    fn the_adapters_that_fork_up_to_a_message_are_told_which_the_way_they_read_it() {
        let state = tempfile::tempdir().unwrap();
        let context = context(state.path());
        let with_acp = |id: &str, program: &str| Detected {
            native: true,
            acp: Some(AcpCommand {
                program: program.into(),
                args: Vec::new(),
                env: Vec::new(),
            }),
            ..installed(id, Some("9.9.9"))
        };
        let air = json!({ "jetbrains": { "air": { "fork": {
            "version": 1,
            "messageId": "{messageId}",
        } } } });
        for (id, program) in [
            (
                "claude",
                "/apps/kou/adapters/node_modules/.bin/claude-agent-acp",
            ),
            // npm's shim on Windows.
            (
                "claude",
                "/kou/adapters/node_modules/.bin/claude-agent-acp.cmd",
            ),
            ("codex", "/x/codex-acp"),
        ] {
            let setup = acp_setup(&with_acp(id, program), &context);
            assert_eq!(
                setup.fork_meta.map(Value::Object),
                Some(air.clone()),
                "{program}"
            );
        }
        // The package Claude's adapter was before its rename, and agents that
        // fork whole sessions or not at all, are not.
        for (id, program) in [
            ("claude", "/x/claude-code-acp"),
            ("opencode", "/x/opencode"),
            ("gemini", "/x/gemini"),
        ] {
            let setup = acp_setup(&with_acp(id, program), &context);
            assert_eq!(setup.fork_meta, None, "{program}");
        }
        assert_eq!(
            acp_setup(&installed("claude", None), &context).fork_meta,
            None
        );
    }

    #[test]
    fn claude_code_talked_to_directly_gets_what_its_terminal_gets() {
        let state = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let claude = installed("claude", Some("2.1.325"));

        // Nothing attached: the program, the pane, and no home of its own.
        let plain = claude_direct(&claude, &context(state.path())).unwrap();
        assert_eq!(plain.program, Path::new("/usr/local/bin/claude"));
        assert!(plain.env.contains(&("KOU_PANE".into(), "p3".into())));
        assert_eq!(plain.home, None);
        assert!(plain.mcp_servers.is_empty() && plain.plugins.is_empty());
        assert_eq!(plain.instructions, None);

        // With the app's tools and skill, under another account: the server
        // with the pane spelled out, the plugin, where the agent is, and the
        // account's home for its records.
        let mut context = orchestrated(state.path(), folder.path(), true);
        let place = folder.path().join("place.md");
        std::fs::write(&place, "You run in a pane of kou desktop.\n").unwrap();
        if let Some(orchestration) = &mut context.orchestration {
            orchestration.place = Some(place);
        }
        context.env = vec![
            ("CLAUDE_CONFIG_DIR".into(), "/homes/work".into()),
            ("HTTPS_PROXY".into(), "http://10.0.0.1:8080".into()),
        ];
        let direct = claude_direct(&claude, &context).unwrap();
        assert_eq!(direct.home.as_deref(), Some(Path::new("/homes/work")));
        assert!(
            direct
                .env
                .contains(&("HTTPS_PROXY".into(), "http://10.0.0.1:8080".into()))
        );
        assert_eq!(direct.mcp_servers.len(), 1);
        assert_eq!(direct.mcp_servers[0].name, "kou");
        assert!(
            direct.mcp_servers[0]
                .env
                .contains(&("KOU_PANE".into(), "p3".into()))
        );
        assert_eq!(direct.plugins, [folder.path().join("kou-plugin")]);
        assert_eq!(
            direct.instructions.as_deref(),
            Some("You run in a pane of kou desktop.")
        );

        // Without a plugin the guidance itself is what it is told.
        let context = orchestrated(state.path(), folder.path(), false);
        let direct = claude_direct(&claude, &context).unwrap();
        assert!(direct.plugins.is_empty());
        assert_eq!(
            direct.instructions.as_deref(),
            Some("Use kou to start agents in other panes.\n")
        );

        // Only Claude Code, and only one that can be started.
        assert_eq!(claude_direct(&installed("codex", None), &context), None);
        let signed_out = Detected {
            readiness: Readiness::NeedsLogin,
            ..installed("claude", None)
        };
        assert_eq!(claude_direct(&signed_out, &context), None);
        let missing = Detected {
            path: None,
            ..installed("claude", None)
        };
        assert_eq!(claude_direct(&missing, &context), None);
    }

    #[test]
    fn quoting_survives_awkward_paths() {
        assert_eq!(
            toml_string(r#"C:\Program Files\kou "x""#),
            r#""C:\\Program Files\\kou \"x\"""#
        );
        assert_eq!(shell_quote("/a/it's here/kou"), r#"'/a/it'\''s here/kou'"#);
        assert_ne!(content_hash(&["ab", "c"]), content_hash(&["a", "bc"]));
    }
}
