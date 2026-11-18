//! The canvas's menus (what to add at a point, a node's, an edge's, the
//! zoom's, which node to wire to), the bar in its corner, what an empty
//! canvas offers, the labels and the end handles of edges, and the bar that
//! offers to undo a deletion. A menu fades in where it was asked for; its
//! highlight travels between rows.

use crate::{
    anim::{Clock, Tween},
    card::glyph,
    catalog::{AddRequest, TemplateRequest, templates},
    geometry::curve,
    model::{Dir, EdgeId, EdgeMode, End, NodeId, NodeKind, PortRef, Wire},
    paint::labels_show,
    view::{CanvasView, Gesture},
};
use gpui::{
    AnyElement, Context, CursorStyle, Div, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use kou_ui::{
    Icon, Theme, button, button_with_icon, icon, icon_button, key_hint, metric, monogram, motion,
    tip,
};
use std::time::Instant;

/// A row of a menu: the app's row.
pub(crate) const ROW: f32 = metric::ROW;
/// The room a separator takes.
const SEPARATOR: f32 = 9.;
/// A menu's width.
const WIDTH: f32 = 264.;
/// The room between a menu's edge and its rows.
const PAD: f32 = metric::UNIT;
/// A menu's title line.
const TITLE: f32 = 26.;
/// The bar in the board's corner: its distance from the corner, and its height.
const BAR_INSET: f32 = 12.;
const BAR_HEIGHT: f32 = 36.;

/// What a menu row does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    Offer(String),
    Note,
    StartAgent,
    Template(usize),
    Enter(NodeId),
    Open(NodeId),
    Focus(NodeId),
    Rename(NodeId),
    Inspect(NodeId),
    Duplicate(NodeId),
    Delete(NodeId),
    WireFrom(NodeId),
    WireTo(PortRef, NodeId),
    EdgeInspect(EdgeId),
    EdgeMode(EdgeId, EdgeMode),
    EdgeEnds(EdgeId, End),
    Retarget(EdgeId, End, NodeId),
    EdgeDelete(EdgeId),
    ZoomIn,
    ZoomOut,
    Zoom100,
    Fit,
    ZoomSelection,
}

/// A row of a menu.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub label: SharedString,
    pub detail: Option<SharedString>,
    pub hint: Option<&'static str>,
    pub icon: Option<Icon>,
    pub code: Option<SharedString>,
    /// A node whose mark the row shows.
    pub node: Option<NodeId>,
    pub danger: bool,
    pub enabled: bool,
    pub separator: bool,
    pub action: Action,
}

impl Item {
    fn new(label: impl Into<SharedString>, action: Action) -> Self {
        Self {
            label: label.into(),
            detail: None,
            hint: None,
            icon: None,
            code: None,
            node: None,
            danger: false,
            enabled: true,
            separator: false,
            action,
        }
    }
    fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }
    fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    fn after_separator(mut self) -> Self {
        self.separator = true;
        self
    }
}

/// An open menu.
pub(crate) struct Menu {
    pub at: (f32, f32),
    pub title: Option<SharedString>,
    pub items: Vec<Item>,
    pub highlight: usize,
    pub travel: Tween,
    pub opened: Instant,
    pub closing: Option<Instant>,
    pub world: (f32, f32),
    pub wire: Option<Wire>,
}

impl Menu {
    fn top_of(&self, index: usize) -> f32 {
        let title = if self.title.is_some() { TITLE } else { 0. };
        PAD + title
            + self.items[..=index.min(self.items.len().saturating_sub(1))]
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    if item.separator && i <= index {
                        SEPARATOR
                    } else {
                        0.
                    }
                })
                .sum::<f32>()
            + index as f32 * ROW
    }

    fn height(&self) -> f32 {
        let title = if self.title.is_some() { TITLE } else { 0. };
        PAD * 2.
            + 2.
            + title
            + self.items.len() as f32 * ROW
            + self.items.iter().filter(|item| item.separator).count() as f32 * SEPARATOR
    }

    pub(crate) fn moving(&self, clock: &Clock) -> bool {
        clock.progress(self.opened, motion::BASE) < 1.
            || self.travel.moving(clock)
            || self.closing.is_some()
    }

    pub(crate) fn gone(&self, clock: &Clock) -> bool {
        self.closing
            .is_some_and(|closing| clock.progress(closing, motion::QUICK) >= 1.)
    }
}

impl CanvasView {
    fn open_menu(
        &mut self,
        at: (f32, f32),
        title: Option<SharedString>,
        items: Vec<Item>,
        (world, wire): ((f32, f32), Option<Wire>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if items.is_empty() {
            return;
        }
        let highlight = items.iter().position(|item| item.enabled).unwrap_or(0);
        let mut menu = Menu {
            at,
            title,
            items,
            highlight,
            travel: Tween::at(0.),
            opened: self.clock.now(),
            closing: None,
            world,
            wire,
        };
        menu.travel = Tween::at(menu.top_of(highlight));
        self.menu = Some(menu);
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu
            && menu.closing.is_none()
        {
            menu.closing = Some(self.clock.now());
            cx.notify();
        }
    }

    pub(crate) fn open_add_menu(
        &mut self,
        local: (f32, f32),
        world: (f32, f32),
        wire: Option<Wire>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut items = Vec::new();
        let fits = |kind: NodeKind| match &wire {
            None => true,
            Some(Wire::From(_)) => !kind.inputs().is_empty(),
            Some(Wire::To(_)) => !kind.outputs().is_empty(),
        };
        let mut last_kind = None;
        for offer in self.offers.iter().filter(|offer| fits(offer.kind)) {
            let mut item = Item::new(offer.title.clone(), Action::Offer(offer.id.clone()));
            item.code = (!offer.code.is_empty()).then(|| offer.code.clone().into());
            if offer.code.is_empty() {
                item.icon = Some(match offer.kind {
                    NodeKind::Terminal => Icon::Terminal,
                    NodeKind::Source => Icon::Source,
                    _ => Icon::Thread,
                });
            }
            if !offer.detail.is_empty() {
                item.detail = Some(offer.detail.clone().into());
            }
            item.enabled = offer.ready;
            item.separator = last_kind.is_some_and(|kind| kind != offer.kind);
            last_kind = Some(offer.kind);
            items.push(item);
        }
        if self.offers.is_empty() {
            let mut item = Item::new("Start an agent here", Action::StartAgent).icon(Icon::Plus);
            if wire.is_some() {
                item = item.detail("The app asks what; wire it afterwards");
            }
            items.push(item);
        }
        if wire.is_none() {
            let note = Item::new("Note", Action::Note)
                .icon(Icon::Note)
                .detail("Text on the board");
            items.push(if items.is_empty() {
                note
            } else {
                note.after_separator()
            });
        }
        let title = match &wire {
            Some(Wire::From(_)) => Some("Add, wired from that output".into()),
            Some(Wire::To(_)) => Some("Add, wired into that input".into()),
            None => None,
        };
        self.open_menu(local, title, items, (world, wire), window, cx);
    }

    pub(crate) fn open_node_menu(
        &mut self,
        id: NodeId,
        at: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.board.node(id) else {
            return;
        };
        let mut items = Vec::new();
        if self.bodies.contains_key(&id) {
            items.push(
                Item::new("Enter", Action::Enter(id))
                    .icon(Icon::ChevronRight)
                    .hint("⏎")
                    .detail("Its body takes the keys"),
            );
        }
        if !node.kind.canvas_owned() {
            items.push(Item::new("Open as a pane", Action::Open(id)).icon(Icon::Pane));
        }
        items.push(
            Item::new("Focus", Action::Focus(id))
                .icon(Icon::Target)
                .hint("F")
                .detail("At 100%, in the middle"),
        );
        items.push(
            Item::new("Rename", Action::Rename(id))
                .icon(Icon::Pencil)
                .hint("F2"),
        );
        items.push(
            Item::new("Inspect", Action::Inspect(id))
                .icon(Icon::Info)
                .hint("I"),
        );
        if !node.kind.outputs().is_empty() {
            items.push(Item::new("Wire its output to…", Action::WireFrom(id)).icon(Icon::Wire));
        }
        items.push(
            Item::new("Duplicate", Action::Duplicate(id))
                .icon(Icon::Duplicate)
                .hint("⌘D"),
        );
        items.push(
            Item::new("Delete", Action::Delete(id))
                .icon(Icon::Trash)
                .hint("⌫")
                .danger()
                .after_separator(),
        );
        let world = self.camera.to_world(at.0, at.1);
        self.open_menu(at, None, items, (world, None), window, cx);
    }

    pub(crate) fn open_edge_menu(
        &mut self,
        edge: EdgeId,
        at: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(e) = self.board.edge(edge) else {
            return;
        };
        let mut items = vec![
            Item::new("Inspect", Action::EdgeInspect(edge))
                .icon(Icon::Info)
                .hint("I")
                .detail("Its mode, delivery and template"),
        ];
        for (mode, label, glyph) in [
            (EdgeMode::Auto, "Let messages through", Icon::Play),
            (
                EdgeMode::Approve,
                "Ask before each message",
                Icon::CheckCircle,
            ),
            (EdgeMode::Off, "Turn it off", Icon::Pause),
        ] {
            if e.mode != mode {
                let item = Item::new(label, Action::EdgeMode(edge, mode)).icon(glyph);
                // M steps to the next mode: its row says so.
                items.push(if e.mode.next() == mode {
                    item.hint("M")
                } else {
                    item
                });
            }
        }
        items
            .push(Item::new("Move its start…", Action::EdgeEnds(edge, End::From)).icon(Icon::Wire));
        items.push(Item::new("Move its end…", Action::EdgeEnds(edge, End::To)).icon(Icon::Wire));
        items.push(
            Item::new("Delete", Action::EdgeDelete(edge))
                .icon(Icon::Trash)
                .hint("⌫")
                .danger()
                .after_separator(),
        );
        let world = self.camera.to_world(at.0, at.1);
        self.open_menu(at, None, items, (world, None), window, cx);
    }

    fn open_zoom_menu(&mut self, at: (f32, f32), window: &mut Window, cx: &mut Context<Self>) {
        let items = vec![
            Item::new("Zoom in", Action::ZoomIn)
                .icon(Icon::Plus)
                .hint("⌘="),
            Item::new("Zoom out", Action::ZoomOut)
                .icon(Icon::Minus)
                .hint("⌘−"),
            Item::new("Zoom to 100%", Action::Zoom100)
                .icon(Icon::ZoomActual)
                .hint("⌘1")
                .detail("The selection at its size"),
            Item::new("Zoom to the selection", Action::ZoomSelection).icon(Icon::Target),
            Item::new("Fit everything", Action::Fit)
                .icon(Icon::Fit)
                .hint("⌘0"),
        ];
        let world = self.camera.to_world(at.0, at.1);
        self.open_menu(at, None, items, (world, None), window, cx);
    }

    /// Which node an output may be wired to: the keyboard's way to draw an edge.
    fn open_wire_menu(&mut self, from: NodeId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(node) = self.board.node(from) else {
            return;
        };
        let at = self.anchor_for(from);
        let mut items = Vec::new();
        for port in node.kind.outputs() {
            for id in self.board.reading_order() {
                let Some(other) = self.board.node(id) else {
                    continue;
                };
                if id == from || other.kind.inputs().is_empty() {
                    continue;
                }
                let label = if node.kind.outputs().len() > 1 {
                    format!("{} ({port} →)", other.title)
                } else {
                    other.title.clone()
                };
                let mut item = Item::new(label, Action::WireTo(PortRef::new(from, *port), id));
                item.detail = Some(other.kind.label().into());
                item.node = Some(id);
                items.push(item);
            }
        }
        if items.is_empty() {
            self.show_toast("No other node takes input", false, window, cx);
            return;
        }
        let world = self.camera.to_world(at.0, at.1);
        self.open_menu(
            at,
            Some(format!("Wire «{}» to", node.title).into()),
            items,
            (world, None),
            window,
            cx,
        );
    }

    /// Which node an edge's end may move to.
    fn open_ends_menu(
        &mut self,
        edge: EdgeId,
        end: End,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(e) = self.board.edge(edge).cloned() else {
            return;
        };
        let (other, dir) = match end {
            End::From => (e.to, Dir::Out),
            End::To => (e.from, Dir::In),
        };
        let items: Vec<Item> = self
            .board
            .reading_order()
            .into_iter()
            .filter_map(|id| self.board.node(id))
            .filter(|node| node.id != other && !node.kind.ports(dir).is_empty())
            .map(|node| {
                let current = match end {
                    End::From => node.id == e.from,
                    End::To => node.id == e.to,
                };
                let mut item = Item::new(node.title.clone(), Action::Retarget(edge, end, node.id));
                item.node = Some(node.id);
                item.detail = Some(if current {
                    "where it is".into()
                } else {
                    node.kind.label().into()
                });
                item.enabled = !current;
                item
            })
            .collect();
        let at = match self
            .board
            .edge(edge)
            .and_then(|e| crate::geometry::edge_curve(&self.board, e))
        {
            Some(c) => {
                let (x, y) = c.middle();
                self.camera.to_screen(x, y)
            }
            None => self.anchor_for(other),
        };
        let title = match end {
            End::From => "Start the edge at",
            End::To => "End the edge at",
        };
        let world = self.camera.to_world(at.0, at.1);
        self.open_menu(at, Some(title.into()), items, (world, None), window, cx);
    }

    fn anchor_for(&self, id: NodeId) -> (f32, f32) {
        self.board.node(id).map_or_else(
            || {
                let (w, h) = self.viewport();
                (w / 2., h / 2.)
            },
            |node| {
                self.camera
                    .to_screen(node.rect.right() - 8., node.rect.y + 30.)
            },
        )
    }

    pub(crate) fn run(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let (world, wire) = self
            .menu
            .as_ref()
            .map_or(((0., 0.), None), |menu| (menu.world, menu.wire.clone()));
        self.close_menu(cx);
        window.focus(&self.focus, cx);
        match action {
            Action::Offer(offer) => cx.emit(crate::BoardEvent::AddRequested(AddRequest {
                offer,
                x: world.0,
                y: world.1,
                wire,
            })),
            Action::Note => self.add_note(world.0, world.1, window, cx),
            Action::StartAgent => cx.emit(crate::CanvasEvent::CreateRequested {
                x: world.0,
                y: world.1,
            }),
            Action::Template(index) => {
                if let Some(template) = templates(&self.offers).into_iter().nth(index) {
                    let (w, h) = self.viewport();
                    let (x, y) = self.camera.to_world(w / 2., h / 2.);
                    cx.emit(crate::BoardEvent::TemplateRequested(TemplateRequest {
                        template,
                        x: x - 400.,
                        y: y - 200.,
                    }));
                }
            }
            Action::Enter(id) => self.enter(id, window, cx),
            Action::Open(id) => cx.emit(crate::CanvasEvent::Opened(id)),
            Action::Focus(id) => self.focus_node(id, cx),
            Action::Rename(id) => self.start_edit(crate::view::Edit::Title(id, false), window, cx),
            Action::Inspect(id) => {
                self.set_selection(vec![id], None, cx);
                self.open_inspector(cx);
            }
            Action::Duplicate(id) => {
                self.set_selection(vec![id], None, cx);
                self.duplicate_selection(window, cx);
            }
            Action::Delete(id) => self.ask_to_remove(&[id], window, cx),
            Action::WireFrom(id) => self.open_wire_menu(id, window, cx),
            Action::WireTo(from, to) => {
                let port = self
                    .board
                    .node(to)
                    .and_then(|node| node.kind.inputs().first().copied());
                if let Some(port) = port
                    && let Err(crate::model::WireError::Exists(id)) =
                        self.add_wire(from, PortRef::new(to, port), cx)
                {
                    self.set_selection(Vec::new(), Some(id), cx);
                    self.show_toast("They are wired so already", false, window, cx);
                }
            }
            Action::EdgeInspect(edge) => self.open_inspector_for_edge(edge, cx),
            Action::EdgeMode(edge, mode) => self.set_edge_mode(edge, mode, cx),
            Action::EdgeEnds(edge, end) => self.open_ends_menu(edge, end, window, cx),
            Action::Retarget(edge, end, node) => {
                let dir = match end {
                    End::From => Dir::Out,
                    End::To => Dir::In,
                };
                let port = self
                    .board
                    .node(node)
                    .and_then(|n| n.kind.ports(dir).first().copied());
                if let Some(port) = port {
                    let before = self.board.clone();
                    match self.board.retarget(edge, end, PortRef::new(node, port)) {
                        Ok(was) => {
                            self.remember_board(before);
                            if let Some(now) = self.board.edge(edge).cloned() {
                                cx.emit(crate::BoardEvent::EdgeChanged {
                                    before: was,
                                    after: now,
                                });
                            }
                            cx.emit(crate::CanvasEvent::Changed);
                        }
                        Err(_) => self.show_toast("They are wired so already", false, window, cx),
                    }
                }
            }
            Action::EdgeDelete(edge) => self.delete_edge(edge, window, cx),
            Action::ZoomIn => self.zoom_step(1, cx),
            Action::ZoomOut => self.zoom_step(-1, cx),
            Action::Zoom100 => self.zoom_100(cx),
            Action::Fit => self.fit(cx),
            Action::ZoomSelection => self.zoom_to_selection(cx),
        }
        cx.notify();
    }

    fn move_highlight(&mut self, step: i32, cx: &mut Context<Self>) {
        let clock = self.clock;
        let Some(menu) = &mut self.menu else {
            return;
        };
        let count = menu.items.len() as i32;
        let mut at = menu.highlight as i32;
        for _ in 0..count {
            at = (at + step).rem_euclid(count);
            if menu.items[at as usize].enabled {
                break;
            }
        }
        menu.highlight = at as usize;
        let top = menu.top_of(menu.highlight);
        menu.travel.retarget(top, motion::QUICK, &clock);
        cx.notify();
    }

    pub(crate) fn hover_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let clock = self.clock;
        let Some(menu) = &mut self.menu else {
            return;
        };
        if menu.highlight == index || !menu.items.get(index).is_some_and(|item| item.enabled) {
            return;
        }
        menu.highlight = index;
        let top = menu.top_of(index);
        menu.travel.retarget(top, motion::QUICK, &clock);
        cx.notify();
    }

    fn menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &self.menu else {
            return;
        };
        if menu.closing.is_some() {
            return;
        }
        match event.keystroke.key.as_str() {
            "down" => self.move_highlight(1, cx),
            "up" => self.move_highlight(-1, cx),
            "home" => {
                self.move_highlight(-(menu.highlight as i32) - 1, cx);
                self.move_highlight(1, cx);
            }
            "end" => {
                let to = menu.items.len() as i32 - menu.highlight as i32;
                self.move_highlight(to, cx);
                self.move_highlight(-1, cx);
            }
            "enter" | "space" => {
                let item = menu
                    .items
                    .get(menu.highlight)
                    .filter(|item| item.enabled)
                    .cloned();
                if let Some(item) = item {
                    self.run(item.action, window, cx);
                }
            }
            "escape" | "tab" => {
                self.close_menu(cx);
                window.focus(&self.focus, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Put the open menu just above the bar in the board's corner, from `x`:
    /// a menu opened from the bar opens upward from it.
    fn lift_menu_above_bar(&mut self, x: f32) {
        let height = self.viewport().1;
        if let Some(menu) = &mut self.menu {
            menu.at = (x, height - BAR_INSET - BAR_HEIGHT - 6. - menu.height());
        }
    }

    /// The open menu, where it was asked for and clear of the canvas's edges.
    pub(crate) fn menu_element(&self, theme: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let (vw, vh) = self.viewport();
        let height = menu.height();
        let x = menu.at.0.min(vw - WIDTH - 8.).max(8.);
        let upward = menu.at.1 + height > vh - 8. && menu.at.1 - height > 8.;
        let y = if upward {
            menu.at.1 - height
        } else {
            menu.at.1
        };
        let open = motion::ease_out(self.clock.progress(menu.opened, motion::BASE));
        let gone = menu.closing.map_or(0., |closing| {
            motion::ease_in(self.clock.progress(closing, motion::QUICK))
        });
        let highlight = menu.travel.value(&self.clock);
        // The highlight travels between the rows rather than jumping.
        let lit = || {
            div()
                .absolute()
                .left(px(PAD))
                .right(px(PAD))
                .top(px(highlight))
                .h(px(ROW))
                .rounded(px(metric::RADIUS))
                .bg(theme.selected)
        };
        // As the app draws it, when it does: the canvas keeps its fading
        // in and out, its keys and its travelling highlight.
        let data = serde_json::json!({
            "x": x,
            "y": y.max(8.),
            "width": WIDTH,
            "height": height,
            "title": menu.title.as_ref().map(SharedString::as_ref),
            "highlight": menu.highlight,
            "items": menu.items.iter().map(|item| self.item_data(item)).collect::<Vec<_>>(),
        });
        let slots = || vec![("highlight", lit().into_any_element())];
        if let Some(drawn) = self.faced("canvasMenu", data, slots, cx) {
            return Some(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .opacity(open * (1. - gone))
                    .track_focus(&self.menu_focus)
                    .on_key_down(cx.listener(Self::menu_key))
                    .child(drawn)
                    .into_any_element(),
            );
        }
        let rows: Vec<AnyElement> = menu
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| self.menu_row(index, item, menu.highlight == index, theme, cx))
            .collect();
        Some(
            div()
                .id("canvas-menu")
                .absolute()
                .left(px(x))
                .top(px(y.max(8.)))
                .w(px(WIDTH))
                .h(px(height))
                .rounded(px(metric::RADIUS_LARGE))
                .overflow_hidden()
                .occlude()
                .bg(theme.raised)
                .border_1()
                .border_color(theme.line_strong)
                .opacity(open * (1. - gone))
                .track_focus(&self.menu_focus)
                .on_key_down(cx.listener(Self::menu_key))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
                .child(lit())
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .w_full()
                        .py(px(PAD))
                        .flex()
                        .flex_col()
                        .when_some(menu.title.clone(), |list, title| {
                            list.child(
                                div()
                                    .h(px(TITLE))
                                    .px(px(PAD + 8.))
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.))
                                    .text_color(theme.fg_muted)
                                    .child(div().truncate().child(title)),
                            )
                        })
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// A menu's row, as a drawing is given it.
    fn item_data(&self, item: &Item) -> serde_json::Value {
        let mark = match (
            &item.code,
            item.node.and_then(|id| self.mark_of_node(id)),
            item.icon,
        ) {
            (Some(code), _, _) => serde_json::json!({ "code": code.as_ref() }),
            (None, Some(mark), _) => mark,
            (None, None, Some(glyph)) => {
                serde_json::json!({ "icon": crate::face::icon_name(glyph) })
            }
            (None, None, None) => serde_json::Value::Null,
        };
        serde_json::json!({
            "label": item.label.as_ref(),
            "detail": item.detail.as_ref().map(SharedString::as_ref),
            "hint": item.hint,
            "mark": mark,
            "danger": item.danger,
            "enabled": item.enabled,
            "separator": item.separator,
        })
    }

    fn menu_row(
        &self,
        index: usize,
        item: &Item,
        lit: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let label_color = if !item.enabled {
            theme.fg_muted
        } else if item.danger {
            theme.error
        } else {
            theme.fg
        };
        let mark: AnyElement = match (
            &item.code,
            item.node.and_then(|id| self.board.node(id)),
            item.icon,
        ) {
            (Some(code), _, _) => monogram(code, theme.hue(code), theme).into_any_element(),
            (None, Some(node), _) => glyph(node, 1., theme),
            (None, None, Some(glyph)) => icon(
                glyph,
                if item.danger {
                    theme.error
                } else if lit && item.enabled {
                    theme.fg
                } else {
                    theme.fg_muted
                },
            )
            .into_any_element(),
            (None, None, None) => div().flex_none().w(px(metric::ICON)).into_any_element(),
        };
        let action = item.action.clone();
        let enabled = item.enabled;
        div()
            .when(item.separator, |row| {
                row.child(
                    div()
                        .h(px(SEPARATOR))
                        .px(px(PAD + 8.))
                        .flex()
                        .items_center()
                        .child(div().w_full().h(px(1.)).bg(theme.line)),
                )
            })
            .child(
                div()
                    .id(("menu-row", index))
                    .h(px(ROW))
                    .mx(px(PAD))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(metric::RADIUS))
                    .when(enabled, |row| row.cursor_pointer())
                    .on_mouse_move(cx.listener(move |this, _, _, cx| this.hover_row(index, cx)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            if enabled {
                                this.run(action.clone(), window, cx);
                            }
                        }),
                    )
                    .child(mark)
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(metric::TEXT))
                            .text_color(label_color)
                            .child(item.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(metric::TEXT_SMALL))
                            .text_color(theme.fg_muted)
                            .children(item.detail.clone()),
                    )
                    .children(item.hint.map(|hint| key_hint(hint, theme))),
            )
            .into_any_element()
    }

    /// The bar's Add: what can be added, in a menu over the bar, for the
    /// middle of the board.
    pub(crate) fn add_from_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (w, h) = self.viewport();
        let world = self.camera.to_world(w / 2., h / 2.);
        self.open_add_menu((BAR_INSET, h / 2.), world, None, window, cx);
        self.lift_menu_above_bar(BAR_INSET);
    }

    /// The bar's zoom: the zooms to go to, in a menu over the bar.
    pub(crate) fn zoom_menu_from_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (w, h) = self.viewport();
        self.open_zoom_menu((w / 2., h / 2.), window, cx);
        self.lift_menu_above_bar(BAR_INSET + 64.);
    }

    /// One of an empty board's offers, taken, in the middle of the board.
    pub(crate) fn offer_from_empty(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (w, h) = self.viewport();
        let local = (w / 2. - 128., h / 2.);
        let world = self.camera.to_world(w / 2., h / 2.);
        match action {
            Action::StartAgent if !self.offers.is_empty() => {
                self.open_add_menu(local, world, None, window, cx)
            }
            Action::Note => self.add_note(world.0 - 140., world.1 - 90., window, cx),
            Action::StartAgent => cx.emit(crate::CanvasEvent::CreateRequested {
                x: world.0,
                y: world.1,
            }),
            other => self.run(other, window, cx),
        }
    }

    /// The bar in the board's corner: add, zoom, fit. As the app draws it,
    /// around the zoom's percentage, when it does (see [`crate::Face`]).
    pub(crate) fn toolbar(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let percent =
            SharedString::from(format!("{}%", (self.target_camera().zoom * 100.).round()));
        let slots = || vec![("percent", kou_ui::scale::text(percent.clone()))];
        self.faced("canvasBar", serde_json::json!({}), slots, cx)
            .unwrap_or_else(|| self.native_toolbar(theme, cx).into_any_element())
    }

    fn native_toolbar(&self, theme: &Theme, cx: &Context<Self>) -> impl IntoElement + use<> {
        let percent =
            SharedString::from(format!("{}%", (self.target_camera().zoom * 100.).round()));
        let clear = gpui::transparent_black();
        div()
            .absolute()
            .left(px(BAR_INSET))
            .bottom(px(BAR_INSET))
            .h(px(BAR_HEIGHT))
            .px(px(PAD))
            .flex()
            .items_center()
            .gap(px(2.))
            .rounded(px(metric::RADIUS_MEDIUM))
            .bg(theme.raised)
            .border_1()
            .border_color(theme.line_strong)
            .occlude()
            .child(
                button_with_icon("canvas-add", Icon::Plus, "Add", false, theme)
                    .border_color(clear)
                    .tooltip(tip("Add a node here (A)"))
                    .on_click(cx.listener(|this, _, window, cx| this.add_from_bar(window, cx))),
            )
            .child(div().w(px(1.)).h(px(16.)).mx(px(3.)).bg(theme.line))
            .child(
                icon_button("canvas-zoom-out", Icon::Minus, theme)
                    .tooltip(tip("Zoom out (⌘−)"))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_step(-1, cx))),
            )
            .child(
                button("canvas-zoom", percent, false, theme)
                    .border_color(clear)
                    .w(px(54.))
                    .justify_center()
                    .font_family(theme.mono_font.clone())
                    .text_size(px(metric::TEXT_SMALL))
                    .text_color(theme.fg_dim)
                    .tooltip(tip("Zoom to 100%, the selection, or everything"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.zoom_menu_from_bar(window, cx)),
                    ),
            )
            .child(
                icon_button("canvas-zoom-in", Icon::Plus, theme)
                    .tooltip(tip("Zoom in (⌘=)"))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_step(1, cx))),
            )
            .child(
                icon_button("canvas-fit", Icon::Fit, theme)
                    .tooltip(tip("Fit everything (⌘0)"))
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
    }

    /// What an empty canvas offers: its templates, a first agent, a note.
    /// As the app draws it, when it does (see [`crate::Face`]).
    pub(crate) fn empty_state(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let list = templates(&self.offers);
        let templates: Vec<serde_json::Value> = list
            .iter()
            .map(|template| {
                serde_json::json!({
                    "title": template.title,
                    "text": template.description,
                    "detail": self.template_detail(template),
                })
            })
            .collect();
        let data = serde_json::json!({
            "templates": templates,
            "offers": !self.offers.is_empty(),
        });
        self.faced("canvasEmpty", data, Vec::new, cx)
            .unwrap_or_else(|| self.native_empty_state(theme, cx).into_any_element())
    }

    /// What a template makes: how many nodes and edges, and of what.
    fn template_detail(&self, template: &crate::Template) -> String {
        let names: Vec<String> = template
            .nodes
            .iter()
            .filter_map(|node| self.offers.iter().find(|offer| offer.id == node.offer))
            .map(|offer| offer.title.clone())
            .fold(Vec::new(), |mut names, name| {
                if !names.contains(&name) {
                    names.push(name);
                }
                names
            });
        format!(
            "{} nodes, {} edges: {}",
            template.nodes.len(),
            template.edges.len(),
            names.join(", ")
        )
    }

    fn native_empty_state(&self, theme: &Theme, cx: &Context<Self>) -> impl IntoElement + use<> {
        struct Card {
            glyph: Icon,
            title: SharedString,
            text: SharedString,
            detail: Option<SharedString>,
            action: Action,
            quiet: bool,
        }
        let list = templates(&self.offers);
        let mut cards: Vec<Card> = list
            .iter()
            .enumerate()
            .map(|(index, template)| Card {
                glyph: Icon::Canvas,
                title: template.title.clone().into(),
                text: template.description.clone().into(),
                detail: Some(self.template_detail(template).into()),
                action: Action::Template(index),
                quiet: false,
            })
            .collect();
        cards.push(Card {
            glyph: Icon::Thread,
            title: "An agent".into(),
            text: if self.offers.is_empty() {
                "Start one here; the app asks which.".into()
            } else {
                "Pick what to start, in the middle of the board.".into()
            },
            detail: None,
            action: Action::StartAgent,
            quiet: false,
        });
        cards.push(Card {
            glyph: Icon::Note,
            title: "A note".into(),
            text: "Text on the board, for the plan or what to remember.".into(),
            detail: None,
            action: Action::Note,
            quiet: true,
        });
        let (hover, accent, fg, chip) = (theme.hover, theme.accent, theme.fg, theme.selected);
        let cards: Vec<AnyElement> = cards
            .into_iter()
            .enumerate()
            .map(|(index, card)| {
                let group = SharedString::from(format!("card-{index}"));
                let action = card.action;
                div()
                    .id(("canvas-card", index))
                    .group(group.clone())
                    .w(px(220.))
                    .min_h(px(128.))
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .rounded(px(metric::RADIUS_LARGE))
                    .border_1()
                    .border_color(theme.line_strong)
                    .when(card.quiet, |card| card.border_dashed())
                    .when(!card.quiet, |card| card.bg(theme.raised))
                    .cursor_pointer()
                    .tab_index(0)
                    .hover(move |style| style.bg(hover))
                    .active(|style| style.opacity(0.8))
                    .focus_visible(move |style| style.border_color(accent))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.offer_from_empty(action.clone(), window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(28.))
                                    .rounded(px(metric::RADIUS))
                                    .bg(chip)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        icon(card.glyph, theme.fg_muted)
                                            .group_hover(group.clone(), move |style| {
                                                style.text_color(fg)
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(metric::TEXT))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.fg)
                                    .child(card.title),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(theme.fg_dim)
                            .child(card.text),
                    )
                    .children(card.detail.map(|detail| {
                        div()
                            .text_size(px(metric::TEXT_SMALL))
                            .line_height(px(16.))
                            .text_color(theme.fg_muted)
                            .child(detail)
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .absolute()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .pb(px(64.))
            .child(
                div()
                    .id("canvas-hint")
                    .p(px(36.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(20.))
                    .child(
                        div()
                            .max_w(px(520.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(theme.fg)
                                    .child("An empty canvas"),
                            )
                            .child(
                                div()
                                    .text_center()
                                    .text_size(px(metric::TEXT))
                                    .line_height(px(20.))
                                    .text_color(theme.fg_muted)
                                    .child(
                                        "Agents, terminals and notes, wired output to input: \
                                         what one puts out goes into the next. Start from a \
                                         template, add a node (A, or double-click the board), \
                                         or right-click for more.",
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .justify_center()
                            .gap(px(10.))
                            .max_w(px(960.))
                            .children(cards),
                    ),
            )
    }

    /// The labels of edges: off, waiting for approval, how many went along it.
    pub(crate) fn edge_labels(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        if !labels_show(self.camera.zoom) {
            return Vec::new();
        }
        let camera = self.camera;
        let label_style = gpui::TextStyle {
            font_family: theme.mono_font.clone(),
            font_weight: FontWeight::MEDIUM,
            ..Default::default()
        };
        let mut labels = Vec::new();
        for edge in self.board.edges() {
            let traffic = self.traffic.get(&edge.id).copied().unwrap_or_default();
            let mut words = Vec::new();
            // A wire an agent drew says whose it is: not the user's own.
            let by = (edge.origin == crate::model::EdgeOrigin::Wired)
                .then_some(edge.by)
                .flatten()
                .map(|by| {
                    self.board
                        .node(by)
                        .map_or("an agent", |node| node.title.as_str())
                });
            if let Some(title) = by {
                let short: String = title
                    .chars()
                    .take(14)
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                words.push(format!("by {short}"));
            }
            if edge.mode == EdgeMode::Off {
                words.push("off".to_owned());
            }
            if traffic.waiting > 0 {
                words.push(format!("{} to approve", traffic.waiting));
            } else if edge.mode == EdgeMode::Approve {
                words.push("approve".to_owned());
            }
            if traffic.delivered > 0 {
                words.push(traffic.delivered.to_string());
            }
            if words.is_empty() {
                continue;
            }
            let (Some(from), Some(to)) = (self.board.node(edge.from), self.board.node(edge.to))
            else {
                continue;
            };
            let c = curve(
                self.drawn_port(from, Dir::Out, &edge.from_port),
                self.drawn_port(to, Dir::In, &edge.to_port),
            );
            let (x, y) = c.middle();
            let (x, y) = camera.to_screen(x, y);
            let text = words.join(" · ");
            // In world units, as the web canvas draws its labels in the board's SVG.
            let unit = camera.zoom;
            let run = label_style.to_run(text.len());
            let line = window
                .text_system()
                .layout_line(&text, px(10.), &[run], None);
            let width = (14. + f32::from(line.width)) * unit;
            let selected = self.edge_selected == Some(edge.id);
            let id = edge.id;
            let pressed = cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                window.focus(&this.focus, cx);
                if event.click_count == 2 {
                    this.open_inspector_for_edge(id, cx);
                } else {
                    this.set_selection(Vec::new(), Some(id), cx);
                }
            });
            // As the app draws it, when it does (see [`crate::Face`]): the
            // pill and what it says, at the zoom; the canvas puts it on its
            // edge, as large as its words.
            let shows = crate::face::Shows::Label {
                text: text.clone(),
                waiting: traffic.waiting > 0,
                selected,
            };
            let data = || {
                serde_json::json!({
                    "edge": id,
                    "text": text,
                    "waiting": traffic.waiting > 0,
                    "selected": selected,
                })
            };
            if let Some(pill) = self.node_faced(id, "canvasLabel", shows, data, Vec::new, unit, cx)
            {
                labels.push(
                    div()
                        .absolute()
                        .left(px(x - width / 2.))
                        .top(px(y - 9. * unit))
                        .w(px(width))
                        .h(px(18. * unit))
                        .on_mouse_down(MouseButton::Left, pressed)
                        .child(pill)
                        .into_any_element(),
                );
                continue;
            }
            let color = if selected {
                theme.accent
            } else if traffic.waiting > 0 {
                theme.blocked
            } else {
                theme.fg_muted
            };
            let id = edge.id;
            labels.push(
                div()
                    .id(("edge-label", id as usize))
                    .absolute()
                    .left(px(x - width / 2.))
                    .top(px(y - 9. * unit))
                    .w(px(width))
                    .h(px(18. * unit))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(metric::RADIUS_SMALL * unit))
                    .bg(theme.raised)
                    .border_1()
                    .border_color(if selected {
                        theme.accent
                    } else if traffic.waiting > 0 {
                        theme.blocked
                    } else {
                        theme.line_strong
                    })
                    .font_family(theme.mono_font.clone())
                    .text_size(px(10. * unit))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color)
                    .cursor_pointer()
                    .child(crate::card::zoomed(text, unit))
                    .on_mouse_down(MouseButton::Left, pressed)
                    .into_any_element(),
            );
        }
        labels
    }

    /// The ends of the selected edge, to drag to other ports.
    pub(crate) fn edge_handles(&self, theme: &Theme, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(id) = self.edge_selected else {
            return Vec::new();
        };
        if matches!(self.gesture, Some(Gesture::Wire(_))) {
            return Vec::new();
        }
        let Some(edge) = self.board.edge(id) else {
            return Vec::new();
        };
        let (Some(from), Some(to)) = (self.board.node(edge.from), self.board.node(edge.to)) else {
            return Vec::new();
        };
        let ends = [
            (End::From, self.drawn_port(from, Dir::Out, &edge.from_port)),
            (End::To, self.drawn_port(to, Dir::In, &edge.to_port)),
        ];
        let accent = theme.accent;
        ends.into_iter()
            .map(|(end, at)| {
                let (x, y) = self.camera.to_screen(at.0, at.1);
                div()
                    .id(SharedString::from(format!("edge-end-{id}-{end:?}")))
                    .absolute()
                    .left(px(x - 6.))
                    .top(px(y - 6.))
                    .size(px(12.))
                    .rounded_full()
                    .border_2()
                    .border_color(accent)
                    .bg(theme.pane)
                    .cursor(CursorStyle::Crosshair)
                    .hover(move |style| style.bg(accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.edge_end_down(id, end, event, window, cx)
                        }),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// The marquee being drawn.
    pub(crate) fn marquee(&self, theme: &Theme) -> Option<Div> {
        let Some(Gesture::Marquee {
            from,
            to,
            moved: true,
            ..
        }) = &self.gesture
        else {
            return None;
        };
        let a = self.camera.to_screen(from.0, from.1);
        let b = self.camera.to_screen(to.0, to.1);
        Some(
            div()
                .absolute()
                .left(px(a.0.min(b.0)))
                .top(px(a.1.min(b.1)))
                .w(px((a.0 - b.0).abs()))
                .h(px((a.1 - b.1).abs()))
                .rounded(px(metric::RADIUS_SMALL))
                .border_1()
                .border_color(theme.accent)
                .bg(theme.accent.opacity(0.07)),
        )
    }

    /// The bar that says what was just deleted, and takes it back.
    pub(crate) fn toast_element(&self, theme: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let toast = self.toast.as_ref()?;
        let now = self.clock.now();
        if toast.until <= now {
            return None;
        }
        let t = motion::ease_out(self.clock.progress(toast.shown, motion::BASE));
        let left = toast.until.saturating_duration_since(now);
        let out = if left < motion::QUICK {
            1. - left.as_secs_f32() / motion::QUICK.as_secs_f32()
        } else {
            0.
        };
        // As the app draws it, when it does: the canvas keeps it rising
        // and fading in, and fading out.
        let data = serde_json::json!({ "text": toast.text, "undo": toast.undo });
        if let Some(bar) = self.faced("canvasToast", data, Vec::new, cx) {
            return Some(
                div()
                    .absolute()
                    .bottom(px(16. + (1. - t) * 10.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .opacity(t * (1. - out))
                    .child(bar)
                    .into_any_element(),
            );
        }
        Some(
            div()
                .absolute()
                .bottom(px(16. + (1. - t) * 10.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .h(px(40.))
                        .pl(px(14.))
                        .pr(px(PAD + 3.))
                        .rounded(px(metric::RADIUS_MEDIUM))
                        .bg(theme.raised)
                        .border_1()
                        .border_color(theme.line_strong)
                        .opacity(t * (1. - out))
                        .occlude()
                        .text_size(px(metric::TEXT))
                        .text_color(theme.fg_dim)
                        .child(icon(
                            if toast.undo { Icon::Trash } else { Icon::Info },
                            theme.fg_muted,
                        ))
                        .child(toast.text.clone())
                        .when(toast.undo, |bar| {
                            bar.child(
                                button_with_icon("canvas-undo", Icon::Undo, "Undo", false, theme)
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toast = None;
                                        this.undo(cx);
                                    })),
                            )
                            .child(key_hint("⌘Z", theme))
                        }),
                )
                .into_any_element(),
        )
    }
}
