//! A blinking cursor: where it was last seen, since when it has been there, and
//! the redraw asked for its next change. The curve is the application's
//! ([`TerminalView::set_cursor_blink`](crate::TerminalView::set_cursor_blink)),
//! so that the terminal's cursor blinks as the application's other carets do.

use gpui::{App, EntityId, Task, Window};
use std::time::{Duration, Instant};

/// How lit a blinking cursor is `elapsed` after it last moved or a key was
/// pressed, from 1 to 0, and how long it holds so: `None` while it fades, when
/// the next frame is wanted.
pub type CursorBlink = fn(Duration) -> (f32, Option<Duration>);

pub(crate) struct Blink {
    seen: Option<(usize, usize)>,
    since: Instant,
    /// It held still when last drawn: it starts lit when it blinks again.
    held: bool,
    due: Option<Instant>,
    timer: Option<Task<()>>,
}

// Two redraws asked for within this of each other are one.
const SLACK: Duration = Duration::from_millis(4);

impl Blink {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            seen: None,
            since: now,
            held: false,
            due: None,
            timer: None,
        }
    }

    /// Lit at once, its cycle started over.
    pub(crate) fn restart(&mut self, now: Instant) {
        self.since = now;
        self.due = None;
        self.timer = None;
    }

    /// Where the cursor is: a move lights it at once.
    pub(crate) fn at(&mut self, place: (usize, usize), now: Instant) {
        if self.seen != Some(place) {
            self.seen = Some(place);
            self.restart(now);
        }
    }

    /// It does not blink now: nothing is asked for, and it starts lit.
    pub(crate) fn hold(&mut self) {
        self.held = true;
        self.due = None;
        self.timer = None;
    }

    /// How lit it is at `now`, having asked for what its blink needs next: the
    /// next frame while it fades, a redraw of `view` when it next changes.
    pub(crate) fn lit(
        &mut self,
        curve: CursorBlink,
        now: Instant,
        view: EntityId,
        window: &mut Window,
        cx: &mut App,
    ) -> f32 {
        if self.held {
            self.held = false;
            self.restart(now);
        }
        let (lit, next) = curve(now.saturating_duration_since(self.since));
        match next {
            None => {
                self.due = None;
                self.timer = None;
                window.request_animation_frame();
            }
            Some(wait) => {
                let due = now + wait;
                let asked = self.timer.is_some()
                    && self.due.is_some_and(|at| {
                        at.max(due).saturating_duration_since(at.min(due)) < SLACK
                    });
                if !asked {
                    let timer = cx.background_executor().timer(wait);
                    self.due = Some(due);
                    self.timer = Some(cx.spawn(async move |cx| {
                        timer.await;
                        cx.update(|cx| cx.notify(view));
                    }));
                }
            }
        }
        lit.clamp(0., 1.)
    }
}
