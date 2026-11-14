//! What the pointer does on the canvas, as the web canvas has it: a drag on
//! the board draws a marquee (Space, the middle button or the wheel pan); a
//! node is dragged by its header, resized by its corner, wired from its
//! ports; an edge is picked along its curve and its ends dragged to other
//! ports; a double-click on the board, or the right button, asks what to add.

use crate::{
    geometry::{Camera, edge_curve},
    model::{Dir, End, NodeId, PortRef, Rect, SNAP, Wire, snap_to},
    view::{CanvasView, Edit, Gesture, WheelGlide, Wiring},
};
use gpui::{
    Context, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PinchEvent, Pixels, Point, ScrollEvent,
    SmoothScroll, Window, px,
};

/// How near the curve, in pixels, a press picks an edge: half the web's hit stroke.
pub(crate) const EDGE_REACH: f32 = 7.;
/// How near, in pixels, a wire docks in a port.
pub(crate) const DOCK: f32 = 20.;
/// How near, in pixels, a port grows to say a wire would go there.
pub(crate) const CLOSE: f32 = 64.;
/// How near a node's side, in pixels, the pointer shows its ports.
pub(crate) const PORTS_NEAR: f32 = 24.;

impl CanvasView {
    /// The edge under a canvas pixel, the one drawn on top first.
    pub(crate) fn edge_at(&self, local: (f32, f32)) -> Option<u64> {
        let camera = self.camera;
        self.board
            .edges()
            .iter()
            .rev()
            .filter_map(|edge| {
                let curve = edge_curve(&self.board, edge)?.map(|p| camera.to_screen(p.0, p.1));
                if !curve
                    .bounds()
                    .inflate(EDGE_REACH)
                    .contains(local.0, local.1)
                {
                    return None;
                }
                let distance = curve.distance(local);
                (distance <= EDGE_REACH).then_some((edge.id, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    pub(crate) fn board_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.close_menu(cx);
        self.commit_edit(cx);
        let local = self.local(event.position);
        if self.space_held {
            self.gesture = Some(Gesture::Pan {
                last: event.position,
            });
            return;
        }
        if let Some(edge) = self.edge_at(local) {
            if event.click_count == 2 {
                self.open_inspector_for_edge(edge, cx);
            } else {
                let keep = if event.modifiers.shift {
                    self.selection.clone()
                } else {
                    Vec::new()
                };
                self.set_selection(keep, Some(edge), cx);
            }
            return;
        }
        if event.click_count == 2 {
            let world = self.camera.to_world(local.0, local.1);
            self.open_add_menu(local, world, None, window, cx);
            return;
        }
        let world = self.camera.to_world(local.0, local.1);
        let additive = event.modifiers.shift;
        self.gesture = Some(Gesture::Marquee {
            from: world,
            to: world,
            base: if additive {
                self.selection.clone()
            } else {
                Vec::new()
            },
            moved: false,
            additive,
        });
    }

    pub(crate) fn board_middle_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.gesture = Some(Gesture::Pan {
            last: event.position,
        });
    }

    pub(crate) fn board_context(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.commit_edit(cx);
        let local = self.local(event.position);
        if let Some(edge) = self.edge_at(local) {
            self.set_selection(Vec::new(), Some(edge), cx);
            self.open_edge_menu(edge, local, window, cx);
            return;
        }
        let world = self.camera.to_world(local.0, local.1);
        self.open_add_menu(local, world, None, window, cx);
    }

    pub(crate) fn node_context(
        &mut self,
        id: NodeId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        self.commit_edit(cx);
        if !self.selection.contains(&id) {
            self.set_selection(vec![id], None, cx);
        }
        let local = self.local(event.position);
        self.open_node_menu(id, local, window, cx);
    }

    /// A press on what a node is dragged by: its header, its footer, or a
    /// body that is a picture rather than a live view.
    pub(crate) fn node_grab(
        &mut self,
        id: NodeId,
        title: bool,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        self.close_menu(cx);
        if self
            .editing
            .as_ref()
            .is_some_and(|editing| editing.what != Edit::Title(id, false))
        {
            self.commit_edit(cx);
        }
        if self.space_held {
            self.gesture = Some(Gesture::Pan {
                last: event.position,
            });
            return;
        }
        if event.click_count == 2 {
            if title {
                self.start_edit(Edit::Title(id, false), window, cx);
            } else if self
                .board
                .node(id)
                .is_some_and(|node| node.kind.canvas_owned())
            {
                self.start_edit(Edit::Note(id), window, cx);
            } else if self.bodies.contains_key(&id) {
                self.enter(id, window, cx);
            } else {
                self.focus_node(id, cx);
            }
            return;
        }
        if event.modifiers.shift {
            let mut next = self.selection.clone();
            if let Some(at) = next.iter().position(|n| *n == id) {
                next.remove(at);
            } else {
                next.push(id);
            }
            self.set_selection(next, None, cx);
            if !self.selection.contains(&id) {
                return;
            }
        } else if !self.selection.contains(&id) || self.edge_selected.is_some() {
            self.set_selection(vec![id], None, cx);
        }
        let starts = self
            .selection
            .iter()
            .filter_map(|n| {
                self.board
                    .node(*n)
                    .map(|node| (*n, node.rect.x, node.rect.y))
            })
            .collect();
        self.gesture = Some(Gesture::Move {
            start: event.position,
            starts,
            moved: false,
            before: self.board.clone(),
            clicked: id,
            shift: event.modifiers.shift,
        });
    }

    /// A press in a live body: the node is selected and its body has the keys.
    pub(crate) fn body_down(&mut self, id: NodeId, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.close_menu(cx);
        self.commit_edit(cx);
        if self.selection != [id] {
            self.set_selection(vec![id], None, cx);
        }
        self.set_entered(Some(id), cx);
        if let Some(focus) = self.bodies.get(&id).and_then(|body| body.focus.clone()) {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(crate) fn resize_down(
        &mut self,
        id: NodeId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        if self.selection != [id] {
            self.set_selection(vec![id], None, cx);
        }
        let Some(rect) = self.board.node(id).map(|node| node.rect) else {
            return;
        };
        self.gesture = Some(Gesture::Resize {
            id,
            start: event.position,
            size: (rect.w, rect.h),
            before: self.board.clone(),
            moved: false,
        });
    }

    pub(crate) fn port_down(
        &mut self,
        port: PortRef,
        dir: Dir,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        self.close_menu(cx);
        let at = self.world_at(event.position);
        self.gesture = Some(Gesture::Wire(Wiring {
            fixed: port,
            dir,
            at,
            target: None,
            close: None,
            retarget: None,
            start: event.position,
        }));
        self.hover.near = None;
        cx.notify();
    }

    /// A press on an end of the selected edge: that end comes loose and
    /// follows the pointer to another port.
    pub(crate) fn edge_end_down(
        &mut self,
        edge: u64,
        end: End,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        let Some(e) = self.board.edge(edge) else {
            return;
        };
        let (fixed, dir) = match end {
            End::To => (e.from_ref(), Dir::Out),
            End::From => (e.to_ref(), Dir::In),
        };
        let at = self.world_at(event.position);
        self.gesture = Some(Gesture::Wire(Wiring {
            fixed,
            dir,
            at,
            target: None,
            close: None,
            retarget: Some((edge, end)),
            start: event.position,
        }));
        cx.notify();
    }

    pub(crate) fn pointer_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pointer = Some(self.local(event.position));
        if self.gesture.is_some() {
            if event.pressed_button.is_none() {
                self.end_gesture(event.position, window, cx);
            } else {
                self.drag_to(event.position, cx);
            }
            return;
        }
        self.track_hover(cx);
    }

    pub(crate) fn pointer_left(&mut self, cx: &mut Context<Self>) {
        if self.gesture.is_none() && self.pointer.is_some() {
            self.pointer = None;
            self.hover.edge = None;
            self.hover.near = None;
            cx.notify();
        }
    }

    // Which edge the pointer is over and which side of which node it is by.
    fn track_hover(&mut self, cx: &mut Context<Self>) {
        let Some(local) = self.pointer else {
            return;
        };
        let edge = self.edge_at(local);
        let zoom = self.camera.zoom;
        let (wx, wy) = self.camera.to_world(local.0, local.1);
        let reach = PORTS_NEAR / zoom;
        let near = self
            .board
            .nodes()
            .iter()
            .rev()
            .filter(|node| wy >= node.rect.y - reach && wy <= node.rect.bottom() + reach)
            .filter_map(|node| {
                let left = if node.kind.inputs().is_empty() {
                    f32::INFINITY
                } else {
                    (wx - node.rect.x).abs()
                };
                let right = if node.kind.outputs().is_empty() {
                    f32::INFINITY
                } else {
                    (wx - node.rect.right()).abs()
                };
                let (distance, dir) = if left <= right {
                    (left, Dir::In)
                } else {
                    (right, Dir::Out)
                };
                (distance <= reach).then_some((node.id, dir, distance))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(id, dir, _)| (id, dir));
        if edge != self.hover.edge || near != self.hover.near {
            self.hover.edge = edge;
            self.hover.near = near;
            cx.notify();
        }
    }

    pub(crate) fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let zoom = self.camera.zoom;
        let world = self.world_at(position);
        let mut select = None;
        match self.gesture.as_mut() {
            Some(Gesture::Pan { last }) => {
                let delta = position - *last;
                *last = position;
                self.glide = None;
                self.wheel = None;
                self.camera.pan(delta.x.into(), delta.y.into());
            }
            Some(Gesture::Marquee {
                from,
                to,
                base,
                moved,
                ..
            }) => {
                *to = world;
                let span = ((to.0 - from.0) * zoom).hypot((to.1 - from.1) * zoom);
                if !*moved && span < 4. {
                    return;
                }
                *moved = true;
                let area = Rect::spanning(*from, *to);
                let mut ids = base.clone();
                for id in self.board.nodes_in(&area) {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                select = Some(ids);
            }
            Some(Gesture::Move {
                start,
                starts,
                moved,
                ..
            }) => {
                let delta = position - *start;
                let (dx, dy): (f32, f32) = (delta.x.into(), delta.y.into());
                if !*moved && dx.hypot(dy) < 3. {
                    return;
                }
                if !*moved {
                    *moved = true;
                    let ids: Vec<NodeId> = starts.iter().map(|s| s.0).collect();
                    self.board.raise(&ids);
                }
                let places: Vec<(NodeId, f32, f32)> = starts
                    .iter()
                    .map(|(id, x, y)| {
                        (
                            *id,
                            snap_to(x + dx / zoom, SNAP),
                            snap_to(y + dy / zoom, SNAP),
                        )
                    })
                    .collect();
                self.board.place_all(&places);
            }
            Some(Gesture::Resize {
                id,
                start,
                size,
                moved,
                ..
            }) => {
                let delta = position - *start;
                let (dx, dy): (f32, f32) = (delta.x.into(), delta.y.into());
                let w = snap_to(size.0 + dx / zoom, SNAP);
                let h = snap_to(size.1 + dy / zoom, SNAP);
                if !*moved && (w, h) == *size {
                    return;
                }
                *moved = true;
                let id = *id;
                self.board.resize(id, w, h);
            }
            Some(Gesture::Wire(wiring)) => {
                wiring.at = world;
                let wanted = wiring.wanted();
                let fixed_node = wiring.fixed.node;
                let docked = self
                    .board
                    .nearest_port(world, wanted, DOCK / zoom, Some(fixed_node))
                    .map(|(port, _)| port);
                // Over a node, the wire takes its first port of the kind.
                let over = self
                    .board
                    .hit(world.0, world.1)
                    .filter(|id| *id != fixed_node)
                    .and_then(|id| {
                        let node = self.board.node(id)?;
                        node.kind
                            .ports(wanted)
                            .first()
                            .map(|port| PortRef::new(id, *port))
                    });
                wiring.target = docked.or(over);
                wiring.close = if wiring.target.is_none() {
                    self.board
                        .nearest_port(world, wanted, CLOSE / zoom, Some(fixed_node))
                        .map(|(port, _)| port)
                } else {
                    None
                };
            }
            None => return,
        }
        if let Some(ids) = select {
            self.set_selection(ids, None, cx);
        }
        cx.notify();
    }

    pub(crate) fn end_gesture(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(gesture) = self.gesture.take() else {
            return;
        };
        match gesture {
            Gesture::Pan { .. } => cx.emit(crate::CanvasEvent::Changed),
            Gesture::Marquee {
                moved, additive, ..
            } => {
                if !moved && !additive {
                    self.set_selection(Vec::new(), None, cx);
                }
            }
            Gesture::Move {
                moved,
                before,
                clicked,
                shift,
                ..
            } => {
                if moved {
                    self.remember_board(before);
                    cx.emit(crate::CanvasEvent::Changed);
                } else if !shift && self.selection != [clicked] {
                    self.set_selection(vec![clicked], None, cx);
                }
            }
            Gesture::Resize { moved, before, .. } => {
                if moved {
                    self.remember_board(before);
                    cx.emit(crate::CanvasEvent::Changed);
                }
            }
            Gesture::Wire(wiring) => self.finish_wire(wiring, position, window, cx),
        }
        cx.notify();
    }

    fn finish_wire(
        &mut self,
        wiring: Wiring,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let travelled = {
            let delta = position - wiring.start;
            let (dx, dy): (f32, f32) = (delta.x.into(), delta.y.into());
            dx.hypot(dy)
        };
        if let Some((edge, end)) = wiring.retarget {
            let Some(target) = wiring.target else {
                return;
            };
            let before = self.board.clone();
            if let Ok(was) = self.board.retarget(edge, end, target) {
                self.remember_board(before);
                if let Some(now) = self.board.edge(edge).cloned() {
                    cx.emit(crate::BoardEvent::EdgeChanged {
                        before: was,
                        after: now,
                    });
                }
                cx.emit(crate::CanvasEvent::Changed);
            }
            return;
        }
        match (wiring.target, wiring.dir) {
            (Some(target), Dir::Out) => {
                if let Err(crate::model::WireError::Exists(id)) =
                    self.add_wire(wiring.fixed, target, cx)
                {
                    self.set_selection(Vec::new(), Some(id), cx);
                    self.show_toast("They are wired so already", false, window, cx);
                }
            }
            (Some(target), Dir::In) => {
                if let Err(crate::model::WireError::Exists(id)) =
                    self.add_wire(target, wiring.fixed, cx)
                {
                    self.set_selection(Vec::new(), Some(id), cx);
                    self.show_toast("They are wired so already", false, window, cx);
                }
            }
            // A click on a port, not a drag: nothing to wire.
            (None, _) if travelled < 4. => {}
            (None, dir) => {
                let wire = match dir {
                    Dir::Out => Wire::From(wiring.fixed),
                    Dir::In => Wire::To(wiring.fixed),
                };
                let local = self.local(position);
                self.wire_request(wiring.at, wire, local, window, cx);
            }
        }
    }

    pub(crate) fn mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end_gesture(event.position, window, cx);
    }

    pub(crate) fn scroll(
        &mut self,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local = self.local(event.position);
        // Over the node in use, the wheel is its body's (see `wheel_guard`):
        // what its body left alone does not move the board under it either.
        if let Some(id) = self.entered
            && self.bodies.contains_key(&id)
            && let Some(node) = self.board.node(id)
        {
            let (wx, wy) = self.camera.to_world(local.0, local.1);
            if node.rect.contains(wx, wy) && !(event.modifiers.platform || event.modifiers.control)
            {
                return;
            }
        }
        let delta = event.delta.pixel_delta(px(16.));
        let (mut dx, mut dy): (f32, f32) = (delta.x.into(), delta.y.into());
        self.glide = None;
        let zooming = event.modifiers.platform || event.modifiers.control;
        // A step of the wheel is a ratio: so much closer, or further.
        let closer = dy.clamp(-60., 60.) * 0.006;
        if !zooming && event.modifiers.shift && dx == 0. {
            (dx, dy) = (dy, 0.);
        }
        // A wheel's line steps glide, as they do in every scroll area of the
        // window. A trackpad's pixels are the fingers' own movement: applied
        // as they come, and the end of a glide.
        let smooth = SmoothScroll::of(cx);
        if smooth.enabled && !event.delta.precise() {
            let now = self.clock.now();
            let wheel = self.wheel.get_or_insert_with(WheelGlide::new);
            if zooming {
                wheel.zoom_by(closer, local, &self.camera, now, smooth.duration);
            } else {
                wheel.pan_by(dx, dy, now, smooth.duration);
            }
        } else {
            self.wheel = None;
            if zooming {
                self.camera.zoom_at(local.0, local.1, closer.exp());
            } else {
                self.camera.pan(dx, dy);
            }
        }
        self.save_camera_later(window, cx);
        cx.notify();
    }

    /// Whether the wheel over a node's body moves the board rather than
    /// what is in the body: unless the body has the keys, or with Cmd or
    /// Ctrl, which zoom.
    pub(crate) fn board_takes_wheel(&self, id: NodeId, event: &ScrollEvent) -> bool {
        self.entered != Some(id) || event.modifiers.platform || event.modifiers.control
    }

    pub(crate) fn pinch(
        &mut self,
        event: &PinchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = self.local(event.position);
        self.glide = None;
        self.wheel = None;
        self.camera.zoom_at(x, y, 1. + event.delta);
        self.save_camera_later(window, cx);
        cx.notify();
    }

    // The camera moved by the wheel: saved once the wheel rests.
    fn save_camera_later(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_generation += 1;
        let generation = self.scroll_generation;
        if self.frozen {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            this.update(cx, |this, cx| {
                if this.scroll_generation == generation {
                    cx.emit(crate::CanvasEvent::Changed);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The camera that shows the whole board, for the first frame.
    pub(crate) fn first_camera(&self) -> Camera {
        let (width, height) = self.viewport();
        match self.board.bounds() {
            Some(bounds) => Camera::fit(&bounds, (width, height), crate::view::FIT_PADDING),
            // An empty board starts with its origin in the middle, so the
            // first node lands where the eye already is.
            None => Camera {
                x: width / 2.,
                y: height / 2.,
                zoom: 1.,
            },
        }
    }
}
