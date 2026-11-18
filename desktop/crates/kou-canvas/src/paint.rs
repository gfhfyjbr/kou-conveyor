//! What the canvas paints rather than lays out: the dots of the grid, the
//! edges (each mode its own line, the selected one in the accent, the one
//! being drawn dashed), their arrowheads, and the messages travelling along
//! them. Lines keep their width at any zoom, as the web canvas's do.

use crate::{
    anim::mix,
    geometry::{Cubic, Lod, curve},
    model::{Dir, EdgeMode, GRID, Node, port_at},
    view::{CanvasView, Gesture},
};
use gpui::{
    Bounds, Context, DispatchPhase, Hsla, IntoElement, MouseMoveEvent, MouseUpEvent, PathBuilder,
    Pixels, Point, Styled as _, Window, canvas, fill, point, px, size,
};
use kou_ui::{Theme, motion};

/// An edge as it is painted, in canvas pixels.
pub(crate) struct Line {
    pub curve: Cubic,
    pub color: Hsla,
    pub width: f32,
    pub dash: Option<(f32, f32)>,
    pub arrow: bool,
}

/// A message on its way along an edge.
pub(crate) struct Dot {
    pub at: (f32, f32),
    pub color: Hsla,
    pub ring: Hsla,
}

/// The edges and messages the backdrop paints, gathered while the view renders.
pub(crate) struct Scene {
    pub lines: Vec<Line>,
    pub dots: Vec<Dot>,
}

/// The closest the grid's dots come, in pixels. Every dot is drawn on every
/// frame the camera moves: at this spacing a 2560 by 1440 board holds at most
/// about 6,400 of them (it was 25,600 at 12 px).
pub(crate) const GRID_MIN_STEP: f32 = 24.;

/// The dots of the board's grid, as a view of their own: GPUI draws them
/// again from the last frame unless the camera or the theme moved them, so
/// what animates over the board does not repaint thousands of dots a frame.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Grid {
    pub step: f32,
    pub origin: (f32, f32),
    pub dot: Hsla,
}

impl Grid {
    /// The grid at a camera: dots four snaps apart, twice as far, and twice
    /// again, while they would be closer than [`GRID_MIN_STEP`] pixels.
    pub(crate) fn at(camera: &crate::geometry::Camera, theme: &Theme) -> Self {
        let mut step = GRID * camera.zoom;
        while step < GRID_MIN_STEP {
            step *= 2.;
        }
        Self {
            step,
            origin: (camera.x, camera.y),
            dot: theme.fg.opacity(0.13),
        }
    }
}

impl gpui::Render for Grid {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let grid = *self;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| paint_grid(&grid, bounds, window),
        )
        .size_full()
    }
}

/// The dash an edge of a mode is drawn with, in pixels: the web canvas's.
pub(crate) fn dash_of(mode: EdgeMode) -> Option<(f32, f32)> {
    match mode {
        EdgeMode::Auto => None,
        EdgeMode::Approve => Some((7., 5.)),
        EdgeMode::Off => Some((2., 6.)),
    }
}

/// The colour an edge of a mode is drawn in, at rest.
pub(crate) fn color_of(mode: EdgeMode, theme: &Theme) -> Hsla {
    match mode {
        EdgeMode::Auto => theme.fg_faint,
        EdgeMode::Approve => mix(theme.fg_faint, theme.blocked, 0.7),
        EdgeMode::Off => theme.fg_faint.opacity(0.55),
    }
}

impl CanvasView {
    /// A port's place, in world units.
    pub(crate) fn drawn_port(&self, node: &Node, dir: Dir, port: &str) -> (f32, f32) {
        port_at(node.kind, &node.rect, dir, port)
    }

    /// What the backdrop paints this frame.
    pub(crate) fn scene(&self, theme: &Theme) -> Scene {
        let camera = self.camera;
        let clock = &self.clock;
        let screen = |p: (f32, f32)| camera.to_screen(p.0, p.1);
        let retargeting = match &self.gesture {
            Some(Gesture::Wire(wiring)) => wiring.retarget.map(|(edge, _)| edge),
            _ => None,
        };
        let mut lines = Vec::new();
        for edge in self.board.edges() {
            if Some(edge.id) == retargeting {
                continue;
            }
            let (Some(from), Some(to)) = (self.board.node(edge.from), self.board.node(edge.to))
            else {
                continue;
            };
            let whole = curve(
                self.drawn_port(from, Dir::Out, &edge.from_port),
                self.drawn_port(to, Dir::In, &edge.to_port),
            )
            .map(screen);
            let fx = self.edge_fx.get(&edge.id);
            let waiting = self.traffic.get(&edge.id).is_some_and(|t| t.waiting > 0);
            let mut color = if waiting {
                theme.blocked
            } else {
                color_of(edge.mode, theme)
            };
            let touches = self.selection.contains(&edge.from) || self.selection.contains(&edge.to);
            if touches {
                color = mix(color, theme.fg_dim, 0.6);
            }
            let (hover, select) = fx.map_or((0., 0.), |fx| {
                (fx.hover.value(clock), fx.select.value(clock))
            });
            color = mix(color, theme.fg_dim, hover * 0.8);
            color = mix(color, theme.accent, select);
            lines.push(Line {
                curve: whole,
                color,
                width: 1.5 + select,
                dash: dash_of(edge.mode),
                arrow: true,
            });
        }
        if let Some(Gesture::Wire(wiring)) = &self.gesture
            && let Some(fixed) = self.board.node(wiring.fixed.node)
        {
            let start = self.drawn_port(fixed, wiring.dir, &wiring.fixed.port);
            let end = wiring
                .target
                .as_ref()
                .and_then(|port| {
                    let node = self.board.node(port.node)?;
                    Some(self.drawn_port(node, wiring.wanted(), &port.port))
                })
                .unwrap_or(wiring.at);
            let ghost = match wiring.dir {
                Dir::Out => curve(start, end),
                Dir::In => curve(end, start),
            };
            lines.push(Line {
                curve: ghost.map(screen),
                color: theme.accent,
                width: 2.,
                dash: Some((6., 4.)),
                arrow: wiring.target.is_some(),
            });
        }
        let mut dots = Vec::new();
        for flow in &self.flows {
            let Some(edge) = self.board.edge(flow.edge) else {
                continue;
            };
            let (Some(from), Some(to)) = (self.board.node(edge.from), self.board.node(edge.to))
            else {
                continue;
            };
            let path = curve(
                self.drawn_port(from, Dir::Out, &edge.from_port),
                self.drawn_port(to, Dir::In, &edge.to_port),
            )
            .map(screen);
            let t = motion::ease_out(clock.progress(flow.start, flow.duration));
            dots.push(Dot {
                at: path.at_length(path.length() * t),
                color: theme.accent,
                ring: theme.rail,
            });
        }
        Scene { lines, dots }
    }

    /// The board's backdrop: grid, edges and messages, under the nodes. It
    /// also measures the canvas, and while a drag is on follows the pointer
    /// out of the canvas, so a drag that leaves the window still ends.
    pub(crate) fn backdrop(&self, scene: Scene, cx: &Context<Self>) -> impl IntoElement + use<> {
        let area = self.area.clone();
        let view = cx.entity().downgrade();
        let dragging = self.gesture.is_some();
        canvas(
            move |bounds, _, _| area.set(bounds),
            move |bounds, _, window, _| {
                if dragging {
                    let moved = view.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble && !bounds.contains(&event.position) {
                            moved
                                .update(cx, |this, cx| {
                                    if event.pressed_button.is_some() {
                                        this.drag_to(event.position, cx);
                                    } else {
                                        this.end_gesture(event.position, window, cx);
                                    }
                                })
                                .ok();
                        }
                    });
                    let released = view.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble && !bounds.contains(&event.position) {
                            released
                                .update(cx, |this, cx| this.end_gesture(event.position, window, cx))
                                .ok();
                        }
                    });
                }
                paint_scene(&scene, bounds, window);
            },
        )
        .absolute()
        .size_full()
    }
}

fn paint_grid(grid: &Grid, bounds: Bounds<Pixels>, window: &mut Window) {
    if grid.step <= 0. {
        return;
    }
    // One path per row, every row the same shape and as many dots long
    // whatever the pan: GPUI keeps a path's triangles by its shape, so a
    // frame tessellates one row at most, none while the zoom holds, and
    // the scene gets a primitive per row rather than one per dot.
    let origin = bounds.origin;
    let (width, height): (f32, f32) = (bounds.size.width.into(), bounds.size.height.into());
    let step = grid.step;
    let columns = (width / step).ceil() as usize + 2;
    let x0 = grid.origin.0.rem_euclid(step) - step;
    let top = grid.origin.1.rem_euclid(step);
    // The first row is built; the others are the same triangles moved down.
    let mut row = PathBuilder::fill();
    for column in 0..columns {
        let x = x0 + column as f32 * step;
        let at = |dx: f32, dy: f32| origin + point(px(x + dx), px(top + dy));
        row.move_to(at(-1., -1.));
        row.line_to(at(1., -1.));
        row.line_to(at(1., 1.));
        row.line_to(at(-1., 1.));
        row.close();
    }
    let Ok(row) = row.build() else {
        return;
    };
    let mut y = top;
    while y < height {
        let mut moved = row.clone();
        let down = px(y - top);
        moved.bounds.origin.y += down;
        for vertex in &mut moved.vertices {
            vertex.xy_position.y += down;
        }
        window.paint_path(moved, grid.dot);
        y += step;
    }
}

fn paint_scene(scene: &Scene, bounds: Bounds<Pixels>, window: &mut Window) {
    let origin = bounds.origin;
    let at = |p: (f32, f32)| origin + point(px(p.0), px(p.1));
    for line in &scene.lines {
        paint_line(line, &at, window);
    }
    for dot in &scene.dots {
        let r = 4.;
        window.paint_quad(
            fill(
                Bounds::new(
                    at((dot.at.0 - r, dot.at.1 - r)),
                    size(px(r * 2.), px(r * 2.)),
                ),
                dot.color,
            )
            .corner_radii(px(r))
            .border_widths(px(1.5))
            .border_color(dot.ring),
        );
    }
}

fn paint_line(line: &Line, at: &impl Fn((f32, f32)) -> Point<Pixels>, window: &mut Window) {
    let c = &line.curve;
    let mut path = PathBuilder::stroke(px(line.width));
    if let Some((on, off)) = line.dash {
        path = path.dash_array(&[px(on), px(off)]);
    }
    path.move_to(at(c.p0));
    path.cubic_bezier_to(at(c.p3), at(c.p1), at(c.p2));
    if let Ok(built) = path.build() {
        window.paint_path(built, line.color);
    }
    if line.arrow {
        // Along the curve's last stretch: edges come into a node from its left.
        let (mut dx, mut dy) = (c.p3.0 - c.p2.0, c.p3.1 - c.p2.1);
        let length = dx.hypot(dy);
        if length < 0.01 {
            (dx, dy) = (1., 0.);
        } else {
            (dx, dy) = (dx / length, dy / length);
        }
        let (long, half) = (8., 3.5);
        let (x, y) = c.p3;
        let base = (x - dx * long, y - dy * long);
        let mut head = PathBuilder::fill();
        head.move_to(at((x, y)));
        head.line_to(at((base.0 - dy * half, base.1 + dx * half)));
        head.line_to(at((base.0 + dy * half, base.1 - dx * half)));
        head.close();
        if let Ok(built) = head.build() {
            window.paint_path(built, line.color);
        }
    }
}

/// Whether labels show at a zoom: not when nodes are placeholders.
pub(crate) fn labels_show(zoom: f32) -> bool {
    Lod::of(zoom) == Lod::Whole
}
