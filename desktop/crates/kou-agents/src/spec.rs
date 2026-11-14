//! What is known about each harness, as data: its binaries, how to ask it for
//! its version and login state, how to start it, whether it speaks ACP, how
//! its hooks are wired for one launch, and what its screen looks like when it
//! works.
//!
//! Every flag here was read from the CLI's own `--help` on the machine this
//! was written on, or from the tool's current documentation. Each spec's
//! [`Spec::sources`] says which, so a wrong line can be traced, and the notes
//! of [`Sessions`] and [`Orchestrate`] say what was seen working and what is
//! only documented or guessed.

/// A static description of one harness.
#[derive(Debug)]
pub struct Spec {
    /// The id it is spawned by.
    pub id: &'static str,
    /// Its name, for people.
    pub name: &'static str,
    /// One or two letters shown in an outlined square.
    pub code: &'static str,
    /// The executables to look for, in order of preference.
    pub binaries: &'static [Binary],
    /// Arguments that print the version. Empty when the CLI has no such flag.
    pub version: &'static [&'static str],
    /// The oldest version kou's launch line and hook wiring work with.
    pub minimum: Option<&'static str>,
    /// How to tell whether the user is signed in.
    pub login: Login,
    /// How to start the terminal interface.
    pub tui: Tui,
    /// The ways it can speak the Agent Client Protocol, best first.
    pub acp: &'static [Acp],
    /// How its hooks are wired for one launch, and what they report.
    pub hooks: Hooks,
    /// Whether a conversation survives a switch between the chat and the
    /// terminal interface.
    pub sessions: Sessions,
    /// How the app's MCP server and guidance reach the terminal interface
    /// for one launch. Over ACP every harness takes the server in `session/new`.
    pub orchestration: Orchestrate,
    /// What its terminal shows while it works or waits, for when hooks are silent.
    pub screen: Screen,
    /// Where these facts come from and how far each was checked.
    pub sources: &'static str,
}

/// One executable name a harness may be installed under.
#[derive(Debug)]
pub struct Binary {
    /// The file name looked up on `PATH`.
    pub name: &'static str,
    /// What its version output must say for it to be this harness. Two tools
    /// ship an `agent` binary; only the output tells them apart.
    pub identity: Identity,
}

/// A check on a binary's version output, lowercased.
#[derive(Debug)]
pub enum Identity {
    /// The name is enough.
    Any,
    /// The output contains this.
    Reports(&'static str),
    /// The output does not contain this.
    DoesNotReport(&'static str),
}

/// How a harness reports its login state.
#[derive(Debug)]
pub enum Login {
    /// It has no status command; whether it is signed in shows only on launch.
    Unknown,
    /// A status command.
    Status {
        /// Its arguments.
        args: &'static [&'static str],
        /// Output that means signed out, compared in lowercase with all
        /// whitespace removed, so plain text and JSON match the same way.
        signed_out: &'static [&'static str],
        /// What to tell the user to run.
        hint: &'static str,
    },
}

/// How one launch option reaches a harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg {
    /// The harness has no way to take it at launch.
    No,
    /// The last argument.
    Positional,
    /// These words, then the value: `--model M`, or `--resume --session-id ID`.
    Words(&'static [&'static str]),
    /// One word: `--model=M`.
    Joined(&'static str),
    /// These words and the value directly after the fixed arguments: `resume ID`.
    Leading(&'static [&'static str]),
    /// An environment variable.
    Env(&'static str),
    /// A key of the settings file generated for the launch.
    Setting(&'static str),
    /// Typed into the terminal once the harness is up ([`crate::Launch::initial_input`]).
    Typed,
}

/// The command line of a harness's terminal interface.
#[derive(Debug)]
pub struct Tui {
    /// Arguments always passed, first.
    pub args: &'static [&'static str],
    /// A first prompt that leaves the session interactive.
    pub prompt: Arg,
    /// A model.
    pub model: Arg,
    /// A session to resume, by the harness's own id.
    pub resume: Arg,
    /// The working directory, for a CLI that wants it spelled out. The process
    /// is started in that directory either way.
    pub cwd: Arg,
    /// Environment set for the launch.
    pub env: &'static [(&'static str, &'static str)],
}

/// One way a harness speaks the Agent Client Protocol on stdio.
#[derive(Debug)]
pub enum Acp {
    /// The harness's own binary, started with these arguments.
    Builtin {
        /// The arguments.
        args: &'static [&'static str],
        /// How detection confirms this installed version has the mode.
        evidence: Evidence,
        /// Environment set for the mode, as its own documentation asks.
        env: &'static [(&'static str, &'static str)],
    },
    /// A separate adapter program that drives the harness.
    Adapter {
        /// Names it is installed under, the current one first.
        binaries: &'static [&'static str],
        /// Its arguments.
        args: &'static [&'static str],
        /// How the app installs it into its own folder, when it can.
        package: Option<Package>,
    },
}

/// An adapter as an npm package, installed with npm's layout into a folder of
/// the app's: the program lands in `node_modules/.bin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Package {
    /// The package's name on the npm registry.
    pub npm: &'static str,
    /// The version installed, exactly.
    pub version: &'static str,
    /// The program it puts in `node_modules/.bin`.
    pub binary: &'static str,
    /// The variable that points the adapter at the harness's own command
    /// line. Set to the installed CLI, the chat runs the same version as the
    /// terminal, which is what lets a conversation move between them, and the
    /// install can leave out the copy of the CLI the package would download.
    pub cli_env: Option<&'static str>,
    /// About what npm puts on disk, in megabytes: with the harness's own CLI
    /// used, and with the package's bundled copy of it.
    pub megabytes: (u32, u32),
}

/// Whether a conversation can move between the chat and the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crossing {
    /// The ACP session id is the id the terminal resumes, and the other way round.
    SameId,
    /// Pi: the terminal resumes a session file, and the ACP id is the one in
    /// that file's first line.
    PiSessionFile,
    /// Not known to be possible: switching starts a new conversation.
    None,
}

/// What [`Crossing`] holds for a harness, and how that is known.
#[derive(Debug)]
pub struct Sessions {
    /// Whether the conversation crosses.
    pub crossing: Crossing,
    /// Where that comes from, and its limits.
    pub note: &'static str,
}

/// How orchestration is attached to the terminal interface for one launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attach {
    /// Nothing can be attached without writing to the user's configuration.
    None,
    /// Claude Code: `--mcp-config=<file>` and `--plugin-dir <dir>`, or
    /// `--append-system-prompt` when there is no plugin.
    ClaudeFlags,
    /// Codex: `-c mcp_servers.<name>.*` and `-c developer_instructions=...`.
    CodexConfig,
    /// OpenCode: `OPENCODE_CONFIG_CONTENT` with the server and the guidance,
    /// and a private server (`--standalone`) on version 2 so it is read.
    OpencodeConfig,
    /// Gemini CLI and Qwen Code: `mcpServers` in the generated
    /// system-defaults file their hooks already use.
    SystemDefaults,
    /// Cursor: `--plugin-dir` with a generated plugin of an `mcp.json` and a rule.
    CursorPlugin,
    /// Copilot CLI: `--additional-mcp-config=@<file>` and
    /// `COPILOT_CUSTOM_INSTRUCTIONS_DIRS`.
    CopilotFlags,
    /// Goose: `--with-extension "<name>:<VAR=value ...> <command>"`.
    GooseExtension,
    /// Pi: `--append-system-prompt <file>`. Pi has no MCP.
    PiPrompt,
    /// Aider: `--read <file>`. Aider has no MCP.
    AiderRead,
    /// Grok Build: `--rules <text>`. Its terminal interface takes no MCP
    /// server for one launch.
    GrokRules,
}

/// What of the guidance reaches the model when it is attached at launch,
/// with or without the MCP server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guidance {
    /// A skill: the model sees its description and loads it when it needs it.
    Skill,
    /// Text put into its context from the start.
    Context,
    /// Nothing known: without the MCP server this harness does not learn of
    /// the other panes at all, and with it only what the server tells it.
    Nothing,
}

/// A harness's orchestration attachment, and how far it was checked.
#[derive(Debug)]
pub struct Orchestrate {
    /// The mechanism.
    pub attach: Attach,
    /// What reaches the model in the terminal interface.
    pub terminal: Guidance,
    /// What reaches the model in the chat. An agent that advertises
    /// `sessionCapabilities.systemPrompt` at run time gets the text even
    /// where this says nothing.
    pub chat: Guidance,
    /// One or two sentences on it: whether the server inherits the pane's
    /// environment, whether a trust question appears, or why there is none.
    pub note: &'static str,
}

/// Why an ACP mode is believed to exist in the installed version.
#[derive(Debug)]
pub enum Evidence {
    /// Running the binary with `args` prints help that contains `needle`.
    Help {
        /// The arguments that print the help.
        args: &'static [&'static str],
        /// What the help must contain.
        needle: &'static str,
    },
    /// The mode is not in the help (a hidden command, or a value the help does
    /// not list); it is taken from the documentation as is.
    Documented,
}

/// How hooks are wired into one launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wiring {
    /// Nothing can be wired without writing to the user's or the project's files.
    None,
    /// Claude Code: `--settings <file>` with a `hooks` block.
    ClaudeSettings,
    /// Codex: `-c notify=[..]`, and `-c hooks.<Event>=[..]` when the hooks feature is on.
    CodexConfig,
    /// Droid: `--settings <file>` with a `hooks` block.
    DroidSettings,
    /// Gemini CLI and Qwen Code: a system-defaults settings file named by an
    /// environment variable, the lowest settings layer.
    SystemDefaults {
        /// The variable that names the file.
        env: &'static str,
        /// The directory of the real system-defaults file on macOS, under
        /// `/Library/Application Support`. Its content is carried over.
        mac_dir: &'static str,
        /// The same on Linux, under `/etc`, and on Windows, under `ProgramData`.
        etc_dir: &'static str,
        /// Whether events use Gemini's names (`BeforeAgent`) or Claude's (`UserPromptSubmit`).
        gemini_events: bool,
    },
    /// Pi: `--extension <file>` with a generated extension.
    PiExtension,
    /// OpenCode 2: a terminal-side plugin named in `OPENCODE_CLI_CONFIG_CONTENT`.
    OpencodePlugin,
    /// Cursor: `--plugin-dir <dir>` with `hooks/hooks.json`.
    CursorPlugin,
    /// Copilot CLI: `--plugin-dir=<dir>` with `hooks.json`.
    CopilotPlugin,
    /// Aider: `--notifications-command`, which runs when a reply is ready.
    AiderNotify,
}

/// How much of a turn the wired hooks report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// Nothing.
    None,
    /// Only that a turn ended; work is still inferred from the terminal.
    TurnEnd,
    /// The start of a turn, its tool calls and its end.
    Full,
}

/// A harness's hooks, as kou uses them.
#[derive(Debug)]
pub struct Hooks {
    /// How they are wired.
    pub wiring: Wiring,
    /// What they cover once they fire.
    pub coverage: Coverage,
    /// Whether they report a permission question. When not, the screen
    /// patterns decide "blocked" even while hooks drive the rest.
    pub blocked: bool,
    /// One sentence on the mechanism or on why there is none.
    pub note: &'static str,
}

/// Claude Code's status line while a turn runs, as 2.1.322 draws it:
/// `✽ Drizzling… (7m 21s · ↓ 39.9k tokens)`. A spinner's glyph, a verb with an
/// ellipsis, and in brackets how long the turn has taken so far. (That
/// version no longer writes "esc to interrupt" there.) The line stands
/// while the session itself works and while it waits for agents it started
/// in the background, of which its hooks say nothing: `Stop` has fired by
/// then.
pub(crate) fn claude_status(line: &str) -> bool {
    let Some((head, taken)) = line.trim_start().split_once("\u{2026} (") else {
        return false;
    };
    let Some((glyph, verb)) = head.split_once(' ') else {
        return false;
    };
    let mut marks = glyph.chars();
    let one_mark = matches!(
        (marks.next(), marks.next()),
        (Some(mark), None) if !mark.is_alphanumeric()
    );
    // The verb changes from turn to turn and can be the user's own; it is
    // always a capitalised word or a few.
    let a_verb = verb.chars().next().is_some_and(char::is_uppercase) && verb.chars().count() <= 60;
    // "44s", "7m 21s", "1h 2m".
    let digits = taken.chars().take_while(char::is_ascii_digit).count();
    let a_time = digits > 0 && matches!(taken[digits..].chars().next(), Some('s' | 'm' | 'h'));
    one_mark && a_verb && a_time
}

/// What the terminal shows, matched in lowercase against the last lines on screen.
#[derive(Debug)]
pub struct Screen {
    /// Text present while a turn runs.
    pub working: &'static [&'static str],
    /// A line the harness draws while a turn runs, where no fixed text says
    /// so: asked of each line read, as it stands on screen.
    pub working_line: Option<fn(&str) -> bool>,
    /// Text of a question that waits for the user.
    pub blocked: &'static [&'static str],
    /// Window title text present while a turn runs.
    pub title_working: &'static [&'static str],
    /// Window title text present while it waits for the user.
    pub title_blocked: &'static [&'static str],
    /// Whether sustained output alone counts as work. Off for a harness whose
    /// `working` text is dependable, so typing and redraws are not mistaken for it.
    pub output: bool,
    /// What a bell or an OSC notification from it means.
    pub notification: Notice,
}

/// What a harness means when it rings the bell during a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// It needs the user: a question is waiting.
    Attention,
    /// The turn is over, whether it finished or stopped to ask.
    Finished,
}

const fn bin(name: &'static str) -> Binary {
    Binary {
        name,
        identity: Identity::Any,
    }
}

/// A harness whose hooks cannot be wired for one launch; `note` says why.
const fn no_hooks(note: &'static str) -> Hooks {
    Hooks {
        wiring: Wiring::None,
        coverage: Coverage::None,
        blocked: false,
        note,
    }
}

/// Working text most terminal agents share, for the ones never observed.
const COMMON_WORKING: &[&str] = &["esc to interrupt", "esc to cancel", "ctrl+c to interrupt"];

/// Where every ACP launch line in this file was checked: the public registry
/// of ACP agents, as of 2026-11-12, with each harness's own documentation or
/// `--help`, and each adapter's package on npm.
pub const ACP_REGISTRY: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

static SPECS: &[Spec] = &[
    Spec {
        id: "kou",
        name: "kou",
        code: "ko",
        binaries: &[bin("kou-conveyor-tui")],
        // Go's flag package; the TUI defines no version flag.
        version: &[],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Words(&["-p"]),
            model: Arg::Words(&["-model"]),
            resume: Arg::Words(&["-session"]),
            cwd: Arg::Words(&["-workspace"]),
            env: &[],
        },
        acp: &[],
        hooks: no_hooks(
            "The TUI has no hooks; it marks a running turn in its window title, which is exact.",
        ),
        sessions: Sessions {
            crossing: Crossing::None,
            note: "kou's own agent opens natively through its runner (kou-native), not through ACP, so this crate has no id to carry across.",
        },
        orchestration: Orchestrate {
            attach: Attach::None,
            terminal: Guidance::Nothing,
            chat: Guidance::Nothing,
            note: "`kou-conveyor-tui --help` shows no flag for an MCP server or added instructions.",
        },
        screen: Screen {
            working: &[],
            blocked: &[],
            // cmd/kou-conveyor-tui/model.go, `title()`: the prefix is set whenever the state is not idle.
            title_working: &["\u{25cf} "],
            title_blocked: &[],
            output: false,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Flags from `kou-conveyor-tui --help` (run here); `--version` is rejected (run here). Title prefix from cmd/kou-conveyor-tui/model.go.",
    },
    Spec {
        id: "claude",
        name: "Claude Code",
        code: "CC",
        binaries: &[bin("claude")],
        version: &["--version"],
        // `--settings` arrived in 1.0.61, SessionStart in 1.0.62, SessionEnd in 1.0.85 (changelog).
        minimum: Some("1.0.85"),
        login: Login::Status {
            args: &["auth", "status"],
            signed_out: &["\"loggedin\":false"],
            hint: "Run `claude auth login`.",
        },
        tui: Tui {
            args: &[],
            prompt: Arg::Positional,
            model: Arg::Words(&["--model"]),
            resume: Arg::Words(&["--resume"]),
            cwd: Arg::No,
            env: &[],
        },
        acp: &[Acp::Adapter {
            // `claude-code-acp` is the name of the package before it was renamed.
            binaries: &["claude-agent-acp", "claude-code-acp"],
            args: &[],
            package: Some(Package {
                npm: "@agentclientprotocol/claude-agent-acp",
                version: "0.106.0",
                binary: "claude-agent-acp",
                cli_env: Some("CLAUDE_CODE_EXECUTABLE"),
                megabytes: (70, 395),
            }),
        }],
        hooks: Hooks {
            wiring: Wiring::ClaudeSettings,
            coverage: Coverage::Full,
            blocked: true,
            note: "`--settings <file>` adds a hooks block for this session; hook entries merge with the user's own.",
        },
        sessions: Sessions {
            crossing: Crossing::SameId,
            note: "Verified here with claude-agent-acp 0.106.0 and Claude Code 2.1.322: the ACP session id is the id Claude Code runs the session under (its CLAUDE_CODE_SESSION_ID was the ACP id), and `session/load` reads `<CLAUDE_CONFIG_DIR>/projects/<cwd>/<id>.jsonl`, the transcript `claude --resume <id>` reads (adapter source, resumed-session.js). Both must run in the same directory. A session with no turn yet is saved by neither, so it cannot be reopened: load answered -32002 here.",
        },
        orchestration: Orchestrate {
            attach: Attach::ClaudeFlags,
            terminal: Guidance::Skill,
            chat: Guidance::Skill,
            note: "`--mcp-config=<file>` and `--plugin-dir <dir>` hold for this session only. Verified here on 2.1.322: the server started with the pane's environment, the plugin's skill was offered, and no trust or approval question appeared. Each call of the server's tools still asks permission, as any tool does, unless the user allows it.",
        },
        screen: Screen {
            working: &["esc to interrupt"],
            // The last is the folder trust question a first launch in a
            // directory asks, seen on 2.1.322.
            blocked: &[
                "do you want to proceed",
                "do you want to allow",
                "yes, i trust this folder",
            ],
            title_working: &[],
            title_blocked: &[],
            output: false,
            notification: Notice::Attention,
            working_line: Some(claude_status),
        },
        sources: "Flags from `claude --help` 2.1.322 and login from `claude auth status` (both run here). Hook events and payloads from code.claude.com/docs/en/hooks; event versions from the changelog. Screen text found in the 2.1.322 binary, not seen on a live screen. ACP: the ACP registry (claude-acp 0.106.0, npm @agentclientprotocol/claude-agent-acp, formerly @zed-industries/claude-code-acp, which npm marks deprecated); installed into a scratch folder with `--omit=optional` and CLAUDE_CODE_EXECUTABLE pointing at the installed CLI, then `initialize`, `session/new` with an MCP server, a system prompt and `_meta.claudeCode.options.plugins`, `session/load` and `session/resume`, without a prompt (run here). Its terminal `--mcp-config` and `--plugin-dir` were started here without a prompt.",
    },
    Spec {
        id: "codex",
        name: "Codex",
        code: "CX",
        binaries: &[bin("codex")],
        version: &["--version"],
        minimum: None,
        login: Login::Status {
            args: &["login", "status"],
            signed_out: &["notloggedin"],
            hint: "Run `codex login`.",
        },
        tui: Tui {
            args: &[],
            prompt: Arg::Positional,
            model: Arg::Words(&["--model"]),
            resume: Arg::Leading(&["resume"]),
            cwd: Arg::No,
            env: &[],
        },
        acp: &[Acp::Adapter {
            binaries: &["codex-acp"],
            args: &[],
            package: Some(Package {
                npm: "@agentclientprotocol/codex-acp",
                version: "2.6.0",
                binary: "codex-acp",
                cli_env: Some("CODEX_PATH"),
                megabytes: (35, 400),
            }),
        }],
        hooks: Hooks {
            wiring: Wiring::CodexConfig,
            coverage: Coverage::Full,
            blocked: true,
            note: "`-c notify=[..]` reports the end of a turn at once. Lifecycle hooks are passed as `-c hooks.<Event>` \
                and run only after the user trusts them once in `/hooks`; until then the terminal is read instead.",
        },
        sessions: Sessions {
            crossing: Crossing::SameId,
            note: "From codex-acp 2.6.0's source: its ACP session id is the Codex thread id (`thread/start` answers it), which `codex resume <id>` takes. Not verified here: on this machine Codex answered both the adapter and its own interface that it needs a login.",
        },
        orchestration: Orchestrate {
            attach: Attach::CodexConfig,
            terminal: Guidance::Context,
            chat: Guidance::Context,
            note: "`-c mcp_servers.<name>.command/args/env`, and the guidance as `-c developer_instructions=...` unless the user's config.toml sets that key, which it would hide. Codex gives an MCP server only HOME, PATH and a few more of its environment (codex-rs/rmcp-client/src/utils.rs), so the pane's variables go in `env`, and it ends a tool call after 60 seconds unless `tool_timeout_sec` says otherwise (developers.openai.com/codex/mcp), so that is raised. The same page says Codex reads a server's MCP `instructions`; the developer instructions carry the guidance either way. Not started here (Codex asked for a login), so whether it asks to trust the server is not known.",
        },
        screen: Screen {
            working: &["esc to interrupt"],
            blocked: &[
                "would you like to run the following command",
                "would you like to make the following edits",
                "would you like to grant these permissions",
                // The update question a launch opens with, seen on 0.160.0.
                "skip until next version",
            ],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Flags from `codex --help`, `codex resume --help`, `codex mcp add --help`, `codex login status` and `codex features list` 0.160.0 (all run here). Hooks and notify from developers.openai.com/codex/hooks and /config-advanced; that `-c` hooks are discovered from codex-rs/hooks/src/engine/discovery.rs. Approval text found in the 2.6.0 adapter and the 0.160.0 binary; `developer_instructions` found in the 0.160.0 binary. ACP: the ACP registry (codex-acp 2.6.0, npm @agentclientprotocol/codex-acp, which replaced @zed-industries/codex-acp; it runs `codex app-server` and wants @openai/codex ^0.166.0); installed into a scratch folder with `--omit=optional` and CODEX_PATH at the installed 0.160.0, then `initialize` and `session/new` (run here): `session/new` answered -32000 because the stored ChatGPT login could not be refreshed. A shared app-server daemon, when one runs, may serve the terminal instead of a private one; whether `-c` overrides reach it was not checked.",
    },
    Spec {
        id: "gemini",
        name: "Gemini CLI",
        code: "G",
        binaries: &[bin("gemini")],
        version: &["--version"],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Words(&["--prompt-interactive"]),
            model: Arg::Words(&["--model"]),
            resume: Arg::Words(&["--resume"]),
            cwd: Arg::No,
            env: &[],
        },
        acp: &[
            Acp::Builtin {
                args: &["--acp"],
                evidence: Evidence::Help {
                    args: &["--help"],
                    needle: "--acp",
                },
                env: &[],
            },
            // The name of the flag before `--acp`; still accepted, as deprecated.
            Acp::Builtin {
                args: &["--experimental-acp"],
                evidence: Evidence::Help {
                    args: &["--help"],
                    needle: "--experimental-acp",
                },
                env: &[],
            },
        ],
        hooks: Hooks {
            wiring: Wiring::SystemDefaults {
                env: "GEMINI_CLI_SYSTEM_DEFAULTS_PATH",
                mac_dir: "GeminiCli",
                etc_dir: "gemini-cli",
                gemini_events: true,
            },
            coverage: Coverage::Full,
            blocked: true,
            note: "A generated system-defaults settings file, the lowest settings layer; hook lists concatenate across layers.",
        },
        sessions: Sessions {
            crossing: Crossing::SameId,
            note: "From the source (packages/cli/src/acp/acpSessionManager.ts): a new ACP session's random UUID is the session id Gemini records the chat under, and `session/load` finds it with the SessionSelector that `--resume` uses; docs/cli/cli-reference.md shows `gemini -r <session-id>`. Not installed here.",
        },
        orchestration: Orchestrate {
            attach: Attach::SystemDefaults,
            terminal: Guidance::Nothing,
            chat: Guidance::Nothing,
            note: "`mcpServers` in the generated system-defaults file. Gemini gives a server its environment minus anything that looks like a secret (docs/tools/mcp-server.md), and the pane's variables are set in `env` as well. There is no launch-scoped way to add instructions or skills (only GEMINI_SYSTEM_MD, which replaces the whole system prompt), so with guidance alone Gemini learns nothing of the other panes. Not installed here: untested, and whether folder trust holds back a server from the system layer is not known.",
        },
        screen: Screen {
            working: &["esc to cancel"],
            blocked: &[
                "waiting for user confirmation",
                "allow execution",
                "apply this change",
            ],
            title_working: &["\u{2726}"],
            title_blocked: &["\u{270b}"],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Not installed here. Flags from the gemini-cli repository's docs/cli/cli-reference.md and packages/cli/src/config/config.ts (`--acp`, and `--experimental-acp` marked deprecated); hooks from docs/hooks/reference.md; settings layers from docs/reference/configuration.md; list merging from settingsSchema.ts; MCP from docs/tools/mcp-server.md; ACP from packages/cli/src/acp (loadSession, stdio, http and sse MCP). Registry: gemini 0.68.0 `--acp`. Screen and title text from memory of the interface, unverified.",
    },
    Spec {
        id: "opencode",
        name: "OpenCode",
        code: "OC",
        binaries: &[bin("opencode")],
        version: &["--version"],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Words(&["--prompt"]),
            // The full-screen interface of OpenCode 2 has no model flag; `run` and `mini` do.
            model: Arg::No,
            resume: Arg::Words(&["--session"]),
            cwd: Arg::Positional,
            env: &[],
        },
        acp: &[Acp::Builtin {
            args: &["acp"],
            evidence: Evidence::Help {
                args: &["--help"],
                needle: "agent client protocol",
            },
            env: &[],
        }],
        hooks: Hooks {
            wiring: Wiring::OpencodePlugin,
            coverage: Coverage::Full,
            blocked: true,
            note: "OpenCode 2 only: a terminal-side plugin named in OPENCODE_CLI_CONFIG_CONTENT forwards session events. \
                OpenCode 1 gets none and is read from the terminal.",
        },
        sessions: Sessions {
            crossing: Crossing::SameId,
            note: "Verified here on 2.2.5: a session made by `opencode acp` is listed by `opencode session list` under the same `ses_` id, `session/load` of it worked, and the terminal's `--session <id>` takes that id. An unknown id is answered -32602.",
        },
        orchestration: Orchestrate {
            attach: Attach::OpencodeConfig,
            terminal: Guidance::Context,
            chat: Guidance::Context,
            note: "OPENCODE_CONFIG_CONTENT with `mcp.<name>` (the version 1 shape, which 2.2.5 reads too) and `instructions`; version 2 needs `--standalone`, or the background service, which never saw the variable, would serve the pane. Verified here on 2.2.5, through `opencode acp` and in the terminal with `--standalone`: the server started from the variable with the pane's environment, and no question was asked.",
        },
        screen: Screen {
            working: &["esc interrupt", "again to interrupt"],
            blocked: &["permission required"],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Flags from `opencode --help`, `opencode acp --help`, `opencode session --help` and `opencode debug config` 2.2.5 (run here). Plugin loading from opencode.ai/docs/cli/config, /docs/cli/plugins and /docs/build/plugins/cli; event names found in the 2.2.5 binary. The plugin was not loaded into a live session. ACP: `initialize`, `session/new` with an MCP server and `session/load` against `opencode acp` 2.2.5 (run here): loadSession, resume, http MCP, no systemPrompt; a terminal auth method `--login`.",
    },
    Spec {
        id: "cursor",
        name: "Cursor",
        code: "CU",
        binaries: &[
            bin("cursor-agent"),
            // Grok Build installs an `agent` too (it does on this machine).
            Binary {
                name: "agent",
                identity: Identity::DoesNotReport("grok"),
            },
        ],
        version: &["--version"],
        minimum: None,
        login: Login::Status {
            args: &["status"],
            signed_out: &["notloggedin", "notauthenticated"],
            hint: "Run `cursor-agent login`.",
        },
        tui: Tui {
            args: &[],
            prompt: Arg::Positional,
            model: Arg::Words(&["--model"]),
            resume: Arg::Words(&["--resume"]),
            cwd: Arg::Words(&["--workspace"]),
            env: &[],
        },
        acp: &[Acp::Builtin {
            args: &["acp"],
            // "hidden from default command help output", so help proves nothing.
            evidence: Evidence::Documented,
            env: &[],
        }],
        hooks: Hooks {
            wiring: Wiring::CursorPlugin,
            coverage: Coverage::Full,
            blocked: false,
            note: "`--plugin-dir <dir>` loads a generated plugin whose hooks/hooks.json reports prompts, tools and stops. \
                Cursor has no hook for a permission question.",
        },
        sessions: Sessions {
            crossing: Crossing::None,
            note: "Its ACP documentation offers `session/load` and the terminal `--resume [chatId]`, but whether the ACP session id is the chat id is not documented. Not installed here.",
        },
        orchestration: Orchestrate {
            attach: Attach::CursorPlugin,
            terminal: Guidance::Context,
            chat: Guidance::Nothing,
            note: "`--plugin-dir` with a generated plugin holding `mcp.json` and a rule made from the guidance (cursor.com/docs/reference/plugins). Cursor asks to approve MCP servers, and `--approve-mcps` would approve every server, the user's included, so it is not passed: expect the question. Its ACP mode reads MCP servers only from `.cursor/mcp.json` (cursor.com/docs/cli/acp), so the server sent in `session/new` may be ignored there. Not installed here: untested.",
        },
        screen: Screen {
            working: COMMON_WORKING,
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Not installed here (`agent` on this machine is Grok Build). Flags, `status` and `acp` from cursor.com/docs/cli/reference/parameters and /docs/cli/acp; MCP from /docs/cli/mcp; hooks from cursor.com/docs/hooks; plugin layout from /docs/reference/plugins. Signed-out wording is a guess.",
    },
    Spec {
        id: "copilot",
        name: "GitHub Copilot CLI",
        code: "CP",
        binaries: &[bin("copilot")],
        version: &["--version"],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Joined("--interactive"),
            model: Arg::Joined("--model"),
            resume: Arg::Joined("--resume"),
            cwd: Arg::No,
            env: &[],
        },
        acp: &[Acp::Builtin {
            args: &["--acp"],
            evidence: Evidence::Help {
                args: &["--help"],
                needle: "--acp",
            },
            env: &[],
        }],
        hooks: Hooks {
            wiring: Wiring::CopilotPlugin,
            coverage: Coverage::Full,
            blocked: true,
            note: "`--plugin-dir=<dir>` loads a generated plugin whose hooks.json reports prompts, tools, stops and permission notifications.",
        },
        sessions: Sessions {
            crossing: Crossing::None,
            note: "The terminal takes `--resume=<session id>`, and the ACP server takes `session/new` with MCP servers; whether its ACP ids are those session ids is not documented. Not installed here.",
        },
        orchestration: Orchestrate {
            attach: Attach::CopilotFlags,
            terminal: Guidance::Context,
            chat: Guidance::Context,
            note: "`--additional-mcp-config=@<file>` adds a server for this session only, ahead of every other source and with no review, and COPILOT_CUSTOM_INSTRUCTIONS_DIRS names a generated folder whose AGENTS.md carries the guidance. Copilot passes a server only PATH of its environment, so the pane's variables go in `env`. Not installed here: untested.",
        },
        screen: Screen {
            working: COMMON_WORKING,
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Not installed here. Flags from docs.github.com Copilot CLI command reference; hooks from its hooks configuration reference; MCP and its environment from how-tos/copilot-cli/customize-copilot/add-mcp-servers; instructions from .../add-custom-instructions; ACP from reference/acp-server. The plugin manifest name `plugin.json` is from memory, not from that reference; not loaded into a live session.",
    },
    Spec {
        id: "amp",
        name: "Amp",
        code: "A",
        binaries: &[bin("amp")],
        version: &["--version"],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Typed,
            model: Arg::No,
            resume: Arg::No,
            cwd: Arg::No,
            // Amp then rings the terminal bell when it finishes or needs input.
            env: &[("AMP_FORCE_BEL", "1")],
        },
        acp: &[Acp::Adapter {
            binaries: &["amp-acp"],
            args: &[],
            // Also released as a binary on GitHub, which the ACP registry uses.
            package: Some(Package {
                npm: "amp-acp",
                version: "0.12.0",
                binary: "amp-acp",
                cli_env: None,
                megabytes: (40, 40),
            }),
        }],
        hooks: no_hooks(
            "Amp plugins load only from the project's .amp/plugins or ~/.config/amp/plugins, which kou does not write to.",
        ),
        sessions: Sessions {
            crossing: Crossing::None,
            note: "The terminal interface here takes no session to resume (`threads continue` is a subcommand this crate does not use). Not installed here.",
        },
        orchestration: Orchestrate {
            attach: Attach::None,
            terminal: Guidance::Nothing,
            chat: Guidance::Nothing,
            note: "The one launch-scoped way, `--settings-file`, replaces the user's settings file instead of adding to it (ampcode.com/docs/cli/settings), so it would hide their settings; nothing is attached. Amp's plugins load only from the project or the user's folder.",
        },
        screen: Screen {
            working: COMMON_WORKING,
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Finished,
            working_line: None,
        },
        sources: "Not installed here. ampcode.com/docs/markdown/cli, /cli/settings (AMP_FORCE_BEL, --settings-file) and /customize/mcp and /customize/plugins; the docs defer to `amp --help` for flags, so none are used. ACP adapter from the ACP registry (amp-acp 0.12.0, a binary release) and npm (`amp-acp` 0.12.0, which `npm view` lists with the binary amp-acp); its size is a guess from the package's unpacked size and its dependency on @ampcode/sdk.",
    },
    Spec {
        id: "droid",
        name: "Droid",
        code: "D",
        binaries: &[bin("droid")],
        version: &["--version"],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Positional,
            model: Arg::Setting("model"),
            resume: Arg::Words(&["--resume"]),
            cwd: Arg::No,
            env: &[],
        },
        acp: &[Acp::Builtin {
            args: &["exec", "--output-format", "acp-daemon"],
            // `droid exec --help` names no format values.
            evidence: Evidence::Documented,
            // What the ACP registry sets, so a chat does not update the CLI under the user.
            env: &[
                ("DROID_DISABLE_AUTO_UPDATE", "true"),
                ("FACTORY_DROID_AUTO_UPDATE_ENABLED", "false"),
            ],
        }],
        hooks: Hooks {
            wiring: Wiring::DroidSettings,
            coverage: Coverage::Full,
            blocked: true,
            note: "`--settings <file>` merges a runtime settings file with a hooks block for this process only.",
        },
        sessions: Sessions {
            crossing: Crossing::None,
            note: "Its ACP mode offers `session/load` and `session/resume`, and the terminal `--resume [sessionId]`; that the two share ids is likely but was not seen (the session could not be made here without a login).",
        },
        orchestration: Orchestrate {
            attach: Attach::None,
            terminal: Guidance::Nothing,
            chat: Guidance::Nothing,
            note: "MCP servers come only from `droid mcp add`, which writes ~/.factory/mcp.json; whether the runtime `--settings` file takes servers is not documented, so nothing is attached.",
        },
        screen: Screen {
            working: &["esc to stop", "esc to interrupt"],
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Flags from `droid --help`, `droid exec --help` and `droid mcp add --help` 0.89.0 (run here). Hooks from docs.factory.ai/reference/hooks-reference, the `model` setting from /cli/configuration/settings. ACP from the ACP registry (droid 1.9.0, with its environment); against 0.89.0 here `initialize` answered (loadSession, resume, no MCP transports beyond stdio) and `session/new` answered that it needs a login.",
    },
    Spec {
        id: "pi",
        name: "Pi",
        code: "pi",
        binaries: &[bin("pi")],
        version: &["--version"],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Positional,
            model: Arg::Words(&["--model"]),
            resume: Arg::Words(&["--session"]),
            cwd: Arg::No,
            env: &[],
        },
        acp: &[Acp::Adapter {
            binaries: &["pi-acp"],
            args: &[],
            package: Some(Package {
                npm: "pi-acp",
                version: "0.0.34",
                binary: "pi-acp",
                cli_env: Some("PI_ACP_PI_COMMAND"),
                megabytes: (20, 20),
            }),
        }],
        hooks: Hooks {
            wiring: Wiring::PiExtension,
            coverage: Coverage::Full,
            blocked: false,
            note: "`--extension <file>` loads a generated extension that forwards agent and tool events. Pi asks no permission questions itself.",
        },
        sessions: Sessions {
            crossing: Crossing::PiSessionFile,
            note: "From pi-acp 0.0.34's source: a session's id is the `id` in the first line of its file under ~/.pi/agent/sessions (or PI_CODING_AGENT_DIR), and pi is started with `--session <file>`, which the terminal takes too (`pi --help` 0.61.1). The id is turned into that file and back.",
        },
        orchestration: Orchestrate {
            attach: Attach::PiPrompt,
            terminal: Guidance::Context,
            chat: Guidance::Nothing,
            note: "`--append-system-prompt <file>` adds the guidance (`pi --help` 0.61.1). Pi has no MCP, and pi-acp keeps the servers of `session/new` without using them, so the server is not attached and the guidance has to lead to the command line instead.",
        },
        screen: Screen {
            working: &[],
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Flags from `pi --help` 0.61.1 (run here); extension API from the installed package's docs/extensions.md. The extension was not loaded into a live session. ACP adapter from the ACP registry (pi-acp 0.0.34, npm) and its published source; its size is a guess from the package's unpacked size and dependencies.",
    },
    Spec {
        id: "aider",
        name: "Aider",
        code: "ai",
        binaries: &[bin("aider")],
        version: &["--version"],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            // `--message` answers once and exits, so a first prompt is typed.
            prompt: Arg::Typed,
            model: Arg::Words(&["--model"]),
            resume: Arg::No,
            cwd: Arg::No,
            env: &[],
        },
        acp: &[],
        hooks: Hooks {
            wiring: Wiring::AiderNotify,
            coverage: Coverage::TurnEnd,
            blocked: false,
            note: "`--notifications-command` runs when a reply is ready and aider waits for input; nothing reports the start of a turn.",
        },
        sessions: Sessions {
            crossing: Crossing::None,
            note: "Aider has no ACP mode.",
        },
        orchestration: Orchestrate {
            attach: Attach::AiderRead,
            terminal: Guidance::Context,
            chat: Guidance::Nothing,
            note: "`--read <file>` adds the guidance as a read-only file (`aider --help` 0.85.2). Aider has no MCP.",
        },
        screen: Screen {
            working: &[],
            blocked: &["(y)es/(n)o"],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Flags from `aider --help` 0.85.2 (run here); notifications from aider.chat/docs/usage/notifications.html. No ACP mode exists.",
    },
    Spec {
        id: "goose",
        name: "Goose",
        code: "GO",
        binaries: &[bin("goose")],
        version: &["--version"],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &["session"],
            prompt: Arg::Typed,
            model: Arg::Env("GOOSE_MODEL"),
            resume: Arg::Words(&["--resume", "--session-id"]),
            cwd: Arg::No,
            env: &[],
        },
        acp: &[Acp::Builtin {
            args: &["acp"],
            evidence: Evidence::Help {
                args: &["--help"],
                needle: "acp",
            },
            env: &[],
        }],
        hooks: no_hooks("Goose documents no lifecycle hooks."),
        sessions: Sessions {
            crossing: Crossing::None,
            note: "Whether `goose acp` names its sessions the way `goose session --resume --session-id` does is not documented. Not installed here.",
        },
        orchestration: Orchestrate {
            attach: Attach::GooseExtension,
            terminal: Guidance::Nothing,
            chat: Guidance::Nothing,
            note: "`goose session --with-extension \"<name>:<VAR=value ...> <command>\"` adds a stdio extension for this session (documentation/docs/guides/goose-cli-commands.md), with the pane's variables in front. How goose splits a quoted path with spaces is not documented; the words are quoted as for a shell. No launch-scoped instructions, so with guidance alone goose learns nothing of the other panes. Not installed here: untested.",
        },
        screen: Screen {
            working: COMMON_WORKING,
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Not installed here. Commands and flags from the goose repository's documentation/docs/guides/goose-cli-commands.md and environment-variables.md. ACP: the ACP registry (goose 1.59.0, `goose acp`).",
    },
    Spec {
        id: "qwen",
        name: "Qwen Code",
        code: "Q",
        binaries: &[bin("qwen")],
        version: &["--version"],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Words(&["--prompt-interactive"]),
            model: Arg::Words(&["--model"]),
            resume: Arg::Words(&["--resume"]),
            cwd: Arg::No,
            env: &[],
        },
        acp: &[
            Acp::Builtin {
                args: &["--acp"],
                evidence: Evidence::Help {
                    args: &["--help"],
                    needle: "--acp",
                },
                env: &[],
            },
            Acp::Builtin {
                args: &["--experimental-acp"],
                evidence: Evidence::Help {
                    args: &["--help"],
                    needle: "--experimental-acp",
                },
                env: &[],
            },
        ],
        hooks: Hooks {
            wiring: Wiring::SystemDefaults {
                env: "QWEN_CODE_SYSTEM_DEFAULTS_PATH",
                mac_dir: "QwenCode",
                etc_dir: "qwen-code",
                gemini_events: false,
            },
            coverage: Coverage::Full,
            blocked: true,
            note: "A generated system-defaults settings file; hooks in system files load regardless of folder trust.",
        },
        sessions: Sessions {
            crossing: Crossing::None,
            note: "A fork of Gemini CLI, which keeps one id for both; Qwen's own ACP code was not read. Not installed here.",
        },
        orchestration: Orchestrate {
            attach: Attach::SystemDefaults,
            terminal: Guidance::Nothing,
            chat: Guidance::Nothing,
            note: "`mcpServers` in the generated system-defaults file, as for Gemini CLI, with the pane's variables in `env`. No launch-scoped instructions. Not installed here: untested.",
        },
        screen: Screen {
            working: &["esc to cancel"],
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Not installed here. Flags and settings layers from the qwen-code repository's docs/users/configuration/settings.md; hooks from docs/users/features/hooks.md. Whether `--resume` takes an id is assumed from Gemini CLI. ACP: the ACP registry (qwen-code 0.27.2, `--acp --experimental-skills`).",
    },
    Spec {
        id: "kiro",
        name: "Kiro",
        code: "KI",
        binaries: &[bin("kiro-cli")],
        version: &["--version"],
        minimum: None,
        login: Login::Status {
            args: &["whoami", "--format", "json"],
            signed_out: &["\"account\":null", "notloggedin"],
            hint: "Run `kiro-cli login`.",
        },
        tui: Tui {
            args: &["chat"],
            prompt: Arg::Positional,
            model: Arg::Words(&["--model"]),
            resume: Arg::Words(&["--resume-id"]),
            cwd: Arg::No,
            env: &[],
        },
        acp: &[Acp::Builtin {
            // The registry's 3.0.0 adds `--agent-engine=v3`, which older versions would refuse.
            args: &["acp"],
            evidence: Evidence::Help {
                args: &["--help"],
                needle: "acp",
            },
            env: &[],
        }],
        hooks: no_hooks(
            "Kiro reads hooks only from the project's .kiro/hooks or ~/.kiro/hooks, which kou does not write to.",
        ),
        sessions: Sessions {
            crossing: Crossing::None,
            note: "Not documented where read. Not installed here.",
        },
        orchestration: Orchestrate {
            attach: Attach::None,
            terminal: Guidance::Nothing,
            chat: Guidance::Nothing,
            note: "Kiro reads MCP servers from its agent configurations under the project's or the user's .kiro; no launch-scoped way was found.",
        },
        screen: Screen {
            working: COMMON_WORKING,
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Not installed here (a dangling `kiro-cli` link is). Commands from kiro.dev/docs/reference/cli-commands and /docs/hooks; the signed-out answer of `whoami --format json` as t3code reads it. ACP: the ACP registry (kiro 3.0.0, `acp --agent-engine=v3`).",
    },
    Spec {
        id: "grok",
        name: "Grok Build",
        code: "GK",
        binaries: &[
            bin("grok"),
            Binary {
                name: "agent",
                identity: Identity::Reports("grok"),
            },
        ],
        version: &["--version"],
        minimum: None,
        // `models` reports it, but that is a network call, not a status command.
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Positional,
            model: Arg::Words(&["--model"]),
            resume: Arg::Words(&["--resume"]),
            cwd: Arg::Words(&["--cwd"]),
            env: &[],
        },
        acp: &[Acp::Builtin {
            args: &["agent", "stdio"],
            evidence: Evidence::Help {
                args: &["agent", "--help"],
                needle: "stdio",
            },
            env: &[],
        }],
        hooks: no_hooks("Grok Build's help shows no flag that adds hooks for one launch."),
        sessions: Sessions {
            crossing: Crossing::SameId,
            note: "Verified here on 1.0.70: an ACP session is stored as ~/.grok/sessions/<cwd>/<id>, where the terminal's `--resume <id>` looks (UUID-shaped values are ids, `grok help`); the terminal resume itself was not started.",
        },
        orchestration: Orchestrate {
            attach: Attach::GrokRules,
            terminal: Guidance::Context,
            chat: Guidance::Skill,
            note: "`--rules <text>` appends the guidance to the system prompt. The terminal interface takes no MCP server or plugin for one launch (`grok mcp add` and `grok plugin install` write its configuration). Its ACP mode takes `--plugin-dir`, which loads a Claude Code layout plugin and is \"always trusted\" (`grok help agent`): verified here, the skill was offered and the server sent in `session/new` started with the pane's environment.",
        },
        screen: Screen {
            working: COMMON_WORKING,
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Attention,
            working_line: None,
        },
        sources: "Flags from `agent --help`, `agent agent --help`, `grok help`, `grok help agent` and `grok plugin validate` 1.0.70 (run here; this is what `agent` is on this machine). ACP: `initialize` and `session/new` with an MCP server and `--plugin-dir` against `grok agent stdio` (run here): loadSession, resume, http and sse MCP, no systemPrompt.",
    },
    Spec {
        id: "shell",
        name: "Shell",
        code: "sh",
        // Resolved from $SHELL, not from PATH.
        binaries: &[],
        version: &[],
        minimum: None,
        login: Login::Unknown,
        tui: Tui {
            args: &[],
            prompt: Arg::Typed,
            model: Arg::No,
            resume: Arg::No,
            cwd: Arg::No,
            env: &[],
        },
        acp: &[],
        hooks: no_hooks("A shell has no turns; output and progress reports are all there is."),
        sessions: Sessions {
            crossing: Crossing::None,
            note: "A shell has no conversation.",
        },
        orchestration: Orchestrate {
            attach: Attach::None,
            terminal: Guidance::Nothing,
            chat: Guidance::Nothing,
            note: "A shell already has the pane's variables and the app's command line; there is nothing to attach.",
        },
        screen: Screen {
            working: &[],
            blocked: &[],
            title_working: &[],
            title_blocked: &[],
            output: true,
            notification: Notice::Finished,
            working_line: None,
        },
        sources: "$SHELL.",
    },
];

/// Every harness kou knows, in display order.
pub fn specs() -> &'static [Spec] {
    SPECS
}

/// The harness with this id.
pub fn spec(id: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|spec| spec.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_required_harnesses_are_described_once_each() {
        let ids: Vec<_> = specs().iter().map(|spec| spec.id).collect();
        for id in [
            "claude", "codex", "gemini", "opencode", "cursor", "amp", "droid", "pi", "aider",
            "goose", "qwen", "copilot", "kiro", "kou", "shell",
        ] {
            assert!(spec(id).is_some(), "{id} is missing");
        }
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert!(spec("emacs").is_none());
    }

    #[test]
    fn specs_are_coherent() {
        for spec in specs() {
            let letters = spec.code.chars().count();
            assert!(
                (1..=2).contains(&letters),
                "{}: code {:?}",
                spec.id,
                spec.code
            );
            assert!(
                !spec.name.is_empty() && !spec.sources.is_empty(),
                "{}",
                spec.id
            );
            assert!(!spec.hooks.note.is_empty(), "{}", spec.id);
            // Hooks that are wired report something, and the other way round.
            assert_eq!(
                spec.hooks.wiring == Wiring::None,
                spec.hooks.coverage == Coverage::None,
                "{}",
                spec.id
            );
            // Without hooks or dependable screen text, output is all there is.
            if spec.hooks.coverage == Coverage::None
                && spec.screen.working.is_empty()
                && spec.screen.title_working.is_empty()
            {
                assert!(spec.screen.output, "{} could never look busy", spec.id);
            }
            for pattern in spec
                .screen
                .working
                .iter()
                .chain(spec.screen.blocked)
                .chain(spec.screen.title_working)
                .chain(spec.screen.title_blocked)
            {
                assert_eq!(*pattern, pattern.to_lowercase(), "{}: {pattern:?}", spec.id);
            }
            // A positional prompt and a positional directory cannot both be last.
            assert!(
                !(spec.tui.prompt == Arg::Positional && spec.tui.cwd == Arg::Positional),
                "{}",
                spec.id
            );
            assert!(
                !spec.sessions.note.is_empty() && !spec.orchestration.note.is_empty(),
                "{}",
                spec.id
            );
            // A conversation crosses only between two interfaces that exist.
            if spec.sessions.crossing != Crossing::None {
                assert!(
                    !spec.acp.is_empty() && spec.tui.resume != Arg::No,
                    "{}",
                    spec.id
                );
            }
            // What reaches the model follows from the mechanism.
            let terminal = spec.orchestration.terminal;
            match spec.orchestration.attach {
                Attach::None | Attach::SystemDefaults | Attach::GooseExtension => {
                    assert_eq!(terminal, Guidance::Nothing, "{}", spec.id)
                }
                Attach::ClaudeFlags => assert_eq!(terminal, Guidance::Skill, "{}", spec.id),
                _ => assert_eq!(terminal, Guidance::Context, "{}", spec.id),
            }
            if spec.acp.is_empty() {
                assert_eq!(spec.orchestration.chat, Guidance::Nothing, "{}", spec.id);
            }
            // The system-defaults attachment rides on the hooks' file.
            if spec.orchestration.attach == Attach::SystemDefaults {
                assert!(
                    matches!(spec.hooks.wiring, Wiring::SystemDefaults { .. }),
                    "{}",
                    spec.id
                );
            }
            for acp in spec.acp {
                if let Acp::Adapter {
                    binaries,
                    package: Some(package),
                    ..
                } = acp
                {
                    // The program the install puts down is the one detection looks for first.
                    assert_eq!(binaries.first(), Some(&package.binary), "{}", spec.id);
                    assert!(package.megabytes.0 <= package.megabytes.1, "{}", spec.id);
                    assert!(
                        package.version.split('.').count() == 3,
                        "{}: pin an exact version",
                        spec.id
                    );
                }
            }
        }
    }
}
