//! The canvas's keys, the web canvas's where they fit a native window. The
//! board has them only while it has the focus itself: a node's body, a field
//! or a menu keeps its own. Tab is never taken: it leaves the board for the
//! next control, as everywhere else in the app.
//!
//! | keys | what |
//! |---|---|
//! | arrows | the nearest node that way |
//! | Alt+arrows, Shift+Alt+arrows | move the selection by one snap, by four |
//! | Enter | enter the node (its body takes the keys); an edge: inspect it |
//! | Cmd+Esc | the keys back to the board, from a node's body |
//! | A, N | add a node where the pointer is, or in the middle |
//! | F | the node at 100% in the middle |
//! | F2 | rename the node |
//! | I | inspect what is selected |
//! | M | the selected edge's next mode |
//! | Delete, Backspace | delete what is selected |
//! | Cmd+D | duplicate |
//! | Cmd+A | select every node |
//! | Cmd+Z, Cmd+Shift+Z | undo, redo |
//! | Cmd+0, Cmd+1, Cmd+=, Cmd+- (and 0, =, -) | fit, 100%, zoom in, zoom out |
//! | Shift+F10, the context key | the menu of what is selected |
//! | Space held | a drag pans |
//! | Escape | close the menu, then the inspector, then clear the selection |

use crate::{
    field::Outcome,
    model::SNAP,
    view::{CanvasView, Edit},
};
use gpui::{ClipboardItem, Context, KeyDownEvent, KeyUpEvent, Window};

impl CanvasView {
    pub(crate) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
        let command = m.platform || m.control;
        let key = keystroke.key.as_str();
        if command {
            match (key, m.shift) {
                ("0", _) => self.fit(cx),
                ("1", _) => self.zoom_100(cx),
                ("=" | "+", _) => self.zoom_step(1, cx),
                ("-" | "_", _) => self.zoom_step(-1, cx),
                ("a", _) => {
                    let all = self.board.nodes().iter().map(|node| node.id).collect();
                    self.set_selection(all, None, cx);
                }
                ("d", _) => self.duplicate_selection(window, cx),
                ("z", false) => {
                    if !self.undo(cx) {
                        self.show_toast("Nothing to undo", false, window, cx);
                    }
                }
                ("z", true) | ("y", _) => {
                    if !self.redo(cx) {
                        self.show_toast("Nothing to do again", false, window, cx);
                    }
                }
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        if m.alt {
            let step = if m.shift { SNAP * 4. } else { SNAP };
            let (dx, dy) = match key {
                "left" => (-step, 0.),
                "right" => (step, 0.),
                "up" => (0., -step),
                "down" => (0., step),
                _ => return,
            };
            self.move_selection(dx, dy, cx);
            cx.stop_propagation();
            return;
        }
        match key {
            "left" => self.walk(-1., 0., cx),
            "right" => self.walk(1., 0., cx),
            "up" => self.walk(0., -1., cx),
            "down" => self.walk(0., 1., cx),
            "enter" => {
                if let Some(id) = self.selected() {
                    self.enter(id, window, cx);
                } else if let Some(edge) = self.edge_selected {
                    self.open_inspector_for_edge(edge, cx);
                }
            }
            "backspace" | "delete" => {
                if let Some(edge) = self.edge_selected {
                    self.delete_edge(edge, window, cx);
                } else if !self.selection.is_empty() {
                    let ids = self.selection.clone();
                    self.ask_to_remove(&ids, window, cx);
                } else {
                    return;
                }
            }
            "escape" => {
                if matches!(self.gesture, Some(crate::view::Gesture::Wire(_))) {
                    // A wire being drawn is let go of, wired to nothing.
                    self.gesture = None;
                    cx.notify();
                } else if self
                    .menu
                    .as_ref()
                    .is_some_and(|menu| menu.closing.is_none())
                {
                    self.close_menu(cx);
                } else if self.inspector.is_some() {
                    self.close_inspector(cx);
                } else if !self.selection.is_empty() || self.edge_selected.is_some() {
                    self.set_selection(Vec::new(), None, cx);
                } else {
                    return;
                }
            }
            "a" | "n" => self.open_add_menu_here(window, cx),
            "f" => {
                if let Some(id) = self.selected() {
                    self.focus_node(id, cx);
                } else {
                    return;
                }
            }
            "f2" => {
                if let Some(id) = self.selected() {
                    self.start_edit(Edit::Title(id, false), window, cx);
                } else {
                    return;
                }
            }
            "i" => self.toggle_inspector(cx),
            "m" => {
                let Some(edge) = self.edge_selected else {
                    return;
                };
                let next = self.board.edge(edge).map(|e| e.mode.next());
                if let Some(mode) = next {
                    self.set_edge_mode(edge, mode, cx);
                }
            }
            "f10" if m.shift => self.menu_for_selection(window, cx),
            "menu" => self.menu_for_selection(window, cx),
            "0" => self.fit(cx),
            "=" | "+" => self.zoom_step(1, cx),
            "-" => self.zoom_step(-1, cx),
            "space" => {
                self.space_held = true;
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    pub(crate) fn key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "space" && self.space_held {
            self.space_held = false;
            cx.notify();
        }
    }

    /// Cmd+Esc takes the keys back from a node, whatever the node does with Esc.
    pub(crate) fn capture_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.platform || keystroke.modifiers.control;
        if keystroke.key == "escape" && command && self.entered.is_some() {
            self.exit(window, cx);
            cx.stop_propagation();
        }
    }

    fn walk(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        let from = self.selection.last().copied().or_else(|| {
            self.edge_selected
                .and_then(|edge| self.board.edge(edge))
                .map(|edge| if dx < 0. { edge.from } else { edge.to })
        });
        let next = match from {
            Some(id) if self.board.node(id).is_some() => self.board.neighbour(id, dx, dy),
            _ => self.board.reading_order().first().copied(),
        };
        if let Some(next) = next {
            self.set_selection(vec![next], None, cx);
            self.reveal(next, cx);
        }
    }

    /// Move the selected nodes by a distance in world units, as Alt and the arrows do.
    pub fn move_selection(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        self.remember();
        let places: Vec<_> = self
            .selection
            .iter()
            .filter_map(|id| {
                self.board
                    .node(*id)
                    .map(|node| (*id, node.rect.x + dx, node.rect.y + dy))
            })
            .collect();
        self.board.place_all(&places);
        cx.emit(crate::CanvasEvent::Changed);
        cx.notify();
    }

    /// Ask what to add, where the pointer is or in the middle: what A does.
    pub fn open_add_menu_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let local = self.pointer.unwrap_or_else(|| {
            let (w, h) = self.viewport();
            (w / 2., h / 2.)
        });
        let world = self.camera.to_world(local.0, local.1);
        self.open_add_menu(local, world, None, window, cx);
    }

    fn menu_for_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.selection.last().copied()
            && let Some(node) = self.board.node(id)
        {
            let (x, y) = self
                .camera
                .to_screen(node.rect.right() - 8., node.rect.y + 30.);
            self.open_node_menu(id, (x, y), window, cx);
        } else if let Some(edge) = self.edge_selected
            && let Some(curve) = self
                .board
                .edge(edge)
                .and_then(|e| crate::geometry::edge_curve(&self.board, e))
        {
            let (x, y) = curve.middle();
            let at = self.camera.to_screen(x, y);
            self.open_edge_menu(edge, at, window, cx);
        } else {
            self.open_add_menu_here(window, cx);
        }
    }

    /// Keys typed into the field being edited.
    pub(crate) fn field_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(editing) = self.editing.as_mut() else {
            return;
        };
        let outcome = editing.field.key(&event.keystroke, clipboard);
        match outcome {
            Outcome::Commit => {
                self.commit_edit(cx);
                window.focus(&self.focus, cx);
            }
            Outcome::Cancel => {
                self.editing = None;
                window.focus(&self.focus, cx);
            }
            Outcome::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Outcome::Edited | Outcome::Moved => {}
            // Keys the field does not use, Tab among them, go on.
            Outcome::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn start_edit(&mut self, what: Edit, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(cx);
        let text = match &what {
            Edit::Title(id, _) => self.board.node(*id).map(|node| node.title.clone()),
            Edit::Note(id) => self.board.node(*id).map(|node| node.detail.clone()),
            Edit::Template(id) => self.board.edge(*id).map(|edge| edge.template.clone()),
        };
        let Some(text) = text else {
            return;
        };
        let multiline = matches!(what, Edit::Note(_));
        let mut field = crate::field::Field::new(&text, multiline);
        if multiline {
            // A note is added to more often than rewritten: the caret goes to its end.
            field.key(&gpui::Keystroke::parse("cmd-a").unwrap_or_default(), None);
            field.key(
                &gpui::Keystroke::parse("cmd-right").unwrap_or_default(),
                None,
            );
        }
        self.editing = Some(crate::view::Editing {
            what,
            field,
            focused: false,
        });
        window.focus(&self.field_focus, cx);
        cx.notify();
    }

    /// Keep what the field holds: a title, a note, a template.
    pub(crate) fn commit_edit(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        let text = editing.field.text().to_owned();
        match editing.what {
            Edit::Title(id, _) => self.rename(id, &text, cx),
            Edit::Note(id) => {
                if self.board.node(id).is_some_and(|node| node.detail != text) {
                    self.remember();
                    if let Some(node) = self.board.node_mut(id) {
                        node.detail = text;
                    }
                    cx.emit(crate::CanvasEvent::Changed);
                }
            }
            Edit::Template(id) => self.change_edge(id, |edge| edge.template = text, cx),
        }
        cx.notify();
    }
}
