//! The cursor in flight: when it moves, its four corners travel to the new
//! place on springs of their own, the ones that lead stiffer than the ones that
//! trail, so it stretches along the way and settles with a small spring. The
//! idea is Neovide's; the arithmetic is this crate's.

use std::time::{Duration, Instant};

/// How the cursor moves between cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorMotion {
    /// Seconds a jump across the screen takes to settle. A step of a cell or
    /// two, as typing makes, takes well under half of it.
    pub duration: f32,
    /// How far the trailing edge lags behind the leading one, 0 (a rigid block)
    /// ..= 1 (a long smear).
    pub trail: f32,
    /// How much it overshoots and springs back on arrival, 0 (none) ..= 1.
    pub bounce: f32,
}

impl Default for CursorMotion {
    fn default() -> Self {
        Self {
            duration: 0.14,
            trail: 0.7,
            bounce: 0.3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Corner {
    at: (f32, f32),
    speed: (f32, f32),
    // The spring's natural frequency for this flight, in radians per second.
    stiffness: f32,
}

/// The cursor's place between frames. One per terminal view.
#[derive(Debug, Default)]
pub(crate) struct Smear {
    corners: [Corner; 4],
    target: Option<[f32; 4]>,
    stepped: Option<Instant>,
    flying: bool,
}

// Each corner's direction from the centre: top left, top right, bottom right, bottom left.
const OUTWARD: [(f32, f32); 4] = [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)];
// The integration step: small enough that 60 and 240 frames a second agree.
const STEP: f32 = 1. / 960.;

fn corners_of([x, y, w, h]: [f32; 4]) -> [(f32, f32); 4] {
    [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
}

impl Smear {
    /// Forget where the cursor was: it will appear at its next place without a flight.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Whether the cursor is between places.
    #[cfg(test)]
    pub(crate) fn flying(&self) -> bool {
        self.flying
    }

    /// Move toward `target` (x, y, width, height) as of `now`. Returns the four
    /// corners to draw while the cursor is in flight, and `None` once it rests
    /// exactly on the target, where the ordinary cursor is drawn.
    pub(crate) fn step(
        &mut self,
        target: [f32; 4],
        cell_width: f32,
        now: Instant,
        motion: &CursorMotion,
    ) -> Option<[(f32, f32); 4]> {
        let destination = corners_of(target);
        let Some(previous) = self.target else {
            // The first place it is seen: nothing to fly from.
            self.target = Some(target);
            for (corner, at) in self.corners.iter_mut().zip(destination) {
                *corner = Corner {
                    at,
                    ..Corner::default()
                };
            }
            self.stepped = Some(now);
            return None;
        };

        if previous != target {
            self.target = Some(target);
            let from = (
                previous[0] + previous[2] / 2.,
                previous[1] + previous[3] / 2.,
            );
            let to = (target[0] + target[2] / 2., target[1] + target[3] / 2.);
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let distance = dx.hypot(dy);
            let duration = motion.duration.clamp(0.02, 2.);
            // Settling to within a fiftieth takes about six time constants.
            let mut base = 6. / duration;
            // A hop along the line, as typing makes, must not feel late.
            if dy.abs() < 0.5 && distance <= cell_width.max(1.) * 2.5 {
                base *= 2.4;
            }
            let trail = motion.trail.clamp(0., 1.);
            for (corner, outward) in self.corners.iter_mut().zip(OUTWARD) {
                let ahead = if distance > 0.01 {
                    (dx * outward.0 + dy * outward.1) / (distance * std::f32::consts::SQRT_2)
                } else {
                    0.
                };
                // From the slowest, for a corner at the back, to the quickest at the front.
                let slow = 1. - 0.6 * trail;
                let quick = 1. + 0.9 * trail;
                corner.stiffness = base * (slow + (quick - slow) * (ahead + 1.) / 2.);
            }
            if !self.flying {
                self.stepped = Some(now);
            }
            self.flying = true;
        }
        if !self.flying {
            return None;
        }

        // A window that was not drawn for a while does not make the cursor leap.
        let elapsed = self
            .stepped
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last))
            .as_secs_f32()
            .min(0.1);
        self.stepped = Some(now);
        let damping = 1. - 0.55 * motion.bounce.clamp(0., 1.);
        let mut remaining = elapsed;
        while remaining > 0. {
            let dt = remaining.min(STEP);
            remaining -= dt;
            for (corner, to) in self.corners.iter_mut().zip(destination) {
                let k = corner.stiffness * corner.stiffness;
                let c = 2. * damping * corner.stiffness;
                corner.speed.0 += (-k * (corner.at.0 - to.0) - c * corner.speed.0) * dt;
                corner.speed.1 += (-k * (corner.at.1 - to.1) - c * corner.speed.1) * dt;
                corner.at.0 += corner.speed.0 * dt;
                corner.at.1 += corner.speed.1 * dt;
            }
        }

        let arrived = self.corners.iter().zip(destination).all(|(corner, to)| {
            (corner.at.0 - to.0).abs() < 0.2
                && (corner.at.1 - to.1).abs() < 0.2
                && corner.speed.0.abs() < 8.
                && corner.speed.1.abs() < 8.
        });
        if arrived {
            for (corner, at) in self.corners.iter_mut().zip(destination) {
                *corner = Corner {
                    at,
                    ..Corner::default()
                };
            }
            self.flying = false;
            return None;
        }
        Some(self.corners.map(|corner| corner.at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: [f32; 4] = [100., 50., 8., 18.];

    fn fly(
        smear: &mut Smear,
        to: [f32; 4],
        start: Instant,
        frames: u32,
        hz: f32,
    ) -> Option<[(f32, f32); 4]> {
        // The flight starts on the frame that first sees the new place.
        let mut quad = smear.step(to, 8., start, &CursorMotion::default());
        for frame in 1..=frames {
            let now = start + Duration::from_secs_f32(frame as f32 / hz);
            quad = smear.step(to, 8., now, &CursorMotion::default());
        }
        quad
    }

    #[test]
    fn the_first_place_is_taken_at_once_and_a_cursor_that_stays_does_not_fly() {
        let mut smear = Smear::default();
        let start = Instant::now();
        assert_eq!(smear.step(CELL, 8., start, &CursorMotion::default()), None);
        assert_eq!(fly(&mut smear, CELL, start, 10, 240.), None);
        assert!(!smear.flying());
    }

    #[test]
    fn a_jump_stretches_along_the_way_and_lands_exactly() {
        let mut smear = Smear::default();
        let start = Instant::now();
        smear.step(CELL, 8., start, &CursorMotion::default());
        let far = [600., 50., 8., 18.];
        // A quarter of the way in time: the front is well ahead of the back.
        let quad = fly(&mut smear, far, start, 8, 240.).expect("in flight");
        let (left, right) = (quad[0].0.min(quad[3].0), quad[1].0.max(quad[2].0));
        assert!(right - left > 8. * 6., "stretched to {} wide", right - left);
        assert!(
            left > 100. && right < 615.,
            "between the two places: {left}..{right}"
        );
        // The leading corners (right side, travelling right) are ahead of the trailing ones.
        assert!(quad[1].0 > quad[0].0 + 40. && quad[2].0 > quad[3].0 + 40.);

        let mut frames = 8;
        while smear.flying() {
            frames += 1;
            assert!(frames < 240, "still flying after a second");
            smear.step(
                far,
                8.,
                start + Duration::from_secs_f32(frames as f32 / 240.),
                &CursorMotion::default(),
            );
        }
        // At rest the corners are the target's, to the bit.
        assert_eq!(smear.corners.map(|corner| corner.at), corners_of(far));
        assert!(
            frames as f32 / 240. < 0.45,
            "took {} s",
            frames as f32 / 240.
        );
    }

    #[test]
    fn a_step_of_one_cell_is_over_in_a_few_hundredths_of_a_second() {
        let mut smear = Smear::default();
        let start = Instant::now();
        smear.step(CELL, 8., start, &CursorMotion::default());
        let next = [108., 50., 8., 18.];
        let mut frames = 0;
        loop {
            frames += 1;
            let now = start + Duration::from_secs_f32(frames as f32 / 240.);
            if smear
                .step(next, 8., now, &CursorMotion::default())
                .is_none()
            {
                break;
            }
            assert!(frames < 60);
        }
        assert!(
            frames as f32 / 240. < 0.12,
            "took {} s",
            frames as f32 / 240.
        );
    }

    #[test]
    fn the_flight_is_the_same_at_sixty_and_at_two_hundred_and_forty_frames_a_second() {
        let far = [600., 300., 8., 18.];
        let at = |hz: f32, frames: u32| {
            let mut smear = Smear::default();
            let start = Instant::now();
            smear.step(CELL, 8., start, &CursorMotion::default());
            fly(&mut smear, far, start, frames, hz).expect("in flight")
        };
        // Fifty milliseconds in, by either clock.
        let (slow, fast) = (at(60., 3), at(240., 12));
        for (a, b) in slow.iter().zip(fast) {
            assert!(
                (a.0 - b.0).abs() < 1.5 && (a.1 - b.1).abs() < 1.5,
                "{a:?} and {b:?}"
            );
        }
    }

    #[test]
    fn a_rigid_setting_does_not_stretch_and_a_forgotten_cursor_does_not_fly() {
        let mut smear = Smear::default();
        let start = Instant::now();
        let rigid = CursorMotion {
            trail: 0.,
            bounce: 0.,
            ..CursorMotion::default()
        };
        smear.step(CELL, 8., start, &rigid);
        let far = [600., 50., 8., 18.];
        let quad = smear
            .step(far, 8., start + Duration::from_millis(30), &rigid)
            .expect("in flight");
        assert!(
            ((quad[1].0 - quad[0].0) - 8.).abs() < 0.01,
            "kept its width"
        );

        smear.reset();
        assert_eq!(
            smear.step(far, 8., start + Duration::from_millis(40), &rigid),
            None
        );
    }
}
