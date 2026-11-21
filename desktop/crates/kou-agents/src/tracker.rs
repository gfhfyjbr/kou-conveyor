//! Whether the harness in a pane is busy, from its hooks and from its terminal.
//!
//! Two sources, never mixed loosely. Until a lifecycle hook arrives, the
//! terminal is read: output, the screen's last lines, the window title,
//! progress reports and bells. From the first lifecycle hook on, hooks are
//! authoritative, and the terminal is consulted only for the two things hooks
//! cannot say: that the user interrupted a turn (most harnesses fire no hook
//! for it), and, where the harness has no hook for it, that a permission
//! question is on screen.
//!
//! The tracker owns no clock and no timer. Every call takes the time, and
//! [`Tracker::deadline`] says when the answer may change by itself.

use crate::hooks::Signal;
use crate::spec::{Notice, Screen, spec};
use std::time::{Duration, Instant};

/// What the harness in a pane is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Activity {
    /// Free: waiting for a prompt.
    #[default]
    Idle,
    /// A turn is running.
    Working,
    /// Waiting for the user to answer a question.
    Blocked,
}

impl From<Activity> for kou_ipc::Activity {
    /// The protocol's spelling; it has one more state, for a process that
    /// exited, which only the app can know.
    fn from(activity: Activity) -> Self {
        match activity {
            Activity::Idle => kou_ipc::Activity::Idle,
            Activity::Working => kou_ipc::Activity::Working,
            Activity::Blocked => kou_ipc::Activity::Blocked,
        }
    }
}

/// How long the terminal must show no sign of work before a pane read from
/// its terminal leaves `Working`. Shorter flaps between a model's pauses.
pub const QUIET: Duration = Duration::from_secs(3);

/// How long output must keep coming before it counts as work. A redraw after
/// a resize or a focus change is one burst, not a turn.
pub const SUSTAIN: Duration = Duration::from_millis(600);

/// A pause longer than this ends a run of output.
const RUN_GAP: Duration = Duration::from_secs(1);

/// Output this soon after the user's own input is its echo.
const ECHO: Duration = Duration::from_millis(250);

/// Output this soon after a turn was reported finished is the last repaint.
const SETTLE: Duration = Duration::from_secs(1);

/// With hooks in charge: a turn whose hooks and terminal have both been
/// silent this long was interrupted. A working harness animates its status
/// line several times a second, so silence is not a slow tool.
pub const HOOK_STALE: Duration = Duration::from_secs(15);

/// How long a harness that draws a status line while it works may go without
/// that line on screen before a turn its hooks call running is taken to
/// have been stopped by hand (Esc fires no hook): long enough for a redraw
/// that leaves it out for a frame, short beside [`HOOK_STALE`], which waits
/// for the screen to fall silent and so never ends while the user types.
pub const UNSEEN: Duration = Duration::from_secs(3);

/// How often the screen is worth reading while output keeps coming without a
/// pause: a harness at work animates its status line, and a reading taken
/// only when output settles would never be taken.
pub const READ_EVERY: Duration = Duration::from_secs(1);

/// How many of the screen's last lines a reading is worth: a harness's
/// status line stands above its prompt field, its hints, and a list of the
/// agents it runs in the background, which can be long.
pub const READ_LINES: usize = 24;

// The fixed texts are looked for in fewer: nearer the bottom, where the
// harness's own interface is and the conversation is not.
const PATTERN_LINES: usize = 14;

/// A progress report nobody updated or cleared for this long belongs to a
/// program that died.
const PROGRESS_STALE: Duration = Duration::from_secs(300);

/// Screen text for a harness kou has no spec for.
static UNKNOWN: Screen = Screen {
    working: &["esc to interrupt", "esc to cancel", "ctrl+c to interrupt"],
    blocked: &[],
    title_working: &[],
    title_blocked: &[],
    output: true,
    notification: Notice::Attention,
    working_line: None,
};

/// The busy state of one pane. Pure: time is passed in.
#[derive(Clone, Debug)]
pub struct Tracker {
    screen: &'static Screen,
    /// Whether the harness's hooks report permission questions.
    hooks_report_blocked: bool,

    /// A lifecycle hook has fired: hooks are in charge from here on.
    hooked: bool,
    /// What hooks last said.
    hook: Activity,
    hook_at: Instant,

    last_output: Option<Instant>,
    /// When the current run of output began.
    run_start: Option<Instant>,
    /// The last moment the terminal showed work.
    evidence: Option<Instant>,
    /// The last moment a screen reading showed the harness's own sign of a
    /// turn in progress.
    screen_evidence: Option<Instant>,
    /// The last moment a reading showed that sign while hooks said working,
    /// in this stretch of work: proof that this version of the harness
    /// draws a sign this client knows, without which its absence says nothing.
    line_seen: Option<Instant>,
    echo_until: Option<Instant>,
    settle_until: Option<Instant>,

    title_working: bool,
    title_blocked: bool,
    progress_at: Option<Instant>,
    /// What the last screen reading showed, and when it was taken.
    screen_working: bool,
    screen_blocked: bool,
    screen_at: Option<Instant>,
    /// A bell rang while working and nothing has explained it yet.
    attention: bool,
}

impl Tracker {
    /// A tracker for a pane running `harness` (a [`crate::Spec::id`]; an
    /// unknown one is read from its terminal with generic patterns).
    pub fn new(harness: &str, now: Instant) -> Self {
        let spec = spec(harness);
        Tracker {
            screen: spec.map_or(&UNKNOWN, |spec| &spec.screen),
            hooks_report_blocked: spec.is_some_and(|spec| spec.hooks.blocked),
            hooked: false,
            hook: Activity::Idle,
            hook_at: now,
            last_output: None,
            run_start: None,
            evidence: None,
            echo_until: None,
            settle_until: None,
            title_working: false,
            title_blocked: false,
            progress_at: None,
            screen_evidence: None,
            line_seen: None,
            screen_working: false,
            screen_blocked: false,
            screen_at: None,
            attention: false,
        }
    }

    /// Whether hooks have taken over from the terminal.
    pub fn hooked(&self) -> bool {
        self.hooked
    }

    /// A hook fired.
    pub fn hook(&mut self, signal: Signal, now: Instant) {
        // Only a lifecycle hook proves the hooks are wired. An end-of-turn
        // report alone may come from a notify program that never reports a start.
        if matches!(
            signal,
            Signal::SessionStarted
                | Signal::PromptSubmitted
                | Signal::ToolStarted
                | Signal::ToolFinished
        ) {
            self.hooked = true;
        }
        self.hook_at = now;
        let was = self.hook;
        self.hook = match signal {
            Signal::PromptSubmitted | Signal::ToolStarted | Signal::ToolFinished => {
                Activity::Working
            }
            Signal::NeedsInput => Activity::Blocked,
            Signal::TurnFinished | Signal::SessionEnded => Activity::Idle,
            // Compaction restarts the session in the middle of a turn.
            Signal::SessionStarted if self.hook == Activity::Working => Activity::Working,
            Signal::SessionStarted => Activity::Idle,
        };
        // A new stretch of work has its sign still to show.
        if self.hook == Activity::Working && was == Activity::Idle {
            self.line_seen = None;
        }
        match signal {
            Signal::TurnFinished | Signal::SessionEnded => self.finish(now),
            Signal::NeedsInput => self.attention = true,
            _ => self.attention = false,
        }
    }

    /// The terminal produced output.
    pub fn output(&mut self, now: Instant) {
        // Latch an interrupted turn before this output can make it look
        // alive again: what comes now is the user typing the next prompt.
        if self.hooked && self.hook == Activity::Working && self.hook_stale(now) {
            self.hook = Activity::Idle;
        }
        let continues = self
            .last_output
            .is_some_and(|last| now.saturating_duration_since(last) <= RUN_GAP);
        if !continues || self.run_start.is_none() {
            self.run_start = Some(now);
        }
        self.last_output = Some(now);
        if before(now, self.echo_until) || before(now, self.settle_until) {
            self.run_start = None;
            return;
        }
        let sustained = self
            .run_start
            .is_some_and(|start| now.saturating_duration_since(start) >= SUSTAIN);
        // Sustained output starts work; once working, any output continues it,
        // so a pause shorter than the quiet period never reads as an end.
        if (sustained || self.evident(now)) && self.screen.output {
            self.evidence = Some(now);
            // The terminal moves again: whoever was asked has answered.
            self.attention = false;
        }
    }

    /// The user (or another pane) typed into the terminal. What the terminal
    /// prints next is the echo, not the harness working.
    pub fn input(&mut self, now: Instant) {
        self.echo_until = Some(now + ECHO);
        self.run_start = None;
        self.attention = false;
    }

    /// The last lines on screen ([`READ_LINES`] of them). Call it when output
    /// settles, and while it does not, whenever [`Tracker::reading_due`]; a
    /// reading stays in force until the next one. It decides for a harness
    /// without hooks, and says of one with hooks what they left out.
    pub fn screen(&mut self, tail: &str, now: Instant) {
        let lines: Vec<&str> = tail.lines().collect();
        let near = lines[lines.len().saturating_sub(PATTERN_LINES)..]
            .join("\n")
            .to_lowercase();
        let has = |patterns: &[&str]| patterns.iter().any(|pattern| near.contains(pattern));
        let status = self
            .screen
            .working_line
            .is_some_and(|working| lines.iter().any(|line| working(line)));
        self.screen_blocked = has(self.screen.blocked);
        self.screen_working = (has(self.screen.working) || status) && !self.screen_blocked;
        self.screen_at = Some(now);
        if self.screen_working && !before(now, self.settle_until) {
            self.evidence = Some(now);
            self.screen_evidence = Some(now);
        }
        if self.screen_working && self.hooked && self.hook == Activity::Working {
            self.line_seen = Some(now);
        }
        // With patterns for it, the screen itself says whether the bell was a question.
        if !self.screen.blocked.is_empty() {
            self.attention = false;
        }
    }

    /// The window title changed.
    pub fn title(&mut self, title: &str, now: Instant) {
        let lower = title.to_lowercase();
        let has = |patterns: &[&str]| patterns.iter().any(|pattern| lower.contains(pattern));
        let working = has(self.screen.title_working) || starts_with_spinner(title);
        // Leaving a working title starts the quiet period; it does not skip it.
        if (working || self.title_working) && !before(now, self.settle_until) {
            self.evidence = Some(now);
        }
        self.title_working = working;
        self.title_blocked = has(self.screen.title_blocked);
    }

    /// A bell or an OSC notification.
    pub fn notification(&mut self, now: Instant) {
        // Hooks say why; a bell only says that.
        if self.hooked || self.activity(now) != Activity::Working {
            return;
        }
        match self.screen.notification {
            Notice::Attention => self.attention = true,
            Notice::Finished => self.finish(now),
        }
    }

    /// A progress report (OSC 9;4): `Some` while one is shown, `None` when cleared.
    pub fn progress(&mut self, value: Option<u8>, now: Instant) {
        if value.is_some() || self.progress_at.is_some() {
            self.evidence = Some(now);
        }
        self.progress_at = value.map(|_| now);
    }

    /// Whether the screen should be read now although output has not paused
    /// (see [`READ_EVERY`]). Not while the user types, and not in the moment
    /// after a turn ended, when what is on screen is still that turn's.
    pub fn reading_due(&self, now: Instant) -> bool {
        !before(now, self.echo_until)
            && !before(now, self.settle_until)
            && self.screen_at.is_none_or(|at| now >= at + READ_EVERY)
    }

    /// What the pane is doing at `now`.
    pub fn activity(&self, now: Instant) -> Activity {
        if self.hooked {
            return self.hooked_activity(now);
        }
        if self.screen_blocked || self.title_blocked || self.attention {
            Activity::Blocked
        } else if self.title_working || self.progress_live(now) || self.evident(now) {
            Activity::Working
        } else {
            Activity::Idle
        }
    }

    /// When [`Tracker::activity`] may change with no new input. The app asks
    /// again then (and gives a fresh screen reading first).
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        let at = match self.activity(now) {
            // Working by the screen's word against the hooks': until that
            // word is as old as any reading may get.
            Activity::Working if self.hooked && self.hook == Activity::Idle => {
                self.screen_evidence? + QUIET
            }
            Activity::Working if self.hooked => {
                // When the screen falls silent for long enough; and sooner,
                // when its sign of work has been gone for long enough,
                // which a reading taken then confirms or denies.
                let stale = self.hook_alive_at() + HOOK_STALE;
                let unseen = self.line_seen.map(|seen| seen + UNSEEN);
                match unseen {
                    Some(unseen) if unseen > now => unseen.min(stale),
                    _ => stale,
                }
            }
            Activity::Working if self.title_working => return None,
            Activity::Working => {
                let quiet = self.evidence.map(|at| at + QUIET);
                let progress = self
                    .progress_live(now)
                    .then(|| self.progress_alive_at().map(|at| at + PROGRESS_STALE))
                    .flatten();
                quiet.max(progress)?
            }
            Activity::Idle | Activity::Blocked => return None,
        };
        (at > now).then_some(at)
    }

    fn hooked_activity(&self, now: Instant) -> Activity {
        // A screen reading taken after the last hook can refine what it said.
        let fresh = self.screen_at.is_some_and(|at| at >= self.hook_at);
        match self.hook {
            // Hooks say nothing runs, and the harness's own screen says
            // otherwise: a turn the hooks did not report. Their program could
            // not be run; the turn was woken without a prompt; or the work
            // is another session of the harness, shown in this pane and
            // reporting as some other. The screen is believed then, from
            // readings taken once the last reported turn had left it.
            // (A question on screen is believed only of a harness whose
            // hooks cannot see one: elsewhere "do you want to proceed" is as
            // likely the agent's own last words.)
            Activity::Idle
                if !self.hooks_report_blocked
                    && self.screen_blocked
                    && self.read_after_the_hook() =>
            {
                Activity::Blocked
            }
            Activity::Idle if self.screen_evident(now) => Activity::Working,
            Activity::Idle => Activity::Idle,
            // Approved: the tool runs, and its hook fires only when it ends.
            Activity::Blocked if fresh && self.screen_working => Activity::Working,
            Activity::Blocked => Activity::Blocked,
            Activity::Working if !self.hooks_report_blocked && fresh && self.screen_blocked => {
                Activity::Blocked
            }
            Activity::Working if self.stopped_unseen(now) => Activity::Idle,
            Activity::Working if self.hook_stale(now) => Activity::Idle,
            Activity::Working => Activity::Working,
        }
    }

    // Hooks say a turn runs, the harness drew its sign of one earlier in
    // this stretch, and the screen has been without it since: the turn was
    // stopped by hand, or its last hook never came. Told from a reading
    // taken after the last hook had time to show on screen, and never while
    // a question stands there.
    fn stopped_unseen(&self, now: Instant) -> bool {
        self.hook == Activity::Working
            && !self.screen_working
            && !self.screen_blocked
            && self
                .screen_at
                .is_some_and(|read| read >= self.hook_at + SETTLE)
            && self.line_seen.is_some_and(|seen| now >= seen + UNSEEN)
    }

    /// The last sign of life while hooks say working.
    fn hook_alive_at(&self) -> Instant {
        self.last_output
            .map_or(self.hook_at, |at| at.max(self.hook_at))
    }

    fn hook_stale(&self, now: Instant) -> bool {
        now >= self.hook_alive_at() + HOOK_STALE
    }

    fn progress_alive_at(&self) -> Option<Instant> {
        let at = self.progress_at?;
        Some(self.last_output.map_or(at, |output| output.max(at)))
    }

    fn progress_live(&self, now: Instant) -> bool {
        self.progress_alive_at()
            .is_some_and(|at| now < at + PROGRESS_STALE)
    }

    fn evident(&self, now: Instant) -> bool {
        self.evidence.is_some_and(|at| now < at + QUIET)
    }

    fn screen_evident(&self, now: Instant) -> bool {
        self.screen_evidence.is_some_and(|at| now < at + QUIET)
    }

    // Whether the last screen reading was taken late enough after the last
    // hook not to show what that hook's turn left behind.
    fn read_after_the_hook(&self) -> bool {
        self.screen_at.is_some_and(|at| at >= self.hook_at + SETTLE)
    }

    /// The turn is known to be over: forget what the terminal suggested, and
    /// do not take its last repaint for new work.
    fn finish(&mut self, now: Instant) {
        self.evidence = None;
        self.screen_evidence = None;
        self.line_seen = None;
        self.run_start = None;
        self.attention = false;
        self.screen_working = false;
        self.progress_at = None;
        self.settle_until = Some(now + SETTLE);
    }
}

fn before(now: Instant, until: Option<Instant>) -> bool {
    until.is_some_and(|until| now < until)
}

/// Many terminal programs animate a braille spinner at the start of the title
/// while they work.
fn starts_with_spinner(title: &str) -> bool {
    title
        .trim_start()
        .chars()
        .next()
        .is_some_and(|first| ('\u{2801}'..='\u{28ff}').contains(&first))
}

#[cfg(test)]
mod tests {
    use super::Activity::{Blocked, Idle, Working};
    use super::*;

    enum Step {
        Hook(Signal),
        Output,
        /// Output every 100 ms from this step's time until the given time.
        OutputUntil(u64),
        Input,
        Screen(&'static str),
        Title(&'static str),
        Bell,
        Progress(Option<u8>),
        Expect(Activity),
    }
    use Step::*;

    /// Play a timeline: each step happens at its millisecond.
    fn play(harness: &str, timeline: &[(u64, Step)]) {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut tracker = Tracker::new(harness, start);
        let mut clock = 0;
        for (index, (ms, step)) in timeline.iter().enumerate() {
            assert!(*ms >= clock, "{harness} step {index}: time runs backwards");
            clock = *ms;
            let now = at(*ms);
            match step {
                Hook(signal) => tracker.hook(*signal, now),
                Output => tracker.output(now),
                OutputUntil(end) => {
                    let mut tick = *ms;
                    while tick <= *end {
                        tracker.output(at(tick));
                        tick += 100;
                    }
                    clock = *end;
                }
                Input => tracker.input(now),
                Screen(tail) => tracker.screen(tail, now),
                Title(title) => tracker.title(title, now),
                Bell => tracker.notification(now),
                Progress(value) => tracker.progress(*value, now),
                Expect(activity) => {
                    assert_eq!(
                        tracker.activity(now),
                        *activity,
                        "{harness} step {index} at {ms} ms"
                    );
                    // A deadline is always ahead, and nothing changes before it.
                    if let Some(deadline) = tracker.deadline(now) {
                        assert!(deadline > now, "{harness} step {index}");
                        let just_before = deadline - Duration::from_millis(1);
                        assert_eq!(
                            tracker.activity(just_before),
                            *activity,
                            "{harness} step {index}: changed before its deadline"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn hooks_drive_a_harness_that_has_them() {
        play(
            "claude",
            &[
                (0, Expect(Idle)),
                (10, Hook(Signal::SessionStarted)),
                (20, Expect(Idle)),
                // Typing a prompt redraws the input box; that is not work.
                (1_000, OutputUntil(4_000)),
                (4_000, Expect(Idle)),
                (4_100, Hook(Signal::PromptSubmitted)),
                (4_100, Expect(Working)),
                (4_200, OutputUntil(9_000)),
                (9_000, Hook(Signal::ToolStarted)),
                (9_050, Expect(Working)),
                (9_100, OutputUntil(20_000)),
                (20_000, Hook(Signal::ToolFinished)),
                // A model thinking for a long while still animates its status line.
                (20_100, OutputUntil(60_000)),
                (60_000, Expect(Working)),
                (60_100, Hook(Signal::TurnFinished)),
                (60_100, Expect(Idle)),
                // The final repaint must not look like a new turn.
                (60_150, OutputUntil(61_500)),
                (61_500, Expect(Idle)),
                (70_000, Expect(Idle)),
            ],
        );
    }

    #[test]
    fn a_permission_question_blocks_until_it_is_answered() {
        play(
            "claude",
            &[
                (0, Hook(Signal::SessionStarted)),
                (100, Hook(Signal::PromptSubmitted)),
                (200, Hook(Signal::ToolStarted)),
                (300, Hook(Signal::NeedsInput)),
                (300, Expect(Blocked)),
                // Nobody answers for a minute: still blocked, never stale.
                (60_000, Expect(Blocked)),
                // A bell while blocked changes nothing.
                (60_100, Bell),
                (60_100, Expect(Blocked)),
                // Approved: the tool runs for a while before its hook fires,
                // and the screen shows the turn is alive again.
                (61_000, OutputUntil(62_000)),
                (62_000, Screen("* Running tests (12s - esc to interrupt)")),
                (62_000, Expect(Working)),
                (62_100, OutputUntil(70_000)),
                (70_000, Hook(Signal::ToolFinished)),
                (70_000, Expect(Working)),
                (70_500, Hook(Signal::TurnFinished)),
                (70_500, Expect(Idle)),
            ],
        );
        // Denied instead: the next hook is the end of the turn.
        play(
            "codex",
            &[
                (0, Hook(Signal::PromptSubmitted)),
                (500, Hook(Signal::NeedsInput)),
                (9_000, Expect(Blocked)),
                (9_100, Hook(Signal::TurnFinished)),
                (9_100, Expect(Idle)),
            ],
        );
    }

    #[test]
    fn an_interrupted_turn_does_not_stay_working_forever() {
        play(
            "claude",
            &[
                (0, Hook(Signal::PromptSubmitted)),
                (100, OutputUntil(5_000)),
                // Esc: no Stop hook fires, the status line just stops moving.
                (5_000, Expect(Working)),
                (19_900, Expect(Working)),
                (20_000, Expect(Idle)),
                // The user types the next prompt; its echo must not revive the turn.
                (25_000, OutputUntil(28_000)),
                (28_000, Expect(Idle)),
                (28_100, Hook(Signal::PromptSubmitted)),
                (28_100, Expect(Working)),
            ],
        );
    }

    #[test]
    fn a_turn_stopped_by_hand_is_over_once_its_status_line_is_gone() {
        let working = "\u{273d} Drizzling\u{2026} (12s \u{b7} \u{2193} 1.9k tokens)\n\n> ";
        let stopped = "  \u{23bf} Interrupted \u{b7} What should Claude do instead?\n\n> ";
        play(
            "claude",
            &[
                (0, Hook(Signal::PromptSubmitted)),
                (100, OutputUntil(2_000)),
                (2_000, Screen(working)),
                (2_100, OutputUntil(3_000)),
                (3_000, Screen(working)),
                (3_500, Hook(Signal::ToolFinished)),
                // Esc: no hook fires. The line is gone at the next reading,
                // which comes too soon after a hook to be believed alone.
                (4_000, Screen(stopped)),
                (4_000, Expect(Working)),
                (5_000, Screen(stopped)),
                (5_900, Expect(Working)),
                (6_000, Expect(Idle)),
                // The user types on: output, and still no turn.
                (6_100, OutputUntil(8_900)),
                (9_000, Screen(stopped)),
                (9_100, OutputUntil(12_000)),
                (12_000, Expect(Idle)),
                (12_100, Hook(Signal::PromptSubmitted)),
                (12_100, Expect(Working)),
                // A redraw that leaves the line out for one reading ends nothing.
                (13_000, Screen(working)),
                (14_000, Screen("> ")),
                (15_000, Screen(working)),
                (17_900, Expect(Working)),
            ],
        );
        // A harness whose sign of work was never seen in this stretch (a
        // version that draws another) is not judged by its absence.
        play(
            "claude",
            &[
                (0, Hook(Signal::PromptSubmitted)),
                (100, OutputUntil(1_900)),
                (2_000, Screen("* Pondering deeply\n\n> ")),
                (2_100, OutputUntil(4_900)),
                (5_000, Screen("* Pondering deeply\n\n> ")),
                (5_100, OutputUntil(9_000)),
                (9_000, Expect(Working)),
            ],
        );
    }

    #[test]
    fn a_session_restart_in_the_middle_of_a_turn_keeps_it_working() {
        play(
            "codex",
            &[
                (0, Hook(Signal::SessionStarted)),
                (100, Hook(Signal::PromptSubmitted)),
                // Compaction: SessionStart with source "compact".
                (5_000, Hook(Signal::SessionStarted)),
                (5_000, Expect(Working)),
                (6_000, Hook(Signal::TurnFinished)),
                (6_100, Hook(Signal::SessionEnded)),
                (6_200, Hook(Signal::SessionStarted)),
                (6_200, Expect(Idle)),
            ],
        );
    }

    #[test]
    fn a_harness_without_hooks_is_read_from_its_output() {
        play(
            "goose",
            &[
                (0, Expect(Idle)),
                // One burst: a redraw.
                (1_000, Output),
                (1_100, Output),
                (1_200, Expect(Idle)),
                // Sustained output: a turn.
                (5_000, OutputUntil(5_500)),
                (5_500, Expect(Idle)),
                (5_600, OutputUntil(12_000)),
                (12_000, Expect(Working)),
                // A pause shorter than the quiet period is not the end.
                (14_500, Expect(Working)),
                (14_600, OutputUntil(20_000)),
                (20_000, Expect(Working)),
                (22_900, Expect(Working)),
                (23_000, Expect(Idle)),
                (60_000, Expect(Idle)),
            ],
        );
    }

    #[test]
    fn typing_is_not_work() {
        play(
            "shell",
            &[
                // A key every 150 ms for five seconds, each echoed at once.
                (0, Input),
                (20, Output),
                (150, Input),
                (170, Output),
                (300, Input),
                (320, Output),
                (450, Input),
                (470, Output),
                (600, Input),
                (620, Output),
                (750, Input),
                (770, Output),
                (900, Input),
                (920, Output),
                (1_000, Expect(Idle)),
                // Enter, and the command prints for a while.
                (1_050, Input),
                (1_060, Output),
                (1_400, OutputUntil(6_000)),
                (6_000, Expect(Working)),
                (9_100, Expect(Idle)),
            ],
        );
    }

    #[test]
    fn the_screen_decides_for_a_harness_with_dependable_text() {
        // Hooks never fired (an old version, or hooks disabled by policy).
        play(
            "claude",
            &[
                (0, OutputUntil(3_000)),
                // Output alone proves nothing for Claude Code: it redraws on every key.
                (3_000, Expect(Idle)),
                (
                    3_100,
                    Screen("> fix the build\n\n* Thinking... (esc to interrupt)"),
                ),
                (3_100, Expect(Working)),
                (3_200, OutputUntil(8_000)),
                (8_000, Screen("* Compiling... (5s - esc to interrupt)")),
                (10_900, Expect(Working)),
                (11_000, Expect(Idle)),
                // A question appears.
                (20_000, Screen("* Working (esc to interrupt)")),
                (
                    20_500,
                    Screen(
                        "Bash command\n\n  rm -rf target\n\nDo you want to proceed?\n> 1. Yes\n  2. No",
                    ),
                ),
                (20_500, Expect(Blocked)),
                (90_000, Expect(Blocked)),
                // Answered.
                (90_100, Screen("* Working (esc to interrupt)")),
                (90_100, Expect(Working)),
                (93_100, Expect(Idle)),
            ],
        );
    }

    #[test]
    fn a_turn_the_hooks_did_not_report_is_read_from_the_screen() {
        play(
            "claude",
            &[
                (10, Hook(Signal::SessionStarted)),
                (4_000, Hook(Signal::PromptSubmitted)),
                (4_100, OutputUntil(9_000)),
                (9_000, Screen("* Thinking... (4s - esc to interrupt)")),
                (9_100, Hook(Signal::TurnFinished)),
                // What the turn left on screen is not a new one.
                (9_300, Screen("* Thinking... (5s - esc to interrupt)")),
                (9_300, Expect(Idle)),
                (11_000, Screen("> ")),
                (20_000, Expect(Idle)),
                // Work the hooks say nothing of: their program could not be
                // run, or it is another session shown in this pane. The
                // status line animates, and is read as it goes.
                (30_000, OutputUntil(31_000)),
                (31_000, Screen("* Cogitating... (1s - esc to interrupt)")),
                (31_000, Expect(Working)),
                (31_100, OutputUntil(32_000)),
                (32_000, Screen("* Cogitating... (2s - esc to interrupt)")),
                (32_100, OutputUntil(33_000)),
                (33_000, Screen("* Cogitating... (3s - esc to interrupt)")),
                (35_900, Expect(Working)),
                // It stops; the screen no longer says so, and neither does the pane.
                (36_000, Expect(Idle)),
                (36_100, Screen("> ")),
                (40_000, Expect(Idle)),
                // A question in the last lines is not taken for the
                // harness's own: its hooks report those, and an agent may
                // end its turn on the same words.
                (
                    50_000,
                    Screen("Shall I delete target?\n\nDo you want to proceed?\n\n> "),
                ),
                (50_000, Expect(Idle)),
                (90_100, Screen("* Working (esc to interrupt)")),
                (90_100, Expect(Working)),
                // A hook that does come is believed again at once.
                (91_000, Hook(Signal::ToolStarted)),
                (91_000, Expect(Working)),
                (92_000, Hook(Signal::TurnFinished)),
                (92_000, Expect(Idle)),
            ],
        );
        // Output alone is no such word, for a harness whose output counts
        // while it has no hooks: typing and scrolling make output too.
        play(
            "codex",
            &[
                (10, Hook(Signal::SessionStarted)),
                (1_000, Hook(Signal::PromptSubmitted)),
                (2_000, Hook(Signal::TurnFinished)),
                (10_000, OutputUntil(20_000)),
                (20_000, Expect(Idle)),
                (20_100, Screen("* Working (3s - esc to interrupt)")),
                (20_100, Expect(Working)),
            ],
        );
    }

    #[test]
    fn a_session_waiting_for_its_background_agents_is_working() {
        // Claude Code 2.1.322 as it stood on screen: the session's own turn
        // has ended (`Stop` fired), an agent it started still runs, and the
        // status line counts on. Twenty-four lines are read, because the
        // list of agents under the hints pushes that line up.
        let waiting = "\u{273b} Waiting for 1 background agent to finish\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\
                       \u{273d} Drizzling\u{2026} (7m 21s \u{b7} \u{2193} 39.9k tokens)\n\n\
                       \u{2500}\u{2500}\u{2500}\u{2500}\n> \n\u{2500}\u{2500}\u{2500}\u{2500}\n\
                       \u{23f5}\u{23f5} auto mode on (shift+tab to cycle) \u{b7} \u{2190} 1 agent\n\n\n\
                       \u{25ef} general-purpose  Reviewing a screenshot";
        let rested = "\u{273b} Baked for 9m 2s\n\n\u{2500}\u{2500}\u{2500}\u{2500}\n> \n\u{2500}\u{2500}\u{2500}\u{2500}\n\
                      \u{23f5}\u{23f5} auto mode on (shift+tab to cycle)";
        play(
            "claude",
            &[
                (10, Hook(Signal::SessionStarted)),
                (1_000, Hook(Signal::PromptSubmitted)),
                (1_100, OutputUntil(5_000)),
                (5_000, Hook(Signal::TurnFinished)),
                (5_000, Expect(Idle)),
                (5_100, OutputUntil(6_200)),
                (6_200, Screen(waiting)),
                (6_200, Expect(Working)),
                (6_300, OutputUntil(7_300)),
                (7_300, Screen(waiting)),
                (10_200, Expect(Working)),
                (10_300, Screen(rested)),
                (10_300, Expect(Idle)),
            ],
        );
        for line in [
            "\u{273d} Drizzling\u{2026} (7m 21s \u{b7} \u{2193} 39.9k tokens)",
            "\u{b7} Drizzling\u{2026} (6m 1s \u{b7} \u{2193} 32.8k tokens)",
            "\u{2722} Schlepping\u{2026} (44s \u{b7} \u{2193} 2.8k tokens \u{b7} thought for 1s)",
            "  \u{2722} Bootstrapping\u{2026} (1h 2m \u{b7} thinking more with xhigh effort)",
        ] {
            assert!(crate::spec::claude_status(line), "{line}");
        }
        for line in [
            "\u{273b} Waiting for 1 background agent to finish",
            "\u{273b} Baked for 9m 2s",
            "  \u{23f5}\u{23f5} auto mode on (shift+tab to cycle) \u{b7} \u{2190} 1 agent",
            "\u{25ef} general-purpose  Reviewing a screenshot",
            // The same words in what an agent wrote, as a quotation and in a list.
            "It shows `\u{273d} Drizzling\u{2026} (7m 21s)` while it works.",
            "- `\u{273d} Drizzling\u{2026} (7m 21s)`",
            "\u{25cf} Done\u{2026} (see the three files)",
            "",
        ] {
            assert!(!crate::spec::claude_status(line), "{line}");
        }
    }

    #[test]
    fn the_screen_is_read_now_and_then_while_output_never_pauses() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut tracker = Tracker::new("claude", start);
        assert!(tracker.reading_due(at(0)), "never read yet");
        tracker.screen("> ", at(0));
        assert!(!tracker.reading_due(at(900)));
        assert!(tracker.reading_due(at(1_000)));
        // Not while the user types: what changes on screen is their text.
        tracker.input(at(1_000));
        assert!(!tracker.reading_due(at(1_100)));
        assert!(tracker.reading_due(at(1_300)));
        // And not in the moment after a turn ended.
        tracker.hook(Signal::PromptSubmitted, at(2_000));
        tracker.hook(Signal::TurnFinished, at(3_000));
        assert!(!tracker.reading_due(at(3_500)));
        assert!(tracker.reading_due(at(4_000)));
    }

    #[test]
    fn a_bell_while_working_means_the_harness_wants_the_user() {
        play(
            "grok",
            &[
                (0, OutputUntil(4_000)),
                (4_000, Expect(Working)),
                (4_100, Bell),
                (4_100, Expect(Blocked)),
                (60_000, Expect(Blocked)),
                // The user answers; the echo is not work, the turn that follows is.
                (60_100, Input),
                (60_100, Expect(Idle)),
                (60_150, Output),
                (60_200, Expect(Idle)),
                (60_400, OutputUntil(62_000)),
                (62_000, Expect(Working)),
                (65_100, Expect(Idle)),
            ],
        );
        // A bell with nothing running is just a bell.
        play(
            "grok",
            &[(0, Bell), (0, Expect(Idle)), (5_000, Expect(Idle))],
        );
        // Amp rings when a turn ends, whatever the reason.
        play(
            "amp",
            &[
                (0, OutputUntil(4_000)),
                (4_000, Expect(Working)),
                (4_100, Bell),
                (4_100, Expect(Idle)),
                (4_150, OutputUntil(4_800)),
                (4_800, Expect(Idle)),
            ],
        );
    }

    #[test]
    fn an_end_of_turn_report_alone_does_not_take_over() {
        // Aider's notification command, or Codex's notify with untrusted hooks.
        play(
            "aider",
            &[
                (0, OutputUntil(6_000)),
                (6_000, Expect(Working)),
                (6_100, Hook(Signal::TurnFinished)),
                (6_100, Expect(Idle)),
                // Still read from the terminal afterwards.
                (10_000, OutputUntil(15_000)),
                (15_000, Expect(Working)),
                (
                    15_100,
                    Screen("Run shell command? (Y)es/(N)o/(D)on't ask again [Yes]:"),
                ),
                (15_100, Expect(Blocked)),
                (40_000, Expect(Blocked)),
                (40_100, Input),
                (40_200, Screen("Running pytest")),
                (40_300, OutputUntil(45_000)),
                (45_000, Expect(Working)),
                (45_100, Hook(Signal::TurnFinished)),
                (45_100, Expect(Idle)),
            ],
        );
    }

    #[test]
    fn titles_and_progress_reports_count_as_work() {
        // kou's own TUI marks a running turn in its title.
        play(
            "kou",
            &[
                (0, Title("desktop \u{2014} new session")),
                (0, Expect(Idle)),
                (1_000, Title("\u{25cf} desktop \u{2014} fix the build")),
                (1_000, Expect(Working)),
                // No output at all for a long while: the title still holds.
                (120_000, Expect(Working)),
                (120_100, Title("desktop \u{2014} fix the build")),
                // The quiet period still applies on the way out.
                (120_100, Expect(Working)),
                (123_100, Expect(Idle)),
            ],
        );
        // A braille spinner at the start of any title.
        play(
            "shell",
            &[
                (0, Title("\u{2819} building")),
                (0, Expect(Working)),
                (500, Title("\u{2839} building")),
                (30_000, Expect(Working)),
                (30_100, Title("zsh")),
                (33_200, Expect(Idle)),
            ],
        );
        // OSC 9;4 from a build tool in a shell.
        play(
            "shell",
            &[
                (0, Progress(Some(10))),
                (0, Expect(Working)),
                (20_000, Progress(Some(80))),
                (60_000, Expect(Working)),
                (60_100, Progress(None)),
                (60_100, Expect(Working)),
                (63_100, Expect(Idle)),
                // A report nobody ever clears expires.
                (100_000, Progress(Some(50))),
                (100_000, Expect(Working)),
                (399_000, Expect(Working)),
                (400_000, Expect(Idle)),
            ],
        );
    }

    #[test]
    fn a_harness_whose_hooks_cannot_see_questions_uses_the_screen_for_them() {
        // Pi has full hooks but none for a question an extension might ask;
        // its spec has no question text either, so hooks alone decide.
        play(
            "pi",
            &[
                (0, Hook(Signal::PromptSubmitted)),
                (100, OutputUntil(3_000)),
                (3_000, Screen("anything at all")),
                (3_000, Expect(Working)),
                (3_100, Hook(Signal::TurnFinished)),
                (3_100, Expect(Idle)),
            ],
        );
        let start = Instant::now();
        // An unknown harness reporting Claude-style hooks is tracked by them.
        let mut tracker = Tracker::new("somebody-elses-agent", start);
        assert!(!tracker.hooked());
        tracker.hook(Signal::PromptSubmitted, start);
        assert!(tracker.hooked());
        assert_eq!(tracker.activity(start), Working);
        assert_eq!(tracker.deadline(start), Some(start + HOOK_STALE));
        tracker.hook(Signal::TurnFinished, start + Duration::from_secs(1));
        assert_eq!(tracker.activity(start + Duration::from_secs(1)), Idle);
        assert_eq!(tracker.deadline(start + Duration::from_secs(1)), None);
    }

    #[test]
    fn deadlines_name_the_moment_the_answer_changes() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut tracker = Tracker::new("goose", start);
        assert_eq!(tracker.deadline(start), None);
        for tick in (0..=2_000).step_by(100) {
            tracker.output(at(tick));
        }
        assert_eq!(tracker.activity(at(2_000)), Working);
        assert_eq!(tracker.deadline(at(2_000)), Some(at(2_000) + QUIET));
        assert_eq!(tracker.activity(at(2_000) + QUIET), Idle);
        assert_eq!(tracker.deadline(at(2_000) + QUIET), None);
        // Blocked waits for the user, not for a clock.
        tracker.output(at(10_000));
        for tick in (10_000..=12_000).step_by(100) {
            tracker.output(at(tick));
        }
        tracker.notification(at(12_100));
        assert_eq!(tracker.activity(at(12_100)), Blocked);
        assert_eq!(tracker.deadline(at(12_100)), None);
    }

    #[test]
    fn devin_is_read_from_its_status_line() {
        // As Devin 3000.14.30821 drew them: the status line over the input
        // while a turn runs, the foot of a permission question, the trust
        // question a first launch in a folder asks, and the prompt at rest.
        let working = "\u{284e}\u{28b1} Thinking \u{b7} 1s (esc twice to interrupt)\n\n\u{276d} Guide Devin while it works";
        let permission = " \u{23fa} Running command\n \u{2514} $ sleep 4\n\n\u{276d} Allow       \u{2190} \u{25fc}\u{25fc}\u{25fc}\u{25fc} \u{2192} Once     Exec(sleep)\n  Bypass mode\n  Deny\n\n\u{2191}\u{2193} select \u{b7} \u{2190}\u{2192} 1-4 scope \u{b7} e edit \u{b7} \u{21b5} confirm \u{b7} esc cancel";
        let trust = " \u{2731} Do you trust the authors of this directory?\n   For security, devin should not be run in directories with untrusted content.\n\n \u{276d} 1 Yes, trust\n   2 No, exit";
        let idle = " done\n\n \u{23fa} /work/api\n\n\u{276d} Ask Devin to build features, fix bugs, or work on your code";
        play(
            "devin",
            &[
                (0, Expect(Idle)),
                // A first launch in this folder asks its trust question.
                (100, Screen(trust)),
                (100, Expect(Blocked)),
                // "1 Yes, trust" typed; the pane rests at its prompt.
                (1_000, Input),
                (1_050, Screen(idle)),
                (1_050, Expect(Idle)),
                // Output alone is not work: typing and redraws make output too.
                (2_000, OutputUntil(5_000)),
                (5_000, Expect(Idle)),
                // A turn: the status line stands over the input.
                (5_100, Screen(working)),
                (5_100, Expect(Working)),
                (8_000, Expect(Working)),
                // A permission question waits for the user.
                (8_100, Screen(permission)),
                (8_100, Expect(Blocked)),
                (60_000, Expect(Blocked)),
                // Allowed: the line is back, and the turn runs on.
                (60_100, Input),
                (60_200, Screen(working)),
                (60_200, Expect(Working)),
                // The turn ends; the quiet period after its last sign runs out.
                (63_100, Screen(idle)),
                (63_100, Expect(Working)),
                (63_200, Expect(Idle)),
                (70_000, Expect(Idle)),
            ],
        );
    }
}
