//! The board: nodes, the ports on their sides, the edges between ports and
//! what an edge means for the messages that go along it, and where a new node
//! goes. Everything here is plain data: the view draws it and turns what the
//! user does into changes of it; the app reads it to route messages.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Identifies a node on its board.
pub type NodeId = u64;
/// Identifies an edge on its board.
pub type EdgeId = u64;

/// The pitch of the dots on the board and of where new nodes are placed, in
/// world units: four snaps, as on kou's web canvas.
pub const GRID: f32 = 32.;
/// What a dragged or resized node snaps to: the web canvas's grid.
pub const SNAP: f32 = 8.;
/// The gap kept between a parent and the column of its children.
pub const GAP_X: f32 = GRID * 3.;
/// The gap kept between siblings.
pub const GAP_Y: f32 = GRID;
/// The least width of a node, as kou's engine has it.
pub const MIN_W: f32 = 160.;
/// The least height of a node.
pub const MIN_H: f32 = 90.;
/// The most a node may measure either way.
pub const MAX_SIZE: f32 = 6000.;
/// How far below a node's top its first port sits: under the header.
pub const PORT_FIRST: f32 = 54.;
/// How far apart the ports of one side are.
pub const PORT_STEP: f32 = 24.;
/// How far a duplicate lands from what it copies.
pub const DUPLICATE_OFFSET: f32 = 32.;

/// What a node holds. The kinds are kou-conveyor's canvas kinds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// An agent: a harness in a terminal, or a native thread.
    #[default]
    Agent,
    /// A plain shell.
    Terminal,
    /// Text the user or an agent pinned to the board.
    Note,
    /// Something agents read from: a repository, a file, a feed.
    Source,
}

impl NodeKind {
    /// The word shown beside a node's title.
    pub fn label(self) -> &'static str {
        match self {
            NodeKind::Agent => "agent",
            NodeKind::Terminal => "terminal",
            NodeKind::Note => "note",
            NodeKind::Source => "source",
        }
    }

    /// The ports messages come in by, top to bottom on the left side.
    pub fn inputs(self) -> &'static [&'static str] {
        match self {
            NodeKind::Agent | NodeKind::Terminal => &["in"],
            NodeKind::Note | NodeKind::Source => &[],
        }
    }

    /// The ports a node gives its output on, top to bottom on the right side:
    /// a terminal also says when its program ended.
    pub fn outputs(self) -> &'static [&'static str] {
        match self {
            NodeKind::Agent | NodeKind::Source => &["out"],
            NodeKind::Terminal => &["out", "exit"],
            NodeKind::Note => &[],
        }
    }

    /// The ports of one side.
    pub fn ports(self, dir: Dir) -> &'static [&'static str] {
        match dir {
            Dir::In => self.inputs(),
            Dir::Out => self.outputs(),
        }
    }

    /// Whether the canvas itself owns nodes of this kind. A note is only text on
    /// the board, so the canvas adds and removes it; anything else runs in a
    /// pane the app owns, and the app decides.
    pub fn canvas_owned(self) -> bool {
        self == NodeKind::Note
    }

    /// How a message along an edge reaches a node of this kind when the edge
    /// does not say: a native agent reads it between its steps, anything else
    /// gets it once it is idle, as a terminal would type it.
    pub fn default_delivery(self) -> Delivery {
        match self {
            NodeKind::Agent => Delivery::Now,
            _ => Delivery::Queue,
        }
    }

    /// The size a new node of this kind starts with: room for a terminal of
    /// eighty columns, a few lines of a note, a source's last events.
    pub fn default_size(self) -> (f32, f32) {
        match self {
            NodeKind::Agent | NodeKind::Terminal => (640., 416.),
            NodeKind::Note => (280., 180.),
            NodeKind::Source => (320., 220.),
        }
    }
}

/// Which way a port faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    /// An input, on a node's left side.
    In,
    /// An output, on a node's right side.
    Out,
}

impl Dir {
    /// The other way.
    pub fn flip(self) -> Dir {
        match self {
            Dir::In => Dir::Out,
            Dir::Out => Dir::In,
        }
    }
}

/// A port of a node, by name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PortRef {
    /// The node.
    pub node: NodeId,
    /// The port's name: `in`, `out`, `exit`, or a source's own.
    pub port: String,
}

impl PortRef {
    /// A port of a node.
    pub fn new(node: NodeId, port: impl Into<String>) -> Self {
        Self {
            node,
            port: port.into(),
        }
    }
}

/// A rectangle in world units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle from its top left corner and size.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    /// The rectangle two corners span, in any order.
    pub fn spanning(a: (f32, f32), b: (f32, f32)) -> Self {
        Self::new(
            a.0.min(b.0),
            a.1.min(b.1),
            (a.0 - b.0).abs(),
            (a.1 - b.1).abs(),
        )
    }
    /// Right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    /// Bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    /// The centre.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2., self.y + self.h / 2.)
    }
    /// Whether two rectangles overlap once each is grown by `margin`.
    pub fn overlaps(&self, other: &Rect, margin: f32) -> bool {
        self.x < other.right() + margin
            && other.x < self.right() + margin
            && self.y < other.bottom() + margin
            && other.y < self.bottom() + margin
    }
    /// The smallest rectangle holding both.
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }
    /// Whether a point is inside.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    /// The same rectangle grown by `by` on every side.
    pub fn inflate(&self, by: f32) -> Rect {
        Rect::new(self.x - by, self.y - by, self.w + by * 2., self.h + by * 2.)
    }
    /// A rectangle a fraction of the way to another, corner by corner.
    pub fn toward(&self, other: &Rect, t: f32) -> Rect {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Rect::new(
            mix(self.x, other.x),
            mix(self.y, other.y),
            mix(self.w, other.w),
            mix(self.h, other.h),
        )
    }
}

/// A value on the snapping grid of `step`.
pub fn snap_to(value: f32, step: f32) -> f32 {
    (value / step).round() * step
}

/// What a new node is, before the board gives it an id and a place.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeSpec {
    /// See [`NodeKind`].
    pub kind: NodeKind,
    /// Its name.
    pub title: String,
    /// A short code for what runs in it: a harness's letters, or empty.
    pub tag: String,
    /// One or more lines under the header: a path, a note's text, the last output.
    pub detail: String,
    /// Width and height in world units; a default per kind when `None`.
    pub size: Option<(f32, f32)>,
}

/// A node on the board.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Its id.
    pub id: NodeId,
    /// See [`NodeKind`].
    pub kind: NodeKind,
    /// Its name.
    pub title: String,
    /// See [`NodeSpec::tag`].
    pub tag: String,
    /// See [`NodeSpec::detail`]. A note's text is its detail.
    pub detail: String,
    /// Where it is.
    pub rect: Rect,
    /// The node that created it.
    pub parent: Option<NodeId>,
    /// Where it works, for its footer: a folder, a branch, the program it runs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub place: String,
    /// The user made its agent the orchestrator of this board: it may wire
    /// the board's nodes together and take wires away, as the user does.
    /// The user gives the right in the node's inspector, and takes it back
    /// there.
    #[serde(default, skip_serializing_if = "is_default")]
    pub orchestrator: bool,
}

impl Node {
    /// Where one of its ports is, in world units: inputs on the left edge,
    /// outputs on the right, under the header and a step apart.
    pub fn port_point(&self, dir: Dir, port: &str) -> (f32, f32) {
        port_at(self.kind, &self.rect, dir, port)
    }
}

/// Where a port of a node of `kind` is when the node is at `rect`.
pub fn port_at(kind: NodeKind, rect: &Rect, dir: Dir, port: &str) -> (f32, f32) {
    let index = kind
        .ports(dir)
        .iter()
        .position(|name| *name == port)
        .unwrap_or(0);
    let x = match dir {
        Dir::In => rect.x,
        Dir::Out => rect.right(),
    };
    // A node too short for its ports keeps them inside it.
    let y = (rect.y + PORT_FIRST + index as f32 * PORT_STEP).min(rect.bottom() - 8.);
    (x, y)
}

/// Whether messages go along an edge, and how: kou's edge modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeMode {
    /// Messages go as they come.
    #[default]
    Auto,
    /// Each message waits for the user on its target.
    Approve,
    /// Nothing goes along it.
    Off,
}

impl EdgeMode {
    /// Every mode, in the order a switch shows them.
    pub const ALL: [EdgeMode; 3] = [EdgeMode::Auto, EdgeMode::Approve, EdgeMode::Off];

    /// Its name on a switch.
    pub fn label(self) -> &'static str {
        match self {
            EdgeMode::Auto => "Auto",
            EdgeMode::Approve => "Approve",
            EdgeMode::Off => "Off",
        }
    }

    /// What it does, in a sentence.
    pub fn description(self) -> &'static str {
        match self {
            EdgeMode::Auto => "Messages go as they come",
            EdgeMode::Approve => "Each message waits for you on its target",
            EdgeMode::Off => "Nothing goes along it",
        }
    }

    /// The next mode, for a key that cycles them.
    pub fn next(self) -> EdgeMode {
        match self {
            EdgeMode::Auto => EdgeMode::Approve,
            EdgeMode::Approve => EdgeMode::Off,
            EdgeMode::Off => EdgeMode::Auto,
        }
    }
}

/// When a message reaches a target that is busy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// It waits until the target is idle.
    Queue,
    /// It goes at once, into what runs.
    Now,
}

impl Delivery {
    /// Its name on a switch.
    pub fn label(self) -> &'static str {
        match self {
            Delivery::Queue => "Wait",
            Delivery::Now => "Now",
        }
    }
}

/// How an edge came to be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeOrigin {
    /// A node started the other: what every edge of a board saved before
    /// edges could be drawn by hand was.
    #[default]
    Spawned,
    /// The user (or a template) wired an output to an input.
    Wired,
    /// One agent sent the other a message on purpose: it shows that they
    /// talk, and carries nothing by itself.
    Sent,
}

impl EdgeOrigin {
    /// Whether an edge that came to be this way pipes what its source
    /// answers: only one wired on purpose does.
    pub fn pipes(self) -> bool {
        self == EdgeOrigin::Wired
    }
}

/// How many times a message may be passed on along edges, one agent's
/// answer becoming the next one's prompt, before it waits for the user: what
/// stops a ring of agents answering each other for ever.
pub const HOP_LIMIT: u32 = 6;

fn out_port() -> String {
    "out".to_owned()
}

fn in_port() -> String {
    "in".to_owned()
}

/// A directed link from a node's output port to another's input: what one
/// node puts out goes to the other, as its mode, delivery and template say.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Edge {
    /// Its id; boards saved before edges had ids get theirs when loaded.
    #[serde(default)]
    pub id: EdgeId,
    /// The node it starts at.
    pub from: NodeId,
    /// The node it ends at.
    pub to: NodeId,
    /// The output port it starts at.
    #[serde(default = "out_port")]
    pub from_port: String,
    /// The input port it ends at.
    #[serde(default = "in_port")]
    pub to_port: String,
    /// See [`EdgeMode`].
    #[serde(default, skip_serializing_if = "is_default")]
    pub mode: EdgeMode,
    /// See [`Delivery`]; `None` is as the target's kind wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver: Option<Delivery>,
    /// Whether a message says which node it comes from; `None` is as the
    /// target's kind wants (on for agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<bool>,
    /// Makes a message's text from the output: `{{text}}` when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub template: String,
    /// See [`EdgeOrigin`].
    #[serde(default)]
    pub origin: EdgeOrigin,
    /// The node whose agent made it, or wired it last; `None` when the user
    /// did. The board says so on the edge, so that the user can tell a wire
    /// of their own from one an agent drew.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<NodeId>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// The line a message that came through the canvas starts with: what it is
/// (`from`, `answer from`), whose it is, and, from the second time it was
/// passed on, how many times that is of how many there may be.
pub fn canvas_header(what: &str, title: &str, id: &str, hops: u32) -> String {
    if hops >= 2 {
        format!("[canvas] {what} «{title}» ({id}), hop {hops} of {HOP_LIMIT}:")
    } else {
        format!("[canvas] {what} «{title}» ({id}):")
    }
}

/// Which end of an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum End {
    /// Where it starts: an output.
    From,
    /// Where it ends: an input.
    To,
}

impl Edge {
    /// The port it starts at.
    pub fn from_ref(&self) -> PortRef {
        PortRef::new(self.from, self.from_port.clone())
    }

    /// The port it ends at.
    pub fn to_ref(&self) -> PortRef {
        PortRef::new(self.to, self.to_port.clone())
    }

    /// Whether anything goes along it.
    pub fn carries(&self) -> bool {
        self.mode != EdgeMode::Off
    }

    /// How it delivers to a target of `kind`.
    pub fn delivery(&self, kind: NodeKind) -> Delivery {
        self.deliver.unwrap_or_else(|| kind.default_delivery())
    }

    /// Whether a message to a target of `kind` starts with where it comes from.
    pub fn header_for(&self, kind: NodeKind) -> bool {
        self.header.unwrap_or(kind == NodeKind::Agent)
    }

    /// The text a message along this edge carries, made from an output of
    /// the node it starts at: the template filled in (`{{text}}`,
    /// `{{from.title}}`, `{{from.id}}`, `{{from.port}}`, `{{edge.id}}`), under
    /// a line naming its source when the header is on for the target.
    pub fn message(&self, text: &str, from_title: &str, target: NodeKind) -> String {
        self.message_at(text, from_title, target, 1)
    }

    /// The same for a message that has been passed on `hops` times, this
    /// time included: from the second on, the line naming its source says
    /// so, and how many there may be (see [`HOP_LIMIT`]).
    pub fn message_at(&self, text: &str, from_title: &str, target: NodeKind, hops: u32) -> String {
        let template = if self.template.trim().is_empty() {
            "{{text}}"
        } else {
            &self.template
        };
        let mut body = String::new();
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            body.push_str(&rest[..start]);
            let Some(len) = rest[start..].find("}}") else {
                body.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let key = rest[start + 2..start + len].trim();
            match key {
                "text" => body.push_str(text),
                "from.title" | "title" => body.push_str(from_title),
                "from.id" => body.push_str(&self.from.to_string()),
                "from.port" => body.push_str(&self.from_port),
                "edge.id" => body.push_str(&self.id.to_string()),
                _ => {}
            }
            rest = &rest[start + len + 2..];
        }
        body.push_str(rest);
        if self.header_for(target) {
            format!(
                "{}\n{body}",
                canvas_header("from", from_title, &self.from.to_string(), hops)
            )
        } else {
            body
        }
    }

    fn same_ends(&self, other: &Edge) -> bool {
        self.from == other.from
            && self.to == other.to
            && self.from_port == other.from_port
            && self.to_port == other.to_port
    }
}

/// Why two ports could not be wired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    /// One of the nodes is not on the board.
    Missing,
    /// A node cannot be wired to itself.
    SameNode,
    /// The ports are not an output and an input of their nodes.
    NoSuchPort,
    /// The same ports are wired so already.
    Exists(EdgeId),
}

/// Where a new node is wired as it is made: the edge the user drew to an
/// empty spot of the board.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Wire {
    /// The new node's input takes this output.
    From(PortRef),
    /// The new node's output goes into this input.
    To(PortRef),
}

/// What restoring an earlier state of the board changed, for the owner to hear.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// An edge came back.
    EdgeAdded(Edge),
    /// An edge went.
    EdgeRemoved(Edge),
    /// An edge's ends or settings went back.
    EdgeChanged {
        /// What it was.
        before: Edge,
        /// What it is now.
        after: Edge,
    },
    /// A node's title went back.
    Renamed {
        /// The node.
        node: NodeId,
        /// Its title now.
        title: String,
    },
    /// A note came back.
    NoteAdded(NodeId),
    /// A note went.
    NoteRemoved(Node),
}

/// Nodes and edges. Order is paint order: later nodes are on top.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "SavedBoard")]
pub struct Board {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    next: NodeId,
    next_edge: EdgeId,
}

// A board as saved, by this version or an earlier one: edges without ids or
// ports, no edge counter.
#[derive(Deserialize)]
struct SavedBoard {
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    edges: Vec<Edge>,
    #[serde(default)]
    next: NodeId,
    #[serde(default)]
    next_edge: EdgeId,
}

impl From<SavedBoard> for Board {
    // Make a board read from disk whole: ids counted past those taken, every
    // edge with an id of its own and both its nodes there, no edge twice.
    fn from(saved: SavedBoard) -> Self {
        let mut seen = HashSet::new();
        let nodes: Vec<Node> = saved
            .nodes
            .into_iter()
            .filter(|node| seen.insert(node.id))
            .collect();
        let next = nodes.iter().map(|node| node.id).fold(saved.next, u64::max);
        let mut next_edge = saved
            .edges
            .iter()
            .map(|edge| edge.id)
            .fold(saved.next_edge, u64::max);
        let mut ids = HashSet::new();
        let mut edges: Vec<Edge> = Vec::new();
        for mut edge in saved.edges {
            if !seen.contains(&edge.from)
                || !seen.contains(&edge.to)
                || edges.iter().any(|kept| kept.same_ends(&edge))
            {
                continue;
            }
            if edge.id == 0 || !ids.insert(edge.id) {
                next_edge += 1;
                edge.id = next_edge;
                ids.insert(edge.id);
            }
            edges.push(edge);
        }
        Board {
            nodes,
            edges,
            next,
            next_edge,
        }
    }
}

impl Board {
    /// An empty board.
    pub fn new() -> Self {
        Self::default()
    }

    /// The nodes, bottom to top.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// The edges.
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// A node by id.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// A node by id, to change it.
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|node| node.id == id)
    }

    /// An edge by id.
    pub fn edge(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.iter().find(|edge| edge.id == id)
    }

    /// The edges that start or end at a node.
    pub fn edges_of(&self, node: NodeId) -> impl Iterator<Item = &Edge> {
        self.edges
            .iter()
            .filter(move |edge| edge.from == node || edge.to == node)
    }

    /// The ring an edge from `from` to `to` would close, if it carried:
    /// the nodes from `to` round to `from`, along edges that pipe by
    /// themselves ([`EdgeOrigin::pipes`], and not switched off). `None` when
    /// what `to` answers never comes back to `from`.
    pub fn would_ring(&self, from: NodeId, to: NodeId) -> Option<Vec<NodeId>> {
        if from == to {
            return Some(vec![from]);
        }
        // Breadth first from `to`, remembering how each node was reached.
        let mut came: Vec<(NodeId, NodeId)> = Vec::new();
        let mut queue = std::collections::VecDeque::from([to]);
        while let Some(node) = queue.pop_front() {
            for edge in &self.edges {
                if edge.from != node || !edge.carries() || !edge.origin.pipes() {
                    continue;
                }
                if edge.to == to || came.iter().any(|(reached, _)| *reached == edge.to) {
                    continue;
                }
                came.push((edge.to, node));
                if edge.to == from {
                    let mut ring = vec![from];
                    let mut at = from;
                    while at != to {
                        at = came.iter().find(|(reached, _)| *reached == at)?.1;
                        ring.push(at);
                    }
                    ring.reverse();
                    return Some(ring);
                }
                queue.push_back(edge.to);
            }
        }
        None
    }

    /// The edges a node's output on `port` goes along: the ones that carry,
    /// in the order they were made. What the app routes a message by.
    pub fn routes_from<'a>(
        &'a self,
        node: NodeId,
        port: &'a str,
    ) -> impl Iterator<Item = &'a Edge> {
        self.edges
            .iter()
            .filter(move |edge| edge.from == node && edge.from_port == port && edge.carries())
    }

    /// Whether the board has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The rectangle holding every node.
    pub fn bounds(&self) -> Option<Rect> {
        self.bounds_of(self.nodes.iter().map(|node| node.id))
    }

    /// The rectangle holding some nodes.
    pub fn bounds_of(&self, ids: impl IntoIterator<Item = NodeId>) -> Option<Rect> {
        ids.into_iter()
            .filter_map(|id| self.node(id).map(|node| node.rect))
            .reduce(|bounds, rect| bounds.union(&rect))
    }

    /// The nodes in reading order: top to bottom, then left to right.
    pub fn reading_order(&self) -> Vec<NodeId> {
        let mut nodes: Vec<&Node> = self.nodes.iter().collect();
        nodes.sort_by(|a, b| {
            a.rect
                .y
                .total_cmp(&b.rect.y)
                .then(a.rect.x.total_cmp(&b.rect.x))
        });
        nodes.into_iter().map(|node| node.id).collect()
    }

    /// The nodes a rectangle touches, bottom to top: what a marquee selects.
    pub fn nodes_in(&self, area: &Rect) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|node| node.rect.overlaps(area, 0.))
            .map(|node| node.id)
            .collect()
    }

    /// Add a node the user placed: centred on the point, moved to the nearest free spot.
    pub fn add_at(&mut self, spec: NodeSpec, x: f32, y: f32) -> NodeId {
        let (w, h) = spec.size.unwrap_or_else(|| spec.kind.default_size());
        let rect = self.free_near(Rect::new(
            snap_to(x - w / 2., GRID / 2.),
            snap_to(y - h / 2., GRID / 2.),
            w,
            h,
        ));
        self.insert(spec, rect, None)
    }

    /// Add a node exactly where it is asked to be, on the snapping grid.
    pub fn add_rect(&mut self, spec: NodeSpec, rect: Rect) -> NodeId {
        let rect = Rect::new(
            snap_to(rect.x, SNAP),
            snap_to(rect.y, SNAP),
            rect.w.clamp(MIN_W, MAX_SIZE),
            rect.h.clamp(MIN_H, MAX_SIZE),
        );
        self.insert(spec, rect, None)
    }

    /// Add a node at the end of a wire the user drew to an empty spot, and
    /// wire it: a node that takes the wire's output has its input at the
    /// point, one that feeds the wire's input has its output there. A wire
    /// that does not fit the kind (a note has no ports) leaves it unwired.
    pub fn add_wired(
        &mut self,
        spec: NodeSpec,
        at: (f32, f32),
        wire: &Wire,
    ) -> (NodeId, Option<EdgeId>) {
        let (w, h) = spec.size.unwrap_or_else(|| spec.kind.default_size());
        let x = match wire {
            Wire::From(_) => at.0,
            Wire::To(_) => at.0 - w,
        };
        let id = self.add_rect(spec, Rect::new(x, at.1 - PORT_FIRST, w, h));
        let kind = self.node(id).map(|node| node.kind).unwrap_or_default();
        let edge = match wire {
            Wire::From(from) => kind
                .inputs()
                .first()
                .and_then(|port| self.connect(from.clone(), PortRef::new(id, *port)).ok()),
            Wire::To(to) => kind
                .outputs()
                .first()
                .and_then(|port| self.connect(PortRef::new(id, *port), to.clone()).ok()),
        };
        (id, edge)
    }

    /// Add a node another node created: to the parent's right, under its
    /// earlier children, linked by an edge from the parent's output.
    pub fn add_child(&mut self, parent: NodeId, spec: NodeSpec) -> Option<NodeId> {
        let from = self.node(parent)?.rect;
        let (w, h) = spec.size.unwrap_or_else(|| spec.kind.default_size());
        let lowest_sibling = self
            .nodes
            .iter()
            .filter(|node| node.parent == Some(parent))
            .map(|node| node.rect.bottom())
            .fold(f32::NEG_INFINITY, f32::max);
        let y = if lowest_sibling.is_finite() {
            lowest_sibling + GAP_Y
        } else {
            from.y
        };
        let rect = self.free_below(Rect::new(
            snap_to(from.right() + GAP_X, GRID / 2.),
            snap_to(y, GRID / 2.),
            w,
            h,
        ));
        let id = self.insert(spec, rect, Some(parent));
        let parent_kind = self.node(parent).map(|node| node.kind).unwrap_or_default();
        let child_kind = self.node(id).map(|node| node.kind).unwrap_or_default();
        if let (Some(out), Some(inp)) = (parent_kind.outputs().first(), child_kind.inputs().first())
        {
            self.next_edge += 1;
            self.edges.push(Edge {
                id: self.next_edge,
                from: parent,
                to: id,
                from_port: (*out).to_owned(),
                to_port: (*inp).to_owned(),
                mode: EdgeMode::Auto,
                deliver: None,
                header: None,
                template: String::new(),
                origin: EdgeOrigin::Spawned,
                by: None,
            });
        }
        Some(id)
    }

    /// Wire an output to an input.
    pub fn connect(&mut self, from: PortRef, to: PortRef) -> Result<EdgeId, WireError> {
        let (Some(source), Some(target)) = (self.node(from.node), self.node(to.node)) else {
            return Err(WireError::Missing);
        };
        if from.node == to.node {
            return Err(WireError::SameNode);
        }
        if !source.kind.outputs().contains(&from.port.as_str())
            || !target.kind.inputs().contains(&to.port.as_str())
        {
            return Err(WireError::NoSuchPort);
        }
        let edge = Edge {
            id: 0,
            from: from.node,
            to: to.node,
            from_port: from.port,
            to_port: to.port,
            mode: EdgeMode::Auto,
            deliver: None,
            header: None,
            template: String::new(),
            origin: EdgeOrigin::Wired,
            by: None,
        };
        if let Some(existing) = self.edges.iter().find(|other| other.same_ends(&edge)) {
            return Err(WireError::Exists(existing.id));
        }
        self.next_edge += 1;
        let id = self.next_edge;
        self.edges.push(Edge { id, ..edge });
        Ok(id)
    }

    /// Link two existing nodes, output to input. Returns false when either is
    /// missing, they are the same node, they have no such ports, or the edge exists.
    pub fn link(&mut self, from: NodeId, to: NodeId) -> bool {
        self.connect(PortRef::new(from, "out"), PortRef::new(to, "in"))
            .is_ok()
    }

    /// Change an edge's settings; its ends stay. Returns what it was.
    pub fn update_edge(&mut self, id: EdgeId, change: impl FnOnce(&mut Edge)) -> Option<Edge> {
        let edge = self.edges.iter_mut().find(|edge| edge.id == id)?;
        let before = edge.clone();
        change(edge);
        // The ends are not a setting: an edge moves only by `retarget`.
        edge.id = before.id;
        edge.from = before.from;
        edge.to = before.to;
        edge.from_port = before.from_port.clone();
        edge.to_port = before.to_port.clone();
        Some(before)
    }

    /// Move one end of an edge to another port of the same direction.
    /// Returns what the edge was.
    pub fn retarget(&mut self, id: EdgeId, end: End, port: PortRef) -> Result<Edge, WireError> {
        let before = self.edge(id).cloned().ok_or(WireError::Missing)?;
        let node = self.node(port.node).ok_or(WireError::Missing)?;
        let (other, fits) = match end {
            End::From => (before.to, node.kind.outputs().contains(&port.port.as_str())),
            End::To => (
                before.from,
                node.kind.inputs().contains(&port.port.as_str()),
            ),
        };
        if !fits {
            return Err(WireError::NoSuchPort);
        }
        if other == port.node {
            return Err(WireError::SameNode);
        }
        let mut after = before.clone();
        match end {
            End::From => {
                after.from = port.node;
                after.from_port = port.port;
            }
            End::To => {
                after.to = port.node;
                after.to_port = port.port;
            }
        }
        if let Some(existing) = self
            .edges
            .iter()
            .find(|edge| edge.id != id && edge.same_ends(&after))
        {
            return Err(WireError::Exists(existing.id));
        }
        if let Some(edge) = self.edges.iter_mut().find(|edge| edge.id == id) {
            *edge = after;
        }
        Ok(before)
    }

    /// Take an edge away.
    pub fn disconnect(&mut self, id: EdgeId) -> Option<Edge> {
        let index = self.edges.iter().position(|edge| edge.id == id)?;
        Some(self.edges.remove(index))
    }

    /// Remove a node and its edges. Its children stay, without a parent.
    pub fn remove(&mut self, id: NodeId) -> Option<Node> {
        let index = self.nodes.iter().position(|node| node.id == id)?;
        let node = self.nodes.remove(index);
        self.edges.retain(|edge| edge.from != id && edge.to != id);
        for other in &mut self.nodes {
            if other.parent == Some(id) {
                other.parent = None;
            }
        }
        Some(node)
    }

    /// Move a node so its top left corner is at the point, and bring it to the top.
    pub fn move_to(&mut self, id: NodeId, x: f32, y: f32) {
        let Some(index) = self.nodes.iter().position(|node| node.id == id) else {
            return;
        };
        let mut node = self.nodes.remove(index);
        node.rect.x = x;
        node.rect.y = y;
        self.nodes.push(node);
    }

    /// Put nodes at given top left corners, in place in the paint order.
    pub fn place_all(&mut self, places: &[(NodeId, f32, f32)]) {
        for (id, x, y) in places {
            if let Some(node) = self.node_mut(*id) {
                node.rect.x = *x;
                node.rect.y = *y;
            }
        }
    }

    /// Bring nodes to the top of the paint order, keeping their own order.
    pub fn raise(&mut self, ids: &[NodeId]) {
        let (mut top, rest): (Vec<Node>, Vec<Node>) = std::mem::take(&mut self.nodes)
            .into_iter()
            .partition(|node| ids.contains(&node.id));
        self.nodes = rest;
        self.nodes.append(&mut top);
    }

    /// Resize a node, held within the least and most a node may measure.
    /// Returns the size it got.
    pub fn resize(&mut self, id: NodeId, w: f32, h: f32) -> Option<(f32, f32)> {
        let node = self.node_mut(id)?;
        node.rect.w = w.clamp(MIN_W, MAX_SIZE);
        node.rect.h = h.clamp(MIN_H, MAX_SIZE);
        Some((node.rect.w, node.rect.h))
    }

    /// Copies of nodes, a little down and to the right, titled as copies;
    /// edges stay with what they wire. Returns (original, copy) pairs.
    pub fn duplicate(&mut self, ids: &[NodeId]) -> Vec<(NodeId, NodeId)> {
        let originals: Vec<Node> = ids
            .iter()
            .filter_map(|id| self.node(*id).cloned())
            .collect();
        originals
            .into_iter()
            .map(|node| {
                let spec = NodeSpec {
                    kind: node.kind,
                    title: format!("{} copy", node.title),
                    tag: node.tag.clone(),
                    detail: node.detail.clone(),
                    size: Some((node.rect.w, node.rect.h)),
                };
                let rect = Rect::new(
                    node.rect.x + DUPLICATE_OFFSET,
                    node.rect.y + DUPLICATE_OFFSET,
                    node.rect.w,
                    node.rect.h,
                );
                let copy = self.insert(spec, rect, None);
                if let Some(made) = self.node_mut(copy) {
                    made.place = node.place;
                }
                (node.id, copy)
            })
            .collect()
    }

    /// The topmost node under a point.
    pub fn hit(&self, x: f32, y: f32) -> Option<NodeId> {
        self.nodes
            .iter()
            .rev()
            .find(|node| node.rect.contains(x, y))
            .map(|node| node.id)
    }

    /// The port nearest a world point, of a direction, within `reach`; and
    /// how far it is. Ports of `except` are left out.
    pub fn nearest_port(
        &self,
        at: (f32, f32),
        dir: Dir,
        reach: f32,
        except: Option<NodeId>,
    ) -> Option<(PortRef, f32)> {
        self.nodes
            .iter()
            .filter(|node| Some(node.id) != except)
            .flat_map(|node| {
                node.kind.ports(dir).iter().map(move |port| {
                    let (x, y) = node.port_point(dir, port);
                    (PortRef::new(node.id, *port), (x - at.0).hypot(y - at.1))
                })
            })
            .filter(|(_, distance)| *distance <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The node nearest to `from` in a direction (`dx`, `dy` in -1..=1), for arrow keys.
    pub fn neighbour(&self, from: NodeId, dx: f32, dy: f32) -> Option<NodeId> {
        let (ox, oy) = self.node(from)?.rect.center();
        self.nodes
            .iter()
            .filter(|node| node.id != from)
            .filter_map(|node| {
                let (x, y) = node.rect.center();
                let (vx, vy) = (x - ox, y - oy);
                let along = vx * dx + vy * dy;
                let across = (vx * dy - vy * dx).abs();
                // Ahead of us, and more ahead than aside.
                (along > 0. && along >= across * 0.5).then_some((node.id, along + across * 2.))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// Makes the edge `id` here what it is in `now` (there, changed or gone):
    /// how a state kept for undo takes a change that was not the user's.
    pub(crate) fn carry_edge(&mut self, id: EdgeId, now: Option<&Edge>) {
        let at = self.edges.iter().position(|edge| edge.id == id);
        match (at, now) {
            (Some(at), Some(now)) => self.edges[at] = now.clone(),
            (Some(at), None) => {
                self.edges.remove(at);
            }
            (None, Some(now)) => {
                self.edges.push(now.clone());
                self.next_edge = self.next_edge.max(now.id);
            }
            (None, None) => {}
        }
    }

    /// Go back to an earlier state of this board, as far as it can be gone back
    /// to: where nodes were and what they were called, the notes, and the edges
    /// between nodes that are still here. Nodes the app owns stay or stay gone,
    /// since their panes came and went for real. Returns what changed.
    pub fn restore(&mut self, earlier: &Board) -> Vec<Change> {
        let mut changes = Vec::new();
        let now: HashMap<NodeId, Node> = self.nodes.iter().map(|n| (n.id, n.clone())).collect();
        let mut nodes = Vec::new();
        for old in &earlier.nodes {
            match now.get(&old.id) {
                Some(current) => {
                    if current.title != old.title {
                        changes.push(Change::Renamed {
                            node: old.id,
                            title: old.title.clone(),
                        });
                    }
                    let mut node = current.clone();
                    node.rect = old.rect;
                    node.title = old.title.clone();
                    if node.kind.canvas_owned() {
                        node.detail = old.detail.clone();
                    }
                    nodes.push(node);
                }
                None if old.kind.canvas_owned() => {
                    changes.push(Change::NoteAdded(old.id));
                    nodes.push(old.clone());
                }
                None => {}
            }
        }
        for current in &self.nodes {
            if nodes.iter().any(|node| node.id == current.id) {
                continue;
            }
            if current.kind.canvas_owned() {
                changes.push(Change::NoteRemoved(current.clone()));
            } else {
                nodes.push(current.clone());
            }
        }
        let present: HashSet<NodeId> = nodes.iter().map(|node| node.id).collect();
        let wanted: Vec<Edge> = earlier
            .edges
            .iter()
            .filter(|edge| present.contains(&edge.from) && present.contains(&edge.to))
            .cloned()
            .collect();
        for edge in &self.edges {
            match wanted.iter().find(|old| old.id == edge.id) {
                None => changes.push(Change::EdgeRemoved(edge.clone())),
                Some(old) if old != edge => changes.push(Change::EdgeChanged {
                    before: edge.clone(),
                    after: old.clone(),
                }),
                Some(_) => {}
            }
        }
        for old in &wanted {
            if !self.edges.iter().any(|edge| edge.id == old.id) {
                changes.push(Change::EdgeAdded(old.clone()));
            }
        }
        self.nodes = nodes;
        self.edges = wanted;
        self.next = self.next.max(earlier.next);
        self.next_edge = self.next_edge.max(earlier.next_edge);
        changes
    }

    fn insert(&mut self, spec: NodeSpec, rect: Rect, parent: Option<NodeId>) -> NodeId {
        self.next += 1;
        self.nodes.push(Node {
            id: self.next,
            kind: spec.kind,
            title: spec.title,
            tag: spec.tag,
            detail: spec.detail,
            rect,
            parent,
            place: String::new(),
            orchestrator: false,
        });
        self.next
    }

    fn blocked(&self, rect: &Rect) -> Option<&Node> {
        self.nodes
            .iter()
            .find(|node| node.rect.overlaps(rect, GAP_Y - 0.5))
    }

    // A child's column: walk down until nothing is in the way.
    fn free_below(&self, mut rect: Rect) -> Rect {
        while let Some(node) = self.blocked(&rect) {
            // Up to the grid, never down: rounded down, it would land on the
            // same node again and walk on the spot for ever.
            let step = GRID / 2.;
            rect.y = ((node.rect.bottom() + GAP_Y) / step).ceil() * step;
        }
        rect
    }

    // A placed node: the nearest free spot on rings of the grid around the wish.
    fn free_near(&self, wish: Rect) -> Rect {
        if self.blocked(&wish).is_none() {
            return wish;
        }
        for ring in 1..64 {
            let reach = ring as f32 * GRID;
            let mut best: Option<(f32, Rect)> = None;
            let steps = ring * 8;
            for step in 0..steps {
                // Walk the ring's perimeter in grid steps.
                let side = step / (ring * 2);
                let along = (step % (ring * 2)) as f32 * GRID - reach;
                let (dx, dy) = match side {
                    0 => (along, -reach),
                    1 => (reach, along),
                    2 => (-along, reach),
                    _ => (-reach, -along),
                };
                let rect = Rect::new(wish.x + dx, wish.y + dy, wish.w, wish.h);
                if self.blocked(&rect).is_none() {
                    let distance = dx * dx + dy * dy;
                    if best.is_none_or(|(least, _)| distance < least) {
                        best = Some((distance, rect));
                    }
                }
            }
            if let Some((_, rect)) = best {
                return rect;
            }
        }
        wish
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(title: &str) -> NodeSpec {
        NodeSpec {
            title: title.into(),
            ..NodeSpec::default()
        }
    }

    fn of(kind: NodeKind, title: &str) -> NodeSpec {
        NodeSpec {
            kind,
            title: title.into(),
            ..NodeSpec::default()
        }
    }

    #[test]
    fn children_stack_to_the_right_of_their_parent_without_touching() {
        let mut board = Board::new();
        let root = board.add_at(agent("lead"), 0., 0.);
        let a = board.add_child(root, agent("a")).unwrap();
        let b = board.add_child(root, agent("b")).unwrap();
        let c = board.add_child(a, agent("c")).unwrap();
        let rect = |id| board.node(id).unwrap().rect;
        assert_eq!(rect(a).x, rect(root).right() + GAP_X);
        assert_eq!(rect(a).y, rect(root).y);
        assert_eq!(rect(b).x, rect(a).x);
        assert_eq!(rect(b).y, rect(a).bottom() + GAP_Y);
        assert_eq!(rect(c).x, rect(a).right() + GAP_X);
        for (i, one) in board.nodes().iter().enumerate() {
            for other in &board.nodes()[i + 1..] {
                assert!(
                    !one.rect.overlaps(&other.rect, 0.),
                    "{} overlaps {}",
                    one.title,
                    other.title
                );
            }
        }
        assert_eq!(board.edges().len(), 3);
        assert_eq!(board.node(c).unwrap().parent, Some(a));
        let edge = &board.edges()[0];
        assert_eq!(
            (edge.from_port.as_str(), edge.to_port.as_str(), edge.origin),
            ("out", "in", EdgeOrigin::Spawned)
        );
        let ids: HashSet<_> = board.edges().iter().map(|edge| edge.id).collect();
        assert_eq!(ids.len(), 3, "every edge has an id of its own");
    }

    #[test]
    fn a_grandchild_column_does_not_land_on_an_uncle() {
        let mut board = Board::new();
        let root = board.add_at(agent("lead"), 0., 0.);
        let a = board.add_child(root, agent("a")).unwrap();
        let far = board.add_child(a, agent("far")).unwrap();
        // A second child of the root, moved by the user into the grandchild column.
        let b = board.add_child(root, agent("b")).unwrap();
        let far_rect = board.node(far).unwrap().rect;
        board.move_to(b, far_rect.x, far_rect.bottom() + GAP_Y);
        let again = board.add_child(a, agent("again")).unwrap();
        let rect = board.node(again).unwrap().rect;
        assert!(rect.y >= board.node(b).unwrap().rect.bottom() + GAP_Y);
    }

    #[test]
    fn a_child_finds_room_below_a_node_resized_off_the_placement_grid() {
        let mut board = Board::new();
        let root = board.add_rect(agent("lead"), Rect::new(0., 0., 560., 380.));
        let first = board.add_child(root, agent("a")).unwrap();
        // Resized by hand to a size the placement grid does not divide.
        board.resize(first, 480., 404.);
        let second = board.add_child(root, agent("b")).unwrap();
        let (a, b) = (
            board.node(first).unwrap().rect,
            board.node(second).unwrap().rect,
        );
        assert!(b.y >= a.bottom() + GAP_Y - 0.5, "{a:?} {b:?}");
        assert_eq!(b.y % (GRID / 2.), 0.);
    }

    #[test]
    fn a_placed_node_moves_to_the_nearest_free_spot() {
        let mut board = Board::new();
        let first = board.add_at(agent("one"), 0., 0.);
        let second = board.add_at(agent("two"), 0., 0.);
        let (one, two) = (
            board.node(first).unwrap().rect,
            board.node(second).unwrap().rect,
        );
        assert!(!one.overlaps(&two, GAP_Y - 1.), "{one:?} {two:?}");
        // Near, not somewhere across the board.
        let (ax, ay) = one.center();
        let (bx, by) = two.center();
        assert!((ax - bx).abs() <= one.w + GAP_X && (ay - by).abs() <= one.h + GAP_X);
    }

    #[test]
    fn removing_a_node_drops_its_edges_and_orphans_its_children() {
        let mut board = Board::new();
        let root = board.add_at(agent("lead"), 0., 0.);
        let a = board.add_child(root, agent("a")).unwrap();
        let b = board.add_child(a, agent("b")).unwrap();
        assert!(board.link(root, b));
        assert!(!board.link(root, b), "the same edge twice");
        assert!(!board.link(root, root));
        assert_eq!(board.remove(a).unwrap().title, "a");
        assert_eq!(board.edges().len(), 1);
        assert_eq!((board.edges()[0].from, board.edges()[0].to), (root, b));
        assert_eq!(board.edges()[0].origin, EdgeOrigin::Wired);
        assert_eq!(board.node(b).unwrap().parent, None);
        assert!(board.remove(a).is_none());
    }

    #[test]
    fn ports_follow_the_kind_and_wiring_checks_them() {
        let mut board = Board::new();
        let shell = board.add_at(of(NodeKind::Terminal, "sh"), 0., 0.);
        let agent_id = board.add_at(agent("a"), 900., 0.);
        let note = board.add_at(of(NodeKind::Note, "n"), 0., 900.);
        let source = board.add_at(of(NodeKind::Source, "feed"), 900., 900.);
        assert_eq!(NodeKind::Terminal.outputs(), ["out", "exit"]);
        assert!(NodeKind::Note.inputs().is_empty() && NodeKind::Note.outputs().is_empty());
        assert!(NodeKind::Source.inputs().is_empty());

        let exit = board
            .connect(PortRef::new(shell, "exit"), PortRef::new(agent_id, "in"))
            .unwrap();
        assert_eq!(board.edge(exit).unwrap().from_port, "exit");
        assert_eq!(
            board.connect(PortRef::new(shell, "exit"), PortRef::new(agent_id, "in")),
            Err(WireError::Exists(exit))
        );
        assert_eq!(
            board.connect(PortRef::new(agent_id, "in"), PortRef::new(shell, "out")),
            Err(WireError::NoSuchPort),
            "an input is not an output"
        );
        assert_eq!(
            board.connect(PortRef::new(source, "out"), PortRef::new(note, "in")),
            Err(WireError::NoSuchPort),
            "a note takes nothing"
        );
        assert_eq!(
            board.connect(PortRef::new(agent_id, "out"), PortRef::new(agent_id, "in")),
            Err(WireError::SameNode)
        );
        assert_eq!(
            board.connect(PortRef::new(99, "out"), PortRef::new(agent_id, "in")),
            Err(WireError::Missing)
        );
        board
            .connect(PortRef::new(source, "out"), PortRef::new(shell, "in"))
            .unwrap();
        assert_eq!(board.routes_from(source, "out").count(), 1);

        // Ports sit under the header, a step apart, on their sides.
        let node = board.node(shell).unwrap();
        let out = node.port_point(Dir::Out, "out");
        let exit = node.port_point(Dir::Out, "exit");
        assert_eq!(out, (node.rect.right(), node.rect.y + PORT_FIRST));
        assert_eq!(exit.1 - out.1, PORT_STEP);
        assert_eq!(node.port_point(Dir::In, "in").0, node.rect.x);
    }

    #[test]
    fn modes_say_what_goes_along_an_edge_and_how() {
        let mut board = Board::new();
        let a = board.add_at(agent("lead"), 0., 0.);
        let b = board.add_at(of(NodeKind::Terminal, "sh"), 900., 0.);
        let edge = board
            .connect(PortRef::new(a, "out"), PortRef::new(b, "in"))
            .unwrap();
        let before = board
            .update_edge(edge, |edge| {
                edge.mode = EdgeMode::Approve;
                // Ends are not settings; this is ignored.
                edge.to = 77;
            })
            .unwrap();
        assert_eq!(before.mode, EdgeMode::Auto);
        let now = board.edge(edge).unwrap();
        assert_eq!((now.mode, now.to), (EdgeMode::Approve, b));
        assert!(now.carries());
        assert_eq!(now.delivery(NodeKind::Terminal), Delivery::Queue);
        assert_eq!(now.delivery(NodeKind::Agent), Delivery::Now);
        assert_eq!(now.message("ls", "lead", NodeKind::Terminal), "ls");
        assert_eq!(
            now.message("done", "lead", NodeKind::Agent),
            format!("[canvas] from «lead» ({a}):\ndone")
        );
        board.update_edge(edge, |edge| {
            edge.mode = EdgeMode::Off;
            edge.template = "review {{text}} for {{from.title}}{{nope}}".into();
            edge.header = Some(false);
        });
        let now = board.edge(edge).unwrap();
        assert!(!now.carries());
        assert_eq!(
            board.routes_from(a, "out").count(),
            0,
            "off edges route nothing"
        );
        assert_eq!(
            now.message("x", "lead", NodeKind::Agent),
            "review x for lead"
        );
        assert_eq!(EdgeMode::Off.next(), EdgeMode::Auto);
    }

    #[test]
    fn an_edge_end_moves_to_another_port_of_its_direction() {
        let mut board = Board::new();
        let a = board.add_at(agent("a"), 0., 0.);
        let b = board.add_at(agent("b"), 900., 0.);
        let c = board.add_at(agent("c"), 900., 900.);
        let edge = board
            .connect(PortRef::new(a, "out"), PortRef::new(b, "in"))
            .unwrap();
        let before = board
            .retarget(edge, End::To, PortRef::new(c, "in"))
            .unwrap();
        assert_eq!(before.to, b);
        assert_eq!(board.edge(edge).unwrap().to, c);
        assert_eq!(
            board.retarget(edge, End::To, PortRef::new(a, "in")),
            Err(WireError::SameNode)
        );
        assert_eq!(
            board.retarget(edge, End::From, PortRef::new(b, "in")),
            Err(WireError::NoSuchPort)
        );
        let other = board
            .connect(PortRef::new(b, "out"), PortRef::new(c, "in"))
            .unwrap();
        assert_eq!(
            board.retarget(edge, End::From, PortRef::new(b, "out")),
            Err(WireError::Exists(other))
        );
        assert_eq!(
            board.edge(edge).unwrap().from,
            a,
            "a refused move changes nothing"
        );
    }

    #[test]
    fn resizing_stays_within_the_limits() {
        let mut board = Board::new();
        let a = board.add_at(agent("a"), 0., 0.);
        assert_eq!(board.resize(a, 10., 10.), Some((MIN_W, MIN_H)));
        assert_eq!(board.resize(a, 9e9, 500.), Some((MAX_SIZE, 500.)));
        assert_eq!(board.resize(99, 300., 300.), None);
        assert_eq!(snap_to(13., SNAP), 16.);
        assert_eq!(snap_to(-13., SNAP), -16.);
        assert_eq!(snap_to(11.9, SNAP), 8.);
    }

    #[test]
    fn a_marquee_takes_what_it_touches() {
        let mut board = Board::new();
        let a = board.add_rect(agent("a"), Rect::new(0., 0., 200., 100.));
        let b = board.add_rect(agent("b"), Rect::new(400., 0., 200., 100.));
        let c = board.add_rect(agent("c"), Rect::new(0., 400., 200., 100.));
        let area = Rect::spanning((420., 90.), (150., 50.));
        assert_eq!(board.nodes_in(&area), vec![a, b]);
        assert!(board.nodes_in(&Rect::new(250., 200., 50., 50.)).is_empty());
        assert_eq!(board.reading_order(), vec![a, b, c]);
        assert_eq!(
            board.bounds_of([a, c]).unwrap(),
            Rect::new(0., 0., 200., 500.)
        );
    }

    #[test]
    fn a_wire_dropped_on_the_board_makes_a_wired_node_there() {
        let mut board = Board::new();
        let a = board.add_rect(agent("a"), Rect::new(0., 0., 400., 300.));
        let (to, edge) = board.add_wired(
            agent("next"),
            (600., 200.),
            &Wire::From(PortRef::new(a, "out")),
        );
        let node = board.node(to).unwrap();
        // The node sits on the grid; its port is as near the point as that allows.
        let port = node.port_point(Dir::In, "in");
        assert_eq!(port.0, 600.);
        assert!((port.1 - 200.).abs() <= SNAP / 2.);
        let edge = board.edge(edge.unwrap()).unwrap();
        assert_eq!(
            (edge.from, edge.to, edge.origin),
            (a, to, EdgeOrigin::Wired)
        );

        let (from, edge) = board.add_wired(
            agent("before"),
            (-200., 200.),
            &Wire::To(PortRef::new(a, "in")),
        );
        let port = board.node(from).unwrap().port_point(Dir::Out, "out");
        assert!((port.0 + 200.).abs() < 1e-3 && (port.1 - 200.).abs() <= SNAP / 2.);
        assert_eq!(board.edge(edge.unwrap()).unwrap().to, a);

        let (_, none) = board.add_wired(
            of(NodeKind::Note, "n"),
            (0., 900.),
            &Wire::From(PortRef::new(a, "out")),
        );
        assert_eq!(none, None, "a note has no input");
    }

    #[test]
    fn duplicates_land_beside_their_originals() {
        let mut board = Board::new();
        let a = board.add_rect(of(NodeKind::Note, "plan"), Rect::new(0., 0., 280., 180.));
        board.node_mut(a).unwrap().detail = "1. schema".into();
        let pairs = board.duplicate(&[a, 99]);
        assert_eq!(pairs.len(), 1);
        let copy = board.node(pairs[0].1).unwrap();
        assert_eq!(copy.title, "plan copy");
        assert_eq!(copy.detail, "1. schema");
        assert_eq!(
            (copy.rect.x, copy.rect.y),
            (DUPLICATE_OFFSET, DUPLICATE_OFFSET)
        );
    }

    #[test]
    fn restoring_brings_back_places_titles_notes_and_edges_but_not_panes() {
        let mut board = Board::new();
        let a = board.add_rect(agent("a"), Rect::new(0., 0., 300., 200.));
        let b = board.add_rect(agent("b"), Rect::new(600., 0., 300., 200.));
        let gone = board.add_rect(agent("gone"), Rect::new(0., 600., 300., 200.));
        let note = board.add_rect(of(NodeKind::Note, "n"), Rect::new(600., 600., 300., 200.));
        let edge = board
            .connect(PortRef::new(a, "out"), PortRef::new(b, "in"))
            .unwrap();
        board
            .connect(PortRef::new(gone, "out"), PortRef::new(b, "in"))
            .unwrap();
        let earlier = board.clone();

        board.move_to(a, 64., 64.);
        board.node_mut(b).unwrap().title = "renamed".into();
        board.remove(note);
        board.remove(gone);
        board.disconnect(edge);
        let fresh = board.add_rect(agent("fresh"), Rect::new(0., 900., 300., 200.));
        let added = board
            .connect(PortRef::new(b, "out"), PortRef::new(fresh, "in"))
            .unwrap();

        let changes = board.restore(&earlier);
        assert_eq!(board.node(a).unwrap().rect.x, 0.);
        assert_eq!(board.node(b).unwrap().title, "b");
        assert!(board.node(note).is_some(), "a note comes back");
        assert!(
            board.node(gone).is_none(),
            "a pane that closed stays closed"
        );
        assert!(board.node(fresh).is_some(), "a pane that opened stays open");
        assert!(board.edge(edge).is_some());
        assert!(board.edge(added).is_none());
        assert!(changes.contains(&Change::Renamed {
            node: b,
            title: "b".into()
        }));
        assert!(changes.contains(&Change::NoteAdded(note)));
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::EdgeAdded(e) if e.id == edge))
        );
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::EdgeRemoved(e) if e.id == added))
        );
        assert_eq!(
            board.edges().len(),
            1,
            "the edge from the closed pane stays gone"
        );
    }

    #[test]
    fn arrow_keys_find_the_node_in_that_direction() {
        let mut board = Board::new();
        let root = board.add_at(agent("lead"), 0., 0.);
        let a = board.add_child(root, agent("a")).unwrap();
        let b = board.add_child(root, agent("b")).unwrap();
        assert_eq!(board.neighbour(root, 1., 0.), Some(a));
        assert_eq!(board.neighbour(a, 0., 1.), Some(b));
        assert_eq!(board.neighbour(b, 0., -1.), Some(a));
        assert_eq!(board.neighbour(a, -1., 0.), Some(root));
        assert_eq!(board.neighbour(root, -1., 0.), None);
    }

    #[test]
    fn a_board_survives_a_round_trip_through_json() {
        let mut board = Board::new();
        let root = board.add_at(agent("lead"), 0., 0.);
        let child = board.add_child(root, agent("a")).unwrap();
        board.node_mut(child).unwrap().place = "~/project · main".into();
        let edge = board.edges()[0].id;
        board.update_edge(edge, |edge| {
            edge.mode = EdgeMode::Approve;
            edge.deliver = Some(Delivery::Queue);
            edge.template = "{{text}}".into();
        });
        let json = serde_json::to_string(&board).unwrap();
        let mut back: Board = serde_json::from_str(&json).unwrap();
        assert_eq!(back, board);
        // Ids keep counting from where they were.
        let next = back.add_at(agent("new"), 900., 900.);
        assert!(board.node(next).is_none());
        let wired = back
            .connect(PortRef::new(root, "out"), PortRef::new(next, "in"))
            .unwrap();
        assert!(board.edge(wired).is_none());
    }

    #[test]
    fn a_board_saved_before_ports_and_modes_still_loads() {
        // As the first version wrote it: edges as bare pairs, no edge ids, no places.
        let old = r#"{
            "nodes": [
                {"id": 1, "kind": "agent", "title": "lead", "tag": "CC", "detail": "~/p",
                 "rect": {"x": 0, "y": 0, "w": 320, "h": 192}, "parent": null},
                {"id": 2, "kind": "agent", "title": "tests", "tag": "CX", "detail": "",
                 "rect": {"x": 416, "y": 0, "w": 320, "h": 192}, "parent": 1},
                {"id": 3, "kind": "note", "title": "plan", "tag": "", "detail": "1. a",
                 "rect": {"x": -400, "y": 0, "w": 224, "h": 128}, "parent": null}
            ],
            "edges": [{"from": 1, "to": 2}, {"from": 1, "to": 2}, {"from": 1, "to": 9}],
            "next": 3
        }"#;
        let mut board: Board = serde_json::from_str(old).unwrap();
        assert_eq!(board.nodes().len(), 3);
        assert_eq!(
            board.edges().len(),
            1,
            "the repeat and the dangling edge go"
        );
        let edge = &board.edges()[0];
        assert_ne!(edge.id, 0);
        assert_eq!(
            (
                edge.from_port.as_str(),
                edge.to_port.as_str(),
                edge.mode,
                edge.origin
            ),
            ("out", "in", EdgeMode::Auto, EdgeOrigin::Spawned)
        );
        assert_eq!(board.node(1).unwrap().place, "");
        let next = board.add_at(agent("new"), 900., 900.);
        assert_eq!(next, 4);
        let wired = board
            .connect(PortRef::new(1, "out"), PortRef::new(next, "in"))
            .unwrap();
        assert_ne!(wired, board.edges()[0].id);
        // And an empty object is an empty board.
        let empty: Board = serde_json::from_str("{}").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn a_ring_of_wires_is_found_before_it_is_closed() {
        let mut board = Board::new();
        let a = board.add_at(agent("a"), 0., 0.);
        let b = board.add_at(agent("b"), 900., 0.);
        let c = board.add_at(agent("c"), 1800., 0.);
        assert!(board.link(a, b) && board.link(b, c));
        // What c answers would come round to c: through a and b.
        assert_eq!(board.would_ring(c, a), Some(vec![a, b, c]));
        assert_eq!(board.would_ring(b, a), Some(vec![a, b]));
        // Nothing comes back along a chain, and nothing wires to itself.
        assert_eq!(board.would_ring(a, c), None);
        assert_eq!(board.would_ring(a, a), Some(vec![a]));
        // An edge that is off, and one that only shows who started or
        // wrote to whom, carry nothing round.
        let first = board.edges()[0].id;
        board.update_edge(first, |edge| edge.mode = EdgeMode::Off);
        assert_eq!(board.would_ring(c, a), None);
        board.update_edge(first, |edge| {
            edge.mode = EdgeMode::Auto;
            edge.origin = EdgeOrigin::Sent;
        });
        assert_eq!(board.would_ring(c, a), None);
        assert!(!EdgeOrigin::Sent.pipes() && !EdgeOrigin::Spawned.pipes());
        assert!(EdgeOrigin::Wired.pipes());
    }

    #[test]
    fn a_message_passed_on_says_how_many_times_and_who_made_a_wire_is_kept() {
        let mut board = Board::new();
        let a = board.add_at(agent("lead"), 0., 0.);
        let b = board.add_at(agent("worker"), 900., 0.);
        let id = board
            .connect(PortRef::new(a, "out"), PortRef::new(b, "in"))
            .unwrap();
        let edge = board.edge(id).unwrap().clone();
        // The first time it is as it always was; from the second on the
        // line counts.
        assert_eq!(
            edge.message_at("done", "lead", NodeKind::Agent, 1),
            edge.message("done", "lead", NodeKind::Agent)
        );
        assert_eq!(
            edge.message_at("done", "lead", NodeKind::Agent, 3),
            format!("[canvas] from «lead» ({a}), hop 3 of {HOP_LIMIT}:\ndone")
        );
        assert_eq!(
            canvas_header("answer from", "worker", "p7", 1),
            "[canvas] answer from «worker» (p7):"
        );
        // Who wired it, and who may: kept with the board, and absent from
        // a board that has neither, which reads as before.
        let plain = serde_json::to_string(&board).unwrap();
        assert!(
            !plain.contains("orchestrator") && !plain.contains("\"by\""),
            "{plain}"
        );
        board.update_edge(id, |edge| edge.by = Some(a));
        board.node_mut(a).unwrap().orchestrator = true;
        let saved = serde_json::to_string(&board).unwrap();
        let back: Board = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.edge(id).unwrap().by, Some(a));
        assert!(back.node(a).unwrap().orchestrator && !back.node(b).unwrap().orchestrator);
        let old: Board = serde_json::from_str(&plain).unwrap();
        assert_eq!(old.edge(id).unwrap().by, None);
    }
}
