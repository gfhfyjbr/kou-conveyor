//! What of the transcript moves, and since when: the bookkeeping behind the
//! web cockpit's motion (`timeline.css`, `timeline.js`).
//!
//! Every animation of an entry is counted from a moment noted here when what
//! it shows happened, never from when an element was built: an entry that
//! rose into place does not rise again when its row is built anew (scrolled
//! back into sight, drawn at another zoom, the pane shown again), and an
//! entry that was already there when the view opened (a session replayed)
//! never rises. What loops takes its phase from one clock, [`Motion::epoch`],
//! as the web's loops take the page's, so everything that blinks blinks
//! together and a loop drawn again goes on where it was.
//!
//! Nothing here asks for frames by itself: the view asks for one while
//! [`Motion::busy`] says something is on its way, and for a blink's next
//! change only when something blinks. At rest nothing does.

use kou_ui::motion::cockpit;
use std::{
    collections::HashMap,
    ops::Range,
    time::{Duration, Instant},
};

/// How long a message the agent took mid-turn stays lit (`.entry[data-arrived] .text`).
const FLASH: Duration = Duration::from_millis(1400);
/// How opaque what an edited prompt replaces is (`.entry[data-after-edit]`),
/// and one of its entries under the pointer.
pub(crate) const FADED: f32 = 0.32;
pub(crate) const LIFTED: f32 = 0.7;

/// A value on its way to another (`transition: opacity 0.15s`, CSS `ease`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tween {
    from: f32,
    to: f32,
    since: Instant,
}

impl Tween {
    /// `value`, at rest.
    fn at(value: f32, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            since: now,
        }
    }

    /// Where it is now; where it goes, with less motion.
    fn value(&self, now: Instant, still: bool) -> f32 {
        if still {
            return self.to;
        }
        let t = now.saturating_duration_since(self.since).as_secs_f32()
            / cockpit::AFTER_EDIT.as_secs_f32();
        if t >= 1. {
            self.to
        } else {
            self.from + (self.to - self.from) * cockpit::ease(t)
        }
    }

    /// On its way to `to` from where it is now.
    fn toward(&self, to: f32, now: Instant) -> Self {
        Self {
            from: self.value(now, false),
            to,
            since: now,
        }
    }

    fn moving(&self, now: Instant) -> bool {
        self.from != self.to && now.saturating_duration_since(self.since) < cockpit::AFTER_EDIT
    }
}

/// The entries after a prompt being edited, which running the edit replaces:
/// they fade while the edit is open, and one under the pointer lifts
/// (`.entry[data-after-edit]`), and they come back once it runs or is
/// cancelled.
#[derive(Clone, Debug)]
pub(crate) struct AfterEdit {
    /// The first entry that fades: the one after the edited prompt.
    pub from: usize,
    /// Whether the edit is open; once it is not, the fade goes back.
    pub open: bool,
    /// How opaque they are without the pointer.
    level: Tween,
    /// How far each entry under the pointer, or just left by it, is lifted,
    /// from 0 to 1.
    lifts: HashMap<usize, Tween>,
}

/// Pieces of an entry's text that came recently, to fade in.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fresh {
    /// How much of the entry's text, as drawn, had been drawn before.
    pub shown: usize,
    /// Which of it came when, as ranges of the text as drawn.
    pub pieces: Vec<(Range<usize>, Instant)>,
}

/// The transcript's motion. See the [module](self).
pub(crate) struct Motion {
    /// Where every loop's phase is counted from.
    pub epoch: Instant,
    /// Entries that arrived while the view watched, and when: they rise.
    pub arrived: HashMap<usize, Instant>,
    /// Messages of the user's the agent took in the middle of its turn, and
    /// when: they light up as they arrive (`.entry[data-arrived]`, `flash`).
    pub flashed: HashMap<usize, Instant>,
    /// Streamed entries' recent pieces of text: they fade in.
    pub fresh: HashMap<usize, Fresh>,
    /// The entry that grew last, and when: its caret is solid until it waits.
    pub grew: Option<(usize, Instant)>,
    /// When a tool call was first seen running, and how long it took once it
    /// ended, for the calls the view watched run.
    pub started: HashMap<usize, Instant>,
    pub took: HashMap<usize, Duration>,
    /// What an edit open, or just closed, fades.
    pub after_edit: Option<AfterEdit>,
    /// Tool calls whose cards are opening or closing: how far open each is,
    /// from 0 to 1.
    pub unfolds: HashMap<usize, Tween>,
}

impl Motion {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            epoch: now,
            arrived: HashMap::new(),
            flashed: HashMap::new(),
            fresh: HashMap::new(),
            grew: None,
            started: HashMap::new(),
            took: HashMap::new(),
            after_edit: None,
            unfolds: HashMap::new(),
        }
    }

    /// Tool call `index` was opened or closed by hand: its card opens or
    /// closes from where it is now.
    pub(crate) fn unfold(&mut self, index: usize, open: bool, now: Instant) {
        let to = if open { 1. } else { 0. };
        let tween = match self.unfolds.get(&index) {
            Some(tween) => tween.toward(to, now),
            None => Tween::at(1. - to, now).toward(to, now),
        };
        self.unfolds.insert(index, tween);
    }

    /// How far open tool call `index`'s card is while it opens or closes,
    /// from 0 to 1; `None` at rest.
    pub(crate) fn unfolding(&self, index: usize, now: Instant) -> Option<f32> {
        self.unfolds
            .get(&index)
            .map(|tween| tween.value(now, false).clamp(0., 1.))
    }

    /// The calls whose cards have come to rest, each with whether it ended
    /// open; they are forgotten.
    pub(crate) fn unfolded(&mut self, now: Instant) -> Vec<(usize, bool)> {
        let done: Vec<(usize, bool)> = self
            .unfolds
            .iter()
            .filter(|(_, tween)| !tween.moving(now))
            .map(|(index, tween)| (*index, tween.to > 0.5))
            .collect();
        for (index, _) in &done {
            self.unfolds.remove(index);
        }
        done
    }

    /// An edit of entry `index` opened: the entries after it fade. One that
    /// replaces an edit still open, or still fading back, goes on from where
    /// that one's fade is.
    pub(crate) fn edit_opened(&mut self, index: usize, now: Instant) {
        let level = match &self.after_edit {
            Some(after) => after.level.toward(FADED, now),
            None => Tween::at(1., now).toward(FADED, now),
        };
        self.after_edit = Some(AfterEdit {
            from: index + 1,
            open: true,
            level,
            lifts: HashMap::new(),
        });
    }

    /// The edit ran or was cancelled: what it faded comes back.
    pub(crate) fn edit_closed(&mut self, now: Instant) {
        if let Some(after) = &mut self.after_edit
            && after.open
        {
            after.open = false;
            after.level = after.level.toward(1., now);
        }
    }

    /// The pointer came over entry `index`, or left it.
    pub(crate) fn edit_hover(&mut self, index: usize, hovered: bool, now: Instant) {
        let Some(after) = &mut self.after_edit else {
            return;
        };
        let lift = after
            .lifts
            .get(&index)
            .copied()
            .unwrap_or_else(|| Tween::at(0., now));
        after
            .lifts
            .insert(index, lift.toward(if hovered { 1. } else { 0. }, now));
    }

    /// How opaque entry `index` is drawn now, when an edit fades it; `None`
    /// when it is drawn as it is. With `still`, each value is where it goes.
    pub(crate) fn faded(&self, index: usize, now: Instant, still: bool) -> Option<f32> {
        let after = self
            .after_edit
            .as_ref()
            .filter(|after| index >= after.from)?;
        let level = after.level.value(now, still);
        let lift = after
            .lifts
            .get(&index)
            .map_or(0., |lift| lift.value(now, still));
        // Under the pointer it is lifted toward 70%, and never dimmed by it.
        let opacity = if level < LIFTED {
            level + (LIFTED - level) * lift
        } else {
            level
        };
        (opacity < 0.999).then_some(opacity)
    }

    /// How far entry `index` has risen into place, from 0 to 1; `None` once
    /// it has, or if it never rises.
    pub(crate) fn rise(&self, index: usize, now: Instant) -> Option<f32> {
        let at = self.arrived.get(&index)?;
        let t = now.saturating_duration_since(*at).as_secs_f32() / cockpit::RISE.as_secs_f32();
        (t < 1.).then(|| cockpit::rise(t))
    }

    /// How lit a message the agent took mid-turn still is, from 1 to 0
    /// (`flash 1.4s ease-out`); `None` once it has faded.
    pub(crate) fn flash(&self, index: usize, now: Instant) -> Option<f32> {
        let at = self.flashed.get(&index)?;
        let t = now.saturating_duration_since(*at).as_secs_f32() / FLASH.as_secs_f32();
        (t < 1.).then(|| 1. - cockpit::ease_out(t))
    }

    /// Notes that the text of entry `index`, as drawn, is now `length` long:
    /// what is past what was drawn before came now.
    pub(crate) fn grow(&mut self, index: usize, length: usize, now: Instant) {
        let fresh = self.fresh.entry(index).or_default();
        // Drawn again, markdown can come out shorter ("**" becomes bold):
        // what was shown past the end is not any more.
        if length > fresh.shown {
            fresh.pieces.push((fresh.shown..length, now));
        }
        fresh.shown = length;
        fresh.pieces.retain_mut(|(range, at)| {
            range.end = range.end.min(length);
            range.start < range.end && now.saturating_duration_since(*at) < cockpit::FADE
        });
    }

    /// How opaque each recent piece of entry `index` is now, as ranges of its
    /// text as drawn. Empty once everything has faded in.
    pub(crate) fn fades(&self, index: usize, now: Instant) -> Vec<(Range<usize>, f32)> {
        let Some(fresh) = self.fresh.get(&index) else {
            return Vec::new();
        };
        fresh
            .pieces
            .iter()
            .filter_map(|(range, at)| {
                let t =
                    now.saturating_duration_since(*at).as_secs_f32() / cockpit::FADE.as_secs_f32();
                (t < 1.).then(|| (range.clone(), cockpit::rise(t)))
            })
            .collect()
    }

    /// Whether anything of the transcript is on its way: an entry rising, a
    /// piece of text fading in. While it is, the view asks for frames.
    pub(crate) fn busy(&self, now: Instant) -> bool {
        self.arrived
            .values()
            .any(|at| now.saturating_duration_since(*at) < cockpit::RISE)
            || self
                .flashed
                .values()
                .any(|at| now.saturating_duration_since(*at) < FLASH)
            || self.fresh.values().any(|fresh| {
                fresh
                    .pieces
                    .iter()
                    .any(|(_, at)| now.saturating_duration_since(*at) < cockpit::FADE)
            })
            || self.after_edit.as_ref().is_some_and(|after| {
                after.level.moving(now) || after.lifts.values().any(|lift| lift.moving(now))
            })
            || self.unfolds.values().any(|tween| tween.moving(now))
    }

    /// Forgets what has finished moving, so that nothing old is looked at.
    pub(crate) fn settle(&mut self, now: Instant) {
        self.arrived
            .retain(|_, at| now.saturating_duration_since(*at) < cockpit::RISE);
        self.flashed
            .retain(|_, at| now.saturating_duration_since(*at) < FLASH);
        for fresh in self.fresh.values_mut() {
            fresh
                .pieces
                .retain(|(_, at)| now.saturating_duration_since(*at) < cockpit::FADE);
        }
        if let Some(after) = &mut self.after_edit {
            after
                .lifts
                .retain(|_, lift| lift.moving(now) || lift.value(now, false) > 0.);
            if !after.open && !after.level.moving(now) {
                self.after_edit = None;
            }
        }
    }

    /// How lit what blinks is now (`blink 1s steps(2)`), and how long until
    /// that changes.
    pub(crate) fn blink(&self, now: Instant) -> (f32, Duration) {
        cockpit::blink(now.saturating_duration_since(self.epoch), cockpit::BLINK)
    }

    /// How lit the caret at the end of entry `index`'s text is now, and when
    /// that changes: solid while the text comes, blinking once it waits
    /// (`stream-caret`: `blink 1.1s steps(2) 0.6s`).
    pub(crate) fn caret(&self, index: usize, now: Instant) -> (f32, Option<Duration>) {
        let since = match self.grew {
            Some((grew, at)) if grew == index => now.saturating_duration_since(at),
            _ => cockpit::CARET_DELAY,
        };
        if since < cockpit::CARET_DELAY {
            return (1., Some(cockpit::CARET_DELAY - since));
        }
        let (lit, next) = cockpit::blink(since - cockpit::CARET_DELAY, cockpit::CARET);
        (lit, Some(next))
    }

    /// The entries from `at` on are gone: what moved of them is forgotten.
    pub(crate) fn forget_from(&mut self, at: usize) {
        self.arrived.retain(|index, _| *index < at);
        self.flashed.retain(|index, _| *index < at);
        self.fresh.retain(|index, _| *index < at);
        self.started.retain(|index, _| *index < at);
        self.took.retain(|index, _| *index < at);
        self.unfolds.retain(|index, _| *index < at);
        if self.grew.is_some_and(|(index, _)| index >= at) {
            self.grew = None;
        }
    }

    /// Notes that tool call `index` runs, or ran, as the view sees it now.
    pub(crate) fn tool(&mut self, index: usize, running: bool, now: Instant) {
        if running {
            self.started.entry(index).or_insert(now);
        } else if let Some(at) = self.started.remove(&index) {
            self.took.insert(index, now.saturating_duration_since(at));
        }
    }
}

/// A duration as the web cockpit writes one (`fmt.duration`): `0.4s`,
/// `12s`, `3m 4s`.
pub(crate) fn duration(took: Duration) -> String {
    let seconds = took.as_secs_f32();
    if seconds < 10. {
        format!("{seconds:.1}s")
    } else if seconds < 60. {
        format!("{}s", seconds as u64)
    } else {
        let whole = took.as_secs();
        format!("{}m {}s", whole / 60, whole % 60)
    }
}

/// How long a run has gone on, as a clock: `0:42`, `12:05`.
pub(crate) fn clock(took: Duration) -> String {
    let whole = took.as_secs();
    format!("{}:{:02}", whole / 60, whole % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_rises_once_from_when_it_arrived() {
        let start = Instant::now();
        let mut motion = Motion::new(start);
        assert_eq!(motion.rise(0, start), None, "an entry that was there");
        motion.arrived.insert(3, start);
        let early = motion.rise(3, start + Duration::from_millis(40)).unwrap();
        assert!(early > 0. && early < 1.);
        assert!(motion.busy(start + Duration::from_millis(40)));
        let later = start + cockpit::RISE;
        assert_eq!(motion.rise(3, later), None);
        assert!(!motion.busy(later));
        motion.settle(later);
        assert!(motion.arrived.is_empty());
    }

    #[test]
    fn streamed_text_fades_in_piece_by_piece_and_shrinking_text_drops_what_went() {
        let start = Instant::now();
        let mut motion = Motion::new(start);
        motion.grow(1, 10, start);
        motion.grow(1, 25, start + Duration::from_millis(100));
        let fades = motion.fades(1, start + Duration::from_millis(100));
        assert_eq!(fades.len(), 2);
        assert_eq!(fades[1].0, 10..25);
        assert_eq!(fades[1].1, 0., "what came just now is not drawn yet");
        // The text came out shorter when drawn again.
        motion.grow(1, 18, start + Duration::from_millis(120));
        let fades = motion.fades(1, start + Duration::from_millis(120));
        assert_eq!(fades[1].0, 10..18);
        assert!(motion.fades(1, start + Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn the_caret_is_solid_while_text_comes_and_blinks_once_it_waits() {
        let start = Instant::now();
        let mut motion = Motion::new(start);
        motion.grew = Some((2, start));
        assert_eq!(motion.caret(2, start + Duration::from_millis(100)).0, 1.);
        let waiting = start + cockpit::CARET_DELAY + Duration::from_millis(400);
        assert_eq!(motion.caret(2, waiting).0, 0.15);
    }

    #[test]
    fn what_an_edit_replaces_fades_lifts_under_the_pointer_and_comes_back() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut motion = Motion::new(start);
        assert_eq!(motion.faded(3, start, false), None);
        motion.edit_opened(2, start);
        assert_eq!(
            motion.faded(2, start, false),
            None,
            "the edited prompt itself"
        );
        assert_eq!(
            motion.faded(3, start, false),
            None,
            "it has not begun to fade"
        );
        let half = motion.faded(3, at(75), false).expect("fading");
        assert!(half < 1. && half > FADED, "{half}");
        assert!(motion.busy(at(75)));
        assert_eq!(motion.faded(3, at(150), false), Some(FADED));
        assert!(!motion.busy(at(150)));
        // With less motion it is where it goes at once.
        assert_eq!(motion.faded(9, start, true), Some(FADED));

        // Under the pointer it lifts to 70% in the same time, and back.
        motion.edit_hover(4, true, at(200));
        assert_eq!(motion.faded(4, at(200), false), Some(FADED));
        assert_eq!(motion.faded(4, at(350), false), Some(LIFTED));
        assert_eq!(motion.faded(5, at(350), false), Some(FADED));
        motion.edit_hover(4, false, at(400));
        assert_eq!(motion.faded(4, at(550), false), Some(FADED));
        motion.settle(at(600));
        assert!(motion.after_edit.as_ref().unwrap().lifts.is_empty());

        // A second edit, of an earlier prompt, fades from there on.
        motion.edit_opened(0, at(600));
        assert_eq!(motion.faded(1, at(600), true), Some(FADED));

        // Closed, it comes back and is forgotten.
        motion.edit_closed(at(700));
        let back = motion.faded(3, at(775), false).expect("coming back");
        assert!(back > FADED && back < 1., "{back}");
        assert_eq!(motion.faded(3, at(850), false), None);
        motion.settle(at(850));
        assert!(motion.after_edit.is_none());
    }

    #[test]
    fn durations_read_as_the_web_writes_them() {
        assert_eq!(duration(Duration::from_millis(420)), "0.4s");
        assert_eq!(duration(Duration::from_secs(12)), "12s");
        assert_eq!(duration(Duration::from_secs(184)), "3m 4s");
        assert_eq!(clock(Duration::from_secs(42)), "0:42");
        assert_eq!(clock(Duration::from_secs(725)), "12:05");
    }
}
