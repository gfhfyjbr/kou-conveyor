//! The agents a conversation's agent started, and what stands beside the
//! conversation: what the app tells the view of them ([`Nested`],
//! [`Beside`]), where each one's card stands in the transcript, and the
//! words and times the cards show. How the view looks around them is a
//! [`Look`] the app chooses; the cards are drawn in [`crate::cards`].
//!
//! Times count only while they are seen: a card whose time runs asks for the
//! view to be drawn again on the next whole second while it is on screen
//! ([`ThreadView::keep_ticking`]), and nothing ticks once none is.

use crate::view::ThreadView;
use gpui::{Context, Task};
use kou_thread::{Entry, ThreadState};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant, SystemTime},
};

/// How a conversation is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Look {
    /// As kou's web cockpit draws it: what the view is today.
    #[default]
    Cockpit,
    /// The quieter look, for a chat with a harness.
    Studio,
}

/// An agent the conversation's agent started.
#[derive(Clone, Debug, PartialEq)]
pub struct Nested {
    /// The app's name for it; it comes back in `ThreadViewEvent::OpenNested`.
    pub id: String,
    pub title: String,
    /// Its harness, as `kou_ui`'s brand marks know it (`claude`, `codex`, ...), and the two letters shown where there is no mark.
    pub harness: String,
    pub code: String,
    pub state: NestedState,
    /// Its latest words on one line; empty when it has said nothing yet.
    pub detail: String,
    /// When it began what it is doing now, or when it finished.
    pub since: SystemTime,
    /// How many entries the conversation had when it was started: its card stands after that many. `None`: after the last entry at the time the view first hears of it.
    pub after: Option<usize>,
}

/// What a nested agent is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NestedState {
    Running,
    Asking,
    Finished,
    Failed,
    Gone,
}

impl NestedState {
    /// Whether it is still at work, and its time counts.
    pub(crate) fn live(self) -> bool {
        matches!(self, NestedState::Running | NestedState::Asking)
    }
}

/// What stands beside a conversation: where it works and who it works with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Beside {
    /// The folder's name, and the folder.
    pub project: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    /// Lines added and removed against what the branch follows, when known.
    pub changes: Option<(u32, u32)>,
    /// The agent that started this conversation's agent, when one did.
    pub parent: Option<Nested>,
    /// How many agents it started are gone for good (closed panes).
    pub earlier: usize,
}

/// What the view shares with the prompt area and its field, which are drawn
/// apart from it: the look, the agents the agent waits on, and how far the
/// column stands off for the card beside it.
#[derive(Default)]
pub(crate) struct Around {
    pub look: Cell<Look>,
    /// The titles of the agents the agent waits on, in the order given.
    pub waiting: RefCell<Vec<String>>,
    /// The commands the agent left running in the background that still
    /// run, as they were typed.
    pub behind: RefCell<Vec<String>>,
    /// How much room the card beside the transcript takes from the column's
    /// right, unscaled: the column is centred in what is left.
    pub inset: Cell<f32>,
    /// The card beside shows the context: the line under the field need not.
    pub context_beside: Cell<bool>,
}

impl Around {
    pub(crate) fn studio(&self) -> bool {
        self.look.get() == Look::Studio
    }
}

/// What the view keeps of the agents its agent started.
#[derive(Default)]
pub(crate) struct Nest {
    /// All of them, in the order started.
    pub agents: Vec<Nested>,
    /// After how many entries each one's card stands, by id: its own
    /// `after`, or how many there were when the view first heard of it.
    pub places: HashMap<String, usize>,
    /// Cards that came while the view watched, and when: they arrive.
    pub arrived: HashMap<String, Instant>,
    /// Those the agent waits for now, by id.
    pub waiting_on: Vec<String>,
    /// The card beside the transcript.
    pub beside: Option<Beside>,
    /// The timer that draws the counting times again, while a card that
    /// counts is on screen.
    pub ticking: Option<Task<()>>,
    /// Whether the last frame drew a card whose time counts.
    pub counting: Rc<Cell<bool>>,
    /// When each turn began and ended, as the view saw it, by the entry of
    /// the prompt it answered ([`NO_PROMPT`] for none).
    pub turns: HashMap<usize, Turn>,
    /// When each entry came, for those that came while the view watched.
    pub seen: HashMap<usize, Instant>,
    /// The runs of work that are shown open, by their first entry
    /// (`Look::Studio` folds each into one line once it is over).
    pub worked_open: HashSet<usize>,
    /// The width the view was last laid out at, unscaled, and the room its
    /// card beside took then; see [`beside_room`].
    pub width: Rc<Cell<f32>>,
    /// The height of the card beside as last drawn, unscaled.
    pub beside_height: Rc<Cell<f32>>,
    /// What the user did with the card beside: pulled it out where the pane
    /// leaves it no room (`true`), or put it away where it has room
    /// (`false`). `None` leaves it to the room there is.
    pub beside_pulled: Option<bool>,
    /// Whether the card beside showed at the last frame, and when it last
    /// began to come (`true`) or to go: it does either over a few frames.
    pub beside_was: bool,
    pub beside_moved: Option<(Instant, bool)>,
    /// Rows of the transcript on screen at the last frame, for the ticks.
    pub shown: Rc<Cell<(usize, usize)>>,
    /// The wall clock, pinned: for tests and captures.
    pub clock: Option<SystemTime>,
}

/// The key of a turn that answered no prompt of this view's.
pub(crate) const NO_PROMPT: usize = usize::MAX;

/// A turn the view saw run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Turn {
    pub began: Instant,
    pub ended: Option<Instant>,
}

/// Where a card that stands after `after` entries is drawn, in a transcript
/// of `count` entries: in which row, and whether at its top (before the
/// entry) rather than its bottom.
pub(crate) fn host_row(after: usize, count: usize) -> (usize, bool) {
    if count == 0 || after == 0 {
        (0, true)
    } else {
        (after.min(count) - 1, false)
    }
}

/// The card beside the transcript: its width, how far it stands from the
/// pane's top and right edges, and the least room between it and the column.
pub(crate) const BESIDE_WIDTH: f32 = crate::studio::sized(244.);
pub(crate) const BESIDE_EDGE: f32 = 10.5;
pub(crate) const BESIDE_TOP: f32 = 12.;
pub(crate) const BESIDE_GAP: f32 = 24.;

/// How much room the card beside takes from the right of a view `width`
/// wide whose column is at most `measure` wide (padding included): none,
/// ever. The card shows only where the column, at its full width and in the
/// middle of the pane, clears it, with as much room on its left as the card
/// takes on its right; in a narrower pane there is no card (`None`). The
/// text never moves over or narrows to make room for it.
pub(crate) fn beside_room(width: f32, measure: f32) -> Option<f32> {
    let reserve = BESIDE_WIDTH + BESIDE_EDGE + BESIDE_GAP;
    (width >= measure + 2. * reserve).then_some(0.)
}

/// How long something has run: `22s`, `13m 20s`, `2h 5m`.
pub(crate) fn ran_for(took: Duration) -> String {
    let seconds = took.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    }
}

/// The same, shorter, for a row with little room: `22s`, `13m`, `2h`.
pub(crate) fn ran_for_short(took: Duration) -> String {
    let seconds = took.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3600)
    }
}

/// When something happened, by the clock on the wall: `3:06 AM`, and the
/// day before it when that was not today: `Nov 10, 3:06 AM`.
pub(crate) fn wall_clock(at: SystemTime, now: SystemTime) -> String {
    let (Some(then), Some(today)) = (local(at), local(now)) else {
        return String::new();
    };
    let hour = match then.hour % 12 {
        0 => 12,
        hour => hour,
    };
    let half = if then.hour < 12 { "AM" } else { "PM" };
    let time = format!("{hour}:{:02} {half}", then.minute);
    if (then.year, then.day) == (today.year, today.day) {
        time
    } else {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        format!(
            "{} {}, {time}",
            MONTHS[then.month.clamp(0, 11) as usize],
            then.date
        )
    }
}

/// A moment on the local clock.
struct Local {
    year: i32,
    /// Day of the year, month (0 is January), day of the month, hour, minute.
    day: i32,
    month: i32,
    date: i32,
    hour: i32,
    minute: i32,
}

/// `at` on the local clock: the system's time zone where the platform says
/// it, else UTC.
fn local(at: SystemTime) -> Option<Local> {
    let seconds = match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).ok()?,
        Err(before) => -i64::try_from(before.duration().as_secs()).ok()?,
    };
    local_from(seconds)
}

#[cfg(all(unix, target_pointer_width = "64"))]
fn local_from(seconds: i64) -> Option<Local> {
    use std::ffi::{c_char, c_int, c_long};
    #[repr(C)]
    struct Tm {
        sec: c_int,
        min: c_int,
        hour: c_int,
        mday: c_int,
        mon: c_int,
        year: c_int,
        wday: c_int,
        yday: c_int,
        isdst: c_int,
        gmtoff: c_long,
        zone: *const c_char,
    }
    unsafe extern "C" {
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }
    let mut tm = Tm {
        sec: 0,
        min: 0,
        hour: 0,
        mday: 0,
        mon: 0,
        year: 0,
        wday: 0,
        yday: 0,
        isdst: 0,
        gmtoff: 0,
        zone: std::ptr::null(),
    };
    // SAFETY: both pointers are to live values of the types the C library
    // declares; `localtime_r` writes only into `tm`.
    let done = unsafe { localtime_r(&seconds, &mut tm) };
    if done.is_null() {
        return utc(seconds);
    }
    Some(Local {
        year: tm.year + 1900,
        day: tm.yday,
        month: tm.mon,
        date: tm.mday,
        hour: tm.hour,
        minute: tm.min,
    })
}

#[cfg(not(all(unix, target_pointer_width = "64")))]
fn local_from(seconds: i64) -> Option<Local> {
    utc(seconds)
}

/// `seconds` since 1970 in UTC (the civil-from-days algorithm).
fn utc(seconds: i64) -> Option<Local> {
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let date = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 2 } else { mp - 10 };
    let year = yoe + era * 400 + i64::from(month <= 1);
    Some(Local {
        year: i32::try_from(year).ok()?,
        day: i32::try_from(days).ok()?,
        month: i32::try_from(month).ok()?,
        date: i32::try_from(date).ok()?,
        hour: (of_day / 3600) as i32,
        minute: (of_day % 3600 / 60) as i32,
    })
}

/// What a nested agent's card says: the muted line under its title, and the
/// words at its right, and whether those count up as time goes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Words {
    pub detail: String,
    pub time: Vec<String>,
    pub counting: bool,
}

/// The words of `agent`'s card at `now`.
pub(crate) fn words(agent: &Nested, now: SystemTime) -> Words {
    let since = now.duration_since(agent.since).unwrap_or_default();
    let detail = agent.detail.trim().to_owned();
    match agent.state {
        NestedState::Running => Words {
            detail: if detail.is_empty() {
                "Running".to_owned()
            } else {
                detail
            },
            time: vec![ran_for(since)],
            counting: true,
        },
        NestedState::Asking => Words {
            detail: if detail.is_empty() {
                "Waiting for an answer".to_owned()
            } else {
                format!("Waiting for an answer · {detail}")
            },
            time: vec![ran_for(since)],
            counting: true,
        },
        NestedState::Finished | NestedState::Gone => Words {
            detail: if detail.is_empty() {
                "Finished".to_owned()
            } else {
                detail
            },
            time: vec!["Finished".to_owned(), wall_clock(agent.since, now)],
            counting: false,
        },
        NestedState::Failed => Words {
            detail: if detail.is_empty() {
                "Failed".to_owned()
            } else {
                detail
            },
            time: vec!["Failed".to_owned(), wall_clock(agent.since, now)],
            counting: false,
        },
    }
}

/// How long a turn worked, as its folded line says it: `Worked for 1m 15s`,
/// or, not having seen it run, how many steps it took.
pub(crate) fn worked(took: Option<Duration>, steps: usize) -> String {
    match took {
        // Under a second is no time to speak of: the steps say more.
        Some(took) if took.as_secs() > 0 => format!("Worked for {}", ran_for(took)),
        _ if steps == 1 => "Worked · 1 step".to_owned(),
        _ => format!("Worked · {steps} steps"),
    }
}

/// The waiting line's words: how many it waits on.
pub(crate) fn waiting_on(count: usize) -> String {
    if count == 1 {
        "Waiting on 1 subagent".to_owned()
    } else {
        format!("Waiting on {count} subagents")
    }
}

/// A token count as the context meter writes it: `361k`, `1m`, `1.25m`.
pub(crate) fn tokens(count: u64) -> String {
    crate::view::tokens(count).to_lowercase()
}

/// A stretch of the conversation between two prompts: what one turn did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stretch {
    /// Its first entry and the entry after its last.
    pub start: usize,
    pub end: usize,
    /// The prompt it answers, when there is one before it.
    pub prompt: Option<usize>,
    /// Whether its turn is over: a prompt follows it, or nothing runs.
    pub finished: bool,
}

/// The stretch entry `index` is in, unless it is a prompt.
pub(crate) fn stretch(entries: &[Entry], state: &ThreadState, index: usize) -> Option<Stretch> {
    if matches!(entries.get(index)?, Entry::User(_)) {
        return None;
    }
    let prompt = entries[..index]
        .iter()
        .rposition(|entry| matches!(entry, Entry::User(_)));
    let start = prompt.map_or(0, |prompt| prompt + 1);
    let end = entries[index..]
        .iter()
        .position(|entry| matches!(entry, Entry::User(_)))
        .map_or(entries.len(), |at| index + at);
    let busy = matches!(state, ThreadState::Working | ThreadState::Blocked);
    Some(Stretch {
        start,
        end,
        prompt,
        finished: end < entries.len() || !busy,
    })
}

/// Whether an entry is work that folds away once it is over: a tool call,
/// reasoning, the plan.
pub(crate) fn folds(entry: &Entry) -> bool {
    match entry {
        // A call that starts a subagent stands in the transcript as that
        // agent's card, as the cards of the agents started in panes do; and
        // a command left running in the background stays in sight for as
        // long as it runs.
        Entry::Tool(call) => call.agent.is_none() && !crate::entries::runs_behind(call),
        Entry::Thought(_) | Entry::Plan(_) => true,
        _ => false,
    }
}

/// A run of work: entries that fold, one after the other, in a stretch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    /// Its first entry and the entry after its last.
    pub start: usize,
    pub end: usize,
    pub stretch: Stretch,
}

impl Run {
    /// Whether it is over: something the agent said follows it, or its turn
    /// has ended.
    pub(crate) fn over(&self) -> bool {
        self.end < self.stretch.end || self.stretch.finished
    }
}

/// The run of work entry `index` is in, when it is work.
pub(crate) fn run_of(entries: &[Entry], state: &ThreadState, index: usize) -> Option<Run> {
    if !folds(entries.get(index)?) {
        return None;
    }
    let stretch = stretch(entries, state, index)?;
    let mut start = index;
    while start > stretch.start && folds(&entries[start - 1]) {
        start -= 1;
    }
    let end = (index..stretch.end)
        .find(|at| !folds(&entries[*at]))
        .unwrap_or(stretch.end);
    Some(Run {
        start,
        end,
        stretch,
    })
}

impl ThreadView {
    /// How the conversation is drawn. [`Look::Cockpit`] by default.
    pub fn set_look(&mut self, look: Look, cx: &mut Context<Self>) {
        if self.around.look.get() == look {
            return;
        }
        self.around.look.set(look);
        self.forget_folds();
        // The lights over the rows are the cockpit's.
        self.lights.clear();
        self.cache.iter_mut().for_each(|parsed| *parsed = None);
        let rows = self.list.item_count();
        self.remeasure_rows(0..rows);
        self.dock.update(cx, |dock, cx| dock.look_changed(cx));
        cx.notify();
    }

    /// The look the conversation is drawn in.
    pub fn look(&self) -> Look {
        self.around.look.get()
    }

    /// All the agents this one started, in the order started; called again whenever any of them changes.
    pub fn set_nested(&mut self, nested: Vec<Nested>, cx: &mut Context<Self>) {
        if self.nest.agents == nested {
            return;
        }
        let count = self.thread.entries().len();
        let now = cx.background_executor().now();
        let mut rows = BTreeSet::new();
        for agent in &self.nest.agents {
            if let Some(after) = self.nest.places.get(&agent.id) {
                rows.insert(host_row(*after, count).0);
            }
        }
        let mut places = HashMap::new();
        for agent in &nested {
            let after = agent
                .after
                .or_else(|| self.nest.places.get(&agent.id).copied())
                .unwrap_or(count);
            // One that comes while the view watches arrives; what was there
            // when the view first heard is simply there.
            let watched = !self.nest.agents.is_empty() || self.thread.entries().len() > after;
            if !self.nest.places.contains_key(&agent.id) && watched {
                self.nest.arrived.insert(agent.id.clone(), now);
            }
            rows.insert(host_row(after, count).0);
            places.insert(agent.id.clone(), after);
        }
        self.nest.places = places;
        self.nest
            .arrived
            .retain(|id, _| nested.iter().any(|agent| &agent.id == id));
        self.nest.agents = nested;
        for row in rows {
            self.remeasure_rows(row..row + 1);
        }
        // An agent that stopped running lets its light go.
        self.prune_lights();
        self.share_waiting(cx);
        cx.notify();
    }

    /// Those of them the agent is waiting for right now (ids); empty when it waits for none.
    pub fn set_waiting_on(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if self.nest.waiting_on != ids {
            self.nest.waiting_on = ids;
            self.share_waiting(cx);
            cx.notify();
        }
    }

    /// The card beside the transcript; `None` takes it away.
    pub fn set_beside(&mut self, beside: Option<Beside>, cx: &mut Context<Self>) {
        if self.nest.beside != beside {
            let shown = self.nest.beside.is_some();
            self.nest.beside = beside;
            if shown != self.nest.beside.is_some() {
                // The column makes room for it, or takes the room back.
                self.fit_beside(cx);
            }
            cx.notify();
        }
    }

    /// Pins the wall clock the cards' times are counted against, or lets it
    /// run again with `None`: for captures, which must come out the same
    /// whenever they are taken.
    #[doc(hidden)]
    pub fn pin_clock(&mut self, now: Option<SystemTime>, cx: &mut Context<Self>) {
        self.nest.clock = now;
        let rows = self.list.item_count();
        self.remeasure_rows(0..rows);
        cx.notify();
    }

    /// The wall clock, as the cards count against it.
    pub(crate) fn wall_now(&self) -> SystemTime {
        self.nest.clock.unwrap_or_else(SystemTime::now)
    }

    /// Tells the prompt area the titles of the agents the agent waits on:
    /// those the app says it waits for, and those it started by itself
    /// that are at work.
    pub(crate) fn share_waiting(&mut self, cx: &mut Context<Self>) {
        let mut titles: Vec<String> = self
            .nest
            .waiting_on
            .iter()
            .map(|id| {
                self.nest
                    .agents
                    .iter()
                    .find(|agent| &agent.id == id)
                    .map_or_else(|| id.clone(), |agent| agent.title.clone())
            })
            .collect();
        titles.extend(
            self.native_agents(cx.background_executor().now())
                .into_iter()
                .filter(|(_, agent)| agent.state == NestedState::Running)
                .map(|(_, agent)| agent.title),
        );
        let behind: Vec<String> = self
            .background_commands(cx.background_executor().now())
            .into_iter()
            .map(|(_, command)| command.title)
            .collect();
        if *self.around.waiting.borrow() != titles || *self.around.behind.borrow() != behind {
            *self.around.waiting.borrow_mut() = titles;
            *self.around.behind.borrow_mut() = behind;
            self.dock.update(cx, |_, cx| cx.notify());
        }
    }

    /// The agents whose cards row `row` draws, at its top and at its bottom.
    pub(crate) fn cards_of(&self, row: usize) -> (Vec<&Nested>, Vec<&Nested>) {
        let count = self.thread.entries().len();
        let (mut top, mut bottom) = (Vec::new(), Vec::new());
        for agent in &self.nest.agents {
            let Some(after) = self.nest.places.get(&agent.id) else {
                continue;
            };
            match host_row(*after, count) {
                (at, true) if at == row => top.push(agent),
                (at, false) if at == row => bottom.push(agent),
                _ => {}
            }
        }
        (top, bottom)
    }

    /// Keeps the cards' times counting: called as a card whose time counts
    /// is painted, it asks for the view to be drawn again on the next whole
    /// second of that time. A card that is not painted asks nothing, so the
    /// timer stops with the last one that scrolled away or finished.
    pub(crate) fn keep_ticking(&mut self, cx: &mut Context<Self>) {
        if self.nest.ticking.is_some() {
            return;
        }
        let now = self.wall_now();
        // The next second of the youngest counting card's time.
        let into = self
            .nest
            .agents
            .iter()
            .filter(|agent| agent.state.live())
            .map(|agent| {
                now.duration_since(agent.since)
                    .unwrap_or_default()
                    .subsec_millis()
            })
            .max()
            .unwrap_or(0);
        let wait = Duration::from_millis(u64::from(1000 - into.min(999)));
        let timer = cx.background_executor().timer(wait);
        self.nest.ticking = Some(cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |view, cx| {
                view.nest.ticking = None;
                view.nest.counting.set(false);
                view.retime_cards();
                cx.notify();
            })
            .ok();
        }));
    }

    /// Rows whose cards count are planned again: their time's words changed.
    fn retime_cards(&mut self) {
        let count = self.thread.entries().len();
        let mut rows: BTreeSet<usize> = self
            .nest
            .agents
            .iter()
            .filter(|agent| agent.state.live())
            .filter_map(|agent| self.nest.places.get(&agent.id))
            .map(|after| host_row(*after, count).0)
            .collect();
        // And those of the commands that run in the background.
        rows.extend(
            self.thread
                .entries()
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    matches!(entry, Entry::Tool(call) if crate::entries::runs_behind(call))
                })
                .map(|(index, _)| index),
        );
        for row in rows {
            self.plans_forget(row);
        }
    }

    /// Whether a timer waits to draw the counting times again, for tests.
    #[cfg(test)]
    pub(crate) fn ticking_for_tests(&self) -> bool {
        self.nest.ticking.is_some()
    }

    /// Notes a turn beginning or ending, for how long it worked.
    pub(crate) fn note_turn(&mut self, started: bool, now: Instant) {
        let prompt = self
            .thread
            .entries()
            .iter()
            .rposition(|entry| matches!(entry, Entry::User(_)))
            .unwrap_or(NO_PROMPT);
        if started {
            self.nest.turns.insert(
                prompt,
                Turn {
                    began: now,
                    ended: None,
                },
            );
        } else if let Some(turn) = self.nest.turns.get_mut(&prompt)
            && turn.ended.is_none()
        {
            turn.ended = Some(now);
        }
    }

    /// The run of work row `index` is in, folded once it is over (the agent
    /// says something after it, or its turn has ended), in either look:
    /// whether it is shown open, and whether this row draws its folded
    /// line. `None` for a row that is no work, and for work still going on.
    pub(crate) fn fold_of(&self, index: usize) -> Option<(Run, bool, bool)> {
        // Worked out once for a whole run: each of its rows asks, and so
        // does whatever looks back over it, at every frame.
        if let Some(known) = self.fold_memo.borrow().get(&index) {
            return known.map(|(run, open)| (run, open, run.start == index));
        }
        let found = run_of(self.thread.entries(), self.thread.state(), index)
            .filter(Run::over)
            .map(|run| (run, self.nest.worked_open.contains(&run.start)));
        let mut memo = self.fold_memo.borrow_mut();
        match found {
            Some((run, open)) => {
                for row in run.start..run.end {
                    memo.insert(row, Some((run, open)));
                }
            }
            None => {
                memo.insert(index, None);
            }
        }
        found.map(|(run, open)| (run, open, run.start == index))
    }

    /// Opens or closes a run of work that is over.
    pub(crate) fn toggle_worked(&mut self, start: usize, cx: &mut Context<Self>) {
        if !self.nest.worked_open.remove(&start) {
            self.nest.worked_open.insert(start);
        }
        self.forget_folds();
        let end = run_of(self.thread.entries(), self.thread.state(), start)
            .map_or(start + 1, |run| run.end);
        self.remeasure_rows(start..end.max(start + 1));
        cx.notify();
    }

    /// How long a run of work took, as the view saw it: from when its turn
    /// began, or the entry before it came, to when what follows it came or
    /// its turn ended.
    pub(crate) fn worked_for(&self, run: &Run) -> Option<Duration> {
        let turn = self
            .nest
            .turns
            .get(&run.stretch.prompt.unwrap_or(NO_PROMPT));
        let from = if run.start == run.stretch.start {
            turn.map(|turn| turn.began)
        } else {
            self.nest.seen.get(&run.start).copied()
        }?;
        let to = if run.end < run.stretch.end {
            self.nest.seen.get(&run.end).copied()
        } else {
            turn.and_then(|turn| turn.ended)
        }?;
        Some(to.saturating_duration_since(from))
    }

    /// The last stretch is measured again: its turn began or ended, or the
    /// agent said something, and its work folds or unfolds.
    pub(crate) fn restudy_last(&mut self) {
        let entries = self.thread.entries();
        let count = entries.len();
        // From the stretch before the last prompt: a new prompt finishes it.
        let from = entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::User(_)))
            .map_or(0, |last| {
                entries[..last]
                    .iter()
                    .rposition(|entry| matches!(entry, Entry::User(_)))
                    .map_or(0, |before| before + 1)
            });
        self.remeasure_rows(from..count + 1);
    }

    /// Makes room for the card beside, or takes it back, after the view's
    /// width or the card changed: every row is laid out again.
    pub(crate) fn fit_beside(&mut self, cx: &mut Context<Self>) {
        let inset = self.beside_inset();
        if (self.around.inset.get() - inset).abs() > 0.01 {
            self.around.inset.set(inset);
            let rows = self.list.item_count();
            self.remeasure_rows(0..rows);
            self.dock.update(cx, |_, cx| cx.notify());
        }
    }

    /// The room the card beside takes from the column now, unscaled.
    pub(crate) fn beside_inset(&self) -> f32 {
        if self.nest.beside.is_none() {
            return 0.;
        }
        let measure = crate::cards::measure(self.around.look.get());
        beside_room(self.nest.width.get(), measure).unwrap_or(0.)
    }

    /// Whether the pane leaves the card beside room: the column, in the
    /// middle of the pane, clears it.
    pub(crate) fn beside_fits(&self) -> bool {
        let measure = crate::cards::measure(self.around.look.get());
        beside_room(self.nest.width.get(), measure).is_some()
    }

    /// Whether the card beside shows now: where it has room, unless the
    /// user put it away; and wherever the user pulled it out, over the text
    /// if need be. Never over the prompt area.
    pub(crate) fn beside_shows(&self) -> bool {
        if self.nest.beside.is_none() {
            return false;
        }
        if !self
            .nest
            .beside_pulled
            .unwrap_or_else(|| self.beside_fits())
        {
            return false;
        }
        let factor = kou_ui::scale::factor();
        let height = self.list.viewport_bounds().size.height.as_f32() / factor.max(0.01);
        let wanted = self.nest.beside_height.get() + BESIDE_TOP + 8.;
        !(height > 0. && wanted + self.dock_height > height)
    }

    /// Pull the card beside out (`out`), wherever the pane leaves it room
    /// or not, or put it away: what the handle at the pane's edge and the
    /// card's own button do.
    pub fn pull_beside(&mut self, out: bool, cx: &mut Context<Self>) {
        // Asked for what the room there is gives anyway, it is left to that.
        self.nest.beside_pulled = (out != self.beside_fits()).then_some(out);
        cx.notify();
    }

    /// How far out the card beside is now, from 0 to 1, as it comes or
    /// goes; and whether it is going. At rest it is where it shows or not.
    pub(crate) fn beside_level(&self, now: Instant, still: bool) -> (f32, bool) {
        let shows = self.beside_shows();
        match self.nest.beside_moved {
            Some((since, coming)) if !still => {
                let t = now.saturating_duration_since(since).as_secs_f32()
                    / kou_ui::motion::studio::BESIDE.as_secs_f32();
                if t >= 1. {
                    (if shows { 1. } else { 0. }, false)
                } else if coming {
                    (kou_ui::motion::studio::arrive(t), false)
                } else {
                    (1. - kou_ui::motion::studio::arrive(t), true)
                }
            }
            _ => (if shows { 1. } else { 0. }, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_stands_after_the_entry_it_follows() {
        assert_eq!(host_row(0, 0), (0, true), "no entries: the last row");
        assert_eq!(host_row(3, 0), (0, true));
        assert_eq!(host_row(0, 5), (0, true), "before the first entry");
        assert_eq!(host_row(1, 5), (0, false));
        assert_eq!(host_row(5, 5), (4, false));
        assert_eq!(
            host_row(9, 5),
            (4, false),
            "after a rewind took entries away"
        );
    }

    #[test]
    fn times_read_as_the_cards_write_them() {
        let s = Duration::from_secs;
        assert_eq!(ran_for(s(22)), "22s");
        assert_eq!(ran_for(s(800)), "13m 20s");
        assert_eq!(ran_for(s(7500)), "2h 5m");
        assert_eq!(ran_for_short(s(800)), "13m");
        assert_eq!(ran_for_short(s(22)), "22s");
        assert_eq!(worked(Some(s(75)), 9), "Worked for 1m 15s");
        assert_eq!(worked(None, 1), "Worked · 1 step");
        assert_eq!(
            worked(Some(Duration::from_millis(300)), 2),
            "Worked · 2 steps"
        );
        assert_eq!(worked(None, 7), "Worked · 7 steps");
        assert_eq!(waiting_on(2), "Waiting on 2 subagents");
        assert_eq!(waiting_on(1), "Waiting on 1 subagent");
        assert_eq!(tokens(361_000), "361k");
        assert_eq!(tokens(1_000_000), "1m");
    }

    #[test]
    fn the_wall_clock_is_twelve_hours_and_says_the_day_when_not_today() {
        // Midnight UTC, and three hours and six minutes after it.
        let day = SystemTime::UNIX_EPOCH + Duration::from_secs(20_000 * 86_400);
        let utc_clock = utc(20_000 * 86_400 + 3 * 3600 + 6 * 60).unwrap();
        assert_eq!((utc_clock.hour, utc_clock.minute), (3, 6));
        assert_eq!(
            (utc_clock.year, utc_clock.month, utc_clock.date),
            (2024, 9, 4)
        );
        let shown = wall_clock(day, day + Duration::from_secs(60));
        assert!(shown.ends_with(" AM") || shown.ends_with(" PM"), "{shown}");
        assert!(!shown.contains(','), "the same day: {shown}");
        let later = wall_clock(day, day + Duration::from_secs(3 * 86_400));
        assert!(later.contains(", "), "another day: {later}");
    }

    #[test]
    fn a_card_says_what_its_agent_does() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut agent = Nested {
            id: "a".into(),
            title: "lite-measure".into(),
            harness: "claude".into(),
            code: "CC".into(),
            state: NestedState::Running,
            detail: String::new(),
            since: at,
            after: None,
        };
        let now = at + Duration::from_secs(800);
        assert_eq!(
            words(&agent, now),
            Words {
                detail: "Running".into(),
                time: vec!["13m 20s".into()],
                counting: true
            }
        );
        agent.state = NestedState::Asking;
        assert_eq!(words(&agent, now).detail, "Waiting for an answer");
        agent.state = NestedState::Finished;
        agent.detail = "  The gates are green.  ".into();
        let finished = words(&agent, now);
        assert_eq!(finished.detail, "The gates are green.");
        assert_eq!(finished.time[0], "Finished");
        assert!(!finished.counting);
        agent.state = NestedState::Gone;
        assert_eq!(
            words(&agent, now).time[0],
            "Finished",
            "gone reads as finished"
        );
    }

    #[test]
    fn the_card_beside_shows_only_where_the_centred_column_clears_it() {
        let reserve = BESIDE_WIDTH + BESIDE_EDGE + BESIDE_GAP;
        // Wide: the column in the middle of the pane clears it, with as
        // much room on its other side. It takes nothing from the column.
        assert_eq!(beside_room(1600., 688.), Some(0.));
        assert_eq!(beside_room(688. + 2. * reserve, 688.), Some(0.));
        // A pixel narrower, the text would have to move over: no card.
        assert_eq!(beside_room(688. + 2. * reserve - 1., 688.), None);
        assert_eq!(beside_room(1000., 688.), None);
        assert_eq!(beside_room(0., 688.), None);
    }

    #[test]
    fn work_folds_by_runs_once_something_follows_it_or_its_turn_ends() {
        use kou_thread::Block;
        let user = || Entry::User(vec![Block::Text("go".into())]);
        let tool = || Entry::Tool(kou_thread::ToolCall::default());
        let entries = vec![
            user(),
            Entry::Thought("hm".into()),
            tool(),
            Entry::Assistant("first".into()),
            tool(),
            tool(),
        ];
        let run = run_of(&entries, &ThreadState::Working, 2).unwrap();
        assert_eq!((run.start, run.end), (1, 3));
        assert!(run.over(), "a message follows it");
        let last = run_of(&entries, &ThreadState::Working, 4).unwrap();
        assert_eq!((last.start, last.end), (4, 6));
        assert!(!last.over(), "the turn still runs");
        assert!(run_of(&entries, &ThreadState::Idle, 5).unwrap().over());
        assert_eq!(run_of(&entries, &ThreadState::Idle, 3), None);
    }

    #[test]
    fn a_turn_s_work_is_the_stretch_between_two_prompts() {
        use kou_thread::Block;
        let user = || Entry::User(vec![Block::Text("go".into())]);
        let entries = vec![
            user(),
            Entry::Thought("hm".into()),
            Entry::Assistant("ok".into()),
            user(),
            Entry::Assistant("done".into()),
        ];
        assert_eq!(stretch(&entries, &ThreadState::Idle, 0), None);
        assert_eq!(
            stretch(&entries, &ThreadState::Working, 2),
            Some(Stretch {
                start: 1,
                end: 3,
                prompt: Some(0),
                finished: true
            })
        );
        assert_eq!(
            stretch(&entries, &ThreadState::Working, 4),
            Some(Stretch {
                start: 4,
                end: 5,
                prompt: Some(3),
                finished: false
            })
        );
        assert!(stretch(&entries, &ThreadState::Idle, 4).unwrap().finished);
    }
}
