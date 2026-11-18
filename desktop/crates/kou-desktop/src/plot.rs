//! The marks of a chart a plugin draws: lines, bars or points over a grid,
//! with what is under the pointer said beside it.
//!
//! A plugin's script works out everything that takes thought (the scale,
//! the round values of its grid, how each number reads) and builds the
//! labels around the plot from boxes and text like anything else it draws.
//! What is left for the app is the part a script has no elements for: the
//! marks themselves, and following the pointer over them.
//!
//! The marks keep to a few fixed sizes: a line is 2 px with its newest point
//! a dot ringed in the ground's colour, a bar is at most 24 px thick with
//! the end that shows its value rounded and 2 px of ground between it and
//! the next, and the grid is a hairline that stays behind.

use gpui::{
    App, Bounds, Corners, DispatchPhase, Element, ElementId, Font, FontStyle, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseMoveEvent, PathBuilder, Pixels, Point, SharedString, Style, TextAlign, TextRun, Window,
    fill, point, px, relative, size,
};
use kou_ui::scale;
use std::{cell::Cell, rc::Rc};

/// How the values are marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Line,
    Bar,
    Points,
}

/// One series: its colour, its values (none where there is no number), and
/// each value as it reads.
#[derive(Clone, Debug, Default)]
pub(crate) struct Series {
    pub name: SharedString,
    pub color: Hsla,
    pub values: Vec<Option<f32>>,
    pub shown: Vec<SharedString>,
}

/// The colours a plot is drawn with besides its series'.
#[derive(Clone, Debug)]
pub(crate) struct Inks {
    /// The ground the plot stands on, for the gaps and rings.
    pub ground: Hsla,
    pub grid: Hsla,
    pub zero: Hsla,
    pub text: Hsla,
    pub quiet: Hsla,
    /// The box that says what is under the pointer, and its rim.
    pub tip: Hsla,
    pub rim: Hsla,
    pub font: SharedString,
}

/// See the [module](self).
pub(crate) struct Plot {
    pub id: ElementId,
    pub kind: Kind,
    pub series: Vec<Series>,
    /// What each point is called along the bottom, for the pointer.
    pub labels: Vec<SharedString>,
    /// The values at the bottom and the top of the plot.
    pub lo: f32,
    pub hi: f32,
    /// The grid's lines, as values; one at zero is drawn a little stronger
    /// when the plot goes below it.
    pub grid: Vec<f32>,
    /// How tall the plot is, unscaled.
    pub height: f32,
    pub inks: Inks,
}

/// How many points a plot has along the bottom.
fn count(series: &[Series]) -> usize {
    series
        .iter()
        .map(|series| series.values.len())
        .max()
        .unwrap_or(0)
}

/// Where point `index` of `count` stands across a plot `width` wide: bars
/// in the middle of their share, the rest from edge to edge.
pub(crate) fn across(kind: Kind, index: usize, count: usize, width: f32) -> f32 {
    match kind {
        Kind::Bar => (index as f32 + 0.5) / count.max(1) as f32 * width,
        _ if count <= 1 => width / 2.,
        _ => index as f32 / (count - 1) as f32 * width,
    }
}

/// The point nearest `x` across a plot `width` wide.
pub(crate) fn nearest(kind: Kind, x: f32, count: usize, width: f32) -> usize {
    if count == 0 || width <= 0. {
        return 0;
    }
    let share = (x / width).clamp(0., 1.);
    match kind {
        Kind::Bar => ((share * count as f32) as usize).min(count - 1),
        _ => (share * (count - 1) as f32).round() as usize,
    }
}

/// How thick each bar of a group is, and the gap between two of them, when
/// `series` of them share `room` across: never more than 24, and thinner
/// rather than touching.
pub(crate) fn bar(room: f32, series: usize) -> (f32, f32) {
    let series = series.max(1) as f32;
    let gap = if room / series >= 6. { 2. } else { 0. };
    let thick = ((room * 0.72 - gap * (series - 1.)) / series).clamp(1., 24.);
    (thick, gap)
}

impl Plot {
    /// How far down the plot `value` stands, from 0 at the top to 1.
    fn down(&self, value: f32) -> f32 {
        let span = self.hi - self.lo;
        if span.abs() < f32::EPSILON {
            return 0.5;
        }
        (1. - (value - self.lo) / span).clamp(0., 1.)
    }
}

impl IntoElement for Plot {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Plot {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = scale::px(self.height).into();
        style.flex_shrink = 0.;
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Hitbox {
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let factor = scale::factor();
        let (left, top) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
        let (width, height) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
        let count = count(&self.series);
        if width <= 0. || height <= 0. {
            return;
        }
        let at = |x: f32, down: f32| point(px(left + x), px(top + down * height));
        let hair = (1. * factor).max(0.5);
        // The grid, behind everything.
        for value in &self.grid {
            let zero = value.abs() < (self.hi - self.lo).abs() * 1e-6 && self.lo < 0.;
            let line = Bounds::new(
                at(0., self.down(*value)) - point(px(0.), px(hair / 2.)),
                size(px(width), px(hair)),
            );
            window.paint_quad(fill(
                line,
                if zero { self.inks.zero } else { self.inks.grid },
            ));
        }
        if count == 0 {
            return;
        }
        // Which point the pointer is over, kept from one frame to the next.
        let over = id.map(|id| {
            window.with_element_state::<Rc<Cell<Option<usize>>>, _>(id, |kept, _| {
                let kept = kept.unwrap_or_default();
                (kept.clone(), kept)
            })
        });
        if let Some(over) = over.clone() {
            let (kind, hitbox) = (self.kind, hitbox.clone());
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let now = hitbox
                    .is_hovered(window)
                    .then(|| nearest(kind, event.position.x.as_f32() - left, count, width));
                if over.get() != now {
                    over.set(now);
                    window.refresh();
                }
            });
        }
        let pointed = over
            .as_ref()
            .and_then(|over| over.get())
            .filter(|index| *index < count);
        let x = |index: usize| across(self.kind, index, count, width);
        match self.kind {
            Kind::Bar => {
                let room = width / count as f32;
                let (thick, gap) = bar(room, self.series.len());
                let (thick, gap) = (thick.min(24. * factor.max(1.)), gap * factor);
                let group = thick * self.series.len() as f32
                    + gap * (self.series.len().saturating_sub(1)) as f32;
                let base = self.down(0f32.clamp(self.lo, self.hi)) * height;
                let round = px((4. * factor).min(thick / 2.));
                for (nth, series) in self.series.iter().enumerate() {
                    for (index, value) in series.values.iter().enumerate() {
                        let Some(value) = value else {
                            continue;
                        };
                        let end = self.down(*value) * height;
                        let from = x(index) - group / 2. + nth as f32 * (thick + gap);
                        let (up, tall) = (end.min(base), (end - base).abs().max(hair));
                        // Round where the value is, square where it stands.
                        let corners = if end <= base {
                            Corners {
                                top_left: round,
                                top_right: round,
                                ..Corners::default()
                            }
                        } else {
                            Corners {
                                bottom_left: round,
                                bottom_right: round,
                                ..Corners::default()
                            }
                        };
                        let quiet = pointed.is_some_and(|pointed| pointed != index);
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    point(px(left + from), px(top + up)),
                                    size(px(thick), px(tall)),
                                ),
                                if quiet {
                                    series.color.opacity(0.45)
                                } else {
                                    series.color
                                },
                            )
                            .corner_radii(corners),
                        );
                    }
                }
            }
            Kind::Line => {
                for series in &self.series {
                    let mut path = PathBuilder::stroke(px(2. * factor));
                    let (mut pen, mut drawn) = (false, 0);
                    for (index, value) in series.values.iter().enumerate() {
                        match value {
                            Some(value) => {
                                let to = at(x(index), self.down(*value));
                                if pen {
                                    path.line_to(to);
                                    drawn += 1;
                                } else {
                                    path.move_to(to);
                                    pen = true;
                                }
                            }
                            None => pen = false,
                        }
                    }
                    if drawn > 0
                        && let Ok(built) = path.build()
                    {
                        window.paint_path(built, series.color);
                    }
                }
                // The newest point of each, marked.
                for series in &self.series {
                    if let Some((index, value)) = series
                        .values
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(index, value)| value.map(|value| (index, value)))
                    {
                        dot(
                            at(x(index), self.down(value)),
                            4. * factor,
                            series.color,
                            Some(self.inks.ground),
                            window,
                        );
                    }
                }
            }
            Kind::Points => {
                // Ringed where there is room for the rings, bare when crowded.
                let roomy = width / count as f32 >= 12. * factor;
                let radius = if roomy { 4. } else { 2.5 } * factor;
                for series in &self.series {
                    for (index, value) in series.values.iter().enumerate() {
                        if let Some(value) = value {
                            dot(
                                at(x(index), self.down(*value)),
                                radius,
                                series.color,
                                roomy.then_some(self.inks.ground),
                                window,
                            );
                        }
                    }
                }
            }
        }
        let Some(index) = pointed else {
            return;
        };
        // What is under the pointer: a line down the plot, each series'
        // point there, and a box that says them.
        let mid = x(index);
        if self.kind != Kind::Bar {
            window.paint_quad(fill(
                Bounds::new(
                    point(px(left + mid - hair / 2.), px(top)),
                    size(px(hair), px(height)),
                ),
                self.inks.quiet.opacity(0.6),
            ));
            for series in &self.series {
                if let Some(Some(value)) = series.values.get(index) {
                    dot(
                        at(mid, self.down(*value)),
                        4. * factor,
                        series.color,
                        Some(self.inks.ground),
                        window,
                    );
                }
            }
        }
        let font = |weight: FontWeight| Font {
            family: self.inks.font.clone(),
            features: Default::default(),
            fallbacks: None,
            weight,
            style: FontStyle::Normal,
        };
        let text_size = px(11. * factor);
        let shape = |text: SharedString, color: Hsla, weight: FontWeight, window: &mut Window| {
            let run = TextRun {
                len: text.len(),
                font: font(weight),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(text, text_size, &[run], None)
        };
        let head = shape(
            self.labels.get(index).cloned().unwrap_or_default(),
            self.inks.text,
            FontWeight::SEMIBOLD,
            window,
        );
        let rows: Vec<_> = self
            .series
            .iter()
            .map(|series| {
                let name = shape(
                    series.name.clone(),
                    self.inks.quiet,
                    FontWeight::NORMAL,
                    window,
                );
                let value = shape(
                    series
                        .shown
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| "—".into()),
                    self.inks.text,
                    FontWeight::MEDIUM,
                    window,
                );
                (series.color, name, value)
            })
            .collect();
        let (pad, line, swatch, between) = (8. * factor, 16. * factor, 8. * factor, 12. * factor);
        let names = rows
            .iter()
            .map(|(_, name, _)| name.width.as_f32())
            .fold(0., f32::max);
        let values = rows
            .iter()
            .map(|(_, _, value)| value.width.as_f32())
            .fold(0., f32::max);
        let inner = (swatch + 6. * factor + names + between + values).max(head.width.as_f32());
        let (wide, tall) = (inner + pad * 2., line * (rows.len() + 1) as f32 + pad * 2.);
        // Beside the line, on the side with the room; inside the plot.
        let away = 10. * factor;
        let mut from = if mid / width > 0.62 {
            mid - away - wide
        } else {
            mid + away
        };
        from = from.clamp(0., (width - wide).max(0.));
        let corner = point(px(left + from), px(top + (4. * factor).min(height)));
        window.paint_quad(
            fill(Bounds::new(corner, size(px(wide), px(tall))), self.inks.tip)
                .corner_radii(px(6. * factor))
                .border_widths(px(hair))
                .border_color(self.inks.rim),
        );
        let mut down = corner.y + px(pad);
        let start = corner.x + px(pad);
        head.paint(
            point(start, down),
            px(line),
            TextAlign::Left,
            None,
            window,
            cx,
        )
        .ok();
        for (color, name, value) in rows {
            down += px(line);
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(start, down + px((line - swatch) / 2.)),
                        size(px(swatch), px(swatch)),
                    ),
                    color,
                )
                .corner_radii(px(2. * factor)),
            );
            name.paint(
                point(start + px(swatch + 6. * factor), down),
                px(line),
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
            let right = corner.x + px(wide - pad) - value.width;
            value
                .paint(
                    point(right, down),
                    px(line),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        }
    }
}

/// A point's mark: a disc, with a ring of the ground's colour around it
/// where one is given, so that it reads where it crosses a line or another.
fn dot(at: Point<Pixels>, radius: f32, color: Hsla, ring: Option<Hsla>, window: &mut Window) {
    let disc = |radius: f32, color: Hsla, window: &mut Window| {
        window.paint_quad(
            fill(
                Bounds::new(
                    at - point(px(radius), px(radius)),
                    size(px(radius * 2.), px(radius * 2.)),
                ),
                color,
            )
            .corner_radii(px(radius)),
        );
    };
    if let Some(ring) = ring {
        disc(radius + 2. * scale::factor(), ring, window);
    }
    disc(radius, color, window);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_stand_from_edge_to_edge_and_bars_in_the_middle_of_their_shares() {
        assert_eq!(across(Kind::Line, 0, 5, 400.), 0.);
        assert_eq!(across(Kind::Line, 4, 5, 400.), 400.);
        assert_eq!(across(Kind::Points, 0, 1, 400.), 200.);
        assert_eq!(across(Kind::Bar, 0, 4, 400.), 50.);
        assert_eq!(across(Kind::Bar, 3, 4, 400.), 350.);
        // The pointer finds the point it is nearest, and the bar it is over.
        assert_eq!(nearest(Kind::Line, 149., 5, 400.), 1);
        assert_eq!(nearest(Kind::Line, 151., 5, 400.), 2);
        assert_eq!(nearest(Kind::Line, 900., 5, 400.), 4);
        assert_eq!(nearest(Kind::Bar, 99., 4, 400.), 0);
        assert_eq!(nearest(Kind::Bar, 101., 4, 400.), 1);
        assert_eq!(nearest(Kind::Bar, 400., 4, 400.), 3);
        assert_eq!(nearest(Kind::Bar, 10., 0, 400.), 0);
    }

    #[test]
    fn a_bar_is_never_thicker_than_24_and_keeps_ground_between_it_and_the_next() {
        // One series with all the room in the world.
        assert_eq!(bar(400., 1), (24., 2.));
        // Three in a narrow share: thinner, the gaps kept.
        let (thick, gap) = bar(40., 3);
        assert_eq!(gap, 2.);
        assert!(thick * 3. + gap * 2. <= 40. * 0.72 + 0.01, "{thick}");
        // Too narrow for gaps: the bars are there all the same.
        let (thick, gap) = bar(8., 4);
        assert_eq!(gap, 0.);
        assert!(thick >= 1.);
    }
}
