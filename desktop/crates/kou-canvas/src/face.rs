//! Parts of the canvas's chrome an app can draw itself: the bar in the
//! board's corner, what an empty board offers, the open menu, the bar that
//! takes a deletion back, the inspector, and each node's header and footer.
//! The canvas hands over what such a part shows, as data, with the parts
//! that stay its own to draw, and does what the drawing asks of it by name;
//! the app gives back an element, or nothing, and then the canvas draws the
//! part as it always did.
//!
//! No part's data follows the camera: a drawing is made again only when
//! what it shows changes, not at each frame of a pan or a zoom. What does
//! move with the frame (the menu's highlight as it travels, the menu, the
//! inspector and the bar fading in and out, the zoom's percentage) stays
//! the canvas's own, around or in the drawing. A node's parts are drawn at
//! the camera's zoom from a drawing made at 1, and each carries a stamp
//! that changes only when what it shows does: from one frame to the next
//! the app looks its drawing up by it, and nothing is put into words again.

use crate::{
    menu::Action,
    model::{Delivery, EdgeMode, End, NodeId, NodeKind},
    view::{CanvasView, Edit},
};
use gpui::{AnyElement, App, Context, IntoElement as _, SharedString, Window};
use kou_ui::Mark;
use serde_json::{Value, json};
use std::rc::Rc;

/// Does what a drawing asks of the canvas: an act's name, and what it is
/// given.
pub type FaceAct = Rc<dyn Fn(&str, &Value, &mut Window, &mut App)>;

/// A part of the canvas's chrome, for the app to draw.
pub struct FaceParts {
    /// Which part: `canvasBar`, `canvasEmpty`, `canvasMenu`, `canvasToast`,
    /// `canvasInspector`; for a node, `canvasHead`, `canvasBody`,
    /// `canvasFoot` and `canvasPending`; for an edge, `canvasLabel`.
    pub part: &'static str,
    /// Which canvas's it is, the same for the canvas's life.
    pub key: SharedString,
    /// Which of the canvas's things it is drawn for (a node's id), or
    /// nothing for a part the canvas has one of.
    pub instance: SharedString,
    /// The same number for as long as what the part shows stays the same
    /// (a node's parts have one): what was drawn for it can be drawn again
    /// without looking at `data`.
    pub stamp: Option<u64>,
    /// What it shows.
    pub data: Rc<Value>,
    /// The parts that stay the canvas's own to draw, by name.
    pub slots: Vec<(&'static str, AnyElement)>,
    /// Does what the drawing asks: `add`, `zoomIn`, `zoomOut`, `zoomMenu`
    /// and `fit` (the bar); `template` (`{ index }`), `agent` and `note`
    /// (an empty board's offers); `run`, `hover` (`{ index }`) and
    /// `close` (the menu); `undo` (the bar after a deletion); the
    /// inspector's (see `canvasInspector` in the app's `canvas` plugin);
    /// `nodeMenu` (`{ node }`, a node's header); `pendingOpened`
    /// (`{ node }`, the tab over a node with what waits for it).
    pub act: FaceAct,
}

/// Draws a part of the canvas's chrome from its parts, in place of the
/// canvas's own. `None` leaves it to the canvas, which then draws it as it
/// always did.
pub type Face = Rc<dyn Fn(FaceParts, &App) -> Option<AnyElement>>;

/// What one of a node's parts shows: while it stays the same, its drawing
/// is the one made before.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Shows {
    // (Its name and its mark are the canvas's own: not shown by the drawing.)
    Head {
        kind: NodeKind,
        mark: Mark,
        elapsed: Option<String>,
        reachable: bool,
        editing: bool,
    },
    Foot {
        place: String,
        note: bool,
    },
    /// What a node with no live body says: its detail, or a note's text.
    Body {
        note: bool,
        detail: String,
    },
    /// The tab over a node with what waits for it.
    Pending {
        text: String,
        approve: bool,
    },
    /// An edge's label.
    Label {
        text: String,
        waiting: bool,
        selected: bool,
    },
}

/// What a node's part last showed, the stamp it was given, and its data.
pub(crate) struct Shown {
    shows: Shows,
    stamp: u64,
    data: Rc<Value>,
}

/// An icon as a drawing names it: by its file's name (`chevron-right`).
pub(crate) fn icon_name(icon: kou_ui::Icon) -> &'static str {
    icon.path()
        .trim_start_matches("icons/")
        .trim_end_matches(".svg")
}

/// A node's mark as a drawing is given it: its harness's code, or its
/// kind's icon (a note's in the note's colour).
pub(crate) fn mark_of(tag: &str, kind: NodeKind) -> Value {
    if kind == NodeKind::Agent && !tag.is_empty() {
        json!({ "code": tag })
    } else {
        json!({
            "icon": icon_name(crate::card::kind_icon(kind)),
            "note": kind == NodeKind::Note,
        })
    }
}

impl CanvasView {
    /// Gives the drawing of some of the canvas's chrome to the app (see
    /// [`Face`]); `None` gives it back to the canvas.
    pub fn set_face(&mut self, face: Option<Face>, cx: &mut Context<Self>) {
        self.face = face;
        cx.notify();
    }

    /// What the drawing of a part asks of this canvas.
    fn face_act_of(&self, cx: &Context<Self>) -> FaceAct {
        let view = cx.weak_entity();
        Rc::new(move |name, given, window, cx| {
            view.update(cx, |this, cx| this.face_act(name, given, window, cx))
                .ok();
        })
    }

    fn face_key(&self, cx: &Context<Self>) -> SharedString {
        SharedString::from(format!("canvas-{}", cx.entity_id().as_u64()))
    }

    /// `part` as the app draws it, when it does.
    pub(crate) fn faced(
        &self,
        part: &'static str,
        data: Value,
        slots: impl FnOnce() -> Vec<(&'static str, AnyElement)>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let face = self.face.clone()?;
        face(
            FaceParts {
                part,
                key: self.face_key(cx),
                instance: SharedString::default(),
                stamp: None,
                data: Rc::new(data),
                slots: slots(),
                act: self.face_act_of(cx),
            },
            cx,
        )
    }

    /// `part` of node `id` (or of edge `id`, for its label) as the app
    /// draws it, when it does, at the camera's `zoom`: from the drawing made
    /// for what it `shows`, and `data` worked out only when that changed
    /// since the last frame.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn node_faced(
        &self,
        id: NodeId,
        part: &'static str,
        shows: Shows,
        data: impl FnOnce() -> Value,
        slots: impl FnOnce() -> Vec<(&'static str, AnyElement)>,
        zoom: f32,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let face = self.face.clone()?;
        let (stamp, data) = {
            let mut shown = self.shown.borrow_mut();
            match shown.get(&(id, part)) {
                Some(known) if known.shows == shows => (known.stamp, known.data.clone()),
                _ => {
                    let stamp = self.stamps.get() + 1;
                    self.stamps.set(stamp);
                    let data = Rc::new(data());
                    // (What nodes and edges that are gone showed is let go.)
                    let things = self.board.nodes().len() * 2 + self.board.edges().len();
                    if shown.len() > things + 16 {
                        shown.retain(|(id, part), _| match *part {
                            "canvasLabel" => self.board.edge(*id).is_some(),
                            _ => self.board.node(*id).is_some(),
                        });
                    }
                    shown.insert(
                        (id, part),
                        Shown {
                            shows,
                            stamp,
                            data: data.clone(),
                        },
                    );
                    (stamp, data)
                }
            }
        };
        let parts = FaceParts {
            part,
            key: self.face_key(cx),
            instance: SharedString::from(id.to_string()),
            stamp: Some(stamp),
            data,
            slots: slots(),
            act: self.face_act_of(cx),
        };
        // Its sizes, and the text in it, at the camera's zoom.
        let drawn = kou_ui::scale::with_factor(zoom, || face(parts, cx))?;
        Some(
            kou_ui::scale::scaled(zoom, drawn)
                .inherit_rem()
                .into_any_element(),
        )
    }

    /// What a drawing of the canvas's chrome asked for.
    fn face_act(&mut self, name: &str, given: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let index = given
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize);
        match name {
            "add" => self.add_from_bar(window, cx),
            "zoomIn" => self.zoom_step(1, cx),
            "zoomOut" => self.zoom_step(-1, cx),
            "zoomMenu" => self.zoom_menu_from_bar(window, cx),
            "fit" => self.fit(cx),
            "template" => {
                if let Some(index) = index {
                    self.offer_from_empty(Action::Template(index), window, cx);
                }
            }
            "agent" => self.offer_from_empty(Action::StartAgent, window, cx),
            "note" => self.offer_from_empty(Action::Note, window, cx),
            "run" => {
                let item = index.and_then(|index| {
                    self.menu
                        .as_ref()?
                        .items
                        .get(index)
                        .filter(|item| item.enabled)
                        .cloned()
                });
                if let Some(item) = item {
                    self.run(item.action, window, cx);
                }
            }
            "hover" => {
                if let Some(index) = index {
                    self.hover_row(index, cx);
                }
            }
            "close" => self.close_menu(cx),
            "pendingOpened" => {
                if let Some(id) = given.get("node").and_then(Value::as_u64) {
                    cx.emit(crate::BoardEvent::PendingOpened(id));
                }
            }
            "nodeMenu" => {
                let id = given.get("node").and_then(Value::as_u64);
                let Some((id, rect)) = id.and_then(|id| Some((id, self.board.node(id)?.rect)))
                else {
                    return;
                };
                let (x, y) = self
                    .camera
                    .to_screen(rect.right() - 4., rect.y + crate::card::HEAD);
                self.open_node_menu(id, (x - 220., y + 2.), window, cx);
            }
            "undo" => {
                self.toast = None;
                self.undo(cx);
            }
            _ => self.inspector_act(name, given, window, cx),
        }
    }

    /// What the inspector's drawing asked for.
    fn inspector_act(
        &mut self,
        name: &str,
        given: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let number = |key: &str| given.get(key).and_then(Value::as_u64);
        let (node, edge) = (number("node"), number("edge"));
        let index = number("index").map(|index| index as usize);
        match (name, node, edge) {
            ("inspectorClose", ..) => {
                self.close_inspector(cx);
                window.focus(&self.focus, cx);
            }
            ("edit", ..) => {
                let what = match (given.get("what").and_then(Value::as_str), node, edge) {
                    (Some("title"), Some(id), _) => Edit::Title(id, true),
                    (Some("note"), Some(id), _) => Edit::Note(id),
                    (Some("template"), _, Some(id)) => Edit::Template(id),
                    _ => return,
                };
                self.start_edit(what, window, cx);
            }
            ("pickNode", Some(id), _) => self.set_selection(vec![id], None, cx),
            ("pickEdge", _, Some(id)) => self.set_selection(Vec::new(), Some(id), cx),
            ("wireFrom", Some(id), _) => self.run(Action::WireFrom(id), window, cx),
            ("orchestrator", Some(id), _) => {
                let on = given.get("on").and_then(Value::as_bool) == Some(true);
                self.set_orchestrator(id, on, cx);
            }
            ("enter", Some(id), _) => self.enter(id, window, cx),
            ("open", Some(id), _) => cx.emit(crate::CanvasEvent::Opened(id)),
            ("focusNode", Some(id), _) => self.focus_node(id, cx),
            ("duplicate", Some(id), _) => {
                self.set_selection(vec![id], None, cx);
                self.duplicate_selection(window, cx);
            }
            ("delete", Some(id), _) => self.ask_to_remove(&[id], window, cx),
            ("zoomSelection", ..) => self.zoom_to_selection(cx),
            ("deleteSelection", ..) => {
                let ids = self.selection.clone();
                self.ask_to_remove(&ids, window, cx);
            }
            ("edgeEnds", _, Some(id)) => {
                let end = match given.get("end").and_then(Value::as_str) {
                    Some("to") => End::To,
                    _ => End::From,
                };
                self.run(Action::EdgeEnds(id, end), window, cx);
            }
            ("edgeMode", _, Some(id)) => {
                if let Some(mode) = index.and_then(|index| EdgeMode::ALL.get(index)) {
                    self.set_edge_mode(id, *mode, cx);
                }
            }
            ("edgeDelivery", _, Some(id)) => {
                let chosen = match index {
                    Some(1) => Delivery::Now,
                    _ => Delivery::Queue,
                };
                self.change_edge(id, |edge| edge.deliver = Some(chosen), cx);
            }
            ("edgeHeader", _, Some(id)) => {
                let chosen = match index {
                    Some(1) => Some(true),
                    Some(2) => Some(false),
                    _ => None,
                };
                self.change_edge(id, |edge| edge.header = chosen, cx);
            }
            ("deleteEdge", _, Some(id)) => self.delete_edge(id, window, cx),
            _ => {}
        }
    }

    /// A node's mark, by its id, as a drawing is given it.
    pub(crate) fn mark_of_node(&self, id: NodeId) -> Option<Value> {
        self.board
            .node(id)
            .map(|node| mark_of(&node.tag, node.kind))
    }
}
