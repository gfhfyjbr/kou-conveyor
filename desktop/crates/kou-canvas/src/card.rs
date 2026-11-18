//! A node. Whole, it is a rounded card: a header with its harness's mark,
//! its name, its kind, its state and its menu; its body, live when the app
//! gave it a view; a footer saying where it works; ports on its sides; a
//! corner to resize it by; a tab over it with what waits for it. Everything
//! in it is drawn at the camera's zoom, its corners included, so it is the
//! same card nearer or further.
//!
//! Zoomed out past [`Lod::LIVE`] it is a placeholder: its name, as large as
//! reads at that zoom, its mark, its hue and its state.

use crate::{
    anim::mix,
    face::Shows,
    field::Part,
    geometry::Lod,
    model::{Dir, Node, NodeId, NodeKind, PortRef},
    view::{CanvasView, Edit, Gesture, Live},
};
use gpui::{
    AnyElement, Context, CursorStyle, DispatchPhase, Div, FontWeight, HitboxBehavior, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _,
    ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _, canvas, div,
    prelude::FluentBuilder as _, px,
};
use kou_ui::{Icon, MONOGRAM, Mark, Palette, Theme, icon, metric, monogram_sized, status_mark};
use serde_json::json;
use std::time::Duration;

/// The header's height, in world units: the web canvas's.
pub(crate) const HEAD: f32 = 33.;
/// The footer's height.
pub(crate) const FOOT: f32 = 22.;
/// How large a placeholder's name is, in world units: at the threshold it is
/// 24 px, and it shrinks with its node from there.
const PLACEHOLDER_TITLE: f32 = 40.;
/// The smallest a placeholder's name gets, in pixels, however far out.
const PLACEHOLDER_TITLE_MIN: f32 = 12.;
/// How much of its hue a placeholder's surface takes.
const PLACEHOLDER_TINT: f32 = 0.11;

/// How long something has been so, the way a status line says it.
pub(crate) fn elapsed(duration: Duration) -> String {
    let s = duration.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

/// A state's word and colour, as a node's header shows them.
pub(crate) fn state_of(mark: Mark, theme: &Theme) -> (&'static str, Hsla) {
    let color = match mark {
        Mark::Working => theme.working,
        Mark::Done => theme.done,
        Mark::Blocked => theme.blocked,
        Mark::Failed => theme.error,
        Mark::Idle => theme.fg_muted,
    };
    (mark.label(), color)
}

/// The icon of a node that has no harness to show.
pub(crate) fn kind_icon(kind: NodeKind) -> Icon {
    match kind {
        NodeKind::Agent => Icon::Thread,
        NodeKind::Terminal => Icon::Terminal,
        NodeKind::Note => Icon::Note,
        NodeKind::Source => Icon::Source,
    }
}

/// A node's mark at a scale (1 is the app's own size): its harness's
/// monogram in its hue, or its kind's icon.
pub(crate) fn glyph(node: &Node, scale: f32, theme: &Theme) -> AnyElement {
    match node.kind {
        NodeKind::Agent if !node.tag.is_empty() => {
            monogram_sized(&node.tag, theme.hue(&node.tag), MONOGRAM * scale, theme)
                .into_any_element()
        }
        kind => icon(
            kind_icon(kind),
            if kind == NodeKind::Note {
                theme.blocked
            } else {
                theme.fg_muted
            },
        )
        .size(px(metric::ICON * scale))
        .into_any_element(),
    }
}

/// Text of a node's chrome, at the canvas's zoom. It is laid out at the
/// size it has at 1 and drawn scaled, its glyphs at one of a few sizes
/// ([`kou_ui::scale`]). Set as ordinary text at `size * zoom`, every letter
/// of every node on screen would be rasterised anew at each frame of a zoom
/// gesture, at a size no frame before had: the frames get long, and the
/// board scales in jerks. The text's box is set with [`gpui::Styled::text_size`]
/// at the zoomed size, as for any text here.
pub(crate) fn zoomed(text: impl Into<SharedString>, zoom: f32) -> AnyElement {
    let text = text.into();
    kou_ui::scale::with_factor(zoom, || {
        kou_ui::scale::scaled(zoom, kou_ui::scale::text(text))
            .inherit_rem()
            .into_any_element()
    })
}

/// The app's status mark at a scale.
pub(crate) fn state_mark(mark: Mark, scale: f32, theme: &Theme) -> Div {
    status_mark(mark, theme).size(px(8. * scale))
}

/// A colour a fraction of the way to another, as `0xRRGGBB`, so a surface
/// can be checked for contrast without a window.
pub(crate) fn tint(base: u32, toward: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let (a, b) = (
            ((base >> shift) & 0xff) as f32,
            ((toward >> shift) & 0xff) as f32,
        );
        ((a + (b - a) * t).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// A placeholder's surface: the pane, tinted with the node's hue (a note's
/// with the note colour).
pub(crate) fn placeholder_surface(palette: &Palette, hue: u32, note: bool) -> u32 {
    tint(
        palette.pane,
        if note { palette.blocked } else { hue },
        PLACEHOLDER_TINT,
    )
}

impl CanvasView {
    /// A node where it is on screen, with what hangs off it: the ring of its
    /// selection, its ports, its corner and the tab of what waits for it.
    pub(crate) fn node_slot(
        &self,
        node: &Node,
        live: &Live,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let zoom = self.camera.zoom;
        let on_screen = self.camera.rect(&node.rect);
        let (w, h) = (on_screen.w, on_screen.h);
        let id = node.id;
        let lod = Lod::of(zoom);
        let whole = lod == Lod::Whole;
        let select = live.select.value(&self.clock);
        let entered = self.entered == Some(id);
        let radius = metric::RADIUS_LARGE * zoom;
        let group = SharedString::from(format!("node-{id}"));

        // The selection closes in on the node from a little way out and
        // settles just outside its border, following its corners.
        let ring = (select > 0.01 && !entered).then(|| {
            let gap = 1. + (1. - select) * 6.;
            div()
                .absolute()
                .left(px(-gap))
                .top(px(-gap))
                .w(px(w + gap * 2.))
                .h(px(h + gap * 2.))
                .rounded(px(radius + gap))
                .border_1()
                .border_color(theme.accent.opacity(select))
        });
        // The node whose body has the keys.
        let held = entered.then(|| {
            let gap = 3.;
            div()
                .absolute()
                .left(px(-gap))
                .top(px(-gap))
                .w(px(w + gap * 2.))
                .h(px(h + gap * 2.))
                .rounded(px(radius + gap))
                .border_2()
                .border_color(theme.accent.opacity(0.45))
        });
        let card = if whole {
            self.card(node, live, theme, cx)
        } else {
            self.placeholder(node, live, (w, h), theme, cx)
        };
        let ports = if whole {
            self.ports(node, live, theme, cx)
        } else {
            Vec::new()
        };
        let corner = whole.then(|| {
            let tick = if select > 0.5 {
                theme.fg_muted
            } else {
                theme.fg_faint
            };
            let accent = theme.accent;
            div()
                .id(("resize", id as usize))
                .absolute()
                .left(px(w - 16.))
                .top(px(h - 16.))
                .size(px(16.))
                .cursor(CursorStyle::ResizeUpLeftDownRight)
                .group("corner")
                .opacity(if select > 0.5 { 1. } else { 0. })
                .group_hover(group.clone(), |style| style.opacity(1.))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        this.resize_down(id, event, window, cx)
                    }),
                )
                // A quarter of a ring inside the node's own corner.
                .child(
                    div()
                        .absolute()
                        .right(px(3.))
                        .bottom(px(3.))
                        .size(px(9.))
                        .border_r_1()
                        .border_b_1()
                        .border_color(tick)
                        .rounded_br(px((radius - 3.).max(1.)))
                        .group_hover("corner", move |style| style.border_color(accent)),
                )
        });
        let badge = self.pending.get(&id).map(|pending| {
            self.pending_tab(id, pending, h, if whole { zoom } else { 1. }, theme, cx)
        });
        div()
            .absolute()
            .left(px(on_screen.x))
            .top(px(on_screen.y))
            .w(px(w))
            .h(px(h))
            .group(group)
            .children(ring)
            .children(held)
            .child(card.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.node_context(id, event, window, cx)
                }),
            ))
            .children(corner)
            .children(ports)
            .children(badge)
    }

    /// The tab over a node with what waits for it. Its size follows the
    /// zoom with the node (`unit`), or stays the app's own over a placeholder.
    fn pending_tab(
        &self,
        id: NodeId,
        pending: &crate::Pending,
        h: f32,
        unit: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let approve = pending.awaiting > 0;
        let text = if self.lod() == Lod::Placeholder {
            // Over a placeholder, the count alone: the words would cover its neighbours.
            if approve {
                pending.awaiting.to_string()
            } else {
                pending.count.to_string()
            }
        } else if approve {
            match &pending.from {
                Some(from) => format!("{} to approve, from {from}", pending.awaiting),
                None => format!("{} to approve", pending.awaiting),
            }
        } else {
            format!("{} waiting", pending.count)
        };
        // As the app draws it, when it does (see [`crate::Face`]): the pill
        // and what it says, at `unit`; the canvas hangs it over the node.
        let shows = Shows::Pending {
            text: text.clone(),
            approve,
        };
        let data = || json!({ "node": id, "text": text, "approve": approve });
        if let Some(pill) = self.node_faced(id, "canvasPending", shows, data, Vec::new, unit, cx) {
            return div()
                .absolute()
                .right(px(0.))
                .bottom(px(h + 4. * unit))
                .child(pill)
                .into_any_element();
        }
        let color = if approve { theme.blocked } else { theme.fg_dim };
        let (hover, ring) = (theme.selected, theme.accent);
        div()
            .id(("pending", id as usize))
            .absolute()
            .right(px(0.))
            .bottom(px(h + 4. * unit))
            .h(px(22. * unit))
            .px(px(8. * unit))
            .flex()
            .items_center()
            .gap(px(5. * unit))
            .rounded(px(metric::RADIUS * unit))
            .border_1()
            .border_color(if approve {
                mix(theme.line_strong, theme.blocked, 0.55)
            } else {
                theme.line_strong
            })
            .bg(theme.raised)
            .text_size(px(metric::TEXT_SMALL * unit))
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .cursor_pointer()
            .whitespace_nowrap()
            .tab_index(0)
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(ring))
            .child(icon(Icon::Queue, color).size(px(12. * unit)))
            .child(zoomed(text, unit))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(crate::BoardEvent::PendingOpened(id));
            }))
            .into_any_element()
    }

    fn card(&self, node: &Node, live: &Live, theme: &Theme, cx: &Context<Self>) -> Div {
        let zoom = self.camera.zoom;
        let z = |value: f32| px(value * zoom);
        let id = node.id;
        let select = live.select.value(&self.clock);
        let entered = self.entered == Some(id);
        let note = node.kind == NodeKind::Note;
        let (word, state_color) = state_of(live.mark, theme);
        let mut border = theme.line_strong;
        match live.mark {
            Mark::Blocked => border = mix(border, theme.blocked, 0.6),
            Mark::Failed => border = mix(border, theme.error, 0.55),
            _ => {}
        }
        border = mix(border, theme.accent, select);
        let surface = if note {
            mix(theme.pane, theme.blocked, 0.05)
        } else {
            theme.pane
        };
        let radius = px(metric::RADIUS_LARGE * zoom);
        // The border is drawn over the node, not inside it: the header, the
        // body and the footer are then exactly the node's world size times
        // the zoom, so a body that divides its size by the zoom (a terminal
        // counting its columns) gets the same answer at every zoom, selected
        // or entered.
        let outline = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .rounded(radius)
            .border_1()
            .border_color(if entered { theme.accent } else { border });
        let frame = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(radius)
            .bg(surface);
        let grab = move |title: bool| {
            cx.listener(move |this: &mut Self, event: &MouseDownEvent, window, cx| {
                this.node_grab(id, title, event, window, cx)
            })
        };

        let editing_title = matches!(
            &self.editing,
            Some(editing) if editing.what == Edit::Title(id, false)
        );
        let title = || {
            if editing_title {
                let editing = self.editing.as_ref().map(|editing| &editing.field);
                div()
                    .flex_1()
                    .min_w_0()
                    .h(z(22.))
                    .px(z(6.))
                    .flex()
                    .items_center()
                    .rounded(z(metric::RADIUS))
                    .border_1()
                    .border_color(theme.accent)
                    .bg(theme.raised)
                    .text_size(z(metric::TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.fg)
                    .track_focus(&self.field_focus)
                    .on_key_down(cx.listener(Self::field_key))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(editing.map(|field| {
                        field_line(("title-field", id as usize), field, theme, z(14.))
                    }))
                    .into_any_element()
            } else {
                div()
                    .flex_initial()
                    .min_w_0()
                    .truncate()
                    .text_size(z(metric::TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.fg)
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(MouseButton::Left, grab(true))
                    .child(zoomed(node.title.clone(), zoom))
                    .into_any_element()
            }
        };
        let since = self.clock.now().saturating_duration_since(live.since);
        // As the app draws them, when it does (see [`crate::Face`]): a
        // drawing made at 1 and drawn at the zoom, inside what takes hold of
        // the node; the mark, the name and the state's dot stay the
        // canvas's, sized as they always were.
        let reachable = self.selection == [id];
        let ticking = (live.mark == Mark::Working).then(|| elapsed(since));
        let shows = Shows::Head {
            kind: node.kind,
            mark: live.mark,
            elapsed: ticking.clone(),
            reachable,
            editing: editing_title,
        };
        let data = || {
            json!({
                "node": id,
                "kind": node.kind.label(),
                "note": note,
                "editing": editing_title,
                "state": live.mark.name(),
                "word": word,
                "elapsed": ticking,
                "reachable": reachable,
            })
        };
        let slots = || {
            let mut slots = vec![("glyph", glyph(node, zoom, theme)), ("title", title())];
            if !note {
                let dot = state_mark(live.mark, zoom, theme).into_any_element();
                slots.push(("mark", dot));
            }
            slots
        };
        let held = |drawn: AnyElement| {
            div()
                .flex_none()
                .cursor(CursorStyle::OpenHand)
                .on_mouse_down(MouseButton::Left, grab(false))
                .child(drawn)
                .into_any_element()
        };
        let header = match self.node_faced(id, "canvasHead", shows, data, slots, zoom, cx) {
            Some(drawn) => held(drawn),
            None => div()
                .id(("head", id as usize))
                .flex_none()
                .h(z(HEAD))
                .pl(z(10.))
                .pr(z(5.))
                .flex()
                .items_center()
                .gap(z(8.))
                .bg(if note { surface } else { theme.hover })
                .border_b_1()
                .border_color(if note {
                    mix(theme.line, theme.blocked, 0.18)
                } else {
                    theme.line
                })
                .overflow_hidden()
                .cursor(CursorStyle::OpenHand)
                .on_mouse_down(MouseButton::Left, grab(false))
                .child(glyph(node, zoom, theme))
                .child(title())
                .when(!note && !editing_title, |header| {
                    header.child(
                        div()
                            .flex_initial()
                            .min_w_0()
                            .truncate()
                            .text_size(z(metric::TEXT_SMALL))
                            .text_color(theme.fg_muted)
                            .child(zoomed(node.kind.label(), zoom)),
                    )
                })
                .child(div().flex_1())
                .when(!note, |header| {
                    header.child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(z(5.))
                            .text_size(z(metric::TEXT_SMALL))
                            .when(live.mark == Mark::Working, |state| {
                                state.child(
                                    div()
                                        .text_color(theme.fg_muted)
                                        .child(zoomed(elapsed(since), zoom)),
                                )
                            })
                            .when(live.mark != Mark::Idle, |state| {
                                state.child(div().text_color(state_color).child(zoomed(word, zoom)))
                            })
                            .child(state_mark(live.mark, zoom, theme)),
                    )
                })
                .child(self.more_button(id, zoom, theme, cx))
                .into_any_element(),
        };

        let body = self.body(node, theme, cx);
        let footer = (!node.place.is_empty()).then(|| {
            let shows = Shows::Foot {
                place: node.place.clone(),
                note,
            };
            let data = || {
                json!({
                    "place": node.place,
                    "note": note,
                })
            };
            if let Some(drawn) = self.node_faced(id, "canvasFoot", shows, data, Vec::new, zoom, cx)
            {
                return held(drawn);
            }
            div()
                .flex_none()
                .h(z(FOOT))
                .px(z(10.))
                .flex()
                .items_center()
                .gap(z(6.))
                .border_t_1()
                .border_color(theme.line)
                .bg(surface)
                .font_family(theme.mono_font.clone())
                .text_size(z(10.5))
                .text_color(theme.fg_muted)
                .cursor(CursorStyle::OpenHand)
                .on_mouse_down(MouseButton::Left, grab(false))
                .child(icon(Icon::Folder, theme.fg_muted).size(z(12.)))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(zoomed(node.place.clone(), zoom)),
                )
                .into_any_element()
        });
        frame
            .child(header)
            .child(body)
            .children(footer)
            .child(outline)
    }

    /// A node far out: its name as large as reads at this zoom, its mark,
    /// its state, on a surface of its hue. Dragged by any of it.
    fn placeholder(
        &self,
        node: &Node,
        live: &Live,
        (w, h): (f32, f32),
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let zoom = self.camera.zoom;
        let id = node.id;
        let note = node.kind == NodeKind::Note;
        let hue = crate::view::hue_of(node);
        let select = live.select.value(&self.clock);
        let title_px = (PLACEHOLDER_TITLE * zoom).max(PLACEHOLDER_TITLE_MIN);
        // The marks grow with the name: at the smallest name, the app's sizes.
        let scale = title_px / PLACEHOLDER_TITLE_MIN;
        let surface: Hsla = gpui::rgb(placeholder_surface(&theme.palette, hue, note)).into();
        let hue_color: Hsla = if note {
            theme.blocked
        } else {
            gpui::rgb(hue).into()
        };
        let mut border = mix(theme.line_strong, hue_color, 0.45);
        match live.mark {
            Mark::Blocked => border = mix(border, theme.blocked, 0.7),
            Mark::Failed => border = mix(border, theme.error, 0.7),
            _ => {}
        }
        border = mix(border, theme.accent, select);
        let title = div()
            .min_w_0()
            .max_w_full()
            .truncate()
            .text_size(px(title_px))
            .line_height(px(title_px * 1.25))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.fg)
            // (Its size follows the zoom down to a least one: scaled from
            // the size it has at 1 by as much as that is.)
            .child(zoomed(node.title.clone(), title_px / PLACEHOLDER_TITLE));
        let pad = (title_px * 0.6).min(w * 0.08);
        // Room first for the name, then its mark, then its state: on a node
        // too narrow for all three, the state goes, then the mark.
        let room = w - pad * 2.;
        let mark_side = MONOGRAM * scale;
        let name_room = title_px * 2.;
        let with_glyph = room >= mark_side + title_px * 0.45 + name_room;
        let mark = (!note && room >= (mark_side + title_px * 0.45) * 2. + name_room)
            .then(|| state_mark(live.mark, scale, theme));
        let frame = div()
            .size_full()
            .overflow_hidden()
            .rounded(px(metric::RADIUS_LARGE * zoom))
            .bg(surface)
            .border_1()
            .border_color(border)
            .px(px(pad))
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.node_grab(id, false, event, window, cx)
                }),
            );
        // Tall enough, the mark stands over the name and the state under it;
        // otherwise they share a line.
        if h >= title_px * 4.2 && w >= title_px * 4. {
            frame.child(
                div()
                    .max_w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(title_px * 0.35))
                    .child(glyph(node, scale * 1.15, theme))
                    .child(title)
                    .children(mark),
            )
        } else {
            frame.child(
                div()
                    .max_w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(title_px * 0.45))
                    .when(with_glyph, |row| {
                        row.child(div().flex_none().child(glyph(node, scale, theme)))
                    })
                    .child(title.flex_initial())
                    .children(mark),
            )
        }
    }

    fn more_button(
        &self,
        id: NodeId,
        zoom: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        // `kou_ui::icon_button`, drawn at the camera's zoom with the rest of the node.
        let z = |value: f32| px(value * zoom);
        let (hover, ring) = (theme.selected, theme.accent);
        // Reached by Tab on the selected node only, so Tab does not walk
        // through every node on its way out of the canvas.
        let reachable = self.selection == [id];
        div()
            .id(("more", id as usize))
            .flex_none()
            .size(z(24.))
            .rounded(z(metric::RADIUS))
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .active(|style| style.opacity(0.6))
            .when(reachable, |button| button.tab_index(0))
            .focus_visible(move |style| style.border_color(ring))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                let Some(rect) = this.board.node(id).map(|node| node.rect) else {
                    return;
                };
                let (x, y) = this.camera.to_screen(rect.right() - 4., rect.y + HEAD);
                this.open_node_menu(id, (x - 220., y + 2.), window, cx);
            }))
            .child(icon(Icon::More, theme.fg_muted).size(z(metric::ICON)))
    }

    fn body(&self, node: &Node, theme: &Theme, cx: &Context<Self>) -> Div {
        let zoom = self.camera.zoom;
        let z = |value: f32| px(value * zoom);
        let id = node.id;
        if let Some(body) = self.bodies.get(&id) {
            // Live, at whatever zoom: the view draws itself at the node's
            // size on screen. Nothing here changes from frame to frame
            // unless the camera does, so GPUI draws the view again from the
            // last frame while the board animates around it.
            return div()
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.body_down(id, window, cx)
                    }),
                )
                .child(self.wheel_guard(id, cx))
                .child(body.view.clone());
        }
        let grab = || {
            cx.listener(move |this: &mut Self, event: &MouseDownEvent, window, cx| {
                this.node_grab(id, false, event, window, cx)
            })
        };
        let editing_note = self
            .editing
            .as_ref()
            .filter(|editing| editing.what == Edit::Note(id));
        if let Some(editing) = editing_note {
            return div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .m(z(6.))
                .py(z(6.))
                .px(z(8.))
                .rounded(z(metric::RADIUS))
                .bg(theme.raised)
                .border_1()
                .border_color(theme.accent)
                .text_size(z(metric::TEXT))
                .line_height(z(19.))
                .text_color(theme.fg)
                .track_focus(&self.field_focus)
                .on_key_down(cx.listener(Self::field_key))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(field_lines(
                    ("body-field", id as usize),
                    &editing.field,
                    theme,
                    z(15.),
                ));
        }
        let base = div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(MouseButton::Left, grab());
        // As the app draws it, when it does (see [`crate::Face`]): what the
        // node says, made at 100% and drawn at the zoom, in what takes hold
        // of the node. (A note being written in is the canvas's.)
        let shows = Shows::Body {
            note: node.kind == NodeKind::Note,
            detail: node.detail.clone(),
        };
        let data = || {
            json!({
                "note": node.kind == NodeKind::Note,
                "empty": node.detail.trim().is_empty(),
                "lines": node.detail.lines().collect::<Vec<_>>(),
            })
        };
        if let Some(drawn) = self.node_faced(id, "canvasBody", shows, data, Vec::new, zoom, cx) {
            return base.child(drawn);
        }
        if node.kind == NodeKind::Note {
            if node.detail.trim().is_empty() {
                return base.px(z(14.)).py(z(10.)).child(
                    div()
                        .text_size(z(12.))
                        .text_color(theme.fg_muted)
                        .child(zoomed("Double-click to write.", zoom)),
                );
            }
            return base
                .px(z(14.))
                .py(z(10.))
                .text_size(z(metric::TEXT))
                .line_height(z(20.))
                .text_color(theme.fg)
                .children(
                    node.detail
                        .lines()
                        .map(|line| div().min_h(z(20.)).child(zoomed(line.to_owned(), zoom))),
                );
        }
        if node.detail.trim().is_empty() {
            return base.p(z(10.)).child(
                div()
                    .text_size(z(12.))
                    .text_color(theme.fg_muted)
                    .child(zoomed("Nothing yet.", zoom)),
            );
        }
        base.p(z(10.))
            .font_family(theme.mono_font.clone())
            .text_size(z(11.5))
            .line_height(z(17.))
            .text_color(theme.fg_dim)
            .children(
                node.detail
                    .lines()
                    .map(|line| div().truncate().child(zoomed(line.to_owned(), zoom))),
            )
    }

    /// Over a node's body, the wheel moves the board, as on the web canvas,
    /// unless the body has the keys: then it scrolls what is in it (Cmd or
    /// Ctrl still zooms the board). Taken before the body sees it, and only
    /// where nothing floats over the body.
    fn wheel_guard(&self, id: NodeId, cx: &Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity().downgrade();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) {
                        return;
                    }
                    let taken = view
                        .update(cx, |this, cx| {
                            let taken = this.board_takes_wheel(id, event);
                            if taken {
                                this.scroll(event, window, cx);
                            }
                            taken
                        })
                        .unwrap_or(false);
                    if taken {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    /// The ports on a node's sides: where wires start and end. They show by
    /// the side the pointer is near, while a wire is drawn, and on the
    /// selected node; hidden, they still take the pointer.
    fn ports(
        &self,
        node: &Node,
        live: &Live,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let zoom = self.camera.zoom;
        let rect = node.rect;
        let wiring = match &self.gesture {
            Some(Gesture::Wire(wiring)) => Some(wiring),
            _ => None,
        };
        let mut ports = Vec::new();
        for (side, dir) in [Dir::In, Dir::Out].into_iter().enumerate() {
            let reveal = live.ports[side].value(&self.clock);
            for name in node.kind.ports(dir) {
                let port = PortRef::new(node.id, *name);
                let (px_, py_) = crate::model::port_at(node.kind, &rect, dir, name);
                let x = (px_ - rect.x) * zoom;
                let y = (py_ - rect.y) * zoom;
                let source = wiring.is_some_and(|w| w.fixed == port);
                let candidate =
                    wiring.is_some_and(|w| w.wanted() == dir && w.fixed.node != node.id);
                let over = wiring.is_some_and(|w| w.target.as_ref() == Some(&port));
                let close = wiring.is_some_and(|w| w.close.as_ref() == Some(&port));
                let lit = source || over;
                let shown = reveal.max(if source || candidate { 1. } else { 0. });
                let scale = 0.6 + 0.4 * shown + if lit || close { 0.3 } else { 0. };
                let unit = zoom.max(0.5);
                let size = 10. * scale * unit;
                let hit = (22. * unit).max(14.);
                let group = SharedString::from(format!("port-{}-{name}", node.id));
                let accent = theme.accent;
                let label = {
                    let text = div()
                        .absolute()
                        .top(px(hit / 2. - 9. * unit))
                        .h(px(18. * unit))
                        .px(px(6. * unit))
                        .flex()
                        .items_center()
                        .rounded(px(metric::RADIUS_SMALL * unit))
                        .border_1()
                        .border_color(if over {
                            mix(theme.line, accent, 0.5)
                        } else {
                            theme.line
                        })
                        .bg(theme.raised)
                        .font_family(theme.mono_font.clone())
                        .text_size(px(10. * unit))
                        .text_color(if over { accent } else { theme.fg_dim })
                        .whitespace_nowrap()
                        .opacity(if over || close { 1. } else { 0. })
                        .group_hover(group.clone(), |style| style.opacity(1.))
                        .child(zoomed(*name, unit));
                    match dir {
                        Dir::In => text.right(px(hit - 2.)),
                        Dir::Out => text.left(px(hit - 2.)),
                    }
                };
                let element = div()
                    .id(SharedString::from(format!("port-{}-{name}", node.id)))
                    .group(group.clone())
                    .absolute()
                    .left(px(x - hit / 2.))
                    .top(px(y - hit / 2.))
                    .size(px(hit))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::Crosshair)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let port = port.clone();
                            move |this, event: &MouseDownEvent, window, cx| {
                                this.port_down(port.clone(), dir, event, window, cx)
                            }
                        }),
                    )
                    .child(
                        div()
                            .size(px(size))
                            .rounded_full()
                            .border_1()
                            .border_color(if candidate || lit {
                                accent
                            } else {
                                theme.fg_faint
                            })
                            .bg(if lit { accent } else { theme.pane })
                            .opacity(shown)
                            .group_hover(group.clone(), move |style| {
                                style.opacity(1.).bg(accent).border_color(accent)
                            }),
                    )
                    .child(label);
                ports.push(element.into_any_element());
            }
        }
        ports
    }
}

/// One line of a field, with its caret and selection.
pub(crate) fn field_line(
    id: impl Into<gpui::ElementId>,
    field: &crate::field::Field,
    theme: &Theme,
    caret: gpui::Pixels,
) -> Div {
    let lines = field.lines();
    let parts = lines.into_iter().next().unwrap_or_default();
    line_of(id.into(), field.place(), parts, theme, caret)
}

/// Every line of a field, drawn: its text, its selection and its caret, which
/// is `caret` tall and blinks (see [`kou_ui::caret`]); `id` is the field's,
/// to keep the blink from one frame to the next.
pub fn field_lines(
    id: impl Into<gpui::ElementId>,
    field: &crate::field::Field,
    theme: &Theme,
    caret: gpui::Pixels,
) -> Div {
    let (id, place) = (id.into(), field.place());
    div().flex().flex_col().children(
        field
            .lines()
            .into_iter()
            .map(|parts| line_of(id.clone(), place, parts, theme, caret)),
    )
}

fn line_of(
    id: gpui::ElementId,
    place: u64,
    parts: Vec<Part>,
    theme: &Theme,
    caret: gpui::Pixels,
) -> Div {
    div()
        .flex()
        .items_center()
        .min_h(caret)
        .whitespace_nowrap()
        .overflow_hidden()
        .children(parts.into_iter().map(|part| {
            match part {
                Part::Caret => {
                    kou_ui::caret(id.clone(), place, theme.accent, gpui::size(px(1.5), caret))
                        .into_any_element()
                }
                Part::Text(text, selected) => div()
                    .flex_none()
                    .when(selected, |text| text.bg(theme.accent.opacity(0.28)))
                    .child(SharedString::from(text.replace(' ', "\u{a0}")))
                    .into_any_element(),
            }
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kou_ui::{DARK, HUES, LIGHT, contrast};

    #[test]
    fn time_in_a_state_reads_as_a_status_line_does() {
        assert_eq!(elapsed(Duration::from_secs(9)), "9s");
        assert_eq!(elapsed(Duration::from_secs(125)), "2m");
        assert_eq!(elapsed(Duration::from_secs(3 * 3600 + 7 * 60)), "3h 7m");
    }

    #[test]
    fn a_placeholders_name_reads_on_every_hue_in_both_themes() {
        assert_eq!(tint(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(tint(0x123456, 0xabcdef, 0.), 0x123456);
        for (theme, palette) in [("dark", DARK), ("light", LIGHT)] {
            for (hue, note) in HUES.iter().map(|hue| (*hue, false)).chain([(0, true)]) {
                let surface = placeholder_surface(&palette, hue, note);
                let ratio = contrast(palette.fg, surface);
                assert!(
                    ratio >= 4.5,
                    "{theme}: the name on {surface:06x} (hue {hue:06x}) is {ratio:.2}:1"
                );
            }
        }
    }

    #[test]
    fn a_placeholders_name_shrinks_with_the_zoom_but_not_below_legible() {
        let size = |zoom: f32| (PLACEHOLDER_TITLE * zoom).max(PLACEHOLDER_TITLE_MIN);
        assert!(size(0.59) > 23. && size(0.59) < 24.);
        assert!(size(0.4) < size(0.5));
        assert_eq!(size(0.1), PLACEHOLDER_TITLE_MIN);
        // Just under the threshold the name is far larger than the terminal
        // text was just over it.
        assert!(size(Lod::LIVE - 0.01) > 2.5 * Lod::TERMINAL_TEXT * Lod::LIVE);
    }
}
