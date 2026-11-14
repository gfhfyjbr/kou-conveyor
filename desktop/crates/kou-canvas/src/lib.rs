//! The canvas: agents, terminals, notes and sources as nodes; ports on their
//! sides wired output to input by edges whose modes say what goes along
//! them; pan and zoom, with every node a placeholder of its name once its
//! text could no longer be read; selection, menus, an inspector. Built after
//! kou-conveyor's web canvas (`cmd/kou-conveyor-web/plugins/canvas/web`).

mod anim;
mod card;
pub mod catalog;
pub mod field;
mod geometry;
mod inspector;
mod keys;
mod menu;
mod model;
mod paint;
mod pointer;
mod render;
#[cfg(test)]
mod tests;
mod view;

pub use card::field_lines;
pub use catalog::{AddRequest, Offer, Template, TemplateEdge, TemplateNode, TemplateRequest};
pub use geometry::{Camera, Cubic, Lod, ZOOM_STEPS, curve, edge_curve};
pub use model::{
    Board, Change, Delivery, Dir, Edge, EdgeId, EdgeMode, EdgeOrigin, End, GAP_X, GAP_Y, GRID,
    MAX_SIZE, MIN_H, MIN_W, Node, NodeId, NodeKind, NodeSpec, PortRef, Rect, SNAP, Wire, WireError,
    port_at, snap_to,
};
pub use view::{BoardEvent, CanvasEvent, CanvasView, Pending, Traffic};
