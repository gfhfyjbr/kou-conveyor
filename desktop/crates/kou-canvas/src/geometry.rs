//! The camera that maps the board to the window, the curve an edge follows
//! and how close a point is to it, and how much of a node shows at a zoom.

use crate::model::{Board, Dir, Edge, Rect};
use serde::{Deserialize, Serialize};

/// A point, in whatever units its caller works in.
pub type Point2 = (f32, f32);

/// Where the board sits in the window: `screen = world * zoom + (x, y)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// Horizontal position of the world origin, in pixels from the canvas's left edge.
    pub x: f32,
    /// Vertical position of the world origin, in pixels from the canvas's top edge.
    pub y: f32,
    /// Pixels per world unit.
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            x: 0.,
            y: 0.,
            zoom: 1.,
        }
    }
}

/// The zooms the zoom keys step through, as on the web canvas.
pub const ZOOM_STEPS: [f32; 11] = [0.1, 0.15, 0.25, 0.35, 0.5, 0.6, 0.75, 1., 1.25, 1.5, 2.];

impl Camera {
    /// The furthest out: the web canvas's.
    pub const MIN_ZOOM: f32 = 0.1;
    /// The furthest in: past this, text is only bigger, not clearer.
    pub const MAX_ZOOM: f32 = 2.;

    /// A world point in canvas pixels.
    pub fn to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.zoom + self.x, y * self.zoom + self.y)
    }

    /// A canvas pixel in world units.
    pub fn to_world(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.x) / self.zoom, (y - self.y) / self.zoom)
    }

    /// A world rectangle in canvas pixels.
    pub fn rect(&self, rect: &Rect) -> Rect {
        let (x, y) = self.to_screen(rect.x, rect.y);
        Rect::new(x, y, rect.w * self.zoom, rect.h * self.zoom)
    }

    /// The world a viewport shows.
    pub fn visible(&self, viewport: (f32, f32)) -> Rect {
        let (x, y) = self.to_world(0., 0.);
        Rect::new(x, y, viewport.0 / self.zoom, viewport.1 / self.zoom)
    }

    /// Slide the board by a pixel distance.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
    }

    /// Scale by `factor`, keeping the world point under a canvas pixel where it is.
    pub fn zoom_at(&mut self, x: f32, y: f32, factor: f32) {
        self.zoom_to(x, y, self.zoom * factor);
    }

    /// Set the zoom, keeping the world point under a canvas pixel where it is.
    pub fn zoom_to(&mut self, x: f32, y: f32, zoom: f32) {
        let zoom = zoom.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        let (wx, wy) = self.to_world(x, y);
        self.zoom = zoom;
        self.x = x - wx * zoom;
        self.y = y - wy * zoom;
    }

    /// The next zoom step in (`1`) or out (`-1`) from this zoom.
    pub fn step(zoom: f32, direction: i32) -> f32 {
        if direction > 0 {
            ZOOM_STEPS
                .iter()
                .copied()
                .find(|step| *step > zoom + 0.001)
                .unwrap_or(Self::MAX_ZOOM)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < zoom - 0.001)
                .unwrap_or(Self::MIN_ZOOM)
        }
    }

    /// The camera that shows all of `bounds` in a viewport, centred, no
    /// larger than `max` (1: never magnified).
    pub fn fit_at_most(bounds: &Rect, viewport: (f32, f32), padding: f32, max: f32) -> Self {
        let (width, height) = (
            (viewport.0 - padding * 2.).max(1.),
            (viewport.1 - padding * 2.).max(1.),
        );
        let zoom = (width / bounds.w.max(1.))
            .min(height / bounds.h.max(1.))
            .min(max)
            .clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        let (cx, cy) = bounds.center();
        Self {
            x: viewport.0 / 2. - cx * zoom,
            y: viewport.1 / 2. - cy * zoom,
            zoom,
        }
    }

    /// The camera that shows all of `bounds` in a viewport, centred, never magnified.
    pub fn fit(bounds: &Rect, viewport: (f32, f32), padding: f32) -> Self {
        Self::fit_at_most(bounds, viewport, padding, 1.)
    }

    /// A rectangle at a zoom, centred in the viewport; one taller than the
    /// viewport shows from its top.
    pub fn centre_on(rect: &Rect, viewport: (f32, f32), zoom: f32) -> Self {
        let zoom = zoom.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        let x = (viewport.0 - rect.w * zoom) / 2. - rect.x * zoom;
        let y = if rect.h * zoom > viewport.1 - 16. {
            8. - rect.y * zoom
        } else {
            (viewport.1 - rect.h * zoom) / 2. - rect.y * zoom
        };
        Self { x, y, zoom }
    }

    /// Whether a world rectangle is wholly inside the viewport, with a margin.
    pub fn shows(&self, rect: &Rect, viewport: (f32, f32), margin: f32) -> bool {
        let on_screen = self.rect(rect);
        on_screen.x >= margin
            && on_screen.y >= margin
            && on_screen.right() <= viewport.0 - margin
            && on_screen.bottom() <= viewport.1 - margin
    }

    /// The camera moved, at its zoom, just far enough that a world rectangle
    /// is inside the viewport with a margin. One larger than the viewport
    /// shows from its top left corner.
    pub fn bring_into_view(&self, rect: &Rect, viewport: (f32, f32), margin: f32) -> Camera {
        let on_screen = self.rect(rect);
        let slide = |start: f32, length: f32, room: f32| {
            if length > room - margin * 2. || start < margin {
                margin - start
            } else if start + length > room - margin {
                room - margin - (start + length)
            } else {
                0.
            }
        };
        let mut camera = *self;
        camera.pan(
            slide(on_screen.x, on_screen.w, viewport.0),
            slide(on_screen.y, on_screen.h, viewport.1),
        );
        camera
    }

    /// The camera a fraction of the way from this one to another, linearly.
    pub fn toward(&self, other: &Camera, t: f32) -> Camera {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Camera {
            x: mix(self.x, other.x),
            y: mix(self.y, other.y),
            zoom: mix(self.zoom, other.zoom),
        }
    }

    /// The camera a fraction of the way to another as the eye travels: the
    /// zoom changes by the same ratio each instant, and the world point at the
    /// middle of the viewport moves in a straight line, so nothing swings out
    /// of view on the way.
    pub fn glide(&self, other: &Camera, t: f32, viewport: (f32, f32)) -> Camera {
        if t <= 0. {
            return *self;
        }
        if t >= 1. {
            return *other;
        }
        let zoom = (self.zoom.ln() + (other.zoom.ln() - self.zoom.ln()) * t).exp();
        let (mx, my) = (viewport.0 / 2., viewport.1 / 2.);
        let a = self.to_world(mx, my);
        let b = other.to_world(mx, my);
        let (wx, wy) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        Camera {
            x: mx - wx * zoom,
            y: my - wy * zoom,
            zoom,
        }
    }
}

/// How much of a node the canvas draws at a zoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lod {
    /// Zoomed out past where a terminal's text can be read: every node, of
    /// every kind, is a placeholder showing its name, its mark, its hue and
    /// its state. No body, no ports, no footer, no labels on edges.
    Placeholder,
    /// The node whole: its header, its live body, its ports, its footer, and
    /// the labels of edges.
    Whole,
}

impl Lod {
    /// How tall a terminal's text is at 100%, in pixels: the app's terminal font.
    pub const TERMINAL_TEXT: f32 = 13.;
    /// The zoom from which nodes are whole. A terminal's 13 px text is 7.8 px
    /// here, about the smallest monospace whose lower case still reads as
    /// letters on a display without Retina scaling; further out its lines
    /// are a texture, and a node says more by its name. It is one of the
    /// zoom steps, so stepping out from 100% keeps nodes whole at 75% and
    /// 60%, and the next step, 50%, shows placeholders.
    pub const LIVE: f32 = 0.6;

    /// The level at a zoom.
    pub fn of(zoom: f32) -> Lod {
        if zoom < Self::LIVE {
            Lod::Placeholder
        } else {
            Lod::Whole
        }
    }
}

/// A cubic Bézier curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cubic {
    /// Where it starts.
    pub p0: Point2,
    /// The first control point.
    pub p1: Point2,
    /// The second control point.
    pub p2: Point2,
    /// Where it ends.
    pub p3: Point2,
}

/// The curve of an edge from an output at `a` to an input at `b`, as the web
/// canvas draws it: out to the right, in from the left, however the nodes
/// stand, the bend growing with the distance.
pub fn curve(a: Point2, b: Point2) -> Cubic {
    let d = 48f32
        .max((b.0 - a.0).abs() / 2.)
        .max(((b.1 - a.1).abs() / 2.).min(160.));
    Cubic {
        p0: a,
        p1: (a.0 + d, a.1),
        p2: (b.0 - d, b.1),
        p3: b,
    }
}

/// The world curve of an edge as its nodes stand now.
pub fn edge_curve(board: &Board, edge: &Edge) -> Option<Cubic> {
    let from = board.node(edge.from)?;
    let to = board.node(edge.to)?;
    Some(curve(
        from.port_point(Dir::Out, &edge.from_port),
        to.port_point(Dir::In, &edge.to_port),
    ))
}

/// How many straight pieces a curve is measured and hit-tested by.
const PIECES: usize = 32;

impl Cubic {
    /// The point at `t` (0..=1).
    pub fn point(&self, t: f32) -> Point2 {
        let u = 1. - t;
        let (a, b, c, d) = (u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t);
        (
            a * self.p0.0 + b * self.p1.0 + c * self.p2.0 + d * self.p3.0,
            a * self.p0.1 + b * self.p1.1 + c * self.p2.1 + d * self.p3.1,
        )
    }

    /// The direction it heads in at `t`, not normalised.
    pub fn tangent(&self, t: f32) -> Point2 {
        let u = 1. - t;
        let (a, b, c) = (3. * u * u, 6. * u * t, 3. * t * t);
        (
            a * (self.p1.0 - self.p0.0) + b * (self.p2.0 - self.p1.0) + c * (self.p3.0 - self.p2.0),
            a * (self.p1.1 - self.p0.1) + b * (self.p2.1 - self.p1.1) + c * (self.p3.1 - self.p2.1),
        )
    }

    /// The same curve with every point mapped: into the window, say.
    pub fn map(&self, f: impl Fn(Point2) -> Point2) -> Cubic {
        Cubic {
            p0: f(self.p0),
            p1: f(self.p1),
            p2: f(self.p2),
            p3: f(self.p3),
        }
    }

    /// Points along it, `pieces + 1` of them, evenly in `t`.
    pub fn flatten(&self, pieces: usize) -> Vec<Point2> {
        (0..=pieces)
            .map(|i| self.point(i as f32 / pieces as f32))
            .collect()
    }

    /// Its length, measured along straight pieces.
    pub fn length(&self) -> f32 {
        self.flatten(PIECES)
            .windows(2)
            .map(|pair| (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1))
            .sum()
    }

    /// The point a distance along it from its start: where a message is, or
    /// where its label sits.
    pub fn at_length(&self, distance: f32) -> Point2 {
        let points = self.flatten(PIECES);
        let mut left = distance.max(0.);
        for pair in points.windows(2) {
            let piece = (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1);
            if left <= piece && piece > 0. {
                let t = left / piece;
                return (
                    pair[0].0 + (pair[1].0 - pair[0].0) * t,
                    pair[0].1 + (pair[1].1 - pair[0].1) * t,
                );
            }
            left -= piece;
        }
        self.p3
    }

    /// The point halfway along it.
    pub fn middle(&self) -> Point2 {
        self.at_length(self.length() / 2.)
    }

    /// How far a point is from the curve.
    pub fn distance(&self, p: Point2) -> f32 {
        self.flatten(PIECES)
            .windows(2)
            .map(|pair| segment_distance(p, pair[0], pair[1]))
            .fold(f32::INFINITY, f32::min)
    }

    /// The box around its control points, which holds the curve.
    pub fn bounds(&self) -> Rect {
        let xs = [self.p0.0, self.p1.0, self.p2.0, self.p3.0];
        let ys = [self.p0.1, self.p1.1, self.p2.1, self.p3.1];
        let (x0, x1) = (
            xs.iter().copied().fold(f32::INFINITY, f32::min),
            xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        );
        let (y0, y1) = (
            ys.iter().copied().fold(f32::INFINITY, f32::min),
            ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        );
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

fn segment_distance(p: Point2, a: Point2, b: Point2) -> f32 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let length = vx * vx + vy * vy;
    let t = if length > 0. {
        (((p.0 - a.0) * vx + (p.1 - a.1) * vy) / length).clamp(0., 1.)
    } else {
        0.
    };
    let (x, y) = (a.0 + vx * t, a.1 + vy * t);
    (p.0 - x).hypot(p.1 - y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeSpec, PortRef};

    #[test]
    fn zooming_keeps_the_point_under_the_pointer_and_stops_at_the_limits() {
        let mut camera = Camera {
            x: 40.,
            y: -10.,
            zoom: 1.,
        };
        let before = camera.to_world(300., 200.);
        camera.zoom_at(300., 200., 1.5);
        let after = camera.to_world(300., 200.);
        assert!((before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3);
        assert_eq!(camera.zoom, 1.5);
        camera.zoom_at(0., 0., 100.);
        assert_eq!(camera.zoom, Camera::MAX_ZOOM);
        camera.zoom_at(0., 0., 0.0001);
        assert_eq!(camera.zoom, Camera::MIN_ZOOM);
    }

    #[test]
    fn zoom_steps_go_through_the_web_canvas_stops() {
        assert_eq!(Camera::step(1., 1), 1.25);
        assert_eq!(Camera::step(1., -1), 0.75);
        assert_eq!(Camera::step(0.8, -1), 0.75);
        assert_eq!(Camera::step(2., 1), Camera::MAX_ZOOM);
        assert_eq!(Camera::step(0.1, -1), Camera::MIN_ZOOM);
    }

    #[test]
    fn fit_centres_the_board_and_never_magnifies() {
        let bounds = Rect::new(100., 100., 2000., 500.);
        let camera = Camera::fit(&bounds, (1000., 800.), 50.);
        assert!((camera.zoom - 0.45).abs() < 1e-4);
        assert!(camera.shows(&bounds, (1000., 800.), 49.));
        let (cx, cy) = camera.to_screen(1100., 350.);
        assert!((cx - 500.).abs() < 1e-3 && (cy - 400.).abs() < 1e-3);

        let small = Rect::new(0., 0., 100., 100.);
        assert_eq!(Camera::fit(&small, (1000., 800.), 50.).zoom, 1.);
        let tall = Rect::new(0., 0., 400., 2000.);
        let at = Camera::centre_on(&tall, (1000., 800.), 1.);
        assert_eq!(at.to_screen(0., 0.).1, 8., "a tall node shows from its top");
    }

    #[test]
    fn bringing_a_rectangle_into_view_slides_no_further_than_it_must() {
        let camera = Camera {
            x: 0.,
            y: 0.,
            zoom: 0.5,
        };
        let viewport = (1000., 800.);
        // Off to the right and below: slid left and up until its far edges show.
        let rect = Rect::new(2400., 1600., 400., 200.);
        let moved = camera.bring_into_view(&rect, viewport, 24.);
        assert_eq!(moved.zoom, 0.5, "the zoom stays");
        assert!(moved.shows(&rect, viewport, 23.9));
        let on_screen = moved.rect(&rect);
        assert!((on_screen.right() - (1000. - 24.)).abs() < 1e-3);
        assert!((on_screen.bottom() - (800. - 24.)).abs() < 1e-3);
        // Already in view: nothing moves.
        let seen = Rect::new(100., 100., 200., 200.);
        assert_eq!(camera.bring_into_view(&seen, viewport, 24.), camera);
        // Larger than the viewport: its top left corner shows.
        let large = Rect::new(-5000., 0., 9000., 400.);
        let moved = camera.bring_into_view(&large, viewport, 24.);
        assert_eq!(moved.to_screen(-5000., 0.).0, 24.);
    }

    #[test]
    fn a_glide_scales_evenly_and_keeps_the_middle_on_a_line() {
        let from = Camera {
            x: 0.,
            y: 0.,
            zoom: 0.25,
        };
        let to = Camera {
            x: -400.,
            y: -100.,
            zoom: 1.,
        };
        let viewport = (1000., 800.);
        let half = from.glide(&to, 0.5, viewport);
        assert!(
            (half.zoom - 0.5).abs() < 1e-4,
            "halfway in ratio, not in difference"
        );
        assert_eq!(from.glide(&to, 0., viewport), from);
        assert_eq!(from.glide(&to, 1., viewport), to);
        let a = from.to_world(500., 400.);
        let b = to.to_world(500., 400.);
        let m = half.to_world(500., 400.);
        assert!((m.0 - (a.0 + b.0) / 2.).abs() < 1e-2 && (m.1 - (a.1 + b.1) / 2.).abs() < 1e-2);
    }

    #[test]
    fn nodes_are_placeholders_where_a_terminals_text_stops_being_legible() {
        assert_eq!(Lod::of(0.1), Lod::Placeholder);
        assert_eq!(Lod::of(0.5), Lod::Placeholder);
        assert_eq!(Lod::of(0.599), Lod::Placeholder);
        assert_eq!(Lod::of(0.6), Lod::Whole);
        assert_eq!(Lod::of(2.), Lod::Whole);
        // 13 px terminal text is 7.8 px at the threshold.
        assert!((Lod::TERMINAL_TEXT * Lod::LIVE - 7.8).abs() < 1e-4);
        // A zoom step, so the zoom keys land on it.
        assert!(ZOOM_STEPS.contains(&Lod::LIVE));
        assert!(Camera::step(1., -1) >= Lod::LIVE && Camera::step(0.6, -1) < Lod::LIVE);
    }

    #[test]
    fn an_edge_leaves_rightwards_and_arrives_from_the_left_however_the_nodes_stand() {
        let forward = curve((0., 0.), (300., 100.));
        assert!(forward.p1.0 > forward.p0.0 && forward.p2.0 < forward.p3.0);
        assert_eq!(forward.p1.1, forward.p0.1);
        assert_eq!(forward.p2.1, forward.p3.1);
        // Backwards, the bend is at least as wide as the gap: it loops round.
        let back = curve((300., 0.), (0., 0.));
        assert_eq!(back.p1.0, 450.);
        assert_eq!(back.p2.0, -150.);
        // Straight up, the bend grows with the height, but not without end.
        let tall = curve((0., 0.), (0., 1000.));
        assert_eq!(tall.p1.0, 160.);
        assert_eq!(curve((0., 0.), (10., 0.)).p1.0, 48.);
    }

    #[test]
    fn a_point_is_near_the_curve_only_along_it() {
        let c = curve((0., 0.), (400., 200.));
        // Points on the curve are at no distance.
        for t in [0., 0.25, 0.5, 0.9, 1.] {
            assert!(c.distance(c.point(t)) < 0.5, "t = {t}");
        }
        // The middle of the box the ends span is on an S curve; a corner is not.
        assert!(c.distance((200., 100.)) < 1.);
        assert!(c.distance((400., 0.)) > 60.);
        assert!(c.distance((0., 200.)) > 60.);
        // Six pixels off the middle, sideways to the curve.
        let (mx, my) = c.point(0.5);
        let (tx, ty) = c.tangent(0.5);
        let n = (tx * tx + ty * ty).sqrt();
        let off = (mx - ty / n * 6., my + tx / n * 6.);
        assert!((c.distance(off) - 6.).abs() < 0.5);
    }

    #[test]
    fn lengths_and_halfway_points_are_measured_along_the_curve() {
        let line = Cubic {
            p0: (0., 0.),
            p1: (100., 0.),
            p2: (200., 0.),
            p3: (300., 0.),
        };
        assert!((line.length() - 300.).abs() < 1e-2);
        assert!((line.at_length(150.).0 - 150.).abs() < 1e-2);
        assert_eq!(line.at_length(9999.), (300., 0.));
        assert_eq!(line.middle(), line.at_length(150.));
        let c = curve((0., 0.), (300., 200.));
        let b = c.bounds();
        assert!(b.contains(150., 100.));
    }

    #[test]
    fn an_edges_curve_runs_between_its_ports() {
        let mut board = Board::new();
        let a = board.add_rect(NodeSpec::default(), Rect::new(0., 0., 400., 300.));
        let b = board.add_rect(NodeSpec::default(), Rect::new(800., 200., 400., 300.));
        board
            .connect(PortRef::new(a, "out"), PortRef::new(b, "in"))
            .unwrap();
        let c = edge_curve(&board, &board.edges()[0]).unwrap();
        assert_eq!(c.p0, board.node(a).unwrap().port_point(Dir::Out, "out"));
        assert_eq!(c.p3, (800., 254.));
    }
}
