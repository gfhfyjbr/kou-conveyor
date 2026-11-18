//! How long things take to move, and along which curve, so the little that
//! moves moves alike.
//!
//! The app is still by default: rows, tabs, panes and layouts are simply
//! there when asked for. What may move is small and local state feedback that
//! never holds content back (a hover, a group folding open, a menu fading in)
//! and the one ease-out of a newly created terminal. The conversation view
//! moves as kou-conveyor's web cockpit does, on the user's word: its timings
//! and curves are in [`cockpit`]. GPUI's animations are driven by time, not
//! by a frame count, so what does move is as smooth at 240 Hz as the display is.

use gpui::Animation;
use std::time::Duration;

/// A press, a hover, a mark changing: felt, not watched.
pub const QUICK: Duration = Duration::from_millis(110);
/// A row, a tab, a menu arriving or leaving.
pub const BASE: Duration = Duration::from_millis(170);
/// A panel or a whole layout changing place.
pub const SLOW: Duration = Duration::from_millis(260);

/// Fast at first, settling at the end: for what arrives.
pub fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

/// Slow at first, fast at the end: for what leaves.
pub fn ease_in(t: f32) -> f32 {
    t.clamp(0., 1.).powi(3)
}

/// Slow at both ends: for what moves from one place to another.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    if t < 0.5 {
        4. * t * t * t
    } else {
        1. - (-2. * t + 2.).powi(3) / 2.
    }
}

/// An animation for something arriving.
pub fn enter(duration: Duration) -> Animation {
    Animation::new(duration).with_easing(ease_out)
}

/// An animation for something leaving.
pub fn leave(duration: Duration) -> Animation {
    Animation::new(duration).with_easing(ease_in)
}

/// An animation for something moving between two places.
pub fn travel(duration: Duration) -> Animation {
    Animation::new(duration).with_easing(ease_in_out)
}

/// The value a fraction of the way from `from` to `to`.
pub fn mix(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing function: how far along a
/// change is, `t` of the way through its time.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    if t == 0. || t == 1. {
        return t;
    }
    // One coordinate of the curve at its parameter `u`, and its slope.
    let at = |a: f32, b: f32, u: f32| {
        let v = 1. - u;
        3. * v * v * u * a + 3. * v * u * u * b + u * u * u
    };
    let slope = |a: f32, b: f32, u: f32| {
        let v = 1. - u;
        3. * v * v * a + 6. * v * u * (b - a) + 3. * u * u * (1. - b)
    };
    // The parameter whose x is `t`: Newton's steps, then halving if they stall.
    let mut u = t;
    for _ in 0..8 {
        let error = at(x1, x2, u) - t;
        if error.abs() < 1e-5 {
            return at(y1, y2, u);
        }
        let d = slope(x1, x2, u);
        if d.abs() < 1e-6 {
            break;
        }
        u = (u - error / d).clamp(0., 1.);
    }
    let (mut low, mut high) = (0f32, 1f32);
    u = t;
    for _ in 0..30 {
        let x = at(x1, x2, u);
        if (x - t).abs() < 1e-5 {
            break;
        }
        if x < t {
            low = u;
        } else {
            high = u;
        }
        u = (low + high) / 2.;
    }
    at(y1, y2, u)
}

/// The web cockpit's motion (`cmd/kou-conveyor-web`), by the names its CSS
/// gives it, for the conversation view that takes its look.
pub mod cockpit {
    use super::cubic_bezier;
    use std::time::Duration;

    /// A streamed entry rising into place (`timeline.css`, `.entry.rising`).
    pub const RISE: Duration = Duration::from_millis(320);
    /// How far it rises from, in pixels (`theme.css`, `@keyframes rise`).
    pub const RISE_BY: f32 = 6.;
    /// A piece of streamed text fading in (`timeline.js`, `FADE`).
    pub const FADE: Duration = Duration::from_millis(420);
    /// A menu, the command list, the picker opening (`.menu`, `.commands`,
    /// `.model-picker`: `rise 0.1s ease-out`).
    pub const MENU: Duration = Duration::from_millis(100);
    /// The queue arriving on the field (`.queue`: `rise 0.16s ease-out`).
    pub const QUEUE: Duration = Duration::from_millis(160);
    /// A queued prompt arriving (`.queue-item[data-new]`: `0.18s`).
    pub const QUEUE_ROW: Duration = Duration::from_millis(180);
    /// The picker's pane leaving and the next arriving (`picker.js`, `swap`).
    pub const PANE_OUT: Duration = Duration::from_millis(190);
    pub const PANE_IN: Duration = Duration::from_millis(280);
    /// How far a pane slides, in pixels.
    pub const PANE_SHIFT: f32 = 44.;
    /// The picker's box taking the next pane's height.
    pub const PANE_HEIGHT: Duration = Duration::from_millis(260);
    /// Each row of a new pane rising in, after the one before it.
    pub const ROW_IN: Duration = Duration::from_millis(240);
    pub const ROW_DELAY: Duration = Duration::from_millis(50);
    pub const ROW_STAGGER: Duration = Duration::from_millis(22);
    /// A fold's chevron turning (`.chev::before`: `transform 0.15s`).
    pub const CHEVRON: Duration = Duration::from_millis(150);
    /// The prompt field's border following the focus (`.composer`: `0.15s`).
    pub const FOCUS: Duration = Duration::from_millis(150);
    /// Hovers, buttons, actions showing (`0.12s`).
    pub const HOVER: Duration = Duration::from_millis(120);
    /// What waits on the user or works blinks (`blink 1s steps(2)`).
    pub const BLINK: Duration = Duration::from_millis(1000);
    /// The caret at the end of streamed text (`1.1s steps(2)`, after 0.6s).
    pub const CARET: Duration = Duration::from_millis(1100);
    pub const CARET_DELAY: Duration = Duration::from_millis(600);
    /// The light across "Thinking" while it is written (`shimmer 1.8s linear`).
    pub const SHIMMER: Duration = Duration::from_millis(1800);
    /// The light going round a tool call that runs (`tool-run 2.4s linear`).
    pub const RUN_LIGHT: Duration = Duration::from_millis(2400);
    /// The activity meter's light across its cells (`meter 1.2s linear`).
    pub const METER: Duration = Duration::from_millis(1200);
    /// The delay from one cell of the meter to the next.
    pub const METER_STEP: Duration = Duration::from_millis(100);
    /// Something pending breathing (`pulse 1.4s ease-in-out`).
    pub const PULSE: Duration = Duration::from_millis(1400);
    /// What an edited prompt replaces fading, and one of its entries under
    /// the pointer lifting (`.entry[data-after-edit]`: `opacity 0.15s`).
    pub const AFTER_EDIT: Duration = Duration::from_millis(150);
    /// A prompt's editor arriving in its place (`.edit`: `fade 0.12s ease-out`).
    pub const EDITOR: Duration = Duration::from_millis(120);

    /// `cubic-bezier(0.23, 1, 0.32, 1)`: an entry rising, text fading in.
    pub fn rise(t: f32) -> f32 {
        cubic_bezier(0.23, 1., 0.32, 1., t)
    }

    /// `cubic-bezier(0.22, 1, 0.36, 1)`: the picker's panes, meters filling.
    pub fn glide(t: f32) -> f32 {
        cubic_bezier(0.22, 1., 0.36, 1., t)
    }

    /// CSS `ease-out`: menus and the queue arriving.
    pub fn ease_out(t: f32) -> f32 {
        cubic_bezier(0., 0., 0.58, 1., t)
    }

    /// CSS `ease`: a chevron turning, a border following the focus.
    pub fn ease(t: f32) -> f32 {
        cubic_bezier(0.25, 0.1, 0.25, 1., t)
    }

    /// CSS `ease-in-out`.
    pub fn ease_in_out(t: f32) -> f32 {
        cubic_bezier(0.42, 0., 0.58, 1., t)
    }

    /// `blink`: `steps(2, jump-none)` to a 15% opacity half way through
    /// `period`. Lit for its first and last quarter, dim between. Returns the
    /// opacity `elapsed` into it, and how long until it changes.
    pub fn blink(elapsed: Duration, period: Duration) -> (f32, Duration) {
        let period = period.as_secs_f32().max(0.001);
        let at = elapsed.as_secs_f32() % period;
        let quarter = period / 4.;
        let (lit, next) = if at < quarter {
            (true, quarter)
        } else if at < 3. * quarter {
            (false, 3. * quarter)
        } else {
            (true, period + quarter)
        };
        (
            if lit { 1. } else { 0.15 },
            Duration::from_secs_f32((next - at).max(0.001)),
        )
    }

    /// `pulse`: down to 45% opacity and back over `period`, eased in and out.
    pub fn pulse(elapsed: Duration, period: Duration) -> f32 {
        let period = period.as_secs_f32().max(0.001);
        let at = (elapsed.as_secs_f32() % period) / period;
        let half = if at < 0.5 { at * 2. } else { 2. - at * 2. };
        1. - 0.55 * ease_in_out(half)
    }

    /// `meter`: how lit the cell `index` of the activity meter is `elapsed`
    /// into it, from 0 (the rule's colour) to 1 (the accent).
    pub fn meter(elapsed: Duration, index: usize) -> f32 {
        let delay = METER_STEP.as_secs_f32() * index as f32;
        let period = METER.as_secs_f32();
        let at = ((elapsed.as_secs_f32() - delay).rem_euclid(period)) / period;
        if at < 0.12 {
            at / 0.12
        } else if at < 0.4 {
            1. - (at - 0.12) / 0.28
        } else {
            0.
        }
    }
}

/// The conversation view's quieter look (`Look::Studio`): what of it moves,
/// how long it takes, and along which curve. Only what the look's reference
/// would have move: a nested agent's card arriving, the dot of one that runs,
/// the spinner of the line that waits on them.
pub mod studio {
    use std::time::Duration;

    /// A nested agent's card arriving in the transcript: risen and faded in.
    pub const ARRIVE: Duration = Duration::from_millis(280);
    /// How far it rises from, in pixels.
    pub const ARRIVE_BY: f32 = 6.;
    /// The card beside the transcript coming out from the pane's edge, or
    /// going back to it: slid and faded, on the same curve.
    pub const BESIDE: Duration = Duration::from_millis(320);
    /// How far it slides, in pixels.
    pub const BESIDE_BY: f32 = 36.;
    /// One turn of the spinner over the prompt field.
    pub const SPIN: Duration = Duration::from_millis(1100);
    /// One ring going out from the dot of an agent that runs.
    pub const HALO: Duration = Duration::from_millis(1800);
    /// How far the ring goes, as a multiple of the dot's radius.
    pub const HALO_REACH: f32 = 2.4;

    /// How far a card has arrived, `t` of [`ARRIVE`] into it: the cockpit's
    /// rise, which is most of the way there early.
    pub fn arrive(t: f32) -> f32 {
        super::cockpit::rise(t.clamp(0., 1.))
    }

    /// Where the spinner is, `elapsed` into it: a turn, from 0 to 1.
    pub fn spin(elapsed: Duration) -> f32 {
        (elapsed.as_secs_f32() / SPIN.as_secs_f32()).fract()
    }

    /// The ring of a running dot, `elapsed` into it: how far out it is, as a
    /// multiple of the dot's radius, and how opaque. It goes out over the
    /// first two thirds of [`HALO`], fading, and waits out the rest unseen.
    pub fn halo(elapsed: Duration) -> (f32, f32) {
        let at = (elapsed.as_secs_f32() / HALO.as_secs_f32()).fract() / (2. / 3.);
        if at >= 1. {
            return (HALO_REACH, 0.);
        }
        let out = super::ease_out(at);
        (1. + (HALO_REACH - 1.) * out, 0.55 * (1. - out))
    }
}

/// A text caret's blink, the same for every caret in the app: the
/// terminal's, the prompt field's, the small fields that rename things. Fully
/// lit for most of the cycle, an eased fade out, a short dark, an eased fade
/// back in; lit at once and from the start of a cycle whenever the caret
/// moves or a key is pressed, so that it never fades while one types.
pub mod caret {
    use std::time::Duration;

    /// Lit, at the start of every cycle.
    pub const LIT: Duration = Duration::from_millis(620);
    /// Fading out, and again fading back in.
    pub const FADE: Duration = Duration::from_millis(180);
    /// Dark, between the two fades.
    pub const DARK: Duration = Duration::from_millis(120);
    /// One cycle: lit, out, dark, in.
    pub const CYCLE: Duration = Duration::from_millis(1100);

    /// When a caret next looks different.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Next {
        /// It is fading: on the next frame.
        Frame,
        /// It holds this long, lit or dark.
        After(Duration),
    }

    /// [`blink`] for a view that cannot name [`Next`] (the terminal's,
    /// `kou_term::CursorBlink`): how long it holds, `None` while it fades.
    pub fn level(elapsed: Duration) -> (f32, Option<Duration>) {
        match blink(elapsed) {
            (lit, Next::Frame) => (lit, None),
            (lit, Next::After(wait)) => (lit, Some(wait)),
        }
    }

    /// How lit a blinking caret is `elapsed` after it last moved, from 1 to
    /// 0, and when that next changes.
    pub fn blink(elapsed: Duration) -> (f32, Next) {
        let ms = |d: Duration| d.as_secs_f32() * 1000.;
        let at = ms(elapsed) % ms(CYCLE);
        let out = ms(LIT);
        let dark = out + ms(FADE);
        let back = dark + ms(DARK);
        let wait =
            |until: f32| Next::After(Duration::from_secs_f32(((until - at) / 1000.).max(0.001)));
        if at < out {
            (1., wait(out))
        } else if at < dark {
            (1. - super::ease_in_out((at - out) / ms(FADE)), Next::Frame)
        } else if at < back {
            (0., wait(back))
        } else {
            (super::ease_in_out((at - back) / ms(FADE)), Next::Frame)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caret_is_lit_most_of_its_cycle_and_eases_out_and_back() {
        use caret::{CYCLE, DARK, FADE, LIT, Next, blink};
        let ms = std::time::Duration::from_millis;
        assert_eq!(LIT + FADE + DARK + FADE, CYCLE);
        assert!(LIT > CYCLE / 2, "lit for most of the cycle");
        // Lit at once, and steady until the fade: no frames asked for.
        let (lit, next) = blink(ms(0));
        assert_eq!(lit, 1.);
        assert!(
            matches!(next, Next::After(wait) if (wait.as_secs_f32() - LIT.as_secs_f32()).abs() < 1e-3)
        );
        // The fade out is eased: slow at first, and asks for every frame.
        let early = blink(LIT + FADE / 10);
        assert!(early.0 > 0.95 && early.0 < 1., "{early:?}");
        assert_eq!(early.1, Next::Frame);
        let middle = blink(LIT + FADE / 2).0;
        assert!((middle - 0.5).abs() < 0.01, "{middle}");
        // Dark, steady until the fade in.
        let (lit, next) = blink(LIT + FADE + DARK / 2);
        assert_eq!(lit, 0.);
        assert!(
            matches!(next, Next::After(wait) if (wait.as_secs_f32() - (DARK / 2).as_secs_f32()).abs() < 1e-3)
        );
        // Back in, eased, and round again.
        let back = blink(LIT + FADE + DARK + FADE / 2).0;
        assert!((back - 0.5).abs() < 0.01, "{back}");
        assert_eq!(blink(CYCLE).0, 1.);
        assert_eq!(blink(CYCLE * 3 + ms(10)).0, 1.);
        // It never steps: neighbouring moments are close.
        let mut last = 1.;
        for step in 0..=1100 {
            let lit = blink(ms(step)).0;
            assert!((lit - last).abs() < 0.03, "{step}: {last} to {lit}");
            last = lit;
        }
    }

    #[test]
    fn the_studio_s_loops_go_round_and_its_ring_fades_out() {
        let ms = std::time::Duration::from_millis;
        assert_eq!(studio::spin(ms(0)), 0.);
        assert!((studio::spin(studio::SPIN / 2) - 0.5).abs() < 1e-3);
        assert!(studio::spin(studio::SPIN + ms(1)) < 0.01);
        let (reach, lit) = studio::halo(ms(0));
        assert_eq!(reach, 1.);
        assert!(lit > 0.5);
        let (reach, lit) = studio::halo(studio::HALO * 2 / 3 + ms(1));
        assert_eq!((reach, lit), (studio::HALO_REACH, 0.));
        assert_eq!(studio::arrive(0.), 0.);
        assert_eq!(studio::arrive(1.), 1.);
        assert!(studio::arrive(0.25) > 0.7);
    }

    #[test]
    fn the_cockpit_s_curves_match_css_and_its_loops_their_keyframes() {
        // CSS `ease` at 0.5 is 0.8024 by its definition.
        assert!((cockpit::ease(0.5) - 0.8024).abs() < 0.002);
        for curve in [
            cockpit::rise,
            cockpit::glide,
            cockpit::ease_out,
            cockpit::ease,
            cockpit::ease_in_out,
        ] {
            assert_eq!(curve(0.), 0.);
            assert_eq!(curve(1.), 1.);
            let mut last = 0.;
            for step in 0..=200 {
                let value = curve(step as f32 / 200.);
                assert!(value >= last - 1e-4 && value <= 1.0001, "{step}: {value}");
                last = value;
            }
        }
        // The rise is most of the way there early: it reads as arriving.
        assert!(cockpit::rise(0.25) > 0.75);
        let ms = std::time::Duration::from_millis;
        assert_eq!(cockpit::blink(ms(0), cockpit::BLINK), (1., ms(250)));
        assert_eq!(cockpit::blink(ms(300), cockpit::BLINK).0, 0.15);
        assert_eq!(cockpit::blink(ms(800), cockpit::BLINK).0, 1.);
        let next = cockpit::blink(ms(800), cockpit::BLINK).1;
        assert!((next.as_secs_f32() - 0.45).abs() < 1e-3, "{next:?}");
        assert_eq!(cockpit::meter(ms(0), 0), 0.);
        assert!((cockpit::meter(ms(144), 0) - 1.).abs() < 0.01);
        assert!(cockpit::meter(ms(144), 1) < 0.5);
        assert_eq!(cockpit::meter(ms(600), 0), 0.);
        assert!((cockpit::pulse(ms(700), cockpit::PULSE) - 0.45).abs() < 0.01);
    }

    #[test]
    fn curves_start_at_rest_end_at_rest_and_never_overshoot() {
        for curve in [ease_out, ease_in, ease_in_out] {
            assert_eq!(curve(0.), 0.);
            assert_eq!(curve(1.), 1.);
            assert_eq!(curve(-3.), 0.);
            assert_eq!(curve(7.), 1.);
            let mut last = 0.;
            for step in 0..=100 {
                let value = curve(step as f32 / 100.);
                assert!((0. ..=1.).contains(&value) && value >= last);
                last = value;
            }
        }
        // Arriving is ahead of the clock, leaving behind it.
        assert!(ease_out(0.3) > 0.3 && ease_in(0.3) < 0.3);
        assert_eq!(mix(10., 20., 0.25), 12.5);
        assert!(QUICK < BASE && BASE < SLOW);
    }
}

/// How a sprung value moves: how fast it gets there and how much it overshoots.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    /// Natural frequency in radians per second: higher arrives sooner.
    pub frequency: f32,
    /// 1 arrives without overshoot; less than 1 passes the target and comes back.
    pub damping: f32,
}

impl Spring {
    /// Quick and exact: a highlight moving to another row, a tab closing.
    pub const SNAPPY: Spring = Spring {
        frequency: 34.,
        damping: 1.,
    };
    /// A little give at the end: a panel sliding open, tiles finding their places.
    pub const SOFT: Spring = Spring {
        frequency: 24.,
        damping: 0.82,
    };
    /// Slower, for large surfaces: the rail, a whole layout.
    pub const WIDE: Spring = Spring {
        frequency: 19.,
        damping: 0.9,
    };
}

#[derive(Clone, Copy, Debug)]
struct Sprung {
    at: f32,
    speed: f32,
    target: f32,
    spring: Spring,
}

/// Named values that glide to wherever they were last told to be.
///
/// A view asks for a value by name every time it draws, with the place the
/// value should be: `springs.to("rail", if hidden { 0. } else { 264. }, Spring::WIDE)`.
/// The answer is where it is now. While anything is still moving,
/// [`Springs::moving`] is true and the view asks for another frame. Motion is
/// computed from the clock, so it is the same at any frame rate, and a value
/// told a new place mid-flight turns toward it without a jump.
#[derive(Debug, Default)]
pub struct Springs {
    values: std::collections::HashMap<gpui::SharedString, Sprung>,
    stepped: Option<std::time::Instant>,
    moving: bool,
    still: bool,
}

// Small enough that 60 and 240 frames a second trace the same curve.
const SPRING_STEP: f32 = 1. / 960.;

impl Springs {
    /// A set with nothing in it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Move everything at once from now on: the user asked the system for less motion.
    pub fn set_still(&mut self, still: bool) {
        self.still = still;
    }

    /// Advance every value to `now`. Call once per frame, before reading any.
    pub fn step(&mut self, now: std::time::Instant) {
        let elapsed = self
            .stepped
            .map_or(0., |last| now.saturating_duration_since(last).as_secs_f32())
            // A window left undrawn for a while does not make things leap on its return.
            .min(0.1);
        self.stepped = Some(now);
        self.moving = false;
        for value in self.values.values_mut() {
            if value.at == value.target && value.speed == 0. {
                continue;
            }
            if self.still {
                value.at = value.target;
                value.speed = 0.;
                continue;
            }
            let k = value.spring.frequency * value.spring.frequency;
            let c = 2. * value.spring.damping * value.spring.frequency;
            let mut remaining = elapsed;
            while remaining > 0. {
                let dt = remaining.min(SPRING_STEP);
                remaining -= dt;
                value.speed += (-k * (value.at - value.target) - c * value.speed) * dt;
                value.at += value.speed * dt;
            }
            // Close enough to call it there: a hundredth of a pixel, or of anything else.
            if (value.at - value.target).abs() < 0.01 && value.speed.abs() < 0.5 {
                value.at = value.target;
                value.speed = 0.;
            } else {
                self.moving = true;
            }
        }
    }

    /// Where `name` is now, on its way to `target`. A name seen for the first
    /// time is already at its target: things do not fly in from zero unless
    /// [`Springs::from`] says where they start.
    pub fn to(&mut self, name: impl Into<gpui::SharedString>, target: f32, spring: Spring) -> f32 {
        let value = self.values.entry(name.into()).or_insert(Sprung {
            at: target,
            speed: 0.,
            target,
            spring,
        });
        if value.target != target {
            value.target = target;
            value.spring = spring;
            if self.still {
                value.at = target;
            } else {
                self.moving = true;
            }
        }
        value.at
    }

    /// Like [`Springs::to`], but a name seen for the first time starts at
    /// `start`: how something new makes an entrance.
    pub fn from(
        &mut self,
        name: impl Into<gpui::SharedString>,
        start: f32,
        target: f32,
        spring: Spring,
    ) -> f32 {
        let name = name.into();
        if !self.values.contains_key(&name) && !self.still {
            self.values.insert(
                name.clone(),
                Sprung {
                    at: start,
                    speed: 0.,
                    target: start,
                    spring,
                },
            );
        }
        self.to(name, target, spring)
    }

    /// Where `name` is now, without telling it anything. `None` if it was never set.
    pub fn at(&self, name: &str) -> Option<f32> {
        self.values.get(name).map(|value| value.at)
    }

    /// Whether `name` has reached its target (or was never set).
    pub fn arrived(&self, name: &str) -> bool {
        self.values
            .get(name)
            .is_none_or(|value| value.at == value.target && value.speed == 0.)
    }

    /// Forget a value: the next time it is asked for, it is new.
    pub fn forget(&mut self, name: &str) {
        self.values.remove(name);
    }

    /// Whether anything is still on its way: ask for another frame.
    pub fn moving(&self) -> bool {
        self.moving
    }
}

#[cfg(test)]
mod spring_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn run(springs: &mut Springs, start: Instant, seconds: f32, hz: f32) {
        let frames = (seconds * hz).round() as u32;
        for frame in 1..=frames {
            springs.step(start + Duration::from_secs_f32(frame as f32 / hz));
        }
    }

    #[test]
    fn a_new_value_is_at_its_target_and_a_changed_target_is_glided_to() {
        let mut springs = Springs::new();
        let start = Instant::now();
        springs.step(start);
        assert_eq!(springs.to("rail", 264., Spring::WIDE), 264.);
        assert!(!springs.moving());

        assert_eq!(
            springs.to("rail", 0., Spring::WIDE),
            264.,
            "it has not moved yet"
        );
        assert!(springs.moving());
        run(&mut springs, start, 0.05, 240.);
        let part = springs.at("rail").unwrap();
        assert!(part < 264. && part > 0., "on its way at {part}");
        run(&mut springs, start + Duration::from_millis(50), 1., 240.);
        assert_eq!(springs.at("rail"), Some(0.));
        assert!(!springs.moving() && springs.arrived("rail"));
    }

    #[test]
    fn the_curve_is_the_same_at_any_frame_rate_and_only_a_soft_spring_overshoots() {
        let at = |hz: f32, spring: Spring| {
            let mut springs = Springs::new();
            let start = Instant::now();
            springs.step(start);
            springs.to("x", 0., spring);
            springs.to("x", 100., spring);
            run(&mut springs, start, 0.05, hz);
            springs.at("x").unwrap()
        };
        for spring in [Spring::SNAPPY, Spring::SOFT, Spring::WIDE] {
            assert!((at(60., spring) - at(240., spring)).abs() < 0.5);
        }

        let peak = |spring: Spring| {
            let mut springs = Springs::new();
            let start = Instant::now();
            springs.step(start);
            springs.to("x", 0., spring);
            springs.to("x", 100., spring);
            let mut highest = 0f32;
            for frame in 1..=480 {
                springs.step(start + Duration::from_secs_f32(frame as f32 / 240.));
                highest = highest.max(springs.at("x").unwrap());
            }
            highest
        };
        assert!(peak(Spring::SNAPPY) <= 100.01);
        let soft = peak(Spring::SOFT);
        assert!(
            soft > 100.3 && soft < 103.,
            "a small overshoot, not a wobble: {soft}"
        );
    }

    #[test]
    fn an_entrance_starts_where_it_is_told_and_stillness_skips_the_trip() {
        let mut springs = Springs::new();
        let start = Instant::now();
        springs.step(start);
        assert_eq!(springs.from("row", 0., 28., Spring::SNAPPY), 0.);
        run(&mut springs, start, 0.5, 120.);
        assert_eq!(springs.from("row", 0., 28., Spring::SNAPPY), 28.);
        springs.forget("row");
        assert_eq!(
            springs.from("row", 0., 28., Spring::SNAPPY),
            0.,
            "new again"
        );

        let mut still = Springs::new();
        still.set_still(true);
        still.step(start);
        assert_eq!(still.from("row", 0., 28., Spring::SNAPPY), 28.);
        still.to("rail", 264., Spring::WIDE);
        assert_eq!(still.to("rail", 0., Spring::WIDE), 0.);
        assert!(!still.moving());
    }

    #[test]
    fn a_target_changed_mid_flight_is_turned_toward_without_a_jump() {
        let mut springs = Springs::new();
        let start = Instant::now();
        springs.step(start);
        springs.to("y", 0., Spring::SNAPPY);
        springs.to("y", 100., Spring::SNAPPY);
        run(&mut springs, start, 0.03, 240.);
        let before = springs.at("y").unwrap();
        assert_eq!(springs.to("y", 20., Spring::SNAPPY), before);
        springs.step(start + Duration::from_secs_f32(0.03 + 1. / 240.));
        let after = springs.at("y").unwrap();
        // It was heading up and still carries that speed for a moment: the
        // next frame is a small step the same way, not a snap toward the new place.
        assert!(after > before && after - before < 8., "{before} to {after}");
        run(&mut springs, start + Duration::from_millis(40), 1., 240.);
        assert_eq!(springs.at("y"), Some(20.));
    }
}
