//! What loops while something happens, as the web cockpit has it: the light
//! that goes round a tool call while it runs (`timeline.css`, `.tool-run`),
//! the shimmer across "Thinking" while it is written (`.who.shimmer`), and the
//! activity meter while the agent works (`theme.css`, `.meter`).
//!
//! Each is a small view of its own, so that a frame of its motion builds only
//! itself: a light over a row of a long transcript does not have the
//! transcript built again at every frame of the display. A row cannot hold
//! such a view (the rows of a list are built with the view around them), so
//! the transcript's lights are drawn on a layer over the list: the row notes
//! where its card or its label is as it is laid out ([`Spots`]), and the light
//! paints there. Its phase is the clock its owner started with, as the web's
//! loops take the page's clock, so one drawn again (its row scrolled back
//! into sight, the view zoomed or shown again) goes on from where it was
//! instead of starting over. With reduced motion there are no lights: the
//! border and the label hold still, and the meter is unlit.

use crate::dock::scaled;
use gpui::{
    Context, Hsla, IntoElement, ParentElement as _, PathBuilder, Pixels, Point, Render,
    SharedString, Styled as _, TextRun, Window, canvas, div, font, point,
};
use kou_ui::{
    metric,
    motion::cockpit,
    scale::{self, px},
    theme,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Instant,
};

/// Which light of which row: the row, and 0 for a call's card, 1 for "Thinking".
pub(crate) type Spot = (usize, u8);

/// Where the rows that have a light were laid out last, as drawn: a call's
/// card, a label. Emptied when the transcript is built again; the rows that
/// show note their places as they are prepainted.
pub(crate) type Spots = Rc<RefCell<HashMap<Spot, gpui::Bounds<Pixels>>>>;

/// What a light shows.
pub(crate) enum Light {
    /// Goes round the border of a card with corners of this radius, unscaled.
    Run { radius: f32 },
    /// Passes across a label written in the monospace.
    Shimmer { label: SharedString },
    /// Passes along a meter of eight cells, with how long the turn has run
    /// since `since` after it (`.activity time`).
    Meter { since: Option<Instant> },
    /// The quieter look's spinner: a ring of dashes `side` across, a brighter
    /// stretch of it going round. Held still, an even ring.
    Spin { color: Hsla, side: f32 },
    /// A ring going out from the dot of an agent that runs, over the
    /// transcript where the dot noted its place. Held still, nothing.
    Halo { color: Hsla },
}

/// A light, drawn by a view of its own. See the [module](self).
pub(crate) struct Lights {
    light: Light,
    /// Where every loop's phase is counted from.
    epoch: Instant,
    /// The factor the view it is drawn in draws at.
    scale: Rc<Cell<f32>>,
    /// Over the transcript: where the row it lights noted its place.
    spot: Option<(Spots, Spot)>,
}

impl Lights {
    pub(crate) fn new(light: Light, epoch: Instant, scale: Rc<Cell<f32>>) -> Self {
        Self {
            light,
            epoch,
            scale,
            spot: None,
        }
    }

    /// A light for the layer over the transcript, painted where row `spot`
    /// noted its place.
    pub(crate) fn over(
        light: Light,
        epoch: Instant,
        scale: Rc<Cell<f32>>,
        spots: Spots,
        spot: Spot,
    ) -> Self {
        Self {
            light,
            epoch,
            scale,
            spot: Some((spots, spot)),
        }
    }
}

/// An element that notes where it is laid out as `spot`: in a row whose light
/// is drawn over the transcript.
pub(crate) fn mark_spot(spots: &Spots, spot: Spot) -> impl IntoElement + use<> {
    let spots = spots.clone();
    canvas(
        move |bounds, _, _| {
            spots.borrow_mut().insert(spot, bounds);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Cells of the activity meter, their side and the gap between them.
pub(crate) const METER_CELLS: usize = 8;
const CELL: f32 = 5.;
const CELL_GAP: f32 = 2.;

impl Render for Lights {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let still = cx.reduce_motion();
        let now = cx.background_executor().now();
        let elapsed = now.saturating_duration_since(self.epoch);
        if !still {
            window.request_animation_frame();
        } else if let Light::Meter { since: Some(since) } = &self.light {
            // Held still, the clock still goes on, a second at a time.
            let into = now.saturating_duration_since(*since).subsec_millis();
            let timer = cx
                .background_executor()
                .timer(std::time::Duration::from_millis(u64::from(1000 - into)));
            cx.spawn(async move |this, cx| {
                timer.await;
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        }
        let theme = theme(cx);
        let factor = self.scale.get();
        let element = scale::with_factor(factor, || match &self.light {
            Light::Run { radius } => {
                let accent = theme.working;
                let radius = *radius;
                let spot = self.spot.clone();
                // Where the light is along the border, as a share of it.
                let at = if still {
                    None
                } else {
                    Some(elapsed.as_secs_f32() / cockpit::RUN_LIGHT.as_secs_f32() % 1.)
                };
                let factor = scale::factor();
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let Some(at) = at else {
                                    return;
                                };
                                // Over the transcript, the card's place as its row noted it.
                                let bounds = match &spot {
                                    Some((spots, spot)) => match spots.borrow().get(spot) {
                                        Some(card) => *card,
                                        None => return,
                                    },
                                    None => bounds,
                                };
                                let edge = RoundedEdge::new(bounds, radius * factor, factor);
                                // The trail, then the head at its front, as the
                                // web's two dashes are: 30% and 12% of the way.
                                for (from, length, alpha) in [(0., 0.30, 0.35), (0.18, 0.12, 1.)] {
                                    if let Some(path) = edge.stroke(at + from, length) {
                                        window.paint_path(path, accent.opacity(alpha));
                                    }
                                }
                            },
                        )
                        .size_full(),
                    )
                    .into_any_element()
            }
            Light::Shimmer { label } => {
                let (dim, lit) = (theme.fg_muted, theme.fg);
                let len = label.chars().count().max(1);
                // `background-position` from 100% to 0% over the period: the
                // gradient, three times the label's width, slides right to left.
                let offset = if still {
                    None
                } else {
                    let t = elapsed.as_secs_f32() / cockpit::SHIMMER.as_secs_f32() % 1.;
                    Some((1. - t) * 2. / 3.)
                };
                let mono = font(theme.mono_font.clone());
                let mut runs = Vec::with_capacity(len);
                for (index, ch) in label.chars().enumerate() {
                    let color = match offset {
                        None => dim,
                        Some(offset) => {
                            let across = (index as f32 + 0.5) / len as f32;
                            shimmer(dim, lit, offset + across / 3.)
                        }
                    };
                    runs.push(TextRun {
                        len: ch.len_utf8(),
                        font: mono.clone(),
                        color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    });
                }
                let runs = bold(runs);
                match self.spot.clone() {
                    // Over the transcript: the label painted where its row
                    // keeps its place.
                    Some((spots, spot)) => {
                        let (label, size) = (label.clone(), px(crate::entries::LABEL));
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .child(
                                canvas(
                                    |_, _, _| {},
                                    move |_, _, window, cx| {
                                        let Some(place) = spots.borrow().get(&spot).copied() else {
                                            return;
                                        };
                                        let line = window.text_system().shape_line(
                                            label.clone(),
                                            size,
                                            &runs,
                                            None,
                                        );
                                        line.paint(
                                            place.origin,
                                            place.size.height,
                                            gpui::TextAlign::Left,
                                            None,
                                            window,
                                            cx,
                                        )
                                        .ok();
                                    },
                                )
                                .size_full(),
                            )
                            .into_any_element()
                    }
                    None => div()
                        .flex_none()
                        .font_family(theme.mono_font.clone())
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_size(px(crate::entries::LABEL))
                        .child(scale::styled_text(label.clone(), runs))
                        .into_any_element(),
                }
            }
            Light::Spin { color, side } => {
                let (color, side) = (*color, *side);
                let turn = (!still).then(|| kou_ui::motion::studio::spin(elapsed));
                let factor = scale::factor();
                div()
                    .flex_none()
                    .size(px(side))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let centre = bounds.center();
                                let radius = (side / 2. - 0.9) * factor;
                                let dash = 1.5 * factor;
                                for (at, alpha) in spin_dashes(turn) {
                                    let angle = at * std::f32::consts::TAU;
                                    let place = point(
                                        centre.x + gpui::px(radius * angle.sin()),
                                        centre.y - gpui::px(radius * angle.cos()),
                                    );
                                    let dot = gpui::Bounds::new(
                                        place - point(gpui::px(dash / 2.), gpui::px(dash / 2.)),
                                        gpui::size(gpui::px(dash), gpui::px(dash)),
                                    );
                                    window.paint_quad(
                                        gpui::fill(dot, color.opacity(alpha))
                                            .corner_radii(gpui::px(dash / 2.)),
                                    );
                                }
                            },
                        )
                        .size_full(),
                    )
                    .into_any_element()
            }
            Light::Halo { color } => {
                let color = *color;
                let spot = self.spot.clone();
                let ring = (!still).then(|| kou_ui::motion::studio::halo(elapsed));
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |_, _, window, _| {
                                let (Some((reach, alpha)), Some((spots, spot))) = (ring, &spot)
                                else {
                                    return;
                                };
                                let Some(dot) = spots.borrow().get(spot).copied() else {
                                    return;
                                };
                                if alpha <= 0. {
                                    return;
                                }
                                let centre = dot.center();
                                let radius = dot.size.width / 2. * reach;
                                let bounds = gpui::Bounds::new(
                                    centre - point(radius, radius),
                                    gpui::size(radius * 2., radius * 2.),
                                );
                                window.paint_quad(gpui::quad(
                                    bounds,
                                    radius,
                                    gpui::transparent_black(),
                                    dot.size.width / 6.5,
                                    color.opacity(alpha),
                                    gpui::BorderStyle::Solid,
                                ));
                            },
                        )
                        .size_full(),
                    )
                    .into_any_element()
            }
            Light::Meter { since } => {
                let (off, on) = (theme.line_strong, theme.working);
                let clock =
                    since.map(|since| crate::motion::clock(now.saturating_duration_since(since)));
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(CELL_GAP))
                            .children((0..METER_CELLS).map(|index| {
                                let lit = if still {
                                    0.
                                } else {
                                    cockpit::meter(elapsed, index)
                                };
                                div()
                                    .size(px(CELL))
                                    .rounded(px(metric::RADIUS_MARK))
                                    .bg(mix(off, on, lit))
                            })),
                    )
                    .children(clock.map(|clock| {
                        div()
                            .flex_none()
                            .font_family(theme.mono_font.clone())
                            .text_size(px(11.5))
                            .text_color(theme.fg_muted)
                            .child(scale::text(clock))
                    }))
                    .into_any_element()
            }
        });
        scaled(factor, element)
    }
}

/// The dashes of the spinner: where each stands round the ring, as a share
/// of a turn from the top, and how lit it is. Turning, a stretch of them is
/// brighter, fading behind its head; held still, all are alike.
pub(crate) fn spin_dashes(turn: Option<f32>) -> impl Iterator<Item = (f32, f32)> {
    const DASHES: usize = 10;
    (0..DASHES).map(move |index| {
        let at = index as f32 / DASHES as f32;
        let alpha = match turn {
            None => 0.6,
            Some(turn) => {
                // How far behind the head this dash is, as a share of a turn.
                let behind = (turn - at).rem_euclid(1.);
                0.28 + 0.72 * (1. - behind).powi(2)
            }
        };
        (at, alpha)
    })
}

/// The runs of a label made bold.
fn bold(mut runs: Vec<TextRun>) -> Vec<TextRun> {
    for run in &mut runs {
        run.font.weight = gpui::FontWeight::BOLD;
    }
    runs
}

/// The colour of the shimmer's gradient at `at` of its width:
/// `linear-gradient(90deg, dim 0 38%, lit 50%, dim 62% 100%)`.
fn shimmer(dim: Hsla, lit: Hsla, at: f32) -> Hsla {
    let t = if (0.38..0.5).contains(&at) {
        (at - 0.38) / 0.12
    } else if (0.5..0.62).contains(&at) {
        1. - (at - 0.5) / 0.12
    } else {
        0.
    };
    mix(dim, lit, t)
}

/// `from` blended towards `to` by `t`, in RGB as CSS blends colours.
pub(crate) fn mix(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0., 1.);
    let (a, b) = (gpui::Rgba::from(from), gpui::Rgba::from(to));
    gpui::Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

/// Where a stretch of the border puts the point a distance along it.
type Place<'a> = dyn Fn(f32) -> Point<Pixels> + 'a;

/// The border of a card with rounded corners, a line half a pixel inside its
/// edge, walked clockwise from where its top edge leaves the top left corner,
/// as an SVG `rect` is.
struct RoundedEdge {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    radius: f32,
    /// The pixel the stroke is as wide as, at the factor drawn at.
    unit: f32,
}

impl RoundedEdge {
    fn new(bounds: gpui::Bounds<Pixels>, radius: f32, factor: f32) -> Self {
        let inset = 0.5 * factor;
        let width = (bounds.size.width.as_f32() - 2. * inset).max(0.);
        let height = (bounds.size.height.as_f32() - 2. * inset).max(0.);
        Self {
            left: bounds.origin.x.as_f32() + inset,
            top: bounds.origin.y.as_f32() + inset,
            width,
            height,
            radius: radius.min(width / 2.).min(height / 2.).max(0.),
            unit: factor,
        }
    }

    /// The edge's length.
    fn length(&self) -> f32 {
        let r = self.radius;
        2. * (self.width + self.height) - 8. * r + std::f32::consts::TAU * r
    }

    /// The point `s` along the edge, `s` from 0 to its length.
    fn at(&self, s: f32) -> Point<Pixels> {
        let r = self.radius;
        let (w, h) = (self.width - 2. * r, self.height - 2. * r);
        let quarter = std::f32::consts::FRAC_PI_2 * r;
        let (x0, y0) = (self.left, self.top);
        let corner = |cx: f32, cy: f32, from: f32, along: f32| {
            let angle = from + if r > 0. { along / r } else { 0. };
            point(
                gpui::px(cx + r * angle.cos()),
                gpui::px(cy + r * angle.sin()),
            )
        };
        let pieces: [(f32, &Place); 8] = [
            (w, &|d| point(gpui::px(x0 + r + d), gpui::px(y0))),
            (quarter, &|d| {
                corner(x0 + r + w, y0 + r, -std::f32::consts::FRAC_PI_2, d)
            }),
            (h, &|d| {
                point(gpui::px(x0 + self.width), gpui::px(y0 + r + d))
            }),
            (quarter, &|d| corner(x0 + r + w, y0 + r + h, 0., d)),
            (w, &|d| {
                point(gpui::px(x0 + r + w - d), gpui::px(y0 + self.height))
            }),
            (quarter, &|d| {
                corner(x0 + r, y0 + r + h, std::f32::consts::FRAC_PI_2, d)
            }),
            (h, &|d| point(gpui::px(x0), gpui::px(y0 + r + h - d))),
            (quarter, &|d| {
                corner(x0 + r, y0 + r, std::f32::consts::PI, d)
            }),
        ];
        let mut left = s.rem_euclid(self.length().max(f32::EPSILON));
        for (length, place) in pieces {
            if left <= length {
                return place(left);
            }
            left -= length;
        }
        point(gpui::px(x0 + r), gpui::px(y0))
    }

    /// A stroke along the edge from `from` for `length`, both shares of the
    /// edge's length; `from` may be past 1, which goes round again.
    fn stroke(&self, from: f32, length: f32) -> Option<gpui::Path<Pixels>> {
        let total = self.length();
        if total < 1. {
            return None;
        }
        let (start, span) = (from * total, length * total);
        // Short enough steps that a corner reads as a curve.
        let steps = (span / (2. * self.unit)).ceil().clamp(2., 400.) as usize;
        let mut path = PathBuilder::stroke(gpui::px(self.unit));
        path.move_to(self.at(start));
        for step in 1..=steps {
            path.line_to(self.at(start + span * step as f32 / steps as f32));
        }
        path.build().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, px as gpx, size};

    #[test]
    fn the_run_light_walks_the_border_clockwise_from_the_top_left() {
        let edge = RoundedEdge::new(
            Bounds::new(point(gpx(0.), gpx(0.)), size(gpx(101.), gpx(41.))),
            6.,
            1.,
        );
        let start = edge.at(0.);
        assert_eq!((start.x.as_f32(), start.y.as_f32()), (6.5, 0.5));
        // A quarter of the way it is on the right edge or its corner, half
        // way on the bottom edge, and it comes back to where it began.
        let half = edge.at(edge.length() / 2.);
        assert!((half.y.as_f32() - 40.5).abs() < 0.01, "{half:?}");
        let round = edge.at(edge.length());
        assert!((round.x.as_f32() - start.x.as_f32()).abs() < 0.01);
        assert!(
            edge.stroke(0.95, 0.3).is_some(),
            "a stroke past the end goes round"
        );
    }

    #[test]
    fn the_shimmer_lights_the_middle_of_its_gradient_only() {
        let (dim, lit) = (gpui::black(), gpui::white());
        assert_eq!(shimmer(dim, lit, 0.2), dim);
        assert_eq!(shimmer(dim, lit, 0.5), lit);
        assert_eq!(shimmer(dim, lit, 0.8), dim);
        assert!(gpui::Rgba::from(shimmer(dim, lit, 0.44)).r > 0.4);
    }
}
