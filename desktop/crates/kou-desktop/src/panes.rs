//! What runs in a pane: finding the harnesses, starting one in a terminal,
//! following whether it is busy, and answering the other panes' requests.

use crate::{
    shell::{HarnessInfo, Shell},
    state::{Id, PaneKind},
};
use gpui::{AppContext as _, Context, Entity, Hsla, Subscription, Window, px, rgb};
use kou_agents::{Activity, Detected, PaneContext, Readiness, Signal, Tracker, TuiOptions};
use kou_ipc::{Incoming, Placement, Reply, Request, Response, Until};
use kou_term::{Spawn, Terminal, TerminalEvent, TerminalStyle, TerminalView};
use kou_thread::ThreadState;
use kou_thread_view::{
    Beside, Header, Nested, NestedState, ThreadView, ThreadViewEvent, ThreadViewRequest,
};
use kou_ui::{Mark, Theme, metric, theme};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// The session a user means by what they typed after `/resume`: its id, the
// start of its id, or its title, whichever is found first in that order.
fn seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The lines added and removed that `git diff --shortstat` names:
/// " 3 files changed, 12 insertions(+), 4 deletions(-)". Nothing changed
/// prints nothing.
fn changed_lines(shortstat: &str) -> (u32, u32) {
    let count = |word: &str| {
        shortstat
            .split(',')
            .find(|part| part.contains(word))
            .and_then(|part| part.split_whitespace().next()?.parse().ok())
            .unwrap_or(0)
    };
    (count("insertion"), count("deletion"))
}

fn past_of_kou(session: kou_native::SessionSummary) -> crate::shell::Past {
    crate::shell::Past {
        id: session.id,
        title: session.title,
        updated: Some(session.updated),
        running: session.running,
        prompts: Some(session.messages),
    }
}

fn pick_session(sessions: &[crate::shell::Past], which: &str) -> Option<String> {
    let which = which.trim();
    let wanted = which.to_lowercase();
    sessions
        .iter()
        .find(|session| session.id == which)
        .or_else(|| {
            sessions
                .iter()
                .find(|session| which.len() >= 6 && session.id.starts_with(which))
        })
        .or_else(|| {
            sessions
                .iter()
                .find(|session| session.title.to_lowercase() == wanted)
        })
        .or_else(|| {
            sessions.iter().find(|session| {
                !wanted.is_empty() && session.title.to_lowercase().contains(&wanted)
            })
        })
        .map(|session| session.id.clone())
}

/// The header and the footer of a canvas node, which a terminal inside it
/// does not get: `HEAD` and `FOOT` in `kou-canvas/src/card.rs`.
const NODE_FRAME: f32 = 33. + 22.;

/// A program that fails sooner than this after it was started never ran:
/// its pane stays, to say so.
const NEVER_RAN: Duration = Duration::from_secs(2);

/// How long a pane that was just handed a prompt counts as busy before its
/// harness has shown it: without this, a wait started right after a send
/// would return at once.
const EXPECTED: Duration = Duration::from_secs(8);
/// How long after the output a prompt caused the pane may stop counting as
/// busy on that prompt's account. The tracker's own quiet period still applies.
const ANSWERED: Duration = Duration::from_millis(1500);
/// How often a shell that is running a command is asked whether it still is.
const SHELL_POLL: Duration = Duration::from_millis(400);
/// How long a shell that marks its commands is given to say that the one it
/// was just handed is over, before it is judged by its prompt alone: a line
/// that runs nothing (one left open for more) never says so.
const SHELL_STARTS: Duration = Duration::from_secs(5);
/// How long a shell the app has asked to mark its prompts is given to show
/// its first one before it is taken to be up anyway: a shell with a long
/// start-up file on a busy machine takes its time, and text sent to it
/// before then would be judged by a prompt that is not there yet.
const SHELL_COMES_UP: Duration = Duration::from_secs(15);
/// How long output has to pause before the screen is read for what it says.
const SETTLE: Duration = Duration::from_millis(250);

/// A pane with a process in it.
pub(crate) struct Running {
    terminal: Entity<Terminal>,
    view: Entity<TerminalView>,
    tracker: Tracker,
    // When the pane was last handed a prompt it has not been seen to act on.
    expected: Option<Instant>,
    // For a shell that marks its commands: how many it will have said are
    // over once the prompt it was handed has been answered. No waiting on
    // how fast the shell got to it.
    finished_target: Option<u64>,
    // The shell was started with the app's prompt marks.
    marks: bool,
    // The shell's process, for a plain shell: asked whether it runs a command.
    job: Option<u32>,
    // A first prompt is waiting for the program to come up before it is typed.
    typing: bool,
    // The shell has been seen at a prompt of the kind that announces itself
    // (see `Terminal::takes_bracketed_paste`); from then on, not being at one
    // means busy, which covers a shell still running its startup files.
    prompts: bool,
    at_prompt: bool,
    launched: Instant,
    // Bumped by each output event; a settle timer acts only if it is still current.
    generation: u64,
    _subscription: Subscription,
}

// Bytes as text, for data that is kept as JSON.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

/// The programs [`Shell::release_panes`] let go, by pane, as the terminal
/// crate takes them back. One that cannot be read is left out: without its
/// descriptor there is nothing to adopt and nothing to end.
pub(crate) fn released_terminals(kept: &serde_json::Value) -> Vec<(Id, kou_term::Released)> {
    let Some(terminals) = kept.get("terminals").and_then(|kept| kept.as_object()) else {
        return Vec::new();
    };
    terminals
        .iter()
        .filter_map(|(pane, kept)| {
            let number = |key: &str| kept.get(key)?.as_u64();
            Some((
                pane.clone(),
                kou_term::Released {
                    master: i32::try_from(kept.get("master")?.as_i64()?).ok()?,
                    pid: u32::try_from(number("pid")?).ok()?,
                    columns: u16::try_from(number("columns")?).ok()?,
                    lines: u16::try_from(number("lines")?).ok()?,
                    cell_width: u16::try_from(number("cell_width")?).ok()?,
                    cell_height: u16::try_from(number("cell_height")?).ok()?,
                    screen: unhex(kept.get("screen")?.as_str()?)?,
                    title: kept
                        .get("title")
                        .and_then(|title| title.as_str())
                        .map(str::to_owned),
                    cwd: kept
                        .get("cwd")
                        .and_then(|cwd| serde_json::from_value(cwd.clone()).ok()),
                },
            ))
        })
        .collect()
}

/// A pane with a native conversation in it.
pub(crate) struct Talking {
    view: Entity<ThreadView>,
    state: ThreadState,
    _subscription: Subscription,
    _requests: Subscription,
}

impl Talking {
    /// The conversation's view.
    pub(crate) fn view(&self) -> &Entity<ThreadView> {
        &self.view
    }

    /// Whether its backend is gone, so the pane can offer to start it again.
    pub(crate) fn closed(&self) -> bool {
        matches!(self.state, ThreadState::Closed(_))
    }
}

impl Running {
    // A shell that announces its prompt has started once it has shown one; one
    // that does not is taken to have started after a moment.
    fn started(&self) -> bool {
        let patience = if self.marks {
            SHELL_COMES_UP
        } else {
            Duration::from_millis(1500)
        };
        self.prompts || self.launched.elapsed() > patience
    }

    // How long a prompt may go unanswered before the pane stops counting as
    // busy for it. A shell starts its command at once; an agent may take a while
    // to show that it is thinking.
    fn grace(&self) -> Duration {
        match (self.job, self.finished_target) {
            (Some(_), Some(_)) => SHELL_STARTS,
            (Some(_), None) => SHELL_POLL,
            (None, _) => EXPECTED,
        }
    }
}

/// A request that is waiting for panes to reach a state.
pub(crate) struct Wait {
    panes: Vec<Id>,
    until: Until,
    any: bool,
    deadline: Option<Instant>,
    reply: Reply,
}

/// What the interface lists for a detected harness.
pub(crate) fn info(detected: &Detected) -> HarnessInfo {
    HarnessInfo {
        id: detected.spec.id.to_owned(),
        name: detected.spec.name.to_owned(),
        code: detected.spec.code.to_owned(),
        ready: matches!(detected.readiness, Readiness::Ready),
        version: detected.version.clone(),
        // kou's own agent is native through its runner; the others through ACP.
        native: if detected.spec.id == "kou" {
            kou_native::locate_runner().is_some()
        } else {
            detected.native
        },
        adapter: match &detected.chat {
            kou_agents::Chat::NeedsAdapter { message, install } => {
                Some((message.clone(), install.as_deref().cloned()))
            }
            _ => None,
        },
        message: detected
            .message
            .clone()
            .or_else(|| match &detected.readiness {
                Readiness::Ready => None,
                Readiness::NeedsLogin => Some("needs you to log in".to_owned()),
                Readiness::Outdated { minimum } => {
                    Some(format!("needs version {minimum} or later"))
                }
                Readiness::Missing => Some("not installed".to_owned()),
                Readiness::Broken(reason) => Some(reason.clone()),
            }),
    }
}

/// The terminal's colours and font, from the app's theme.
pub(crate) fn terminal_style(theme: &Theme, smooth_cursor: bool) -> TerminalStyle {
    TerminalStyle {
        font_family: theme.mono_font.clone(),
        font_size: px(13.),
        // The reference's terminal: roomy rows, which is also what box drawing needs.
        line_height: 1.4,
        foreground: theme.fg,
        background: theme.pane,
        cursor: theme.fg,
        selection: theme.accent.opacity(0.3),
        ansi: theme.ansi.0.map(|color| Hsla::from(rgb(color))),
        padding: px(12.),
        cursor_motion: smooth_cursor.then(kou_term::CursorMotion::default),
    }
}

fn mark_of(activity: Activity) -> Mark {
    match activity {
        Activity::Idle => Mark::Idle,
        Activity::Working => Mark::Working,
        Activity::Blocked => Mark::Blocked,
    }
}

impl Shell {
    /// Look for harnesses again, off the main thread: each probe runs a process.
    pub(crate) fn scan(&mut self, cx: &mut Context<Self>) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        cx.notify();
        // Chat adapters the app installed itself are in a folder of its own,
        // looked in before the user's PATH.
        let adapters = self.adapters();
        let found = cx.background_spawn(async move { kou_agents::detect_with(&adapters) });
        cx.spawn(async move |this, cx| {
            let detected = found.await;
            this.update(cx, |this, cx| {
                this.scanning = false;
                this.harnesses = detected.iter().map(info).collect();
                this.detected = detected;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Start the harness chosen in the picker, in the picker's session.
    pub(crate) fn start(
        &mut self,
        harness: &str,
        kind: PaneKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (session, parent, tab, beside) = match &self.picker {
            Some(picker) => (
                picker.session.clone(),
                picker.parent.clone(),
                picker.tab.clone(),
                picker.beside.clone(),
            ),
            None => match self.store.selected.clone() {
                Some(session) => (session, None, None, None),
                None => return,
            },
        };
        let beside = beside.filter(|(at, _)| self.store.pane(at).is_some());
        let pane = match (beside, tab.filter(|tab| self.store.tab(tab).is_some())) {
            // Chosen for the new half of a split: it takes that half, and
            // works where the pane it was split from works.
            (Some((at, axis)), _) => {
                let cwd = self.store.pane(&at).map(|pane| pane.cwd.clone());
                let pane = self.add_pane_at(
                    &session,
                    harness,
                    parent,
                    kind,
                    crate::shell::Place::Auto,
                    crate::state::Seat::Beside { at, axis },
                    window,
                    cx,
                );
                if let (Some(pane), Some(cwd)) = (&pane, cwd)
                    && let Some(saved) = self.store.pane_mut(pane)
                {
                    saved.cwd = cwd;
                }
                pane
            }
            // Asked for on a tab's canvas, it is a node of that canvas.
            (None, Some(tab)) => self.add_pane_at(
                &session,
                harness,
                parent,
                kind,
                crate::shell::Place::Auto,
                crate::state::Seat::In(tab),
                window,
                cx,
            ),
            (None, None) => self.add_pane(&session, harness, parent, kind, window, cx),
        };
        if let Some(pane) = pane {
            // What was sent from the empty screen's field is its first
            // prompt: its words here, and for a conversation its blocks
            // (pictures, files), taken where the conversation is started.
            let prompt = self
                .first_prompt
                .as_ref()
                .map(|(text, _)| text.trim().to_owned())
                .filter(|prompt| !prompt.is_empty());
            self.ask_choice = None;
            self.ask_menu = false;
            // The model chosen in the field, for this harness, is the
            // pane's. ("default" is the harness's own choice: nothing to say.)
            if let Some((of, model)) = self.ask_model.take()
                && of == harness
                && model != "default"
                && let Some(saved) = self.store.pane_mut(&pane)
            {
                saved.model = Some(model);
            }
            // The account it was asked to run under, when that is one of
            // this harness.
            let profile = self.launch_profile.take().filter(|id| {
                self.store
                    .profile(id)
                    .is_some_and(|profile| profile.harness == harness)
            });
            self.ask_profile = None;
            if let Some(saved) = self.store.pane_mut(&pane) {
                saved.profile.clone_from(&profile);
            }
            if harness != "shell" {
                self.store.last_harness = Some(harness.to_owned());
                self.store.last_profile = profile;
            }
            self.launch(&pane, prompt, window, cx);
            self.first_prompt = None;
            self.save(cx);
        }
    }

    /// What a harness started in a pane is told about where it runs, and
    /// what it is given to work with the app's other agents (see
    /// `state::Orchestration`): attached to this launch, never written into
    /// the harness's own configuration.
    pub(crate) fn pane_context(&self, saved: &crate::state::Pane) -> anyhow::Result<PaneContext> {
        use crate::state::Orchestration as Way;
        let bin = match &self.bin {
            Some(bin) => bin.clone(),
            None => std::env::current_exe()?,
        };
        let orchestration = match (self.store.orchestration, &self.guidance) {
            // The app's tools, and the guidance that speaks of tools.
            (Way::Mcp, Some(guidance)) => Some(kou_agents::Orchestration {
                mcp: Some(kou_agents::McpServer {
                    name: "kou".to_owned(),
                    command: bin.clone(),
                    args: vec!["mcp".to_owned()],
                    env: Vec::new(),
                }),
                claude_plugin: Some(guidance.tools.claude_plugin.clone()),
                // The short text, which names the long guide's file.
                instructions: Some(guidance.tools.brief.clone()),
            }),
            // Codex runs an agent's shell in a sandbox that cannot reach the
            // app's socket, so its command line is no use there; an MCP
            // server it starts outside that sandbox. It gets the tools.
            (Way::Skills, Some(guidance)) if saved.harness == "codex" => {
                Some(kou_agents::Orchestration {
                    mcp: Some(kou_agents::McpServer {
                        name: "kou".to_owned(),
                        command: bin.clone(),
                        args: vec!["mcp".to_owned()],
                        env: Vec::new(),
                    }),
                    claude_plugin: Some(guidance.tools.claude_plugin.clone()),
                    instructions: Some(guidance.tools.brief.clone()),
                })
            }
            // No server: the guidance that speaks of the command line, which
            // the pane's environment already lets the agent use.
            (Way::Skills, Some(guidance)) => Some(kou_agents::Orchestration {
                mcp: None,
                claude_plugin: Some(guidance.commands.claude_plugin.clone()),
                instructions: Some(guidance.commands.brief.clone()),
            }),
            (Way::Off, _) | (_, None) => None,
        };
        Ok(PaneContext {
            pane: saved.id.clone(),
            session: saved.session.clone(),
            socket: self.socket.clone(),
            bin,
            cwd: saved.cwd.clone(),
            state_dir: self.home.join("panes").join(&saved.id),
            orchestration,
            env: self.account_env(&saved.harness, saved.profile.as_deref()),
        })
    }

    /// What a harness's environment gets to run as another account of it
    /// (see [`crate::state::Profile`]): its home variable, set to that
    /// account's folder. Nothing for the harness's own account, for an
    /// account that is gone, and for a harness whose home cannot be moved.
    pub(crate) fn account_env(
        &self,
        harness: &str,
        profile: Option<&str>,
    ) -> Vec<(String, String)> {
        profile
            .and_then(|id| self.store.profile(id))
            .filter(|profile| profile.harness == harness)
            .zip(kou_usage::home_variable(harness))
            .map(|(profile, variable)| {
                (
                    variable.to_owned(),
                    profile.home.to_string_lossy().into_owned(),
                )
            })
            .into_iter()
            .collect()
    }

    /// How a harness opens when nothing says otherwise: as a chat in the
    /// app's own view when it can be driven that way here, in the terminal
    /// interface it ships otherwise. A shell is a terminal.
    pub(crate) fn default_kind(&self, harness: &str) -> PaneKind {
        if harness != "shell" && self.harness(harness).is_some_and(|harness| harness.native) {
            PaneKind::Thread
        } else {
            PaneKind::Terminal
        }
    }

    /// Where the chat adapters the app installs are kept: a folder of its
    /// own under its home, nothing shared with the rest of the machine.
    pub(crate) fn adapters(&self) -> std::path::PathBuf {
        self.home.join("adapters")
    }

    /// Install the adapter a harness's chat goes through, after asking: the
    /// program that does it and what it downloads are the harness
    /// description's (`kou_agents::Install`), the decision is the user's.
    pub(crate) fn install_adapter(
        &mut self,
        harness: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(install) = self
            .harness(harness)
            .and_then(|harness| harness.adapter.as_ref()?.1.clone())
        else {
            return;
        };
        if self.installing.is_some() {
            return;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Info,
            &format!("Install the chat adapter for {}?", install.harness),
            Some(&format!(
                "{} About {} MB, from {}, into {}. Nothing outside that folder is changed.",
                install.summary,
                install.megabytes,
                install.source,
                install.writes.display()
            )),
            &["Install", "Not now"],
            cx,
        );
        let harness = harness.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let started = this.update(cx, |this, cx| {
                this.installing = Some(harness.clone());
                this.notice = None;
                cx.notify();
            });
            if started.is_err() {
                return;
            }
            let run = install.clone();
            let outcome = cx
                .background_spawn(async move {
                    std::fs::create_dir_all(&run.cwd)?;
                    let output = std::process::Command::new(&run.program)
                        .args(&run.args)
                        .current_dir(&run.cwd)
                        .stdin(std::process::Stdio::null())
                        .output()?;
                    anyhow::ensure!(
                        output.status.success() && run.installed(),
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                            .lines()
                            .rev()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or("the install did not leave the adapter where it should be")
                            .to_owned()
                    );
                    anyhow::Ok(())
                })
                .await;
            this.update(cx, |this, cx| {
                this.installing = None;
                if let Err(error) = outcome {
                    this.notice = Some(
                        format!(
                            "The chat adapter for {} was not installed: {error:#}",
                            install.harness
                        )
                        .into(),
                    );
                }
                // Found by the same search that finds everything else.
                this.scan(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open a pane the other way: a chat as its harness's terminal
    /// interface, a terminal as a chat. The conversation goes with it where
    /// the harness keeps one session for both; otherwise it starts anew.
    /// False when there is no other way to open this pane here.
    pub(crate) fn switch_interface(
        &mut self,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(saved) = self.store.pane(pane).cloned() else {
            return false;
        };
        let other = match saved.kind {
            PaneKind::Thread => PaneKind::Terminal,
            PaneKind::Terminal => PaneKind::Thread,
        };
        let can = saved.harness != "shell"
            && match other {
                PaneKind::Thread => self
                    .harness(&saved.harness)
                    .is_some_and(|harness| harness.native),
                PaneKind::Terminal => self
                    .harness(&saved.harness)
                    .is_some_and(|harness| harness.ready),
            };
        if !can {
            return false;
        }
        // A conversation the harness runs in the background cannot be
        // resumed while it runs there: it is stopped there first, and then
        // goes on here as a chat.
        if other == PaneKind::Thread
            && let Some(session) = saved.resume.as_deref()
            && let Some(open) =
                self.background_of(&saved.harness, saved.profile.as_deref(), session)
        {
            self.out_of_background(pane, open, None, window, cx);
            return true;
        }
        self.switch_now(pane, window, cx)
    }

    // The switch itself, once nothing stands in its way.
    fn switch_now(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(saved) = self.store.pane(pane).cloned() else {
            return false;
        };
        let other = match saved.kind {
            PaneKind::Thread => PaneKind::Terminal,
            PaneKind::Terminal => PaneKind::Thread,
        };
        // A chat nobody has said anything in is not a conversation the
        // terminal could resume: most harnesses save a session with its
        // first turn. Its terminal starts anew.
        let said = match saved.kind {
            PaneKind::Thread => self
                .threads
                .get(pane)
                .is_none_or(|talking| talking.view.read(cx).has_conversation()),
            PaneKind::Terminal => true,
        };
        // The session under the name the other interface resumes it by.
        let spec = self
            .detected
            .iter()
            .find(|detected| detected.spec.id == saved.harness)
            .map(|detected| detected.spec);
        let resume = match (spec, saved.resume.as_deref().filter(|_| said), other) {
            (Some(spec), Some(session), PaneKind::Terminal) => {
                kou_agents::terminal_resume(spec, session)
            }
            (Some(spec), Some(session), PaneKind::Thread) => kou_agents::chat_resume(spec, session),
            _ => None,
        };
        self.forget(pane, cx);
        if let Some(saved) = self.store.pane_mut(pane) {
            saved.kind = other;
            saved.resume = resume;
        }
        self.save(cx);
        self.launch(pane, None, window, cx);
        true
    }

    /// Start a stopped pane's harness again, resuming its session when the harness can.
    pub(crate) fn restart(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.launch(pane, None, window, cx);
    }

    /// Run a pane's harness in a terminal. A failure is shown in the pane.
    pub(crate) fn launch(
        &mut self,
        pane: &str,
        prompt: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let thread = self
            .store
            .pane(pane)
            .is_some_and(|pane| pane.kind == PaneKind::Thread);
        let launched = if thread {
            self.try_launch_thread(pane, prompt, window, cx)
        } else {
            self.try_launch(pane, prompt, window, cx)
        };
        if let Err(error) = launched {
            self.running.remove(pane);
            self.threads.remove(pane);
            self.stop_pane(pane, &format!("It could not be started: {error:#}"), cx);
            if let Some(live) = self.live.get_mut(pane) {
                live.mark = Mark::Failed;
            }
        }
    }

    fn try_launch(
        &mut self,
        pane: &str,
        prompt: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let saved = self
            .store
            .pane(pane)
            .ok_or_else(|| anyhow::anyhow!("the pane is gone"))?
            .clone();
        // A session its harness runs in the background is attached to, not
        // resumed: resuming it is refused while it runs there.
        let attach = saved.resume.as_deref().and_then(|session| {
            self.background_name(&saved.harness, saved.profile.as_deref(), session)
        });
        let detected = self
            .detected
            .iter()
            .find(|detected| detected.spec.id == saved.harness)
            .ok_or_else(|| anyhow::anyhow!("{} was not found on this machine", saved.harness))?;
        let context = self.pane_context(&saved)?;
        let options = TuiOptions {
            prompt,
            // A harness that takes no model when it starts keeps its own.
            model: saved
                .model
                .clone()
                .filter(|_| detected.spec.tui.model != kou_agents::Arg::No),
            resume: saved.resume.clone().filter(|_| attach.is_none()),
            attach,
        };
        let launch = kou_agents::tui_launch(detected, &context, &options)?;
        // A harness with no prompt argument gets its prompt typed once it is up.
        let typed = kou_agents::typed_prompt(detected, &options);
        // A shell is asked to mark its prompts, so a resize does not leave
        // pieces of old ones behind; see `integration.rs`. Without the marks it
        // still runs, so a failure to write the file is not a failure to launch.
        let mut env = launch.env;
        let kept = std::env::var("ZDOTDIR").ok();
        let mut marked = false;
        match crate::integration::environment(&launch.program, &self.home, kept) {
            Ok(marks) => {
                marked = !marks.is_empty();
                env.extend(marks);
            }
            Err(error) => log::warn!("shell integration was not set up: {error}"),
        }
        let terminal = Terminal::open(
            Spawn {
                program: launch.program,
                args: launch.args,
                env,
                cwd: launch.cwd,
            },
            cx,
        )?;
        self.install_terminal(
            &saved,
            terminal,
            options.prompt.is_some(),
            typed,
            false,
            window,
            cx,
        );
        if let Some(running) = self.running.get_mut(pane) {
            running.marks = marked && running.job.is_some();
        }
        Ok(())
    }

    /// Put a terminal with its program in a pane: its view, what the app
    /// reads from it, its place on the canvas. `prompted` when it was started
    /// with a prompt it is expected to act on, `typed` a prompt to type once
    /// it is up, `adopted` when its program was running before this terminal
    /// was (see [`Shell::adopt_panes`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn install_terminal(
        &mut self,
        saved: &crate::state::Pane,
        terminal: Entity<Terminal>,
        prompted: bool,
        typed: Option<String>,
        adopted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let style = terminal_style(theme(cx), self.store.smooth_cursor);
        // The grid the pane will have, before its first layout says so exactly:
        // a shell prints its greeting at once, and at the default 80 columns
        // that would be wrapped wrongly for good. A program that was running
        // already keeps the grid it has until the layout says otherwise.
        if !adopted {
            let (columns, lines) =
                self.grid_estimate(&saved.session, &saved.id, &style, window, cx);
            terminal.read(cx).resize(columns, lines);
        }
        let view = cx.new(|cx| TerminalView::new(terminal.clone(), style, window, cx));
        let id = saved.id.clone();
        let subscription = cx.subscribe_in(
            &terminal,
            window,
            move |this, _, event: &TerminalEvent, window, cx| {
                crate::hot::hot!(this.terminal_event(&id, event, window, cx));
            },
        );
        let now = Instant::now();
        self.running.insert(
            saved.id.clone(),
            Running {
                terminal: terminal.clone(),
                view: view.clone(),
                tracker: Tracker::new(&saved.harness, now),
                expected: prompted.then_some(now),
                finished_target: None,
                marks: false,
                job: (saved.harness == "shell")
                    .then(|| terminal.read(cx).pid())
                    .flatten(),
                typing: typed.is_some(),
                prompts: false,
                at_prompt: false,
                // A program that ends just after it was adopted did not
                // "never run": it ran before.
                launched: if adopted {
                    now.checked_sub(NEVER_RAN * 2).unwrap_or(now)
                } else {
                    now
                },
                generation: 0,
                _subscription: subscription,
            },
        );
        self.set_content(&saved.id, view.clone().into(), cx);
        if let Some(live) = self.live.get_mut(&saved.id) {
            live.changed = now;
        }
        if self.is_focused(&saved.id) {
            window.focus(&gpui::Focusable::focus_handle(view.read(cx), cx), cx);
        }
        if let Some(text) = typed {
            self.type_when_ready(&saved.id, text, window, cx);
        }
        self.show_on_canvas(&saved.id, cx);
        self.refresh(&saved.id, window, cx);
    }

    /// The terminal in a pane, when a program runs there.
    #[cfg(test)]
    pub(crate) fn terminal_of(&self, pane: &str) -> Option<Entity<Terminal>> {
        self.running
            .get(pane)
            .map(|running| running.terminal.clone())
    }

    /// Let go of what runs in the panes without ending it, for a window put
    /// up again to carry on with ([`Shell::adopt_panes`]): each terminal's
    /// program (see `kou_term::Released`), which panes held a conversation,
    /// and what each agent was doing. As plain data.
    pub(crate) fn release_panes(&mut self, cx: &mut Context<Self>) -> serde_json::Value {
        use serde_json::{Map, Value, json};
        // Every reader is told to stop first: each takes a moment to notice,
        // and this way they notice together.
        for running in self.running.values() {
            running.terminal.read(cx).stop_reading();
        }
        let mut terminals = Map::new();
        for (pane, running) in &self.running {
            let released = running
                .terminal
                .update(cx, |terminal, _| terminal.release());
            if let Some(released) = released {
                terminals.insert(
                    pane.clone(),
                    json!({
                        "master": released.master,
                        "pid": released.pid,
                        "columns": released.columns,
                        "lines": released.lines,
                        "cell_width": released.cell_width,
                        "cell_height": released.cell_height,
                        "screen": hex(&released.screen),
                        "title": released.title,
                        "cwd": released.cwd,
                    }),
                );
            }
        }
        let marks: Map<String, Value> = self
            .live
            .iter()
            .map(|(pane, live)| {
                let mark = match live.mark {
                    Mark::Working => "working",
                    Mark::Blocked => "blocked",
                    Mark::Failed => "failed",
                    Mark::Done => "done",
                    Mark::Idle => "idle",
                };
                (pane.clone(), json!({ "mark": mark, "seen": live.seen }))
            })
            .collect();
        json!({
            "terminals": terminals,
            "threads": self.threads.keys().collect::<Vec<_>>(),
            "marks": marks,
        })
    }

    /// Carry on with what [`Shell::release_panes`] let go: each program in
    /// a terminal of this build's making, in the pane it was in. A
    /// conversation is started again where it left off, by its session, once
    /// the harnesses have been found (see [`Shell::settle`]). A program whose
    /// pane is gone, or that cannot be adopted, is ended.
    pub(crate) fn adopt_panes(
        &mut self,
        kept: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (pane, released) in released_terminals(kept) {
            let Some(saved) = self.store.pane(&pane).cloned() else {
                released.discard();
                continue;
            };
            match Terminal::adopt(released, cx) {
                Ok(terminal) => {
                    self.install_terminal(&saved, terminal, false, None, true, window, cx);
                }
                Err(error) => self.stop_pane(&pane, &format!("{error:#}"), cx),
            }
        }
        if let Some(marks) = kept.get("marks").and_then(|marks| marks.as_object()) {
            for (pane, was) in marks {
                let Some(live) = self.live.get_mut(pane) else {
                    continue;
                };
                // What nothing runs in any more is not doing anything.
                if !matches!(live.content, crate::shell::Content::View(_)) {
                    continue;
                }
                live.mark = match was.get("mark").and_then(|mark| mark.as_str()) {
                    Some("working") => Mark::Working,
                    Some("blocked") => Mark::Blocked,
                    Some("failed") => Mark::Failed,
                    Some("done") => Mark::Done,
                    _ => Mark::Idle,
                };
                live.seen = was
                    .get("seen")
                    .and_then(|seen| seen.as_bool())
                    .unwrap_or(true);
            }
        }
        self.revive = kept
            .get("threads")
            .and_then(|threads| serde_json::from_value(threads.clone()).ok())
            .unwrap_or_default();
        cx.notify();
    }

    fn grid_estimate(
        &mut self,
        session: &str,
        pane: &str,
        style: &TerminalStyle,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> (usize, usize) {
        let viewport = window.viewport_size();
        let mut width = f32::from(viewport.width) - self.rail_width();
        let mut height = f32::from(viewport.height) - metric::BAR;
        // What is above and below the terminal inside its frame: a pane's
        // header in a tab; a node's header and footer on the canvas.
        let mut around = 34.;
        let on_canvas = self
            .store
            .tab_of(pane)
            .filter(|tab| tab.layout == crate::state::Layout::Canvas)
            .map(|tab| tab.id.clone());
        if let Some(tab) = on_canvas {
            // On the canvas it has its node's size, whatever the zoom: the
            // text scales with the zoom, the grid does not. The node is asked
            // of the canvas itself; the saved board learns of it a moment later.
            let node = self.store.pane(pane).map(|pane| pane.node);
            let rect = self.canvas(&tab, cx).zip(node).and_then(|(canvas, node)| {
                canvas.read(cx).board().node(node).map(|node| node.rect)
            });
            if let Some(rect) = rect {
                (width, height, around) = (rect.w, rect.h, NODE_FRAME);
            }
        } else if let Some(place) = self.store.session(session).and_then(|session| {
            // Its share of the tab it is in, as `workspace.rs` lays the splits out.
            let tab = &session.tabs[session.tab_of(pane)?];
            let (places, _) = tab.root.lay_out([0., 0., width, height], 1.);
            places
                .into_iter()
                .find(|(id, _)| id == pane)
                .map(|(_, place)| place)
        }) {
            (width, height) = (place[2], place[3]);
        }
        let size = f32::from(style.font_size);
        let padding = f32::from(style.padding) * 2.;
        // 0.6 of the size is the advance of the usual monospace faces.
        let columns = ((width - padding) / (size * 0.602)).floor();
        let lines = ((height - around - padding) / (size * style.line_height)).floor();
        (columns.max(20.) as usize, lines.max(5.) as usize)
    }

    fn try_launch_thread(
        &mut self,
        pane: &str,
        prompt: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let saved = self
            .store
            .pane(pane)
            .ok_or_else(|| anyhow::anyhow!("the pane is gone"))?
            .clone();
        let name = self
            .harness(&saved.harness)
            .map_or_else(|| saved.harness.clone(), |harness| harness.name.clone());
        let backend = if saved.harness == "kou" {
            let runner = kou_native::locate_runner()
                .ok_or_else(|| anyhow::anyhow!("kou-conveyor-runner was not found on PATH"))?;
            kou_native::connect(kou_native::Options {
                runner,
                cwd: saved.cwd.clone(),
                session: saved.resume.clone(),
                model: saved.model.clone(),
            })?
        } else {
            let detected = self
                .detected
                .iter()
                .find(|detected| detected.spec.id == saved.harness)
                .ok_or_else(|| anyhow::anyhow!("{name} was not found on this machine"))?;
            let context = self.pane_context(&saved)?;
            let launch = kou_agents::acp_launch(detected, &context)
                .ok_or_else(|| anyhow::anyhow!("{name} has no native mode installed"))?;
            // What this session is opened with: the app's tools and
            // guidance, attached the way this harness takes them.
            let setup = kou_agents::acp_setup(detected, &context);
            kou_acp::connect(
                kou_acp::Launch {
                    program: launch.program,
                    args: launch.args,
                    env: launch.env,
                    cwd: launch.cwd,
                },
                kou_acp::Options {
                    resume: saved.resume.clone(),
                    mcp_servers: setup
                        .mcp_servers
                        .into_iter()
                        .map(|server| kou_acp::McpServer::Stdio {
                            name: server.name,
                            command: server.command,
                            args: server.args,
                            env: server.env,
                        })
                        .collect(),
                    instructions: setup.instructions,
                    meta: setup.meta,
                    login_hint: setup.login_hint,
                    // How this agent is told where to branch a session, for
                    // editing a prompt already sent; none for most.
                    fork_meta: setup.fork_meta,
                    display_terminals: setup.display_terminals,
                    model: saved.model.clone(),
                    ..kou_acp::Options::new(
                        saved.cwd.clone(),
                        "kou-desktop",
                        env!("CARGO_PKG_VERSION"),
                    )
                },
            )?
        };
        let view = self.install_thread(&saved, backend, name, window, cx);
        // From the empty screen's field a prompt comes whole, pictures and
        // files included; from anywhere else it is words.
        match (self.first_prompt.take(), prompt) {
            (Some((_, blocks)), Some(_)) if blocks.len() > 1 => {
                view.update(cx, |view, cx| view.send_blocks(blocks, cx));
            }
            (_, Some(prompt)) => view.update(cx, |view, cx| view.send(&prompt, cx)),
            (_, None) => {}
        }
        Ok(())
    }

    /// Put a conversation over `backend` in a pane: its view, what the app
    /// answers for it, and its place on the canvas.
    pub(crate) fn install_thread(
        &mut self,
        saved: &crate::state::Pane,
        backend: kou_thread::Backend,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ThreadView> {
        let header = Header {
            agent: name.into(),
            // Its harness's tile, for the cards of the agents it starts
            // by itself.
            code: self
                .harness(&saved.harness)
                .map_or_else(
                    || saved.harness.to_uppercase(),
                    |harness| harness.code.clone(),
                )
                .into(),
            cwd: saved.cwd.clone(),
        };
        let history = (saved.harness != "kou").then(|| {
            self.home
                .join("history")
                .join(format!("{}.json", saved.session))
        });
        let view = cx.new(|cx| {
            let mut view = ThreadView::new(backend, header, window, cx);
            // The pane draws the header for every kind of pane.
            view.set_header_visible(false, cx);
            // `/new`, `/sessions`, `/resume`, `/rename` and `/settings` are
            // answered by the app, below.
            view.set_app_commands(true, cx);
            // kou keeps its prompt history in the project, shared with its
            // other cockpits. Another agent's project is not ours to write to.
            if let Some(history) = history {
                view.set_history_file(Some(history), cx);
            }
            // Every agent's conversation looks and works as kou's own does
            // (the view's cockpit look, which is what it starts in): one
            // design for a chat, whoever is on the other end.
            view
        });
        let id = saved.id.clone();
        let requests = cx.subscribe_in(
            &view,
            window,
            move |this, _, request: &ThreadViewRequest, window, cx| {
                crate::hot::hot!(this.thread_request(&id, request, window, cx));
            },
        );
        let id = saved.id.clone();
        let subscription = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &ThreadViewEvent, window, cx| {
                crate::hot::hot!(this.thread_event(&id, event, window, cx));
            },
        );
        self.threads.insert(
            saved.id.clone(),
            Talking {
                view: view.clone(),
                state: ThreadState::Starting,
                _subscription: subscription,
                _requests: requests,
            },
        );
        self.set_content(&saved.id, view.clone().into(), cx);
        if self.is_focused(&saved.id) {
            window.focus(&gpui::Focusable::focus_handle(view.read(cx), cx), cx);
        }
        self.show_on_canvas(&saved.id, cx);
        // Who it works with and where: the agents it started before (a
        // conversation resumed after a restart still has them), its folder.
        self.sync_started(&saved.id, cx);
        self.count_changes(&saved.id, cx);
        view
    }

    /// Remember that the agent in `parent` started the one in `child`, at
    /// this point of its conversation.
    pub(crate) fn note_started(&mut self, parent: &str, child: &str, cx: &mut Context<Self>) {
        let Some(started) = self.store.pane(child).map(|saved| crate::state::Started {
            id: saved.id.clone(),
            title: saved.title.clone(),
            harness: saved.harness.clone(),
            after: self
                .threads
                .get(parent)
                .map(|talking| talking.view.read(cx).thread().entries().len()),
            state: crate::state::StartedState::Running,
            since: seconds_now(),
            detail: String::new(),
        }) else {
            return;
        };
        if let Some(saved) = self.store.pane_mut(parent)
            && saved.started.iter().all(|known| known.id != started.id)
        {
            saved.started.push(started);
        }
        self.sync_started(parent, cx);
    }

    /// Bring what `parent` remembers of the agents it started in line with
    /// what they are doing, and show it in its conversation: their cards,
    /// who it waits for, and the card beside the transcript.
    pub(crate) fn sync_started(&mut self, parent: &str, cx: &mut Context<Self>) {
        use crate::state::StartedState as State;
        let Some(saved) = self.store.pane(parent).cloned() else {
            return;
        };
        let now = Instant::now();
        let mut started = saved.started.clone();
        for known in &mut started {
            let state = match self.store.pane(&known.id) {
                None => State::Gone,
                Some(child) => {
                    known.title = child.title.clone();
                    known.harness = child.harness.clone();
                    if let Some(words) = self.latest_words(&known.id, cx) {
                        known.detail = words;
                    }
                    // As the rail shows it.
                    match self.live.get(&known.id).map(|live| live.mark) {
                        Some(Mark::Working) => State::Running,
                        Some(Mark::Blocked) => State::Asking,
                        Some(Mark::Failed) => State::Failed,
                        _ => State::Finished,
                    }
                }
            };
            if state != known.state {
                known.state = state;
                known.since = seconds_now();
            }
        }
        if started != saved.started {
            if let Some(saved) = self.store.pane_mut(parent) {
                saved.started = started.clone();
            }
            self.save(cx);
        }
        let Some(view) = self.threads.get(parent).map(|talking| talking.view.clone()) else {
            return;
        };
        let nested: Vec<Nested> = started.iter().map(|known| self.nested(known)).collect();
        let beside = self.beside(&saved, now);
        view.update(cx, |view, cx| {
            view.set_nested(nested, cx);
            view.set_beside(Some(beside), cx);
        });
        self.sync_waiting(cx);
    }

    fn nested(&self, known: &crate::state::Started) -> Nested {
        use crate::state::StartedState as State;
        Nested {
            id: known.id.clone(),
            title: known.title.clone(),
            code: self.code_of(&known.harness),
            harness: known.harness.clone(),
            state: match known.state {
                State::Running => NestedState::Running,
                State::Asking => NestedState::Asking,
                State::Finished => NestedState::Finished,
                State::Failed => NestedState::Failed,
                State::Gone => NestedState::Gone,
            },
            detail: known.detail.clone(),
            since: UNIX_EPOCH + Duration::from_secs(known.since),
            after: known.after,
        }
    }

    // The two letters a harness goes by where it has no mark.
    fn code_of(&self, harness: &str) -> String {
        self.harness(harness).map_or_else(
            || harness.chars().take(2).collect::<String>().to_uppercase(),
            |harness| harness.code.clone(),
        )
    }

    // What stands beside a pane's conversation: its folder and branch, the
    // agent that started it, and how many of those it started are gone.
    fn beside(&self, saved: &crate::state::Pane, now: Instant) -> Beside {
        let parent = saved
            .parent
            .as_deref()
            .and_then(|parent| self.store.pane(parent))
            .map(|parent| {
                let live = self.live.get(&parent.id);
                Nested {
                    id: parent.id.clone(),
                    title: parent.title.clone(),
                    code: self.code_of(&parent.harness),
                    harness: parent.harness.clone(),
                    state: match live.map(|live| live.mark) {
                        Some(Mark::Working) => NestedState::Running,
                        Some(Mark::Blocked) => NestedState::Asking,
                        Some(Mark::Failed) => NestedState::Failed,
                        _ => NestedState::Finished,
                    },
                    detail: String::new(),
                    since: live.map_or_else(SystemTime::now, |live| {
                        SystemTime::now() - now.saturating_duration_since(live.changed)
                    }),
                    after: None,
                }
            });
        Beside {
            project: saved
                .cwd
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            path: saved.cwd.clone(),
            branch: crate::state::branch_of(&saved.cwd),
            changes: self.changed_lines.get(&saved.id).copied(),
            parent,
            // Every agent it started is passed to the view, the gone ones
            // too, and the view counts those that are over as its previous
            // agents: none are left for this count.
            earlier: 0,
        }
    }

    // An agent's latest words, on one line: the end of what its conversation
    // last answered. A terminal has no such thing to read.
    fn latest_words(&self, pane: &str, cx: &Context<Self>) -> Option<String> {
        let talking = self.threads.get(pane)?;
        let view = talking.view.read(cx);
        let said = view
            .thread()
            .entries()
            .iter()
            .rev()
            .find_map(|entry| match entry {
                kou_thread::Entry::Assistant(text) if !text.trim().is_empty() => Some(text),
                _ => None,
            })?;
        let line = said.lines().rev().find(|line| !line.trim().is_empty())?;
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(line.chars().take(240).collect())
    }

    /// Tell each conversation which of the agents it started its agent is
    /// waiting for: an open wait whose panes were all started by one pane is
    /// that pane's.
    pub(crate) fn sync_waiting(&mut self, cx: &mut Context<Self>) {
        let mut waited: std::collections::HashMap<String, Vec<String>> = Default::default();
        for wait in &self.waits {
            let mut parents = wait
                .panes
                .iter()
                .map(|pane| self.store.pane(pane).and_then(|saved| saved.parent.clone()));
            let Some(Some(parent)) = parents.next() else {
                continue;
            };
            if parents.all(|other| other.as_deref() == Some(parent.as_str())) {
                let ids = waited.entry(parent).or_default();
                for pane in &wait.panes {
                    if !ids.contains(pane) {
                        ids.push(pane.clone());
                    }
                }
            }
        }
        let panes: Vec<String> = self.threads.keys().cloned().collect();
        for pane in panes {
            let ids = waited.remove(&pane).unwrap_or_default();
            if self
                .waiting_on
                .get(&pane)
                .map_or(ids.is_empty(), |shown| *shown == ids)
            {
                continue;
            }
            self.waiting_on.insert(pane.clone(), ids.clone());
            if let Some(talking) = self.threads.get(&pane) {
                talking
                    .view
                    .clone()
                    .update(cx, |view, cx| view.set_waiting_on(ids, cx));
            }
        }
    }

    // Count the lines a pane's folder has changed against what its branch
    // follows (against its last commit without one), off the main thread,
    // for the card beside its conversation.
    fn count_changes(&mut self, pane: &str, cx: &mut Context<Self>) {
        let Some(cwd) = self.store.pane(pane).map(|saved| saved.cwd.clone()) else {
            return;
        };
        let counted = cx.background_executor().spawn(async move {
            let stat = |against: &str| {
                let output = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&cwd)
                    .args(["diff", "--shortstat", against])
                    .stdin(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .output()
                    .ok()?;
                output
                    .status
                    .success()
                    .then(|| changed_lines(&String::from_utf8_lossy(&output.stdout)))
            };
            stat("@{upstream}").or_else(|| stat("HEAD"))
        });
        let pane = pane.to_owned();
        cx.spawn(async move |this, cx| {
            let counted = counted.await;
            this.update(cx, |this, cx| {
                let known = this.changed_lines.get(&pane).copied();
                if counted == known {
                    return;
                }
                match counted {
                    Some(counted) => this.changed_lines.insert(pane.clone(), counted),
                    None => this.changed_lines.remove(&pane),
                };
                this.sync_started(&pane, cx);
            })
            .ok();
        })
        .detach();
    }

    // What a conversation's prompt field asks of the app.
    fn thread_request(
        &mut self,
        pane: &str,
        request: &ThreadViewRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match request {
            ThreadViewRequest::NewSession => self.reopen(pane, None, window, cx),
            ThreadViewRequest::ResumeSession(which) => {
                let Some(saved) = self.store.pane(pane).cloned() else {
                    return;
                };
                // kou's sessions can be asked for by title; another agent's
                // only by the id it gave.
                let id = if saved.harness == "kou" {
                    kou_native::sessions(&saved.cwd).ok().and_then(|sessions| {
                        let sessions: Vec<_> = sessions.into_iter().map(past_of_kou).collect();
                        pick_session(&sessions, which)
                    })
                } else {
                    Some(which.clone())
                };
                match id {
                    Some(id) => self.reopen(pane, Some(id), window, cx),
                    None => {
                        self.notice = Some(format!("No session called \"{which}\" here.").into());
                        cx.notify();
                    }
                }
            }
            ThreadViewRequest::OpenSessions => self.open_sessions(pane, cx),
            ThreadViewRequest::Rename(title) => self.rename(pane, title, cx),
            ThreadViewRequest::OpenSettings => self.show_settings(cx),
        }
    }

    /// Start a pane's conversation over: on a session of its agent's, or on a new one.
    pub(crate) fn reopen(
        &mut self,
        pane: &str,
        session: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resuming = None;
        self.notice = None;
        // A conversation its harness runs in the background is stopped
        // there before a chat can resume it. (A terminal attaches to it
        // instead, where it goes on running.)
        if let Some(saved) = self.store.pane(pane).cloned()
            && saved.kind == PaneKind::Thread
            && let Some(session) = session.as_deref()
            && let Some(open) =
                self.background_of(&saved.harness, saved.profile.as_deref(), session)
        {
            self.out_of_background(pane, open, Some(session.to_owned()), window, cx);
            return;
        }
        self.reopen_now(pane, session, window, cx);
    }

    fn reopen_now(
        &mut self,
        pane: &str,
        session: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forget(pane, cx);
        if let Some(saved) = self.store.pane_mut(pane) {
            saved.resume = session;
        }
        self.save(cx);
        self.launch(pane, None, window, cx);
    }

    /// Take a conversation out of its harness's background, so that this
    /// pane can open it as a chat: asked first when it is in a turn, which
    /// stopping it ends. Then the pane switches to its chat, or, with
    /// `resume`, its chat reopens on that session.
    fn out_of_background(
        &mut self,
        pane: &str,
        open: kou_agents::OpenSession,
        resume: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(saved) = self.store.pane(pane).cloned() else {
            return;
        };
        let Some(detected) = self
            .detected
            .iter()
            .find(|detected| detected.spec.id == saved.harness)
            .cloned()
        else {
            return;
        };
        let Some(name) = open.background.clone() else {
            return;
        };
        let harness_name = detected.spec.name;
        let answer = open.busy.then(|| {
            window.prompt(
                gpui::PromptLevel::Warning,
                &format!("{harness_name} is working in this conversation, in the background"),
                Some(
                    "Opening it as a chat stops it there: the turn it is in ends, and the \
                     conversation goes on in the chat.",
                ),
                &["Stop it and open the chat", "Leave it"],
                cx,
            )
        });
        let pane = pane.to_owned();
        // The session is its account's: stopped, and asked about, there.
        let profile = saved.profile.clone();
        let env = self.account_env(&saved.harness, profile.as_deref());
        cx.spawn_in(window, async move |this, cx| {
            if let Some(answer) = answer
                && answer.await != Ok(0)
            {
                return;
            }
            let stopped = cx
                .background_spawn({
                    let (detected, name) = (detected.clone(), name.clone());
                    async move { kou_agents::stop_background(&detected, &name, &env) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                // What the harness has open is asked anew: it has changed.
                this.open_now
                    .retain(|key, _| key.split('@').next() != Some(detected.spec.id));
                let session = resume.clone().or_else(|| {
                    this.store
                        .pane(&pane)
                        .and_then(|saved| saved.resume.clone())
                });
                let still = session.as_deref().is_some_and(|session| {
                    this.background_name(detected.spec.id, profile.as_deref(), session)
                        .is_some()
                });
                match stopped {
                    Err(reason) => {
                        this.notice = Some(
                            format!(
                                "{harness_name} did not stop that conversation in the \
                                 background: {reason}"
                            )
                            .into(),
                        );
                        cx.notify();
                    }
                    Ok(()) if still => {
                        this.notice = Some(
                            format!(
                                "{harness_name} still runs that conversation in the background."
                            )
                            .into(),
                        );
                        cx.notify();
                    }
                    Ok(()) => match resume {
                        Some(session) => this.reopen_now(&pane, Some(session), window, cx),
                        None => {
                            this.switch_now(&pane, window, cx);
                        }
                    },
                }
            })
            .ok();
        })
        .detach();
    }

    /// The session, if its harness runs it in the background right now.
    pub(crate) fn background_of(
        &mut self,
        harness: &str,
        profile: Option<&str>,
        session: &str,
    ) -> Option<kou_agents::OpenSession> {
        self.open_now_of(harness, profile)
            .into_iter()
            .find(|open| open.session == session && open.background.is_some())
    }

    /// Ask a harness which models a new session in `cwd` would offer, off
    /// the main thread: kou of its connection, another agent through its
    /// chat adapter (a session is opened and left; nothing is said in it).
    /// A harness with no chat here offers none.
    pub(crate) fn ask_for_models(
        &mut self,
        harness: &str,
        cwd: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let asked = if harness == "kou" {
            None
        } else {
            let bin = self
                .bin
                .clone()
                .or_else(|| std::env::current_exe().ok())
                .unwrap_or_default();
            // Nothing of the app's goes with it: no pane, no tools. It is
            // asked as the account the prompt would go to.
            let mut context = kou_agents::PaneContext::new(
                "",
                "",
                self.socket.clone(),
                bin,
                cwd.clone(),
                self.home.join("panes").join("models"),
            );
            let account = self
                .asked_profile()
                .filter(|profile| profile.harness == harness)
                .map(|profile| profile.id.clone());
            context.env = self.account_env(harness, account.as_deref());
            let launch = self
                .detected
                .iter()
                .find(|detected| detected.spec.id == harness)
                .and_then(|detected| kou_agents::acp_launch(detected, &context));
            let Some(launch) = launch else {
                if let Some(known) = self.offered.get_mut(harness) {
                    known.at = Some(Instant::now());
                }
                return;
            };
            Some(kou_acp::Launch {
                program: launch.program,
                args: launch.args,
                env: launch.env,
                cwd: launch.cwd,
            })
        };
        let folder = cwd.clone();
        let read = cx.background_executor().spawn(async move {
            match asked {
                None => {
                    let offered = kou_native::models(&folder);
                    Ok(Some((offered.models, offered.providers, offered.current)))
                }
                Some(launch) => kou_acp::models(
                    &launch,
                    &folder,
                    ("kou-desktop", env!("CARGO_PKG_VERSION")),
                    Duration::from_secs(20),
                )
                .map(|offered| {
                    offered.map(|offered| (offered.models, offered.providers, offered.current))
                }),
            }
        });
        let harness = harness.to_owned();
        cx.spawn(async move |this, cx| {
            let answer = read.await;
            this.update(cx, |this, cx| {
                let known = this.offered.entry(harness.clone()).or_default();
                known.cwd = cwd;
                known.at = Some(Instant::now());
                match answer {
                    Ok(Some((models, providers, current))) => {
                        known.models = models;
                        known.providers = providers;
                        known.current = current;
                    }
                    Ok(None) => {
                        known.models.clear();
                        known.providers.clear();
                        known.current = None;
                    }
                    // What was known stays; the field just offers no more.
                    Err(error) => log::warn!("{harness} did not say its models: {error:#}"),
                }
                this.offered_changes += 1;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The name a session is attached by when its harness runs it in the
    /// background right now (see [`kou_agents::OpenSession`]). The harness
    /// is asked, and its answer kept for a few seconds: panes restored
    /// together ask once.
    pub(crate) fn background_name(
        &mut self,
        harness: &str,
        profile: Option<&str>,
        session: &str,
    ) -> Option<String> {
        self.open_now_of(harness, profile)
            .into_iter()
            .find(|open| open.session == session)
            .and_then(|open| open.background)
    }

    /// What a harness has open right now, under one of its accounts
    /// (`None`: its own).
    fn open_now_of(
        &mut self,
        harness: &str,
        profile: Option<&str>,
    ) -> Vec<kou_agents::OpenSession> {
        const FRESH: Duration = Duration::from_secs(5);
        // Kept by harness and account: `claude`, `claude@a3`.
        let key = match profile {
            Some(profile) => format!("{harness}@{profile}"),
            None => harness.to_owned(),
        };
        if let Some((at, open)) = self.open_now.get(&key)
            && at.elapsed() < FRESH
        {
            return open.clone();
        }
        let env = self.account_env(harness, profile);
        let open = self
            .detected
            .iter()
            .find(|detected| detected.spec.id == harness)
            .map(|detected| kou_agents::open_sessions(detected, &env))
            .unwrap_or_default();
        self.open_now.insert(key, (Instant::now(), open.clone()));
        open
    }

    // Give a pane a name, everywhere it is shown.
    fn rename(&mut self, pane: &str, title: &str, cx: &mut Context<Self>) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        if let Some(saved) = self.store.pane_mut(pane) {
            saved.title = title.to_owned();
        }
        self.show_on_canvas(pane, cx);
        self.save(cx);
    }

    // Show the earlier conversations a pane can go back to. They are read off
    // the main thread: the first reading of a long history takes a moment.
    fn open_sessions(&mut self, pane: &str, cx: &mut Context<Self>) {
        let Some(saved) = self.store.pane(pane).cloned() else {
            return;
        };
        let name = self
            .harness(&saved.harness)
            .map_or_else(|| saved.harness.clone(), |harness| harness.name.clone());
        // kou's own are read from its files; another agent is asked for
        // its own, the way it is talked to.
        let asked = if saved.harness == "kou" {
            None
        } else {
            let launch = self
                .detected
                .iter()
                .find(|detected| detected.spec.id == saved.harness)
                .zip(self.pane_context(&saved).ok())
                .and_then(|(detected, context)| kou_agents::acp_launch(detected, &context));
            let Some(launch) = launch else {
                self.notice = Some(
                    format!(
                        "{name} does not list its sessions here. /resume <id> opens one by its id."
                    )
                    .into(),
                );
                cx.notify();
                return;
            };
            Some(kou_acp::Launch {
                program: launch.program,
                args: launch.args,
                env: launch.env,
                cwd: launch.cwd,
            })
        };
        // Those open right now are marked: one in the background opens in
        // its terminal.
        let open: Vec<String> = self
            .open_now_of(&saved.harness, saved.profile.as_deref())
            .into_iter()
            .map(|open| open.session)
            .collect();
        self.resuming = Some(crate::shell::Resuming {
            pane: pane.to_owned(),
            sessions: None,
        });
        let cwd = saved.cwd.clone();
        let read = cx.background_executor().spawn(async move {
            match asked {
                None => kou_native::sessions(&cwd).map(|sessions| {
                    Some(sessions.into_iter().map(past_of_kou).collect::<Vec<_>>())
                }),
                Some(launch) => kou_acp::sessions(
                    &launch,
                    &cwd,
                    ("kou-desktop", env!("CARGO_PKG_VERSION")),
                    Duration::from_secs(20),
                )
                .map(|sessions| {
                    sessions.map(|sessions| {
                        sessions
                            .into_iter()
                            .map(|session| crate::shell::Past {
                                running: open.contains(&session.id),
                                id: session.id,
                                title: session.title,
                                updated: session.updated,
                                prompts: None,
                            })
                            .collect()
                    })
                }),
            }
        });
        let pane = pane.to_owned();
        cx.spawn(async move |this, cx| {
            let sessions = read.await;
            this.update(cx, |this, cx| {
                let Some(resuming) = this.resuming.as_mut().filter(|open| open.pane == pane) else {
                    return;
                };
                match sessions {
                    Ok(Some(sessions)) => resuming.sessions = Some(sessions),
                    Ok(None) => {
                        this.resuming = None;
                        this.notice = Some(
                            format!(
                                "{name} does not list its sessions. /resume <id> opens one by \
                                 its id."
                            )
                            .into(),
                        );
                    }
                    Err(error) => {
                        this.resuming = None;
                        this.notice =
                            Some(format!("The sessions could not be read: {error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn thread_event(
        &mut self,
        pane: &str,
        event: &ThreadViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ThreadViewEvent::Title(title) if !title.is_empty() => {
                let title = title.to_string();
                let Some(saved) = self.store.pane_mut(pane) else {
                    return;
                };
                saved.title = title.clone();
                let (session, node) = (saved.session.clone(), saved.node);
                if let Some(canvas) = self.canvas(&session, cx) {
                    canvas.update(cx, |canvas, cx| {
                        canvas.update_node(node, |node| node.title = title, cx)
                    });
                }
                self.save(cx);
            }
            ThreadViewEvent::Title(_) => {}
            // A click on an agent this one started, in its conversation.
            ThreadViewEvent::OpenNested(id) => {
                if self.store.pane(id).is_some() {
                    self.focus_pane(id, cx);
                }
            }
            ThreadViewEvent::OpenFolder(path) => {
                let _ = std::process::Command::new("open").arg(path).spawn();
            }
            // The model chosen in a conversation is the pane's from then on:
            // what its session is opened with the next time, as a chat or
            // in its terminal. ("default" is the harness's own choice, as
            // in the empty screen's field: nothing to keep.)
            ThreadViewEvent::Model(model) => {
                let model = (model != "default").then(|| model.clone());
                if let Some(saved) = self.store.pane_mut(pane)
                    && saved.model != model
                {
                    saved.model = model;
                    self.save(cx);
                }
            }
            ThreadViewEvent::State(state) => {
                let Some(talking) = self.threads.get_mut(pane) else {
                    return;
                };
                let finished = matches!(talking.state, ThreadState::Working | ThreadState::Blocked)
                    && matches!(state, ThreadState::Idle);
                talking.state = state.clone();
                // What a later launch resumes: the backend's own session id.
                let session = talking.view.read(cx).thread().session.clone();
                if session.is_some()
                    && let Some(saved) = self.store.pane_mut(pane)
                    && saved.resume != session
                {
                    saved.resume = session;
                    self.save(cx);
                }
                let mark = match state {
                    // Coming up is not work: other panes are told it is busy
                    // (see `activity`), but nothing has been done to mark done.
                    ThreadState::Starting | ThreadState::Idle | ThreadState::Closed(None) => {
                        Mark::Idle
                    }
                    ThreadState::Working => Mark::Working,
                    ThreadState::Blocked => Mark::Blocked,
                    ThreadState::Closed(Some(_)) => Mark::Failed,
                };
                self.set_mark(pane, mark, window, cx);
                self.answer_waits(cx);
                if finished {
                    self.route_answer(pane, window, cx);
                }
            }
        }
    }

    /// On its canvas what runs in a tab's panes is drawn at the canvas's
    /// zoom, text scaled rather than re-wrapped: a terminal keeps its grid, a
    /// conversation its line breaks. As panes each is its own size. Called
    /// when a tab changes layout and when one of its panes gets its view.
    pub(crate) fn follow_canvas(&mut self, tab: &str, cx: &mut Context<Self>) {
        let on_canvas = self
            .store
            .tab(tab)
            .is_some_and(|tab| tab.layout == crate::state::Layout::Canvas);
        let zoom = if on_canvas {
            self.canvas(tab, cx)
                .map(|canvas| canvas.read(cx).zoom_handle())
        } else {
            None
        };
        let panes: Vec<Id> = self
            .store
            .panes_in(tab)
            .into_iter()
            .map(|pane| pane.id.clone())
            .collect();
        for pane in panes {
            if let Some(running) = self.running.get(&pane) {
                let zoom = zoom.clone();
                running
                    .view
                    .update(cx, |view, cx| view.follow_zoom(zoom, cx));
            }
            // A conversation is drawn at the zoom too: its text with the rest of it.
            if let Some(talking) = self.threads.get(&pane) {
                let zoom = zoom.clone();
                talking
                    .view
                    .update(cx, |view, cx| view.follow_zoom(zoom, cx));
            }
        }
    }

    /// The focus handle of what runs in a pane: where the keys go when its
    /// node is entered on the canvas.
    pub(crate) fn body_focus(&self, pane: &str, cx: &Context<Self>) -> Option<gpui::FocusHandle> {
        if let Some(running) = self.running.get(pane) {
            return Some(gpui::Focusable::focus_handle(running.view.read(cx), cx));
        }
        self.threads
            .get(pane)
            .map(|talking| gpui::Focusable::focus_handle(talking.view.read(cx), cx))
    }

    /// Hand a pane a message that came along an edge, and submit it.
    pub(crate) fn deliver(
        &mut self,
        to: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.send_text(to, text, true, cx) {
            self.refresh(to, window, cx);
        }
    }

    // What a conversation ended its turn with goes on along its node's edges,
    // as on the web canvas: straight through an open edge, held on one that asks.
    fn route_answer(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(answer) = self
            .threads
            .get(pane)
            .and_then(|talking| talking.view.read(cx).last_answer())
            .filter(|answer| !answer.trim().is_empty())
        else {
            return;
        };
        let Some(saved) = self.store.pane(pane).cloned() else {
            return;
        };
        // What a pane says goes along the wires of its own tab's canvas.
        let Some(tab) = self.store.tab_of(pane).map(|tab| tab.id.clone()) else {
            return;
        };
        let Some(canvas) = self.canvas(&tab, cx) else {
            return;
        };
        let routes: Vec<_> = canvas
            .read(cx)
            .board()
            .routes_from(saved.node, "out")
            // Only what was wired on purpose pipes by itself. The edge made
            // when one agent started another shows who made whom: the parent
            // talks to its child when it means to, with `pane send`.
            .filter(|edge| edge.origin == kou_canvas::EdgeOrigin::Wired)
            .cloned()
            .collect();
        for edge in routes {
            let Some(target) = self
                .store
                .panes_in(&tab)
                .into_iter()
                .find(|other| other.node == edge.to)
                .cloned()
            else {
                continue;
            };
            let kind = canvas
                .read(cx)
                .board()
                .node(edge.to)
                .map_or(kou_canvas::NodeKind::Agent, |node| node.kind);
            let text = edge.message(&answer, &saved.title, kind);
            if self.pass_along(&saved.id, &target.id, &text, false, cx) {
                self.deliver(&target.id, &text, window, cx);
            }
        }
    }

    // Whether a pane is the one in front. Behind the tab that waits for
    // its agent, none is.
    fn is_focused(&self, pane: &str) -> bool {
        !self.pending_in_front()
            && self
                .store
                .current()
                .is_some_and(|session| session.focused.as_deref() == Some(pane))
    }

    // Wait until the program has drawn something and gone quiet, then type.
    fn type_when_ready(
        &mut self,
        pane: &str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = pane.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..75 {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let ready = this.update(cx, |this, cx| {
                    this.running.get(&pane).map(|running| {
                        running
                            .terminal
                            .read(cx)
                            .last_output()
                            .is_some_and(|at| at.elapsed() >= Duration::from_millis(600))
                    })
                });
                match ready {
                    Ok(Some(true)) => break,
                    Ok(Some(false)) => {}
                    _ => return,
                }
            }
            this.update_in(cx, |this, window, cx| {
                this.send_text(&pane, &text, true, cx);
                this.refresh(&pane, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Give a pane input as another agent would: pasted, then Enter.
    fn send_text(&mut self, pane: &str, text: &str, submit: bool, cx: &mut Context<Self>) -> bool {
        // A conversation takes whole prompts: there is nothing to type into.
        if let Some(talking) = self.threads.get(pane) {
            if talking.closed() {
                return false;
            }
            talking.view.update(cx, |view, cx| view.send(text, cx));
            return true;
        }
        let Some(running) = self.running.get_mut(pane) else {
            return false;
        };
        let now = Instant::now();
        running.tracker.input(now);
        running.typing = false;
        if submit {
            running.expected = Some(now);
            // A shell that marks its commands says "over" once for each
            // one it ran, this one included: one more than it has so far.
            // (A shell still coming up runs what it was sent when it gets
            // to its first prompt, and says so then.)
            let finished = running.terminal.read(cx).commands_finished();
            running.finished_target =
                (running.job.is_some() && (running.marks || finished > 0)).then_some(finished + 1);
            running.terminal.read(cx).submit(text);
        } else {
            running.terminal.read(cx).paste(text);
        }
        true
    }

    fn terminal_event(
        &mut self,
        pane: &str,
        event: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let Some(running) = self.running.get_mut(pane) else {
            return;
        };
        match event {
            TerminalEvent::Wakeup => {
                running.tracker.output(now);
                running.generation += 1;
                let (generation, id) = (running.generation, pane.to_owned());
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(SETTLE).await;
                    this.update_in(cx, |this, window, cx| {
                        crate::hot::hot!(this.settled(&id, generation, window, cx))
                    })
                    .ok();
                })
                .detach();
            }
            TerminalEvent::Title(title) => running.tracker.title(title, now),
            TerminalEvent::Bell | TerminalEvent::Notification { .. } => {
                running.tracker.notification(now)
            }
            TerminalEvent::Progress(value) => running.tracker.progress(*value, now),
            TerminalEvent::Cwd(path) => {
                if let Some(saved) = self.store.pane_mut(pane) {
                    saved.cwd = path.clone();
                }
                self.save(cx);
                return;
            }
            TerminalEvent::Exited(status) => {
                let failed = !matches!(status, Some(0) | None);
                let never_ran = running.launched.elapsed() < NEVER_RAN;
                self.running.remove(pane);
                if failed && never_ran {
                    // It did not get going: closing its pane would hide why.
                    let reason = match status {
                        Some(status) => format!("It ended at once, with status {status}."),
                        None => "It ended at once.".to_owned(),
                    };
                    self.stop_pane(pane, &reason, cx);
                    let focused = self.is_focused(pane);
                    if let Some(live) = self.live.get_mut(pane) {
                        live.mark = Mark::Failed;
                        live.seen = focused;
                    }
                } else {
                    // The program is done, and so is its pane: as in any terminal,
                    // `exit` closes it, and what was beside it takes its room.
                    self.close_pane(pane, window, cx);
                }
                self.answer_waits(cx);
                return;
            }
        }
        self.refresh(pane, window, cx);
    }

    // Output paused: read the last lines for what they say, and show them on the canvas.
    fn settled(
        &mut self,
        pane: &str,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(running) = self.running.get_mut(pane) else {
            return;
        };
        if running.generation != generation {
            return;
        }
        let tail = running
            .terminal
            .read(cx)
            .text(Some(kou_agents::READ_LINES), false);
        running.tracker.screen(&tail, Instant::now());
        if let Some(saved) = self.store.pane(pane) {
            let (session, node) = (saved.session.clone(), saved.node);
            if let Some(canvas) = self.canvas(&session, cx) {
                let preview: Vec<&str> = tail
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .collect();
                let preview = preview[preview.len().saturating_sub(6)..].join("\n");
                canvas.update(cx, |canvas, cx| {
                    canvas.update_node(node, |node| node.detail = preview, cx)
                });
            }
        }
        self.refresh(pane, window, cx);
    }

    fn activity(&self, pane: &str, now: Instant) -> Option<Activity> {
        if let Some(talking) = self.threads.get(pane) {
            return match talking.state {
                ThreadState::Starting | ThreadState::Working => Some(Activity::Working),
                ThreadState::Idle => Some(Activity::Idle),
                ThreadState::Blocked => Some(Activity::Blocked),
                ThreadState::Closed(_) => None,
            };
        }
        let running = self.running.get(pane)?;
        // Handed a prompt a moment ago and not yet seen working: still counts as busy.
        let expected = running.typing
            || running
                .expected
                .is_some_and(|at| now.duration_since(at) < running.grace());
        // A plain shell is busy while a command has its terminal, and, for a
        // shell that announces its prompt, whenever it is not at one.
        if let Some(busy) = running.job.and_then(crate::foreground::job_running) {
            let busy = busy || (running.prompts && !running.at_prompt);
            return Some(if busy || expected || !running.started() {
                Activity::Working
            } else {
                Activity::Idle
            });
        }
        let activity = running.tracker.activity(now);
        Some(if activity == Activity::Idle && expected {
            Activity::Working
        } else {
            activity
        })
    }

    /// Bring a pane's mark in line with its tracker, and come back when the
    /// tracker says its answer may change without anything new happening.
    fn refresh(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let Some(running) = self.running.get_mut(pane) else {
            return;
        };
        // A harness's screen is read now and then whenever its state is
        // looked at: while it animates its status line and never pauses,
        // and when the tracker's own deadline brings the app back, which is
        // how it learns that the line is gone (a turn stopped by hand) or
        // there against what the hooks said.
        if running.job.is_none() && running.tracker.reading_due(now) {
            let tail = running
                .terminal
                .read(cx)
                .text(Some(kou_agents::READ_LINES), false);
            running.tracker.screen(&tail, now);
        }
        // A shell that marks its commands says exactly when the one it was
        // handed is over; any other pane has answered its prompt once it is
        // seen at work.
        let command_over = running
            .finished_target
            .map(|target| running.terminal.read(cx).commands_finished() >= target);
        match command_over {
            Some(true) => {
                running.expected = None;
                running.finished_target = None;
            }
            Some(false) => {}
            None => {
                if running.tracker.activity(now) == Activity::Working {
                    running.expected = None;
                }
            }
        }
        if running.job.is_some() {
            running.at_prompt = running.terminal.read(cx).takes_bracketed_paste();
            running.prompts |= running.at_prompt;
        }
        let mut deadline = running.tracker.deadline(now);
        let soonest = |deadline: Option<Instant>, at: Instant| {
            Some(deadline.map_or(at, |deadline| deadline.min(at)))
        };
        if let Some(at) = running.expected {
            // The prompt has been answered once output came after it and
            // stopped. (Not for a shell that says so itself: its echo of
            // the line is output too, and comes before the command does.)
            let answered = running
                .terminal
                .read(cx)
                .last_output()
                .filter(|output| *output > at && command_over.is_none())
                .map(|output| output + ANSWERED);
            let limit = at + running.grace();
            let ends = answered.map_or(limit, |answered| answered.min(limit));
            if now >= ends {
                running.expected = None;
                running.finished_target = None;
            } else {
                deadline = soonest(deadline, ends);
            }
        }
        // Nothing announces a shell command finishing, or a shell coming up:
        // look again shortly while it is not at rest.
        let unsettled = running.job.is_some()
            && (!running.started()
                || running.expected.is_some()
                || (running.prompts && !running.at_prompt)
                || running.job.and_then(crate::foreground::job_running) == Some(true));
        if unsettled {
            deadline = soonest(deadline, now + SHELL_POLL);
        }
        let Some(activity) = self.activity(pane, now) else {
            return;
        };
        self.set_mark(pane, mark_of(activity), window, cx);
        self.answer_waits(cx);
        if let Some(deadline) = deadline {
            let (id, pause) = (pane.to_owned(), deadline.saturating_duration_since(now));
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(pause + Duration::from_millis(20))
                    .await;
                this.update_in(cx, |this, window, cx| this.refresh(&id, window, cx))
                    .ok();
            })
            .detach();
        }
    }

    fn describe(&self, pane: &str) -> Option<kou_ipc::Pane> {
        let saved = self.store.pane(pane)?;
        let live = self.live.get(pane)?;
        Some(kou_ipc::Pane {
            id: saved.id.clone(),
            session: saved.session.clone(),
            harness: saved.harness.clone(),
            title: saved.title.clone(),
            cwd: saved.cwd.clone(),
            kind: match saved.kind {
                PaneKind::Terminal => kou_ipc::PaneKind::Terminal,
                PaneKind::Thread => kou_ipc::PaneKind::Thread,
            },
            activity: match self.activity(pane, Instant::now()) {
                Some(activity) => activity.into(),
                None => kou_ipc::Activity::Exited,
            },
            parent: saved.parent.clone(),
            since_ms: live.changed.elapsed().as_millis() as u64,
        })
    }

    fn describe_tab(
        &self,
        session: &crate::state::Session,
        tab: &crate::state::Tab,
    ) -> kou_ipc::Tab {
        kou_ipc::Tab {
            id: tab.id.clone(),
            session: session.id.clone(),
            name: self.tab_name(tab),
            named: tab.name.is_some(),
            icon: tab.icon.clone(),
            layout: match tab.layout {
                crate::state::Layout::Panes => kou_ipc::Layout::Panes,
                crate::state::Layout::Canvas => kou_ipc::Layout::Canvas,
            },
            panes: tab.root.panes().into_iter().cloned().collect(),
            active: tab.active.clone(),
            front: session.front().is_some_and(|front| front.id == tab.id),
        }
    }

    fn describe_session(&self, session: &crate::state::Session) -> kou_ipc::Session {
        kou_ipc::Session {
            id: session.id.clone(),
            space: session.space.clone(),
            name: session.name.clone(),
            cwd: session.cwd.clone(),
            archived: session.archived,
            tabs: session.tabs.len() as u32,
            selected: self.store.selected.as_deref() == Some(session.id.as_str()),
        }
    }

    fn describe_space(&self, space: &crate::state::Space) -> kou_ipc::Space {
        kou_ipc::Space {
            id: space.id.clone(),
            name: space.name.clone(),
            path: space.path.clone(),
            sessions: self.store.sessions_of(&space.id, false).count() as u32,
        }
    }

    // The session a request is about: the one it names, else the asking pane's.
    fn asked_session(&self, session: Option<&str>, pane: Option<&str>) -> Option<Id> {
        match session {
            Some(session) => self.store.session(session).map(|saved| saved.id.clone()),
            None => pane
                .and_then(|pane| self.store.pane(pane))
                .map(|pane| pane.session.clone()),
        }
    }

    fn reached(&self, pane: &str, until: Until) -> bool {
        match (self.describe(pane).map(|pane| pane.activity), until) {
            // A pane that is gone will never be busy again.
            (None, _) | (Some(kou_ipc::Activity::Exited), _) => true,
            // An agent that waits for an answer is not working either, and
            // will not be until it gets one: whoever waits for it has to
            // know now, not when its patience runs out. The pane's state
            // says which of the two it is.
            (Some(kou_ipc::Activity::Idle | kou_ipc::Activity::Blocked), Until::Idle) => true,
            _ => false,
        }
    }

    fn answer_waits(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut still = Vec::new();
        for wait in std::mem::take(&mut self.waits) {
            let reached = |pane: &Id| self.reached(pane, wait.until);
            let done = if wait.any {
                wait.panes.iter().any(reached)
            } else {
                wait.panes.iter().all(reached)
            };
            let timed_out = !done && wait.deadline.is_some_and(|deadline| now >= deadline);
            if done || timed_out {
                let panes = wait
                    .panes
                    .iter()
                    .filter_map(|pane| self.describe(pane))
                    .collect();
                wait.reply.send(Response::Waited { panes, timed_out });
            } else {
                still.push(wait);
            }
        }
        self.waits = still;
        self.sync_waiting(cx);
    }

    /// Answer a request from the command line client: an agent in a pane, or a harness's hook.
    pub fn handle(&mut self, incoming: Incoming, window: &mut Window, cx: &mut Context<Self>) {
        let Incoming { request, reply } = incoming;
        let missing = |pane: &str| Response::Error {
            message: format!("no pane {pane}"),
        };
        let refused = |message: String| Response::Error { message };
        let response = match request {
            Request::Ping => Response::Pong {
                version: kou_ipc::VERSION,
            },
            Request::Hook {
                pane,
                harness,
                event,
                payload,
            } => {
                if let Some(signal) = kou_agents::parse_hook(&harness, &event, &payload) {
                    // The harness's own session id is what a later resume needs.
                    if signal == Signal::SessionStarted
                        && let Some(session) = payload.get("session_id").and_then(|id| id.as_str())
                        && let Some(saved) = self.store.pane_mut(&pane)
                    {
                        saved.resume = Some(session.to_owned());
                        self.save(cx);
                    }
                    if let Some(running) = self.running.get_mut(&pane) {
                        running.tracker.hook(signal, Instant::now());
                        self.refresh(&pane, window, cx);
                    }
                }
                Response::Done
            }
            Request::PaneInfo { pane } => self
                .describe(&pane)
                .map_or_else(|| missing(&pane), Response::Pane),
            Request::Harnesses => Response::Harnesses {
                harnesses: self
                    .detected
                    .iter()
                    .map(|detected| {
                        let info = info(detected);
                        kou_ipc::Harness {
                            id: info.id,
                            name: info.name,
                            version: info.version,
                            ready: info.ready,
                            message: info.message,
                            // As the app itself sees it: kou's own agent is
                            // a chat through its runner, not through ACP.
                            native: info.native,
                        }
                    })
                    .collect(),
            },
            Request::PaneList { session } => Response::Panes {
                panes: self
                    .store
                    .panes
                    .iter()
                    .filter(|pane| {
                        session
                            .as_ref()
                            .is_none_or(|session| &pane.session == session)
                    })
                    .filter_map(|pane| self.describe(&pane.id))
                    .collect(),
            },
            Request::PaneSpawn(spawn) => {
                let session = spawn
                    .from
                    .as_deref()
                    .and_then(|from| self.store.pane(from))
                    .map(|from| from.session.clone())
                    .or_else(|| self.store.selected.clone());
                let ready = self
                    .harness(&spawn.harness)
                    .is_some_and(|harness| harness.ready);
                match session {
                    None => Response::Error {
                        message: "no session is open to start a pane in".into(),
                    },
                    Some(_) if !ready => Response::Error {
                        message: format!(
                            "{} is not available here; `kou-desktop harness list` shows what is",
                            spawn.harness
                        ),
                    },
                    Some(session) => {
                        // A child keeps its parent's place on screen: the user is
                        // looking at the parent, so the new pane does not take over
                        // unless it was asked to open as a tab.
                        let previous = self.store.session(&session).and_then(|s| s.focused.clone());
                        let native = self
                            .harness(&spawn.harness)
                            .is_some_and(|harness| harness.native);
                        let kind = if spawn.native && native {
                            PaneKind::Thread
                        } else {
                            self.default_kind(&spawn.harness)
                        };
                        // Asked for as a split, it takes half of its parent's
                        // room; as a tab, it is one; left to the app, it joins
                        // its parent on a canvas and is a tab otherwise.
                        let seat = match (spawn.placement, spawn.from.as_deref()) {
                            (Placement::Split, Some(parent))
                                if self.store.pane(parent).is_some() =>
                            {
                                crate::state::Seat::Beside {
                                    at: parent.to_owned(),
                                    // To its right, or below it when asked.
                                    axis: if spawn.below {
                                        crate::state::Axis::Column
                                    } else {
                                        crate::state::Axis::Row
                                    },
                                }
                            }
                            (Placement::Tab, _) => {
                                crate::state::Seat::Tab(kou_canvas::Board::new())
                            }
                            (_, parent) => self.seat_for(parent),
                        };
                        match self.add_pane_at(
                            &session,
                            &spawn.harness,
                            spawn.from.clone(),
                            kind,
                            crate::shell::Place::Auto,
                            seat,
                            window,
                            cx,
                        ) {
                            Some(pane) => {
                                // A split asked for at a size other than half.
                                if spawn.placement == Placement::Split
                                    && let Some(share) = spawn.share
                                    && let Some(tab) =
                                        self.store.tab_of(&pane).map(|tab| tab.id.clone())
                                    && let Some(tab) = self.store.tab_mut(&tab)
                                {
                                    tab.root.set_share(&pane, f32::from(share) / 100.);
                                }
                                if let Some(saved) = self.store.pane_mut(&pane) {
                                    saved.model = spawn.model.clone();
                                    if let Some(cwd) = spawn.cwd.clone() {
                                        saved.cwd = cwd;
                                    }
                                    if let Some(title) = spawn.title.clone() {
                                        saved.title = title;
                                    }
                                }
                                // The canvas names the node as the tab names the pane.
                                if spawn.title.is_some() {
                                    self.show_on_canvas(&pane, cx);
                                    if let (Some(title), Some(saved)) =
                                        (spawn.title.clone(), self.store.pane(&pane))
                                    {
                                        let (node, tab) = (saved.node, self.store.tab_of(&pane));
                                        if let Some(tab) = tab.map(|tab| tab.id.clone())
                                            && let Some(tab) = self.store.tab_mut(&tab)
                                            && let Some(node) = tab.board.node_mut(node)
                                        {
                                            node.title = title;
                                        }
                                    }
                                }
                                if spawn.placement != Placement::Tab
                                    && let Some(previous) = previous
                                {
                                    self.store.focus(&previous);
                                }
                                self.launch(&pane, spawn.prompt.clone(), window, cx);
                                // Its starter's conversation shows it from here on.
                                if let Some(parent) = spawn.from.clone()
                                    && self.store.pane(&parent).is_some()
                                {
                                    self.note_started(&parent, &pane, cx);
                                }
                                self.save(cx);
                                self.describe(&pane)
                                    .map_or_else(|| missing(&pane), Response::Pane)
                            }
                            None => Response::Error {
                                message: format!("could not add a pane for {}", spawn.harness),
                            },
                        }
                    }
                }
            }
            Request::PaneSend {
                pane,
                text,
                submit,
                from,
            } => {
                // A message from another pane travels the edge between them, and
                // waits on it if the user set that edge to ask first.
                let held = submit
                    && from
                        .as_deref()
                        .is_some_and(|from| !self.pass_along(from, &pane, &text, true, cx));
                if held {
                    Response::Done
                } else if self.send_text(&pane, &text, submit, cx) {
                    self.refresh(&pane, window, cx);
                    Response::Done
                } else if self.store.pane(&pane).is_some() {
                    Response::Error {
                        message: format!("nothing is running in pane {pane}"),
                    }
                } else {
                    missing(&pane)
                }
            }
            Request::PaneRead {
                pane,
                lines,
                scrollback,
            } => match (self.running.get(&pane), self.threads.get(&pane)) {
                (Some(running), _) => Response::Text {
                    text: running
                        .terminal
                        .read(cx)
                        .text(lines.map(|lines| lines as usize), scrollback),
                },
                // A conversation is read by its latest answer.
                (None, Some(talking)) => Response::Text {
                    text: talking.view.read(cx).last_answer().unwrap_or_default(),
                },
                (None, None) if self.store.pane(&pane).is_some() => Response::Text {
                    text: String::new(),
                },
                (None, None) => missing(&pane),
            },
            Request::PaneWait {
                panes,
                until,
                any,
                timeout_ms,
            } => {
                if let Some(unknown) = panes.iter().find(|pane| self.store.pane(pane).is_none()) {
                    missing(unknown)
                } else {
                    let timeout = timeout_ms.map(Duration::from_millis);
                    self.waits.push(Wait {
                        panes,
                        until,
                        any,
                        deadline: timeout.map(|timeout| Instant::now() + timeout),
                        reply,
                    });
                    self.answer_waits(cx);
                    if let Some(timeout) = timeout {
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(timeout + Duration::from_millis(20))
                                .await;
                            this.update(cx, |this, cx| this.answer_waits(cx)).ok();
                        })
                        .detach();
                    }
                    return;
                }
            }
            Request::PaneClose { pane } => {
                if self.store.pane(&pane).is_some() {
                    self.close_pane(&pane, window, cx);
                    Response::Done
                } else {
                    missing(&pane)
                }
            }
            Request::PaneFocus { pane } => {
                if self.store.pane(&pane).is_some() {
                    self.focus_pane(&pane, cx);
                    Response::Done
                } else {
                    missing(&pane)
                }
            }
            Request::TabList { session } => Response::Tabs {
                tabs: self
                    .store
                    .sessions
                    .iter()
                    .filter(|saved| session.as_deref().is_none_or(|id| id == saved.id))
                    .flat_map(|saved| saved.tabs.iter().map(|tab| self.describe_tab(saved, tab)))
                    .collect(),
            },
            Request::TabSet {
                tab,
                pane,
                name,
                icon,
            } => {
                let found = match &tab {
                    Some(tab) => self.store.tab(tab),
                    None => pane.as_deref().and_then(|pane| self.store.tab_of(pane)),
                }
                .map(|tab| tab.id.clone());
                match found {
                    None => refused(match tab {
                        Some(tab) => {
                            format!(
                                "there is no tab {tab}; `kou-desktop tab list --all` shows them"
                            )
                        }
                        None => "no tab was named, and the asking pane is in none".into(),
                    }),
                    Some(_) if name.is_none() && icon.is_none() => {
                        refused("nothing to change: give the tab a name, an icon, or both".into())
                    }
                    Some(tab) => {
                        // The icon first: a name is not kept beside one refused.
                        let icon = icon.map_or(Ok(()), |icon| self.set_tab_icon(&tab, &icon, cx));
                        match icon {
                            Err(message) => refused(message),
                            Ok(()) => {
                                if let Some(name) = name {
                                    self.name_tab(&tab, &name, cx);
                                }
                                let described =
                                    self.store.session_of_tab(&tab).and_then(|session| {
                                        Some(self.describe_tab(session, session.tab_by(&tab)?))
                                    });
                                described.map_or_else(
                                    || refused(format!("tab {tab} is gone")),
                                    Response::Tab,
                                )
                            }
                        }
                    }
                }
            }
            Request::TabIcons => Response::Text {
                text: std::iter::once("auto")
                    .chain(kou_ui::TAB_ICONS.iter().map(|(name, _)| *name))
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
            Request::SessionList { space } => Response::Sessions {
                sessions: self
                    .store
                    .sessions
                    .iter()
                    .filter(|saved| space.as_deref().is_none_or(|id| id == saved.space))
                    .map(|saved| self.describe_session(saved))
                    .collect(),
            },
            Request::SessionRename {
                session,
                pane,
                name,
            } => match self.asked_session(session.as_deref(), pane.as_deref()) {
                None => refused(match session {
                    Some(session) => {
                        format!(
                            "there is no session {session}; `kou-desktop session list` shows them"
                        )
                    }
                    None => "no session was named, and the asking pane is in none".into(),
                }),
                Some(session) => {
                    if self.name_session(&session, &name, cx) {
                        self.store
                            .session(&session)
                            .map(|saved| self.describe_session(saved))
                            .map_or(Response::Done, Response::Session)
                    } else {
                        refused("a session's name cannot be empty".into())
                    }
                }
            },
            Request::SpaceList => Response::Spaces {
                spaces: self
                    .store
                    .spaces
                    .iter()
                    .map(|space| self.describe_space(space))
                    .collect(),
            },
            Request::SpaceRename { space, pane, name } => {
                let found = match &space {
                    Some(space) => self.store.space(space).map(|saved| saved.id.clone()),
                    None => self
                        .asked_session(None, pane.as_deref())
                        .and_then(|session| self.store.session(&session))
                        .map(|session| session.space.clone()),
                };
                match found {
                    None => refused(match space {
                        Some(space) => {
                            format!(
                                "there is no space {space}; `kou-desktop space list` shows them"
                            )
                        }
                        None => "no space was named, and the asking pane is in none".into(),
                    }),
                    Some(space) => {
                        if self.name_space(&space, &name, cx) {
                            self.store
                                .space(&space)
                                .map(|saved| self.describe_space(saved))
                                .map_or(Response::Done, Response::Space)
                        } else {
                            refused("a space's name cannot be empty".into())
                        }
                    }
                }
            }
        };
        reply.send(response);
    }

    /// Redraw every terminal in the current theme.
    pub(crate) fn retheme(&mut self, cx: &mut Context<Self>) {
        let style = terminal_style(theme(cx), self.store.smooth_cursor);
        for running in self.running.values() {
            running
                .view
                .update(cx, |view, cx| view.set_style(style.clone(), cx));
        }
        cx.notify();
    }

    /// Forget a pane's process: dropping the terminal ends it.
    pub(crate) fn forget(&mut self, pane: &str, cx: &mut Context<Self>) {
        if let Some(running) = self.running.remove(pane) {
            running.terminal.read(cx).kill();
            drop(running.view);
        }
        // Dropping the view drops the backend's channels, which ends its session.
        self.threads.remove(pane);
        self.answer_waits(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::pick_session;
    use crate::shell::Past;
    use std::time::SystemTime;

    fn session(id: &str, title: &str) -> Past {
        Past {
            id: id.into(),
            title: title.into(),
            updated: Some(SystemTime::UNIX_EPOCH),
            running: false,
            prompts: Some(1),
        }
    }

    #[test]
    fn changed_lines_are_read_from_gits_summary() {
        use super::changed_lines;
        assert_eq!(
            changed_lines(" 3 files changed, 12 insertions(+), 4 deletions(-)\n"),
            (12, 4)
        );
        assert_eq!(changed_lines(" 1 file changed, 1 insertion(+)\n"), (1, 0));
        assert_eq!(changed_lines(" 2 files changed, 7 deletions(-)\n"), (0, 7));
        assert_eq!(changed_lines(""), (0, 0));
    }

    #[test]
    fn resume_takes_an_id_the_start_of_one_or_a_title() {
        let sessions = [
            session("39cd11cd-aaaa", "Rewrite the parser"),
            session("7b2e90f1-bbbb", "parser tests"),
            session("parser", "Something else"),
        ];
        let pick = |which: &str| pick_session(&sessions, which);
        // An id wins over a title that reads the same.
        assert_eq!(pick("parser").as_deref(), Some("parser"));
        assert_eq!(pick("7b2e90f1").as_deref(), Some("7b2e90f1-bbbb"));
        // Too short to be meant as an id: not matched as the start of one.
        assert_eq!(pick("39c"), None);
        assert_eq!(pick("  PARSER TESTS ").as_deref(), Some("7b2e90f1-bbbb"));
        assert_eq!(pick("rewrite").as_deref(), Some("39cd11cd-aaaa"));
        assert_eq!(pick("nothing like it"), None);
        assert_eq!(pick(""), None);
    }
}
