//! The inspector: a panel over the right of the board with what is selected,
//! as the web canvas has it. A node: its title, what it is, its state, where
//! it works, its wires in and out. Several nodes: which. An edge: its ends,
//! its mode, when it delivers, whether a message says where it comes from,
//! its template and what that makes of an output. A change applies as it is
//! made, and undo takes it back. The panel follows the selection while open.

use crate::{
    anim::Clock,
    card::{elapsed, field_line, glyph, state_of},
    model::{Delivery, EdgeId, EdgeMode, EdgeOrigin, End, NodeId, NodeKind},
    view::{CanvasView, Edit},
};
use gpui::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px,
};
use kou_ui::{
    Icon, Mark, Theme, button_with_icon, icon, icon_button, metric, motion, status_mark, tip,
};
use std::time::Instant;

/// The panel's width.
const WIDTH: f32 = 320.;

/// What the inspector shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Nodes(Vec<NodeId>),
    Edge(EdgeId),
}

pub(crate) struct Inspector {
    pub target: Target,
    pub opened: Instant,
    pub closing: Option<Instant>,
}

impl Inspector {
    pub(crate) fn moving(&self, clock: &Clock) -> bool {
        clock.progress(self.opened, motion::BASE) < 1. || self.closing.is_some()
    }

    pub(crate) fn gone(&self, clock: &Clock) -> bool {
        self.closing
            .is_some_and(|closing| clock.progress(closing, motion::QUICK) >= 1.)
    }
}

impl CanvasView {
    /// Open the inspector on what is selected: what I does.
    pub fn inspect(&mut self, cx: &mut Context<Self>) {
        self.open_inspector(cx);
    }

    pub(crate) fn open_inspector(&mut self, cx: &mut Context<Self>) {
        let target = match self.edge_selected {
            Some(edge) if self.selection.is_empty() => Target::Edge(edge),
            _ if !self.selection.is_empty() => Target::Nodes(self.selection.clone()),
            _ => return,
        };
        let opened = match &self.inspector {
            Some(inspector) if inspector.closing.is_none() => inspector.opened,
            _ => self.clock.now(),
        };
        self.inspector = Some(Inspector {
            target,
            opened,
            closing: None,
        });
        cx.notify();
    }

    pub(crate) fn open_inspector_for_edge(&mut self, edge: EdgeId, cx: &mut Context<Self>) {
        self.set_selection(Vec::new(), Some(edge), cx);
        self.open_inspector(cx);
    }

    pub(crate) fn close_inspector(&mut self, cx: &mut Context<Self>) {
        if let Some(inspector) = &mut self.inspector
            && inspector.closing.is_none()
        {
            inspector.closing = Some(self.clock.now());
            cx.notify();
        }
    }

    pub(crate) fn toggle_inspector(&mut self, cx: &mut Context<Self>) {
        if self.inspector.as_ref().is_some_and(|i| i.closing.is_none()) {
            self.close_inspector(cx);
        } else {
            self.open_inspector(cx);
        }
    }

    /// While open, the inspector shows what is selected now; with nothing
    /// selected, it closes.
    pub(crate) fn follow_selection(&mut self) {
        let now = self.clock.now();
        let Some(inspector) = &mut self.inspector else {
            return;
        };
        if inspector.closing.is_some() {
            return;
        }
        if !self.selection.is_empty() {
            inspector.target = Target::Nodes(self.selection.clone());
        } else if let Some(edge) = self.edge_selected {
            inspector.target = Target::Edge(edge);
        } else {
            inspector.closing = Some(now);
        }
    }

    pub(crate) fn inspector_element(
        &self,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let inspector = self.inspector.as_ref()?;
        let open = motion::ease_out(self.clock.progress(inspector.opened, motion::BASE));
        let gone = inspector.closing.map_or(0., |closing| {
            motion::ease_in(self.clock.progress(closing, motion::QUICK))
        });
        let shown = open * (1. - gone);
        let (title, glyph_element, body): (SharedString, AnyElement, Vec<AnyElement>) =
            match &inspector.target {
                Target::Nodes(ids) if ids.len() == 1 => {
                    let node = self.board.node(ids[0])?;
                    (
                        node.title.clone().into(),
                        glyph(node, 1., theme),
                        self.node_sections(ids[0], theme, cx),
                    )
                }
                Target::Nodes(ids) => (
                    format!("{} nodes selected", ids.len()).into(),
                    icon(Icon::Canvas, theme.fg_muted).into_any_element(),
                    self.many_sections(ids, theme, cx),
                ),
                Target::Edge(id) => (
                    "Edge".into(),
                    icon(Icon::Wire, theme.accent).into_any_element(),
                    self.edge_sections(*id, theme, cx),
                ),
            };
        Some(
            div()
                .id("canvas-inspector")
                .absolute()
                .top(px(12.))
                .bottom(px(12.))
                .right(px(12. - (1. - shown) * 24.))
                .w(px(WIDTH))
                .flex()
                .flex_col()
                .rounded(px(metric::RADIUS_LARGE))
                .overflow_hidden()
                .bg(theme.raised)
                .border_1()
                .border_color(theme.line_strong)
                .opacity(shown)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" && this.editing.is_none() {
                        this.close_inspector(cx);
                        window.focus(&this.focus, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    div()
                        .flex_none()
                        .h(px(metric::BAR + 6.))
                        .pl(px(14.))
                        .pr(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .border_b_1()
                        .border_color(theme.line)
                        .child(glyph_element)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(metric::TEXT))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.fg)
                                .child(title),
                        )
                        .child(
                            icon_button("inspector-close", Icon::Close, theme)
                                .tooltip(tip("Close (I or Esc)"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_inspector(cx);
                                    window.focus(&this.focus, cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .id("inspector-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(px(14.))
                        .pb(px(16.))
                        .children(body),
                )
                .into_any_element(),
        )
    }

    fn node_sections(&self, id: NodeId, theme: &Theme, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(node) = self.board.node(id) else {
            return Vec::new();
        };
        let live = self.live.get(&id);
        let mark = live.map_or(Mark::Idle, |live| live.mark);
        let (word, color) = state_of(mark, theme);
        let since = live.map(|live| self.clock.now().saturating_duration_since(live.since));
        let mut sections = Vec::new();

        let title = self.editable(
            "inspector-title",
            Edit::Title(id, true),
            &node.title,
            "Title",
            theme,
            cx,
        );
        let kind = match node.kind {
            NodeKind::Agent if !node.tag.is_empty() => format!("agent, {}", node.tag),
            kind => kind.label().to_owned(),
        };
        let mut head = section(None, None, theme).child(title).child(
            div()
                .flex()
                .justify_between()
                .text_size(px(metric::TEXT_SMALL))
                .text_color(theme.fg_muted)
                .child(kind)
                .child(
                    div()
                        .font_family(theme.mono_font.clone())
                        .child(format!("#{id}")),
                ),
        );
        if node.kind != NodeKind::Note {
            head = head.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(metric::TEXT_SMALL))
                    .child(status_mark(mark, theme))
                    .child(
                        div()
                            .text_color(if mark == Mark::Idle {
                                theme.fg_muted
                            } else {
                                color
                            })
                            .child(word),
                    )
                    .children(since.map(|since| {
                        div()
                            .text_color(theme.fg_muted)
                            .child(format!("for {}", elapsed(since)))
                    })),
            );
        }
        if !node.place.is_empty() {
            head = head.child(row(Icon::Folder, "Where", node.place.clone(), theme));
        }
        sections.push(head.into_any_element());

        if node.kind == NodeKind::Note {
            let text = self.editable(
                "inspector-note",
                Edit::Note(id),
                &node.detail,
                "Text",
                theme,
                cx,
            );
            sections.push(
                section(Some(Icon::Note), Some("Note"), theme)
                    .child(text)
                    .into_any_element(),
            );
        }

        if !node.kind.inputs().is_empty() || !node.kind.outputs().is_empty() {
            let mut wires = section(Some(Icon::Wire), Some("Wires"), theme);
            let edges: Vec<_> = self.board.edges_of(id).cloned().collect();
            if edges.is_empty() {
                wires = wires.child(note_text(
                    "Nothing wired. Drag from a port on its side, or wire it here.",
                    theme,
                ));
            }
            // The rows sit together, as the rail's do.
            let mut rows = div().flex().flex_col();
            for edge in edges {
                let (arrow, other, port) = if edge.from == id {
                    ("→", edge.to, edge.from_port.clone())
                } else {
                    ("←", edge.from, edge.to_port.clone())
                };
                let other_node = self.board.node(other);
                let other = other_node.map_or_else(|| other.to_string(), |node| node.title.clone());
                let mode = match edge.mode {
                    EdgeMode::Auto => String::new(),
                    mode => format!(", {}", mode.label().to_lowercase()),
                };
                let edge_id = edge.id;
                rows = rows.child(
                    choice_row(
                        SharedString::from(format!("inspector-wire-{edge_id}")),
                        other_node.map(|node| glyph(node, 1., theme)),
                        format!("{port} {arrow} {other}{mode}"),
                        theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_selection(Vec::new(), Some(edge_id), cx);
                    })),
                );
            }
            wires = wires.child(rows);
            if !node.kind.outputs().is_empty() {
                wires = wires.child(
                    div().flex().child(
                        action_button(
                            "inspector-wire-to",
                            Icon::Wire,
                            "Wire its output to…",
                            false,
                            theme,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run(crate::menu::Action::WireFrom(id), window, cx);
                            },
                        )),
                    ),
                );
            }
            sections.push(wires.into_any_element());
        }

        let mut actions = div().flex().flex_wrap().gap(px(6.));
        if self.bodies.contains_key(&id) {
            actions = actions.child(
                action_button("inspector-enter", Icon::ChevronRight, "Enter", false, theme)
                    .tooltip(tip("Its body takes the keys (Enter)"))
                    .on_click(cx.listener(move |this, _, window, cx| this.enter(id, window, cx))),
            );
        }
        if !node.kind.canvas_owned() {
            actions = actions.child(
                action_button("inspector-open", Icon::Pane, "Open as a pane", false, theme)
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(crate::CanvasEvent::Opened(id))),
                    ),
            );
        }
        actions = actions
            .child(
                action_button("inspector-focus", Icon::Target, "Focus", false, theme)
                    .tooltip(tip("At 100%, in the middle (F)"))
                    .on_click(cx.listener(move |this, _, _, cx| this.focus_node(id, cx))),
            )
            .child(
                action_button(
                    "inspector-duplicate",
                    Icon::Duplicate,
                    "Duplicate",
                    false,
                    theme,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_selection(vec![id], None, cx);
                    this.duplicate_selection(window, cx);
                })),
            )
            .child(
                action_button("inspector-delete", Icon::Trash, "Delete", true, theme).on_click(
                    cx.listener(move |this, _, window, cx| this.ask_to_remove(&[id], window, cx)),
                ),
            );
        sections.push(section(None, None, theme).child(actions).into_any_element());
        sections
    }

    fn many_sections(&self, ids: &[NodeId], theme: &Theme, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut rows = div().flex().flex_col();
        for id in ids {
            let Some(node) = self.board.node(*id) else {
                continue;
            };
            let id = *id;
            rows = rows.child(
                choice_row(
                    SharedString::from(format!("inspector-pick-{id}")),
                    Some(glyph(node, 1., theme)),
                    node.title.clone(),
                    theme,
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.set_selection(vec![id], None, cx)),
                ),
            );
        }
        let list = section(None, None, theme)
            .child(note_text("Pick one to inspect it alone.", theme))
            .child(rows);
        let actions = div()
            .flex()
            .gap(px(6.))
            .child(
                action_button(
                    "inspector-fit-selection",
                    Icon::Target,
                    "Zoom to them",
                    false,
                    theme,
                )
                .on_click(cx.listener(|this, _, _, cx| this.zoom_to_selection(cx))),
            )
            .child(
                action_button("inspector-delete-all", Icon::Trash, "Delete", true, theme).on_click(
                    cx.listener(|this, _, window, cx| {
                        let ids = this.selection.clone();
                        this.ask_to_remove(&ids, window, cx);
                    }),
                ),
            );
        vec![
            list.into_any_element(),
            section(None, None, theme).child(actions).into_any_element(),
        ]
    }

    fn edge_sections(&self, id: EdgeId, theme: &Theme, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(edge) = self.board.edge(id).cloned() else {
            return Vec::new();
        };
        let from = self.board.node(edge.from);
        let to = self.board.node(edge.to);
        let from_title = from.map_or_else(|| edge.from.to_string(), |n| n.title.clone());
        let to_title = to.map_or_else(|| edge.to.to_string(), |n| n.title.clone());
        let target_kind = to.map_or(NodeKind::Agent, |n| n.kind);
        let mut sections = Vec::new();

        let end = |node: Option<&crate::model::Node>, title: String, port: String| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .children(node.map(|node| glyph(node, 1., theme)))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.fg)
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(
                    div()
                        .font_family(theme.mono_font.clone())
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_muted)
                        .child(port),
                )
        };
        let ends = section(None, None, theme)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_size(px(metric::TEXT))
                    .child(end(from, from_title.clone(), edge.from_port.clone()))
                    .child(div().pl(px(4.)).text_color(theme.fg_muted).child("↓"))
                    .child(end(to, to_title, edge.to_port.clone())),
            )
            .child(note_text(
                match edge.origin {
                    EdgeOrigin::Spawned => "Made when one started the other.",
                    EdgeOrigin::Wired => "Wired by hand.",
                },
                theme,
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(
                        action_button(
                            "inspector-move-start",
                            Icon::Wire,
                            "Move its start…",
                            false,
                            theme,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run(crate::menu::Action::EdgeEnds(id, End::From), window, cx)
                            },
                        )),
                    )
                    .child(
                        action_button(
                            "inspector-move-end",
                            Icon::Wire,
                            "Move its end…",
                            false,
                            theme,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run(crate::menu::Action::EdgeEnds(id, End::To), window, cx)
                            },
                        )),
                    ),
            );
        sections.push(ends.into_any_element());

        let modes = segmented(
            "inspector-mode",
            EdgeMode::ALL
                .iter()
                .map(|mode| (mode.label(), *mode == edge.mode))
                .collect(),
            theme,
            cx,
            move |this, index, cx| this.set_edge_mode(id, EdgeMode::ALL[index], cx),
        );
        sections.push(
            section(Some(Icon::Sliders), Some("Mode"), theme)
                .child(modes)
                .child(note_text(edge.mode.description(), theme))
                .into_any_element(),
        );

        let delivery = edge.delivery(target_kind);
        let deliveries = [Delivery::Queue, Delivery::Now];
        sections.push(
            section(Some(Icon::Queue), Some("When the target is busy"), theme)
                .child(segmented(
                    "inspector-delivery",
                    deliveries
                        .iter()
                        .map(|d| (d.label(), *d == delivery))
                        .collect(),
                    theme,
                    cx,
                    move |this, index, cx| {
                        let chosen = deliveries[index];
                        this.change_edge(id, |edge| edge.deliver = Some(chosen), cx)
                    },
                ))
                .child(note_text(
                    match delivery {
                        Delivery::Queue => "The message waits until its target is idle.",
                        Delivery::Now => "The message goes at once, into what runs.",
                    },
                    theme,
                ))
                .into_any_element(),
        );

        let header_default = NodeKind::Agent == target_kind;
        let headers: [(String, Option<bool>); 3] = [
            (
                format!("Default ({})", if header_default { "on" } else { "off" }),
                None,
            ),
            ("On".to_owned(), Some(true)),
            ("Off".to_owned(), Some(false)),
        ];
        let labels: Vec<(&str, bool)> = headers
            .iter()
            .map(|(label, value)| (label.as_str(), *value == edge.header))
            .collect();
        let choices: Vec<Option<bool>> = headers.iter().map(|h| h.1).collect();
        sections.push(
            section(
                Some(Icon::Info),
                Some("A line naming where it comes from"),
                theme,
            )
            .child(segmented(
                "inspector-header",
                labels,
                theme,
                cx,
                move |this, index, cx| {
                    let chosen = choices[index];
                    this.change_edge(id, |edge| edge.header = chosen, cx)
                },
            ))
            .into_any_element(),
        );

        let template = self.editable(
            "inspector-template",
            Edit::Template(id),
            &edge.template,
            "{{text}}",
            theme,
            cx,
        );
        let preview = edge.message("its output", &from_title, target_kind);
        sections.push(
            section(Some(Icon::Pencil), Some("Template"), theme)
                .child(template)
                .child(note_text(
                    "{{text}} {{from.title}} {{from.id}} {{from.port}} {{edge.id}}",
                    theme,
                ))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.fg_muted)
                        .child("What an output makes"),
                )
                .child(
                    div()
                        .p(px(8.))
                        .rounded(px(metric::RADIUS))
                        .border_1()
                        .border_color(theme.line)
                        .bg(theme.pane)
                        .font_family(theme.mono_font.clone())
                        .text_size(px(11.5))
                        .line_height(px(17.))
                        .text_color(theme.fg_dim)
                        .children(
                            preview
                                .lines()
                                .map(|line| div().child(SharedString::from(line.to_owned()))),
                        ),
                )
                .into_any_element(),
        );

        let traffic = self.traffic.get(&id).copied().unwrap_or_default();
        sections.push(
            section(Some(Icon::Thread), Some("Messages"), theme)
                .child(note_text(
                    if traffic == crate::view::Traffic::default() {
                        "Nothing went along it yet.".to_owned()
                    } else {
                        format!(
                            "{} delivered, {} waiting for your approval",
                            traffic.delivered, traffic.waiting
                        )
                    },
                    theme,
                ))
                .child(
                    div().flex().child(
                        action_button(
                            "inspector-delete-edge",
                            Icon::Trash,
                            "Delete the edge",
                            true,
                            theme,
                        )
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.delete_edge(id, window, cx)
                            }),
                        ),
                    ),
                )
                .into_any_element(),
        );
        sections
    }

    /// A value that becomes a field when clicked (or Enter on it).
    fn editable(
        &self,
        id: &'static str,
        what: Edit,
        value: &str,
        placeholder: &'static str,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let editing = self.editing.as_ref().filter(|editing| editing.what == what);
        let multiline = matches!(what, Edit::Note(_));
        // A name or a note is prose; a template is closer to code.
        let machine = matches!(what, Edit::Template(_));
        let base = div()
            .id(id)
            .min_h(px(32.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(metric::RADIUS))
            .border_1()
            .bg(theme.pane)
            .when(machine, |field| {
                field
                    .font_family(theme.mono_font.clone())
                    .text_size(px(12.))
            })
            .when(!machine, |field| field.text_size(px(metric::TEXT)))
            .line_height(px(17.));
        if let Some(editing) = editing {
            return base
                .border_color(theme.accent)
                .text_color(theme.fg)
                .track_focus(&self.field_focus)
                .on_key_down(cx.listener(Self::field_key))
                .child(if multiline {
                    crate::card::field_lines("inspector-field", &editing.field, theme, px(15.))
                } else {
                    field_line("inspector-field", &editing.field, theme, px(15.))
                })
                .into_any_element();
        }
        let (ring, hover) = (theme.accent, theme.fg_faint);
        let lines: Vec<String> = if value.is_empty() {
            vec![placeholder.to_owned()]
        } else {
            value.lines().map(str::to_owned).collect()
        };
        base.border_color(theme.line_strong)
            .text_color(if value.is_empty() {
                theme.fg_muted
            } else {
                theme.fg
            })
            .cursor(gpui::CursorStyle::IBeam)
            .tab_index(0)
            .hover(move |style| style.border_color(hover))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(
                cx.listener(move |this, _, window, cx| this.start_edit(what.clone(), window, cx)),
            )
            .children(lines.into_iter().map(|line| div().truncate().child(line)))
            .into_any_element()
    }
}

/// A part of the inspector, under a line, with its icon and heading.
fn section(glyph: Option<Icon>, title: Option<&'static str>, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .py(px(14.))
        .border_b_1()
        .border_color(theme.line)
        .when_some(title, |section, title| {
            section.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .children(glyph.map(|glyph| icon(glyph, theme.fg_muted)))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.fg_muted)
                            .child(title),
                    ),
            )
        })
}

fn row(glyph: Icon, label: &'static str, value: String, theme: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(metric::TEXT_SMALL))
        .child(icon(glyph, theme.fg_muted))
        .child(div().flex_none().text_color(theme.fg_muted).child(label))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_right()
                .font_family(theme.mono_font.clone())
                .text_color(theme.fg)
                .child(value),
        )
}

fn note_text(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .text_size(px(metric::TEXT_SMALL))
        .line_height(px(16.))
        .text_color(theme.fg_muted)
        .child(text.into())
}

/// A row to pick, as the rail's rows are: the mark of what it leads to, if any.
fn choice_row(
    id: SharedString,
    mark: Option<AnyElement>,
    label: String,
    theme: &Theme,
) -> Stateful<Div> {
    let (hover, ring) = (theme.hover, theme.accent);
    div()
        .id(id)
        .h(px(metric::ROW))
        .mx(px(-8.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(gpui::transparent_black())
        .text_size(px(12.))
        .text_color(theme.fg_dim)
        .cursor_pointer()
        .tab_index(0)
        .hover(move |style| style.bg(hover))
        .focus_visible(move |style| style.border_color(ring))
        .children(mark)
        .child(div().min_w_0().truncate().child(label))
}

/// The app's button with an icon before its label; a danger in the error colour.
fn action_button(
    id: &'static str,
    glyph: Icon,
    label: &'static str,
    danger: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let button = button_with_icon(id, glyph, label, false, theme).text_size(px(12.));
    if danger {
        button.text_color(theme.error)
    } else {
        button
    }
}

/// A choice of a few, as one rounded strip of buttons with the chosen one filled.
fn segmented(
    id: &'static str,
    options: Vec<(&str, bool)>,
    theme: &Theme,
    cx: &Context<CanvasView>,
    pick: impl Fn(&mut CanvasView, usize, &mut Context<CanvasView>) + Clone + 'static,
) -> Div {
    let (hover, ring, chosen) = (theme.hover, theme.accent, theme.selected);
    div()
        .flex()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(theme.line_strong)
        .bg(theme.pane)
        .children(options.into_iter().enumerate().map(|(index, (label, on))| {
            let pick = pick.clone();
            div()
                .id((id, index))
                .flex_1()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(metric::RADIUS_SMALL))
                .border_1()
                .border_color(gpui::transparent_black())
                .text_size(px(12.))
                .text_color(if on { theme.fg } else { theme.fg_muted })
                .when(on, |button| {
                    button.bg(chosen).font_weight(FontWeight::MEDIUM)
                })
                .when(!on, |button| button.hover(move |style| style.bg(hover)))
                .cursor_pointer()
                .tab_index(0)
                .focus_visible(move |style| style.border_color(ring))
                .on_click(cx.listener(move |this, _, _, cx| pick(this, index, cx)))
                .child(SharedString::from(label.to_owned()))
                .into_any_element()
        }))
}
