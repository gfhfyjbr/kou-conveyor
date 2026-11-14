//! What the canvas's motion is made of. Every animation is a function of the
//! canvas's clock, not of a frame count: it is drawn on every frame the
//! display gives while something moves, and a capture can stop the clock at
//! any instant and draw that instant.

use gpui::{Hsla, Rgba};
use kou_ui::motion;
use std::time::{Duration, Instant};

/// The canvas's time: the wall clock, or an instant held still for a capture.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    held: Option<Instant>,
    epoch: Instant,
    reduced: bool,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            held: None,
            epoch: Instant::now(),
            reduced: false,
        }
    }
}

impl Clock {
    /// Now, as the canvas tells it.
    pub fn now(&self) -> Instant {
        self.held.unwrap_or_else(Instant::now)
    }

    /// Hold the clock at the instant it is now; animations started while it
    /// is held start at that instant, which `hold_at` measures from.
    pub fn hold(&mut self) {
        self.epoch = Instant::now();
        self.held = Some(self.epoch);
    }

    /// Hold the clock a time after the instant it was first held at.
    pub fn hold_at(&mut self, elapsed: Duration) {
        if self.held.is_none() {
            self.hold();
        }
        self.held = Some(self.epoch + elapsed);
    }

    /// Let the clock run again.
    pub fn release(&mut self) {
        self.held = None;
    }

    /// Whether the clock is held.
    pub fn is_held(&self) -> bool {
        self.held.is_some()
    }

    /// Whether the system asks for less motion: every animation then ends as it starts.
    pub fn set_reduced(&mut self, reduced: bool) {
        self.reduced = reduced;
    }

    /// How far through an animation of `duration` that started at `start` is,
    /// 0..=1, before easing.
    pub fn progress(&self, start: Instant, duration: Duration) -> f32 {
        if self.reduced || duration.is_zero() {
            return 1.;
        }
        let elapsed = self.now().saturating_duration_since(start);
        (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
    }
}

/// A value that moves from one number to another over a time, easing out:
/// what a hover, a selection or a menu's highlight is drawn with.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl Tween {
    /// A value that holds still.
    pub fn at(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: Instant::now(),
            duration: Duration::ZERO,
        }
    }

    /// Move to a new value from wherever the value is now.
    pub fn retarget(&mut self, to: f32, duration: Duration, clock: &Clock) {
        if (to - self.to).abs() < f32::EPSILON {
            return;
        }
        self.from = self.value(clock);
        self.to = to;
        self.start = clock.now();
        self.duration = duration;
    }

    /// The value now.
    pub fn value(&self, clock: &Clock) -> f32 {
        let t = clock.progress(self.start, self.duration);
        self.from + (self.to - self.from) * motion::ease_out(t)
    }

    /// Whether it is still on its way.
    pub fn moving(&self, clock: &Clock) -> bool {
        clock.progress(self.start, self.duration) < 1.
    }
}

/// A colour a fraction of the way to another, mixed in RGB as CSS's
/// `color-mix` does, so a tint of the accent on a surface is that surface,
/// warmed, not a different hue.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let t = t.clamp(0., 1.);
    let m = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
        a: m(a.a, b.a),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_clock_draws_any_instant_of_an_animation() {
        let mut clock = Clock::default();
        clock.hold();
        let mut tween = Tween::at(0.);
        tween.retarget(100., Duration::from_millis(200), &clock);
        assert_eq!(tween.value(&clock), 0.);
        clock.hold_at(Duration::from_millis(100));
        let half = tween.value(&clock);
        assert!(
            half > 50. && half < 100.,
            "ease out is ahead of the clock: {half}"
        );
        assert!(tween.moving(&clock));
        clock.hold_at(Duration::from_millis(200));
        assert_eq!(tween.value(&clock), 100.);
        assert!(!tween.moving(&clock));
        // Retargeted mid-way, it leaves from where it is, not from where it began.
        clock.hold_at(Duration::from_millis(100));
        tween.retarget(0., Duration::from_millis(200), &clock);
        assert!((tween.value(&clock) - half).abs() < 1e-3);
        clock.set_reduced(true);
        assert_eq!(tween.value(&clock), 0., "less motion: it is already there");
    }

    #[test]
    fn mixing_colours_runs_between_them() {
        let black: Hsla = gpui::rgb(0x000000).into();
        let white: Hsla = gpui::rgb(0xffffff).into();
        let grey = mix(black, white, 0.5).to_rgb();
        assert!((grey.r - 0.5).abs() < 1e-3 && (grey.g - 0.5).abs() < 1e-3);
        assert_eq!(mix(black, white, 0.).to_rgb(), black.to_rgb());
    }
}
