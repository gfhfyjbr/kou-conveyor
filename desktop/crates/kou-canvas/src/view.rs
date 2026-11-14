//! The canvas as a GPUI view: its state, what the app calls, what it tells
//! the app, and the frame it draws. Pointer and keyboard handling, painting,
//! the nodes, the menus and the inspector are in their own files, each an
//! `impl CanvasView` block over the state defined here.
//!
//! A node is there the moment it is added and gone the moment it is removed,
//! and so are its edges. What moves: the camera on its way somewhere, the
//! hover and selection of nodes and edges, a menu or the inspector fading,
//! a message's dot along its edge, and the bar that offers to undo.

use crate::{
    anim::{Clock, Tween},
    catalog::{AddRequest, Offer, TemplateRequest},
    field::Field,
    geometry::{Camera, Lod, edge_curve},
    inspector::Inspector,
    menu::Menu,
    model::{
        Board, Change, Dir, Edge, EdgeId, EdgeMode, End, Node, NodeId, NodeKind, NodeSpec, PortRef,
        Rect, Wire,
    },
    paint::Grid,
};
use gpui::{
    AnyView, App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable,
    Pixels, Point, ScrollGlide, Window,
};
use kou_ui::{HUES, Mark, hue_index, motion};
use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

/// How long the camera takes to travel.
pub(crate) const GLIDE: Duration = Duration::from_millis(280);
/// Space kept between the board and the window edge when fitting.
pub(crate) const FIT_PADDING: f32 = 56.;
/// Space kept around a node the camera slides to.
const REVEAL_MARGIN: f32 = 24.;
/// How many changes undo remembers.
const HISTORY: usize = 100;
/// How long the bar that offers to undo a deletion stays.
const TOAST_FOR: Duration = Duration::from_secs(8);

/// What the canvas tells its owner.
#[derive(Clone, Debug, PartialEq)]
pub enum CanvasEvent {
    /// The user asked to open a node as a pane of its own: "Open as a pane" in
    /// its menu, or Enter on a node that has no live body to enter.
    Opened(NodeId),
    /// The user asked for a new node at a world point, and the app has given the
    /// canvas no offers to show: the app asks what to start.
    CreateRequested {
        /// World x.
        x: f32,
        /// World y.
        y: f32,
    },
    /// The user asked to remove a node the app owns. The owner decides, then
    /// calls [`CanvasView::remove`].
    RemoveRequested(NodeId),
    /// The board or the camera changed in a way worth saving.
    Changed,
}

/// What the user did to the board's nodes, edges and selection, for the
/// owner to act on: a second event type beside [`CanvasEvent`], so an owner
/// that matches every `CanvasEvent` keeps compiling. Subscribe to both.
#[derive(Clone, Debug, PartialEq)]
pub enum BoardEvent {
    /// The user picked one of the app's offers. The app starts it and calls
    /// [`CanvasView::fulfil`].
    AddRequested(AddRequest),
    /// The user picked a template. The app starts its nodes and calls
    /// [`CanvasView::build_template`].
    TemplateRequested(TemplateRequest),
    /// The user asked for a copy of a node the app owns, at a world point. The
    /// app starts the same thing and calls [`CanvasView::create_at`].
    DuplicateRequested {
        /// The node to copy.
        node: NodeId,
        /// World x of the copy's centre.
        x: f32,
        /// World y of the copy's centre.
        y: f32,
    },
    /// The user removed a node the canvas owns (a note); it can be undone.
    NodeRemoved(Node),
    /// The user renamed a node, or undo gave it its old name back.
    Renamed {
        /// The node.
        node: NodeId,
        /// Its title now.
        title: String,
    },
    /// The user wired an output to an input (or undo brought an edge back).
    EdgeAdded(Edge),
    /// An edge's ends or settings changed by the user's hand.
    EdgeChanged {
        /// What it was.
        before: Edge,
        /// What it is now.
        after: Edge,
    },
    /// The user removed an edge (or undo took one away).
    EdgeRemoved(Edge),
    /// The keys went into a node's body, or back to the board (`None`).
    Entered(Option<NodeId>),
    /// The selection changed.
    SelectionChanged {
        /// The nodes selected.
        nodes: Vec<NodeId>,
        /// The edge selected.
        edge: Option<EdgeId>,
    },
    /// The user opened what waits for a node from its badge.
    PendingOpened(NodeId),
}

/// What has gone along an edge, as the app counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traffic {
    /// Messages delivered along it.
    pub delivered: u32,
    /// Messages on it waiting for the user's approval.
    pub waiting: u32,
}

/// What waits for a node, as the app counts it: shown on a tab over the node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    /// Messages waiting for it to be ready.
    pub count: u32,
    /// Of those, the ones waiting for the user's approval.
    pub awaiting: u32,
    /// The title of the node the first of those comes from.
    pub from: Option<String>,
}

/// What the canvas keeps about a node beside the board: its state, and the
/// small tweens of its selection and its ports.
pub(crate) struct Live {
    pub mark: Mark,
    pub since: Instant,
    pub select: Tween,
    /// How far its input and output ports are out.
    pub ports: [Tween; 2],
}

impl Live {
    pub(crate) fn new(clock: &Clock) -> Self {
        Self {
            mark: Mark::Idle,
            since: clock.now(),
            select: Tween::at(0.),
            ports: [Tween::at(0.), Tween::at(0.)],
        }
    }
}

pub(crate) struct Body {
    pub view: AnyView,
    pub focus: Option<FocusHandle>,
}

pub(crate) struct Glide {
    pub from: Camera,
    pub to: Camera,
    pub start: Instant,
}

/// A wheel's line steps on their way: the board glides to where they add up
/// to, the way every scroll area of the window does ([`ScrollGlide`]). A
/// trackpad's pixels are not part of it; they move the board as they come.
pub(crate) struct WheelGlide {
    // How far the wheel has asked the board to move, in pixels on screen,
    // and how much of that the camera has moved by.
    pan: (ScrollGlide, ScrollGlide),
    panned: (f32, f32),
    // The zoom the wheel has asked for, as its natural logarithm (a step of
    // the wheel is a ratio, so the glide is even in it), and the canvas
    // pixel under which the board holds still.
    zoom: Option<ScrollGlide>,
    at: (f32, f32),
}

impl WheelGlide {
    pub fn new() -> Self {
        Self {
            pan: (ScrollGlide::new(0.), ScrollGlide::new(0.)),
            panned: (0., 0.),
            zoom: None,
            at: (0., 0.),
        }
    }

    /// Move the board further, by pixels on screen.
    pub fn pan_by(&mut self, dx: f32, dy: f32, now: Instant, duration: Duration) {
        let (x, y) = &mut self.pan;
        x.glide_to(x.target() + dx, now, duration);
        y.glide_to(y.target() + dy, now, duration);
    }

    /// Zoom `camera` further by a factor given as its logarithm, around the
    /// canvas pixel `at`. The zoom's limits bound where the glide goes, so a
    /// step back from one moves at once.
    pub fn zoom_by(
        &mut self,
        by: f32,
        at: (f32, f32),
        camera: &Camera,
        now: Instant,
        duration: Duration,
    ) {
        let mut zoom = self
            .zoom
            .unwrap_or_else(|| ScrollGlide::new(camera.zoom.ln()));
        let target = (zoom.target() + by).clamp(Camera::MIN_ZOOM.ln(), Camera::MAX_ZOOM.ln());
        zoom.glide_to(target, now, duration);
        self.zoom = Some(zoom);
        self.at = at;
    }

    /// Put `camera` where the glide is at `now`. Returns whether the glide
    /// is still on its way.
    pub fn advance(&mut self, camera: &mut Camera, now: Instant) -> bool {
        let to = (self.pan.0.position(now), self.pan.1.position(now));
        camera.pan(to.0 - self.panned.0, to.1 - self.panned.1);
        self.panned = to;
        if let Some(zoom) = &self.zoom {
            camera.zoom_to(self.at.0, self.at.1, zoom.position(now).exp());
        }
        self.pan.0.is_moving(now)
            || self.pan.1.is_moving(now)
            || self.zoom.is_some_and(|zoom| zoom.is_moving(now))
    }
}

pub(crate) struct EdgeFx {
    pub hover: Tween,
    pub select: Tween,
}

pub(crate) struct Flow {
    pub edge: EdgeId,
    pub start: Instant,
    pub duration: Duration,
}

/// A press and drag in progress.
pub(crate) enum Gesture {
    Pan {
        last: Point<Pixels>,
    },
    Marquee {
        from: (f32, f32),
        to: (f32, f32),
        base: Vec<NodeId>,
        moved: bool,
        additive: bool,
    },
    Move {
        start: Point<Pixels>,
        starts: Vec<(NodeId, f32, f32)>,
        moved: bool,
        before: Board,
        clicked: NodeId,
        shift: bool,
    },
    Resize {
        id: NodeId,
        start: Point<Pixels>,
        size: (f32, f32),
        before: Board,
        moved: bool,
    },
    Wire(Wiring),
}

/// A wire being drawn from a port, or an edge's end being moved.
pub(crate) struct Wiring {
    /// The port the wire is held at.
    pub fixed: PortRef,
    /// Which way the fixed port faces.
    pub dir: Dir,
    /// The free end, in world units.
    pub at: (f32, f32),
    /// The port the free end docks in.
    pub target: Option<PortRef>,
    /// The port the free end is near, not docked yet.
    pub close: Option<PortRef>,
    /// The edge whose end is moving, and which end.
    pub retarget: Option<(EdgeId, End)>,
    pub start: Point<Pixels>,
}

impl Wiring {
    /// The direction of the ports the free end can end in.
    pub fn wanted(&self) -> Dir {
        self.dir.flip()
    }
}

#[derive(Default)]
pub(crate) struct Hover {
    pub edge: Option<EdgeId>,
    pub near: Option<(NodeId, Dir)>,
    pub port: Option<PortRef>,
}

/// What a field being written in belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    Title(NodeId, bool),
    Template(EdgeId),
    Note(NodeId),
}

pub(crate) struct Editing {
    pub what: Edit,
    pub field: Field,
    pub focused: bool,
}

pub(crate) struct Toast {
    pub text: String,
    pub undo: bool,
    pub shown: Instant,
    pub until: Instant,
}

/// The canvas view.
pub struct CanvasView {
    pub(crate) board: Board,
    pub(crate) camera: Camera,
    /// The camera's zoom as of the frame being drawn, for bodies that scale
    /// themselves to it. See [`CanvasView::zoom_handle`].
    pub(crate) zoom: Rc<Cell<f32>>,
    /// The grid's dots, a view of their own (see [`Grid`]).
    pub(crate) grid: Entity<Grid>,
    pub(crate) live: HashMap<NodeId, Live>,
    pub(crate) selection: Vec<NodeId>,
    pub(crate) edge_selected: Option<EdgeId>,
    pub(crate) gesture: Option<Gesture>,
    pub(crate) glide: Option<Glide>,
    pub(crate) wheel: Option<WheelGlide>,
    pub(crate) focus: FocusHandle,
    pub(crate) area: Rc<Cell<Bounds<Pixels>>>,
    pub(crate) bodies: HashMap<NodeId, Body>,
    pub(crate) fit_pending: bool,
    pub(crate) frozen: bool,
    pub(crate) clock: Clock,
    pub(crate) hover: Hover,
    pub(crate) edge_fx: HashMap<EdgeId, EdgeFx>,
    pub(crate) flows: Vec<Flow>,
    pub(crate) traffic: HashMap<EdgeId, Traffic>,
    pub(crate) pending: HashMap<NodeId, Pending>,
    pub(crate) menu: Option<Menu>,
    pub(crate) menu_focus: FocusHandle,
    pub(crate) inspector: Option<Inspector>,
    pub(crate) editing: Option<Editing>,
    pub(crate) field_focus: FocusHandle,
    pub(crate) entered: Option<NodeId>,
    /// A node entered while the camera still had to bring it close: its
    /// body takes the keys once it is drawn.
    pub(crate) entering: Option<NodeId>,
    pub(crate) offers: Vec<Offer>,
    pub(crate) history: Vec<Board>,
    pub(crate) future: Vec<Board>,
    pub(crate) toast: Option<Toast>,
    pub(crate) space_held: bool,
    pub(crate) pointer: Option<(f32, f32)>,
    pub(crate) ticking: bool,
    pub(crate) scroll_generation: u64,
}

impl EventEmitter<CanvasEvent> for CanvasView {}
impl EventEmitter<BoardEvent> for CanvasView {}

impl Focusable for CanvasView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A node's hue: its harness's, or its title's for a node with no harness.
pub(crate) fn hue_of(node: &Node) -> u32 {
    HUES[hue_index(name_of(node))]
}

fn name_of(node: &Node) -> &str {
    if node.tag.is_empty() {
        &node.title
    } else {
        &node.tag
    }
}

impl CanvasView {
    /// A canvas showing `board`, fitted to the window on its first frame.
    pub fn new(board: Board, cx: &mut Context<Self>) -> Self {
        let clock = Clock::default();
        let live = board
            .nodes()
            .iter()
            .map(|node| (node.id, Live::new(&clock)))
            .collect();
        let camera = Camera::default();
        Self {
            board,
            camera,
            zoom: Rc::new(Cell::new(camera.zoom)),
            // Placed and coloured on the first frame.
            grid: cx.new(|_| Grid {
                step: 0.,
                origin: (0., 0.),
                dot: gpui::transparent_black(),
            }),
            live,
            selection: Vec::new(),
            edge_selected: None,
            gesture: None,
            glide: None,
            wheel: None,
            // The board is a stop on the way through the window's controls.
            focus: cx.focus_handle().tab_index(0).tab_stop(true),
            area: Rc::new(Cell::new(Bounds::default())),
            bodies: HashMap::new(),
            fit_pending: true,
            frozen: false,
            clock,
            hover: Hover::default(),
            edge_fx: HashMap::new(),
            flows: Vec::new(),
            traffic: HashMap::new(),
            pending: HashMap::new(),
            menu: None,
            menu_focus: cx.focus_handle(),
            inspector: None,
            editing: None,
            field_focus: cx.focus_handle(),
            entered: None,
            entering: None,
            offers: Vec::new(),
            history: Vec::new(),
            future: Vec::new(),
            toast: None,
            space_held: false,
            pointer: None,
            ticking: false,
            scroll_generation: 0,
        }
    }

    /// The board as it is now.
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// The camera as it is now.
    pub fn camera(&self) -> Camera {
        self.camera
    }

    /// The camera's zoom as of the canvas frame being drawn. The canvas sets
    /// it on every frame, a frame of a glide or a pinch included, before the
    /// bodies of its nodes are laid out, so a body that reads it while it
    /// draws is never a frame late. A body that scales itself by it (a
    /// terminal following the zoom) keeps its text as it is at any zoom:
    /// it is laid out in the node's on-screen rectangle, which is its
    /// unscaled size times this.
    pub fn zoom_handle(&self) -> Rc<Cell<f32>> {
        self.zoom.clone()
    }

    /// Restore a saved camera instead of fitting on the first frame.
    pub fn set_camera(&mut self, camera: Camera, cx: &mut Context<Self>) {
        self.camera = camera;
        self.fit_pending = false;
        self.glide = None;
        self.wheel = None;
        cx.notify();
    }

    /// The selected node, when exactly one is.
    pub fn selected(&self) -> Option<NodeId> {
        match self.selection.as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    }

    /// The selected nodes, and the selected edge.
    pub fn selection(&self) -> (&[NodeId], Option<EdgeId>) {
        (&self.selection, self.edge_selected)
    }

    /// The node whose body has the keys.
    pub fn entered(&self) -> Option<NodeId> {
        self.entered
    }

    /// Where a world point is in the window now: to anchor something at a node.
    pub fn window_point(&self, x: f32, y: f32) -> Point<Pixels> {
        let (sx, sy) = self.camera.to_screen(x, y);
        self.area.get().origin + gpui::point(gpui::px(sx), gpui::px(sy))
    }

    /// How much of a node shows at the zoom now.
    pub fn lod(&self) -> Lod {
        Lod::of(self.camera.zoom)
    }

    /// Tell the canvas what the app can start, for its add menu and its
    /// templates. With none, adding asks the app (`CreateRequested`).
    pub fn set_offers(&mut self, offers: Vec<Offer>, cx: &mut Context<Self>) {
        self.offers = offers;
        cx.notify();
    }

    /// Put a node where the user asked, selected. It is there at once; when
    /// it is out of sight the camera glides to it.
    pub fn create_at(
        &mut self,
        x: f32,
        y: f32,
        spec: NodeSpec,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> NodeId {
        let id = self.board.add_at(spec, x, y);
        self.welcome(id);
        self.set_selection(vec![id], None, cx);
        self.reveal(id, cx);
        cx.emit(CanvasEvent::Changed);
        cx.notify();
        id
    }

    /// Make the node the user asked for with [`BoardEvent::AddRequested`]:
    /// at the point it was asked for, wired to the edge the user drew there.
    pub fn fulfil(
        &mut self,
        request: &AddRequest,
        spec: NodeSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> NodeId {
        let Some(wire) = &request.wire else {
            return self.create_at(request.x, request.y, spec, window, cx);
        };
        let (id, edge) = self.board.add_wired(spec, (request.x, request.y), wire);
        self.welcome(id);
        if let Some(edge) = edge.and_then(|edge| self.board.edge(edge).cloned()) {
            cx.emit(BoardEvent::EdgeAdded(edge));
        }
        self.set_selection(vec![id], None, cx);
        self.reveal(id, cx);
        cx.emit(CanvasEvent::Changed);
        cx.notify();
        id
    }

    /// Put a template on the board: `specs` are its nodes as the app started
    /// them, in the template's order. Returns their ids.
    pub fn build_template(
        &mut self,
        request: &TemplateRequest,
        specs: Vec<NodeSpec>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<NodeId> {
        self.remember();
        let ids: Vec<NodeId> = request
            .template
            .nodes
            .iter()
            .zip(specs)
            .map(|(node, spec)| {
                let (w, h) = spec.size.unwrap_or_else(|| spec.kind.default_size());
                let id = self.board.add_rect(
                    spec,
                    Rect::new(request.x + node.x, request.y + node.y, w, h),
                );
                self.welcome(id);
                id
            })
            .collect();
        for edge in &request.template.edges {
            let (Some(from), Some(to)) = (ids.get(edge.from), ids.get(edge.to)) else {
                continue;
            };
            let (Some(a), Some(b)) = (self.board.node(*from), self.board.node(*to)) else {
                continue;
            };
            let (Some(out), Some(inp)) = (a.kind.outputs().first(), b.kind.inputs().first()) else {
                continue;
            };
            if let Ok(id) = self
                .board
                .connect(PortRef::new(*from, *out), PortRef::new(*to, *inp))
            {
                self.board.update_edge(id, |e| e.mode = edge.mode);
            }
        }
        self.set_selection(ids.clone(), None, cx);
        if let Some(bounds) = self.board.bounds_of(ids.iter().copied()) {
            let to = Camera::fit(&bounds, self.viewport(), FIT_PADDING);
            self.glide_to(to, cx);
        }
        cx.emit(CanvasEvent::Changed);
        cx.notify();
        ids
    }

    /// A node creates another: the child is placed beside its parent, wired
    /// from it, and there at once with its edge. `None` when there is no
    /// such parent.
    pub fn spawn_child(
        &mut self,
        parent: NodeId,
        spec: NodeSpec,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<NodeId> {
        let id = self.board.add_child(parent, spec)?;
        self.welcome(id);
        self.reveal(id, cx);
        cx.emit(CanvasEvent::Changed);
        cx.notify();
        Some(id)
    }

    /// Take a node off the board, with its edges, at once.
    pub fn remove(&mut self, id: NodeId, _: &mut Window, cx: &mut Context<Self>) {
        if self.take_off(id, cx).is_some() {
            cx.emit(CanvasEvent::Changed);
            cx.notify();
        }
    }

    /// Show what a node's agent is doing.
    pub fn set_mark(
        &mut self,
        id: NodeId,
        mark: Mark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = self.clock.now();
        let Some(live) = self.live.get_mut(&id) else {
            return;
        };
        if live.mark == mark {
            return;
        }
        live.mark = mark;
        live.since = now;
        self.keep_ticking(window, cx);
        cx.notify();
    }

    /// Set a node's mark as it already was, on a board being restored.
    pub fn restore_mark(&mut self, id: NodeId, mark: Mark, cx: &mut Context<Self>) {
        if let Some(live) = self.live.get_mut(&id) {
            live.mark = mark;
            cx.notify();
        }
    }

    /// Replace a node's title, tag, detail or place.
    pub fn update_node(
        &mut self,
        id: NodeId,
        change: impl FnOnce(&mut Node),
        cx: &mut Context<Self>,
    ) {
        if let Some(node) = self.board.node_mut(id) {
            change(node);
            cx.notify();
        }
    }

    /// Draw a live view inside a node instead of its detail text: at every
    /// zoom at which nodes are whole ([`Lod::Whole`]), laid out in the
    /// node's on-screen rectangle under its header. A press in it gives it
    /// the keys. A view that should keep its text as it is at any zoom
    /// scales itself by [`CanvasView::zoom_handle`].
    pub fn set_body(&mut self, id: NodeId, body: AnyView, cx: &mut Context<Self>) {
        let focus = self.bodies.remove(&id).and_then(|old| old.focus);
        self.bodies.insert(id, Body { view: body, focus });
        cx.notify();
    }

    /// Say which focus handle a node's body takes the keys with, so entering
    /// the node gives it the keys and the canvas knows when it has them.
    pub fn set_body_focus(&mut self, id: NodeId, focus: FocusHandle, cx: &mut Context<Self>) {
        if let Some(body) = self.bodies.get_mut(&id) {
            body.focus = Some(focus);
            cx.notify();
        }
    }

    /// Show what went along an edge: a count on its label, and what waits on it.
    pub fn set_traffic(&mut self, edge: EdgeId, traffic: Traffic, cx: &mut Context<Self>) {
        if traffic == Traffic::default() {
            self.traffic.remove(&edge);
        } else {
            self.traffic.insert(edge, traffic);
        }
        cx.notify();
    }

    /// A message went along an edge: a dot runs along it.
    pub fn flow(&mut self, edge: EdgeId, cx: &mut Context<Self>) {
        let Some(curve) = self
            .board
            .edge(edge)
            .and_then(|e| edge_curve(&self.board, e))
        else {
            return;
        };
        let length = curve.length() * self.camera.zoom;
        let duration = Duration::from_millis((320. + length / 2.).min(900.) as u64);
        self.flows.push(Flow {
            edge,
            start: self.clock.now(),
            duration,
        });
        // The next frame draws it, and asks for the frames after while it runs.
        cx.notify();
    }

    /// Show what waits for a node on a tab over it.
    pub fn set_pending(&mut self, node: NodeId, pending: Pending, cx: &mut Context<Self>) {
        if pending.count == 0 {
            self.pending.remove(&node);
        } else {
            self.pending.insert(node, pending);
        }
        cx.notify();
    }

    /// Wire an output to an input from the app's side (no `EdgeAdded` echoes back).
    pub fn connect(
        &mut self,
        from: PortRef,
        to: PortRef,
        cx: &mut Context<Self>,
    ) -> Option<EdgeId> {
        let id = self.board.connect(from, to).ok()?;
        cx.emit(CanvasEvent::Changed);
        cx.notify();
        Some(id)
    }

    /// Change an edge's settings from the app's side.
    pub fn update_edge(
        &mut self,
        id: EdgeId,
        change: impl FnOnce(&mut Edge),
        cx: &mut Context<Self>,
    ) {
        if self.board.update_edge(id, change).is_some() {
            cx.emit(CanvasEvent::Changed);
            cx.notify();
        }
    }

    /// Take an edge away from the app's side.
    pub fn disconnect(&mut self, id: EdgeId, cx: &mut Context<Self>) {
        if self.board.disconnect(id).is_some() {
            self.forget_edge(id);
            if self.edge_selected == Some(id) {
                self.set_selection(self.selection.clone(), None, cx);
            }
            cx.emit(CanvasEvent::Changed);
            cx.notify();
        }
    }

    /// Select a node, or nothing.
    pub fn select(&mut self, id: Option<NodeId>, cx: &mut Context<Self>) {
        self.set_selection(id.into_iter().collect(), None, cx);
    }

    /// Select nodes.
    pub fn select_nodes(&mut self, ids: &[NodeId], cx: &mut Context<Self>) {
        let ids = ids
            .iter()
            .copied()
            .filter(|id| self.board.node(*id).is_some())
            .collect();
        self.set_selection(ids, None, cx);
    }

    /// Select an edge, or no edge.
    pub fn select_edge(&mut self, id: Option<EdgeId>, cx: &mut Context<Self>) {
        let id = id.filter(|id| self.board.edge(*id).is_some());
        self.set_selection(Vec::new(), id, cx);
    }

    /// Glide the camera so the whole board is in view.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        let Some(bounds) = self.board.bounds() else {
            return;
        };
        let to = Camera::fit(&bounds, self.viewport(), FIT_PADDING);
        self.glide_to(to, cx);
    }

    /// Glide the camera to the selection, as large as fits up to 100%.
    pub fn zoom_to_selection(&mut self, cx: &mut Context<Self>) {
        let ids = self.selected_or_edge_nodes();
        let Some(bounds) = self.board.bounds_of(ids) else {
            return self.fit(cx);
        };
        let to = Camera::fit(&bounds, self.viewport(), FIT_PADDING);
        self.glide_to(to, cx);
    }

    /// The selection at its own size, in the middle; or 100% about the middle.
    pub fn zoom_100(&mut self, cx: &mut Context<Self>) {
        let viewport = self.viewport();
        let to = match self.board.bounds_of(self.selection.iter().copied()) {
            Some(bounds) => Camera::centre_on(&bounds, viewport, 1.),
            None => {
                let mut to = self.target_camera();
                to.zoom_to(viewport.0 / 2., viewport.1 / 2., 1.);
                to
            }
        };
        self.glide_to(to, cx);
    }

    /// Show one node at 100% in the middle, smaller if it does not fit, but
    /// never so small that it is a placeholder.
    pub fn focus_node(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let Some(rect) = self.board.node(id).map(|node| node.rect) else {
            return;
        };
        let viewport = self.viewport();
        let zoom = 1f32
            .min((viewport.0 - 32.) / rect.w)
            .min((viewport.1 - 32.) / rect.h)
            .max(Lod::LIVE);
        self.set_selection(vec![id], None, cx);
        self.glide_to(Camera::centre_on(&rect, viewport, zoom), cx);
    }

    /// Give a node's body the keys, where it is. A node out of sight, or one
    /// drawn as a placeholder, is brought into view whole first. A node with
    /// no body to enter is opened as a pane instead (`Opened`).
    pub fn enter(&mut self, id: NodeId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rect) = self.board.node(id).map(|node| node.rect) else {
            return;
        };
        if !self.bodies.contains_key(&id) {
            cx.emit(CanvasEvent::Opened(id));
            return;
        }
        let camera = self.target_camera();
        let far = Lod::of(camera.zoom) == Lod::Placeholder;
        if far || !camera.shows(&rect, self.viewport(), 0.) {
            self.focus_node(id, cx);
        }
        self.set_selection(vec![id], None, cx);
        self.set_entered(Some(id), cx);
        if far && !self.frozen {
            // Its body is not drawn yet: it takes the keys when it is.
            self.entering = Some(id);
        } else if let Some(focus) = self.bodies.get(&id).and_then(|body| body.focus.clone()) {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// Take the keys back from a node's body to the board.
    pub fn exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entering = None;
        if self.entered.is_some() {
            self.set_entered(None, cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Take back the last change the user made on the board.
    pub fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(earlier) = self.history.pop() else {
            return false;
        };
        self.future.push(self.board.clone());
        self.go_back_to(earlier, cx);
        true
    }

    /// Make again what undo took back.
    pub fn redo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(later) = self.future.pop() else {
            return false;
        };
        self.history.push(self.board.clone());
        self.go_back_to(later, cx);
        true
    }

    /// Stop every animation at an instant, so a frame can be captured and
    /// compared: the canvas's clock holds still that long after it was first
    /// held (by [`CanvasView::set_frozen`] or this). For tests and captures.
    pub fn freeze_at(&mut self, elapsed_ms: f32, _: &mut Window, cx: &mut Context<Self>) {
        self.frozen = true;
        self.clock
            .hold_at(Duration::from_secs_f32(elapsed_ms.max(0.) / 1000.));
        cx.notify();
    }

    /// How many animations are under way: see [`CanvasView::describe_transitions`].
    pub fn running(&self, cx: &App) -> usize {
        self.describe_transitions(cx).len()
    }

    /// One line per animation under way: the camera's glide, a message's dot,
    /// a menu, the inspector or the undo bar fading, a node's or an edge's
    /// hover or selection. Nodes and edges themselves never animate in or out.
    pub fn describe_transitions(&self, _: &App) -> Vec<String> {
        let clock = &self.clock;
        let percent = |start: Instant, duration: Duration| {
            (clock.progress(start, duration) * 100.).round() as u32
        };
        let mut lines = Vec::new();
        if let Some(glide) = &self.glide {
            lines.push(format!(
                "camera: glide to {:.0}%, {}% of the way",
                glide.to.zoom * 100.,
                percent(glide.start, GLIDE)
            ));
        }
        for flow in &self.flows {
            lines.push(format!(
                "edge {}: message {}% along",
                flow.edge,
                percent(flow.start, flow.duration)
            ));
        }
        for (id, live) in &self.live {
            if live.select.moving(clock) || live.ports.iter().any(|port| port.moving(clock)) {
                lines.push(format!("node {id}: selection or ports"));
            }
        }
        for (id, fx) in &self.edge_fx {
            if fx.hover.moving(clock) || fx.select.moving(clock) {
                lines.push(format!("edge {id}: hover or selection"));
            }
        }
        if self.menu.as_ref().is_some_and(|menu| menu.moving(clock)) {
            lines.push("menu: fading".to_owned());
        }
        if self
            .inspector
            .as_ref()
            .is_some_and(|inspector| inspector.moving(clock))
        {
            lines.push("inspector: fading".to_owned());
        }
        if let Some(toast) = &self.toast
            && clock.progress(toast.shown, motion::BASE) < 1.
        {
            lines.push("undo bar: coming up".to_owned());
        }
        lines.sort();
        lines
    }

    /// Hold the canvas's clock still (true) or let it run: held, every
    /// animation started from now on starts at the held instant and
    /// [`CanvasView::freeze_at`] picks the instant drawn.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
        if frozen {
            self.clock.hold();
        } else {
            self.clock.release();
        }
    }

    // ------------------------------------------------------------ inside

    pub(crate) fn viewport(&self) -> (f32, f32) {
        let size = self.area.get().size;
        (size.width.into(), size.height.into())
    }

    pub(crate) fn local(&self, position: Point<Pixels>) -> (f32, f32) {
        let origin = self.area.get().origin;
        (
            (position.x - origin.x).into(),
            (position.y - origin.y).into(),
        )
    }

    pub(crate) fn world_at(&self, position: Point<Pixels>) -> (f32, f32) {
        let (x, y) = self.local(position);
        self.camera.to_world(x, y)
    }

    /// The camera the view is on its way to.
    pub(crate) fn target_camera(&self) -> Camera {
        self.glide.as_ref().map_or(self.camera, |glide| glide.to)
    }

    pub(crate) fn glide_to(&mut self, to: Camera, cx: &mut Context<Self>) {
        self.wheel = None;
        self.glide = Some(Glide {
            from: self.camera,
            to,
            start: self.clock.now(),
        });
        cx.notify();
    }

    pub(crate) fn zoom_step(&mut self, direction: i32, cx: &mut Context<Self>) {
        let (width, height) = self.viewport();
        let mut to = self.target_camera();
        to.zoom_to(width / 2., height / 2., Camera::step(to.zoom, direction));
        self.glide_to(to, cx);
    }

    // Keep what the canvas draws about a node that has just come onto the board.
    pub(crate) fn welcome(&mut self, id: NodeId) {
        if self.board.node(id).is_some() {
            self.live.insert(id, Live::new(&self.clock));
        }
    }

    // Drop what the canvas drew about an edge that is gone.
    pub(crate) fn forget_edge(&mut self, id: EdgeId) {
        self.edge_fx.remove(&id);
        self.traffic.remove(&id);
        self.flows.retain(|flow| flow.edge != id);
    }

    // Bring a node that is out of sight into view: the camera slides just far
    // enough at the zoom it has, so what is on screen stays as legible as it
    // was (fitting the whole board could turn every node into a placeholder).
    pub(crate) fn reveal(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let viewport = self.viewport();
        if viewport.0 <= 0. {
            return;
        }
        let Some(rect) = self.board.node(id).map(|node| node.rect) else {
            return;
        };
        let camera = self.target_camera();
        if !camera.shows(&rect, viewport, REVEAL_MARGIN) {
            self.glide_to(camera.bring_into_view(&rect, viewport, REVEAL_MARGIN), cx);
        }
    }

    pub(crate) fn set_selection(
        &mut self,
        nodes: Vec<NodeId>,
        edge: Option<EdgeId>,
        cx: &mut Context<Self>,
    ) {
        if nodes == self.selection && edge == self.edge_selected {
            return;
        }
        self.selection = nodes;
        self.edge_selected = edge;
        // The selection's fade starts now, not at the next frame, so a
        // capture with the clock held draws it as it is that long after.
        let clock = self.clock;
        for (id, live) in self.live.iter_mut() {
            let to = if self.selection.contains(id) { 1. } else { 0. };
            live.select.retarget(to, motion::BASE, &clock);
        }
        for (id, fx) in self.edge_fx.iter_mut() {
            let to = if self.edge_selected == Some(*id) {
                1.
            } else {
                0.
            };
            fx.select.retarget(to, motion::BASE, &clock);
        }
        self.follow_selection();
        cx.emit(BoardEvent::SelectionChanged {
            nodes: self.selection.clone(),
            edge: self.edge_selected,
        });
        cx.notify();
    }

    pub(crate) fn set_entered(&mut self, id: Option<NodeId>, cx: &mut Context<Self>) {
        if self.entered != id {
            self.entered = id;
            cx.emit(BoardEvent::Entered(id));
        }
    }

    pub(crate) fn selected_or_edge_nodes(&self) -> Vec<NodeId> {
        if !self.selection.is_empty() {
            return self.selection.clone();
        }
        self.edge_selected
            .and_then(|id| self.board.edge(id))
            .map(|edge| vec![edge.from, edge.to])
            .unwrap_or_default()
    }

    /// Keep the board as it is before a change the user makes, for undo.
    pub(crate) fn remember(&mut self) {
        self.remember_board(self.board.clone());
    }

    pub(crate) fn remember_board(&mut self, board: Board) {
        if self.history.last() == Some(&board) {
            return;
        }
        self.history.push(board);
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.future.clear();
    }

    fn go_back_to(&mut self, earlier: Board, cx: &mut Context<Self>) {
        let changes = self.board.restore(&earlier);
        for change in changes {
            match change {
                Change::EdgeAdded(edge) => cx.emit(BoardEvent::EdgeAdded(edge)),
                Change::EdgeRemoved(edge) => {
                    self.forget_edge(edge.id);
                    cx.emit(BoardEvent::EdgeRemoved(edge));
                }
                Change::EdgeChanged { before, after } => {
                    cx.emit(BoardEvent::EdgeChanged { before, after })
                }
                Change::Renamed { node, title } => cx.emit(BoardEvent::Renamed { node, title }),
                Change::NoteAdded(id) => self.welcome(id),
                Change::NoteRemoved(node) => {
                    self.live.remove(&node.id);
                    cx.emit(BoardEvent::NodeRemoved(node));
                }
            }
        }
        self.selection.retain(|id| self.board.node(*id).is_some());
        if self
            .edge_selected
            .is_some_and(|id| self.board.edge(id).is_none())
        {
            self.edge_selected = None;
        }
        self.follow_selection();
        cx.emit(CanvasEvent::Changed);
        cx.notify();
    }

    // Take a node and its edges off the board; the caller says why.
    pub(crate) fn take_off(&mut self, id: NodeId, cx: &mut Context<Self>) -> Option<Node> {
        let node = self.board.node(id)?.clone();
        self.live.remove(&id);
        let edges: Vec<EdgeId> = self.board.edges_of(id).map(|edge| edge.id).collect();
        for edge in &edges {
            self.forget_edge(*edge);
        }
        self.board.remove(id);
        self.bodies.remove(&id);
        self.pending.remove(&id);
        if self.entered == Some(id) {
            self.set_entered(None, cx);
        }
        if self.entering == Some(id) {
            self.entering = None;
        }
        if self.selection.contains(&id)
            || self.edge_selected.is_some_and(|edge| edges.contains(&edge))
        {
            let rest = self
                .selection
                .iter()
                .copied()
                .filter(|n| *n != id)
                .collect();
            let edge = self
                .edge_selected
                .filter(|edge| self.board.edge(*edge).is_some());
            self.set_selection(rest, edge, cx);
        }
        if matches!(&self.editing, Some(editing) if matches!(editing.what, Edit::Title(n, _) | Edit::Note(n) if n == id))
        {
            self.editing = None;
        }
        Some(node)
    }

    pub(crate) fn show_toast(
        &mut self,
        text: impl Into<String>,
        undo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = self.clock.now();
        self.toast = Some(Toast {
            text: text.into(),
            undo,
            shown: now,
            until: now + TOAST_FOR,
        });
        if !self.frozen {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(TOAST_FOR + motion::BASE)
                    .await;
                this.update(cx, |this, cx| {
                    if this
                        .toast
                        .as_ref()
                        .is_some_and(|t| t.until <= this.clock.now())
                    {
                        this.toast = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    // Busy nodes count their seconds: one redraw a second while any works.
    pub(crate) fn keep_ticking(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let working = self.live.values().any(|live| live.mark == Mark::Working);
        if self.ticking || !working || self.frozen {
            return;
        }
        self.ticking = true;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let go_on = this
                    .update(cx, |this, cx| {
                        let working = this.live.values().any(|live| live.mark == Mark::Working);
                        if working {
                            cx.notify();
                        } else {
                            this.ticking = false;
                        }
                        working
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
            }
        })
        .detach();
    }

    // Keep the hover and selection tweens of nodes and edges headed where
    // the board and the pointer say, and drop what has finished. Returns
    // whether anything still moves.
    pub(crate) fn settle_motion(&mut self) -> bool {
        let clock = self.clock;
        let wiring_dir = match &self.gesture {
            Some(Gesture::Wire(wiring)) => Some(wiring.wanted()),
            _ => None,
        };
        let mut moving = false;
        for node in self.board.nodes() {
            let Some(live) = self.live.get_mut(&node.id) else {
                continue;
            };
            let selected = if self.selection.contains(&node.id) {
                1.
            } else {
                0.
            };
            live.select.retarget(selected, motion::BASE, &clock);
            moving |= live.select.moving(&clock);
            for (i, dir) in [Dir::In, Dir::Out].into_iter().enumerate() {
                let out = self.hover.near == Some((node.id, dir))
                    || self.hover.port.as_ref().is_some_and(|port| {
                        port.node == node.id && node.kind.ports(dir).contains(&port.port.as_str())
                    })
                    || (wiring_dir == Some(dir) && !node.kind.ports(dir).is_empty())
                    || (self.entered.is_none()
                        && self.selection == [node.id]
                        && self.gesture.is_none()
                        && self.hover.near.is_none()
                        && self.pointer.is_none());
                live.ports[i].retarget(if out { 1. } else { 0. }, motion::QUICK, &clock);
                moving |= live.ports[i].moving(&clock);
            }
        }
        for edge in self.board.edges() {
            let fx = self.edge_fx.entry(edge.id).or_insert_with(|| EdgeFx {
                hover: Tween::at(0.),
                select: Tween::at(0.),
            });
            let hover = if self.hover.edge == Some(edge.id) {
                1.
            } else {
                0.
            };
            let selected = if self.edge_selected == Some(edge.id) {
                1.
            } else {
                0.
            };
            fx.hover.retarget(hover, motion::QUICK, &clock);
            fx.select.retarget(selected, motion::BASE, &clock);
            moving |= fx.hover.moving(&clock) || fx.select.moving(&clock);
        }
        self.flows
            .retain(|flow| clock.progress(flow.start, flow.duration) < 1.);
        moving |= !self.flows.is_empty();
        if let Some(menu) = &self.menu {
            moving |= menu.moving(&clock);
        }
        if self.menu.as_ref().is_some_and(|menu| menu.gone(&clock)) {
            self.menu = None;
        }
        if let Some(inspector) = &self.inspector {
            moving |= inspector.moving(&clock);
        }
        if self
            .inspector
            .as_ref()
            .is_some_and(|inspector| inspector.gone(&clock))
        {
            self.inspector = None;
        }
        if let Some(toast) = &self.toast {
            moving |= clock.progress(toast.shown, motion::BASE) < 1.;
        }
        moving
    }

    // A node the canvas does not own is started and stopped by the app; the
    // canvas only asks. Several are asked about one by one.
    pub(crate) fn ask_to_remove(
        &mut self,
        ids: &[NodeId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut notes = Vec::new();
        for id in ids {
            match self.board.node(*id).map(|node| node.kind) {
                Some(kind) if kind.canvas_owned() => notes.push(*id),
                Some(_) => cx.emit(CanvasEvent::RemoveRequested(*id)),
                None => {}
            }
        }
        if notes.is_empty() {
            return;
        }
        self.remember();
        for id in &notes {
            if let Some(node) = self.take_off(*id, cx) {
                cx.emit(BoardEvent::NodeRemoved(node));
            }
        }
        let text = if notes.len() == 1 {
            "Note deleted".to_owned()
        } else {
            format!("{} notes deleted", notes.len())
        };
        self.show_toast(text, true, window, cx);
        cx.emit(CanvasEvent::Changed);
        cx.notify();
    }

    pub(crate) fn delete_edge(&mut self, id: EdgeId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edge) = self.board.edge(id).cloned() else {
            return;
        };
        self.remember();
        self.board.disconnect(id);
        self.forget_edge(id);
        if self.edge_selected == Some(id) {
            self.set_selection(self.selection.clone(), None, cx);
        }
        cx.emit(BoardEvent::EdgeRemoved(edge));
        self.show_toast("Edge deleted", true, window, cx);
        cx.emit(CanvasEvent::Changed);
        cx.notify();
    }

    pub(crate) fn set_edge_mode(&mut self, id: EdgeId, mode: EdgeMode, cx: &mut Context<Self>) {
        self.change_edge(id, |edge| edge.mode = mode, cx);
    }

    pub(crate) fn change_edge(
        &mut self,
        id: EdgeId,
        change: impl FnOnce(&mut Edge),
        cx: &mut Context<Self>,
    ) {
        let Some(before) = self.board.edge(id).cloned() else {
            return;
        };
        let mut after = before.clone();
        change(&mut after);
        if after == before {
            return;
        }
        self.remember();
        let after_kept = after.clone();
        self.board.update_edge(id, |edge| *edge = after_kept);
        let Some(after) = self.board.edge(id).cloned() else {
            return;
        };
        cx.emit(BoardEvent::EdgeChanged { before, after });
        cx.emit(CanvasEvent::Changed);
        cx.notify();
    }

    pub(crate) fn rename(&mut self, id: NodeId, title: &str, cx: &mut Context<Self>) {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        let title: String = title.chars().take(120).collect();
        if title.is_empty() || self.board.node(id).is_none_or(|node| node.title == title) {
            return;
        }
        self.remember();
        if let Some(node) = self.board.node_mut(id) {
            node.title = title.clone();
        }
        cx.emit(BoardEvent::Renamed { node: id, title });
        cx.emit(CanvasEvent::Changed);
        cx.notify();
    }

    pub(crate) fn duplicate_selection(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selection.clone();
        let (notes, others): (Vec<NodeId>, Vec<NodeId>) = ids.into_iter().partition(|id| {
            self.board
                .node(*id)
                .is_some_and(|node| node.kind.canvas_owned())
        });
        for id in others {
            if let Some(node) = self.board.node(id) {
                let (x, y) = node.rect.center();
                cx.emit(BoardEvent::DuplicateRequested {
                    node: id,
                    x: x + crate::model::DUPLICATE_OFFSET,
                    y: y + crate::model::DUPLICATE_OFFSET,
                });
            }
        }
        if notes.is_empty() {
            return;
        }
        self.remember();
        let pairs = self.board.duplicate(&notes);
        let copies: Vec<NodeId> = pairs.iter().map(|pair| pair.1).collect();
        for copy in &copies {
            self.welcome(*copy);
        }
        self.set_selection(copies, None, cx);
        cx.emit(CanvasEvent::Changed);
        cx.notify();
    }

    pub(crate) fn add_note(&mut self, x: f32, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.remember();
        let spec = NodeSpec {
            kind: NodeKind::Note,
            title: self.free_title("Note"),
            ..NodeSpec::default()
        };
        let (w, h) = NodeKind::Note.default_size();
        let id = self.board.add_rect(spec, Rect::new(x, y, w, h));
        self.welcome(id);
        self.set_selection(vec![id], None, cx);
        self.reveal(id, cx);
        self.start_edit(Edit::Note(id), window, cx);
        cx.emit(CanvasEvent::Changed);
    }

    pub(crate) fn free_title(&self, base: &str) -> String {
        let taken = |title: &str| {
            self.board
                .nodes()
                .iter()
                .any(|node| node.title.eq_ignore_ascii_case(title))
        };
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|title| !taken(title))
            .unwrap_or_else(|| base.to_owned())
    }

    pub(crate) fn add_wire(
        &mut self,
        from: PortRef,
        to: PortRef,
        cx: &mut Context<Self>,
    ) -> Result<EdgeId, crate::model::WireError> {
        let before = self.board.clone();
        let id = self.board.connect(from, to)?;
        self.remember_board(before);
        if let Some(edge) = self.board.edge(id).cloned() {
            cx.emit(BoardEvent::EdgeAdded(edge));
        }
        self.set_selection(Vec::new(), Some(id), cx);
        cx.emit(CanvasEvent::Changed);
        cx.notify();
        Ok(id)
    }

    pub(crate) fn wire_request(
        &mut self,
        at: (f32, f32),
        wire: Wire,
        local: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_add_menu(local, at, Some(wire), window, cx);
    }
}
