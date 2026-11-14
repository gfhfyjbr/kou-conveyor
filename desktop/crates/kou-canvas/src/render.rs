//! The frame the canvas draws: the board (grid, edges, labels, nodes), what
//! floats over it (the toolbar, the inspector, the undo bar, a menu), and the
//! listeners that turn what the user does into the canvas's actions.

use crate::{
    geometry::Lod,
    paint::Grid,
    view::{CanvasView, GLIDE, Gesture},
};
use gpui::{
    Context, CursorStyle, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
    px,
};
use kou_ui::{metric, motion, theme};

/// How far out of the canvas, in pixels, a node is still built: room for
/// what hangs off it (its ports' labels, the tab over it).
const OFF_SCREEN: f32 = 96.;

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.clock.set_reduced(cx.reduce_motion());
        let measured = self.viewport().0 > 0.;
        if measured && self.fit_pending {
            self.fit_pending = false;
            self.camera = self.first_camera();
        }
        let mut animating = false;
        if let Some(glide) = &self.glide {
            let t = self.clock.progress(glide.start, GLIDE);
            self.camera = glide
                .from
                .glide(&glide.to, motion::ease_in_out(t), self.viewport());
            if t >= 1. {
                self.glide = None;
                cx.emit(crate::CanvasEvent::Changed);
            } else {
                animating = true;
            }
        }
        if let Some(wheel) = &mut self.wheel {
            if wheel.advance(&mut self.camera, self.clock.now()) {
                animating = true;
            } else {
                self.wheel = None;
            }
        }
        // The camera of this frame is settled: the bodies laid out below read
        // its zoom while they draw, in this same frame.
        self.zoom.set(self.camera.zoom);
        self.follow_keys(window, cx);
        // A field left by a click elsewhere keeps what it holds, as on the web.
        if let Some(editing) = &mut self.editing {
            if self.field_focus.is_focused(window) {
                editing.focused = true;
            } else if editing.focused {
                self.commit_edit(cx);
            }
        }
        animating |= self.settle_motion();
        // Unmeasured, the next frame is needed even when nothing animates.
        if !measured || (animating && !self.clock.is_held()) {
            window.request_animation_frame();
        }

        let grid = Grid::at(&self.camera, theme(cx));
        if *self.grid.read(cx) != grid {
            self.grid.update(cx, |old, cx| {
                *old = grid;
                cx.notify();
            });
        }
        let theme = theme(cx);
        let scene = self.scene(theme);
        let empty = self.board.is_empty();
        // What is out of sight is not built at all, so a board of many live
        // nodes costs what is on screen; the node with the keys is kept, so
        // its body keeps them while it is panned away.
        let visible = self.camera.visible(self.viewport());
        let reach = OFF_SCREEN / self.camera.zoom;
        // The selection paints over the rest, as the web canvas raises it.
        let mut order: Vec<&crate::model::Node> = self
            .board
            .nodes()
            .iter()
            .filter(|node| {
                !measured || node.rect.overlaps(&visible, reach) || self.entered == Some(node.id)
            })
            .collect();
        order.sort_by_key(|node| self.selection.contains(&node.id));
        let nodes: Vec<_> = order
            .into_iter()
            .filter_map(|node| {
                let live = self.live.get(&node.id)?;
                Some(self.node_slot(node, live, theme, cx))
            })
            .collect();
        let labels = self.edge_labels(theme, cx);
        let handles = self.edge_handles(theme, cx);
        let marquee = self.marquee(theme);
        let cursor = match &self.gesture {
            Some(Gesture::Pan { .. } | Gesture::Move { moved: true, .. }) => {
                CursorStyle::ClosedHand
            }
            Some(Gesture::Resize { .. }) => CursorStyle::ResizeUpLeftDownRight,
            Some(Gesture::Wire(_)) => CursorStyle::Crosshair,
            _ if self.space_held => CursorStyle::OpenHand,
            _ if self.hover.edge.is_some() => CursorStyle::PointingHand,
            _ => CursorStyle::Arrow,
        };
        let ring = theme.accent.opacity(0.35);
        div()
            .id("canvas")
            .track_focus(&self.focus)
            .tab_index(0)
            .size_full()
            .relative()
            .overflow_hidden()
            .rounded(px(metric::RADIUS))
            .bg(theme.rail)
            .font_family(theme.ui_font.clone())
            .border_1()
            .border_color(gpui::transparent_black())
            // The board has the keys: a faint frame, not one that reads as selected.
            .focus_visible(move |style| style.border_color(ring))
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::board_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::board_middle_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::board_context))
            .on_mouse_move(cx.listener(Self::pointer_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.pointer_left(cx);
                }
            }))
            .on_scroll(cx.listener(Self::scroll))
            .on_pinch(cx.listener(Self::pinch))
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .capture_key_down(cx.listener(Self::capture_key))
            .child(
                div()
                    .absolute()
                    .size_full()
                    .child(div().absolute().size_full().child(self.grid.clone()))
                    .child(self.backdrop(scene, cx))
                    .children(labels)
                    .when(empty, |board| board.child(self.empty_state(theme, cx)))
                    // With a camera restored from disk there is nothing to wait for.
                    .when(measured || !self.fit_pending, |board| board.children(nodes))
                    .children(handles)
                    .children(marquee),
            )
            .child(self.toolbar(theme, cx))
            .children(self.inspector_element(theme, cx))
            .children(self.toast_element(theme, cx))
            .children(self.menu_element(theme, cx))
    }
}

impl CanvasView {
    // Who has the keys, as the camera and the focus stand this frame.
    fn follow_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.frozen {
            return;
        }
        // Entered from afar, a node takes the keys once the camera has
        // brought it close enough to be whole and its body is drawn.
        if let Some(id) = self.entering
            && self.glide.is_none()
        {
            self.entering = None;
            if self.entered == Some(id)
                && Lod::of(self.camera.zoom) == Lod::Whole
                && let Some(focus) = self.bodies.get(&id).and_then(|body| body.focus.clone())
            {
                window.focus(&focus, cx);
            }
        }
        if self.entering.is_some() {
            return;
        }
        let Some(id) = self.entered else {
            return;
        };
        // Zoomed out to placeholders, a body is not drawn and cannot keep
        // the keys; and a body that lost them is no longer entered.
        let placeholder = Lod::of(self.target_camera().zoom) == Lod::Placeholder;
        let lost = match self.bodies.get(&id).and_then(|body| body.focus.as_ref()) {
            Some(focus) => !focus.contains_focused(window, cx),
            None => self.focus.is_focused(window),
        };
        if placeholder {
            self.set_entered(None, cx);
            window.focus(&self.focus, cx);
        } else if lost {
            self.set_entered(None, cx);
        }
    }
}
