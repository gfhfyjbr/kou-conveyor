//! What agents may do to the canvas they stand on, and how messages keep
//! count of the times they were passed on.
//!
//! **Wires.** The user draws the wires of a tab's canvas. An agent may too
//! once the user has made its node the tab's orchestrator (a switch in the
//! node's inspector, which the user can turn off again): it then wires the
//! tab's nodes together, changes a wire's mode and takes wires away, with
//! `edge add`, `edge set` and `edge rm`. What it wired says so on the board
//! ([`kou_canvas::Edge::by`]). Anyone may list a tab's edges.
//!
//! **Asking.** `pane ask` sends a pane a question whose answer comes back to
//! the asker once, as a message, with nothing wired between the two: the
//! way to hear from another agent without waiting on it and without a wire
//! that would go on carrying every answer after.
//!
//! **Hops.** A wire that pipes by itself can close a ring, and then two
//! agents answer each other for ever. So a message knows how many times it
//! was passed on: what the user types counts 0, and what an agent's turn
//! sends or answers counts one more than what started that turn. The line
//! a routed message begins with says the count from the second time on,
//! and past [`kou_canvas::HOP_LIMIT`] a message is not delivered by itself:
//! it waits on its target for the user, as on an edge that asks first.
//!
//! **Models.** `harness models` lists what a harness can run with, which is
//! what the prompt field's model control is filled from (asked of the
//! harness once and kept a while); `pane spawn --model` is checked against
//! it. When the list has to be asked for first, the request waits for it.

use crate::shell::{Held, Shell};
use crate::state::Id;
use gpui::{Context, Window};
use kou_canvas::{EdgeMode, EdgeOrigin, HOP_LIMIT, NodeId, PortRef, WireError, canvas_header};
use kou_ipc::{Reply, Response};
use std::time::Duration;

/// How long a harness's models are taken as known before it is asked again.
const MODELS_FRESH: Duration = Duration::from_secs(600);

/// A question one pane asked another: the asked pane's next answer goes to
/// the asker, once.
pub(crate) struct Ask {
    pub from: Id,
    pub to: Id,
}

/// A request that waits for a harness to say its models.
pub(crate) enum Deferred {
    /// `harness models`.
    Models { harness: String, reply: Reply },
    /// `pane spawn --model`, to be checked against them and then done.
    Spawn {
        spawn: Box<kou_ipc::Spawn>,
        reply: Reply,
    },
}

/// What an orchestrator asks of its tab's edges.
pub(crate) enum EdgeChange {
    Add(Option<kou_ipc::EdgeMode>),
    Set(kou_ipc::EdgeMode),
    Remove,
}

fn mode_of(mode: kou_ipc::EdgeMode) -> EdgeMode {
    match mode {
        kou_ipc::EdgeMode::Auto => EdgeMode::Auto,
        kou_ipc::EdgeMode::Approve => EdgeMode::Approve,
        kou_ipc::EdgeMode::Off => EdgeMode::Off,
    }
}

fn refused(message: impl Into<String>) -> Response {
    Response::Error {
        message: message.into(),
    }
}

impl Shell {
    /// How a node of a tab's canvas is named to agents: its pane's id, or
    /// `n` and its number when it is no pane.
    fn node_name(&self, tab: &str, node: NodeId) -> String {
        self.store
            .panes_in(tab)
            .into_iter()
            .find(|pane| pane.node == node)
            .map_or_else(|| format!("n{node}"), |pane| pane.id.clone())
    }

    /// The node of a tab's canvas an agent names, the other way round.
    fn node_named(&mut self, tab: &str, name: &str, cx: &mut Context<Self>) -> Option<NodeId> {
        if let Some(pane) = self
            .store
            .panes_in(tab)
            .into_iter()
            .find(|pane| pane.id == name)
        {
            return Some(pane.node);
        }
        let node: NodeId = name.strip_prefix('n')?.parse().ok()?;
        let canvas = self.canvas(tab, cx)?;
        canvas.read(cx).board().node(node).map(|node| node.id)
    }

    /// The edges of a tab's canvas, as agents are told them.
    pub(crate) fn tab_edges(&mut self, tab: &str, cx: &mut Context<Self>) -> Vec<kou_ipc::Edge> {
        let Some(canvas) = self.canvas(tab, cx) else {
            return Vec::new();
        };
        let edges: Vec<kou_canvas::Edge> = canvas.read(cx).board().edges().to_vec();
        edges
            .into_iter()
            .map(|edge| kou_ipc::Edge {
                from: self.node_name(tab, edge.from),
                to: self.node_name(tab, edge.to),
                mode: match edge.mode {
                    EdgeMode::Auto => kou_ipc::EdgeMode::Auto,
                    EdgeMode::Approve => kou_ipc::EdgeMode::Approve,
                    EdgeMode::Off => kou_ipc::EdgeMode::Off,
                },
                wired: edge.origin == EdgeOrigin::Wired,
                sent: edge.origin == EdgeOrigin::Sent,
                by: edge.by.map(|by| self.node_name(tab, by)),
                waiting: self
                    .traffic
                    .get(&(tab.to_owned(), edge.id))
                    .map_or(0, |traffic| traffic.waiting),
            })
            .collect()
    }

    /// Whether the user made a pane its tab's orchestrator.
    pub(crate) fn orchestrates(&mut self, pane: &str, cx: &mut Context<Self>) -> bool {
        let Some((node, tab)) = self
            .store
            .pane(pane)
            .map(|saved| saved.node)
            .zip(self.store.tab_of(pane).map(|tab| tab.id.clone()))
        else {
            return false;
        };
        self.canvas(&tab, cx).is_some_and(|canvas| {
            canvas
                .read(cx)
                .board()
                .node(node)
                .is_some_and(|node| node.orchestrator)
        })
    }

    /// `edge list`: the edges of the tab a pane is in.
    pub(crate) fn list_edges(&mut self, pane: &str, cx: &mut Context<Self>) -> Response {
        let Some(tab) = self.store.tab_of(pane).map(|tab| tab.id.clone()) else {
            return refused(format!("no pane {pane}"));
        };
        Response::Edges {
            edges: self.tab_edges(&tab, cx),
            said: Vec::new(),
        }
    }

    /// `edge add`, `edge set`, `edge rm`: an orchestrator changes a wire of
    /// its own tab.
    pub(crate) fn change_edge(
        &mut self,
        pane: &str,
        from: &str,
        to: &str,
        change: EdgeChange,
        cx: &mut Context<Self>,
    ) -> Response {
        let Some((caller, tab)) = self
            .store
            .pane(pane)
            .map(|saved| saved.node)
            .zip(self.store.tab_of(pane).map(|tab| tab.id.clone()))
        else {
            return refused(format!("no pane {pane}"));
        };
        if !self.orchestrates(pane, cx) {
            return refused(
                "you are not this tab's orchestrator, so its wires are the user's to make: the \
                 user gives a node that right in its inspector on the canvas. `edge list` shows \
                 the wires there are.",
            );
        }
        let Some(canvas) = self.canvas(&tab, cx) else {
            return refused("this tab has no canvas");
        };
        let mut ends = [0, 0];
        for (end, name) in ends.iter_mut().zip([from, to]) {
            match self.node_named(&tab, name, cx) {
                Some(node) => *end = node,
                None => {
                    return refused(format!(
                        "there is no {name} on this tab's canvas: an orchestrator wires the \
                         nodes of its own tab, which `where` lists"
                    ));
                }
            }
        }
        let [from_node, to_node] = ends;
        let existing = canvas
            .read(cx)
            .board()
            .edges()
            .iter()
            .find(|edge| {
                edge.from == from_node
                    && edge.to == to_node
                    && edge.from_port == "out"
                    && edge.to_port == "in"
            })
            .cloned();
        let mut said = Vec::new();
        // A ring is the orchestrator's to make, and to be told of.
        let ring =
            |this: &Self, canvas: &gpui::Entity<kou_canvas::CanvasView>, cx: &Context<Self>| {
                canvas
                    .read(cx)
                    .board()
                    .would_ring(from_node, to_node)
                    .map(|ring| {
                        let names: Vec<String> = ring
                            .iter()
                            .chain(ring.first())
                            .map(|node| this.node_name(&tab, *node))
                            .collect();
                        format!(
                            "That closes a ring: {}. What one answers will go round it; a message \
                         passed on more than {HOP_LIMIT} times waits on its target for the user.",
                            names.join(" -> ")
                        )
                    })
            };
        match change {
            EdgeChange::Add(mode) => {
                let mode = mode_of(mode.unwrap_or_default());
                let warned = (mode != EdgeMode::Off)
                    .then(|| ring(self, &canvas, cx))
                    .flatten();
                match existing {
                    Some(edge) if edge.origin == EdgeOrigin::Wired => {
                        said.push(format!("{from} -> {to} was wired already."));
                        if edge.mode != mode {
                            canvas.update(cx, |canvas, cx| {
                                canvas.update_edge(
                                    edge.id,
                                    |edge| {
                                        edge.mode = mode;
                                        edge.by = Some(caller);
                                    },
                                    cx,
                                )
                            });
                        }
                    }
                    // The line that showed who started or wrote to whom
                    // becomes the wire: there is one edge between two ports.
                    Some(edge) => canvas.update(cx, |canvas, cx| {
                        canvas.update_edge(
                            edge.id,
                            |edge| {
                                edge.origin = EdgeOrigin::Wired;
                                edge.mode = mode;
                                edge.by = Some(caller);
                            },
                            cx,
                        )
                    }),
                    None => {
                        let made = canvas.update(cx, |canvas, cx| {
                            canvas.try_connect(
                                PortRef::new(from_node, "out"),
                                PortRef::new(to_node, "in"),
                                cx,
                            )
                        });
                        match made {
                            Ok(id) => canvas.update(cx, |canvas, cx| {
                                canvas.update_edge(
                                    id,
                                    |edge| {
                                        edge.mode = mode;
                                        edge.by = Some(caller);
                                    },
                                    cx,
                                )
                            }),
                            Err(WireError::SameNode) => {
                                return refused("a node cannot be wired to itself");
                            }
                            Err(WireError::NoSuchPort) => {
                                return refused(format!(
                                    "{from} has nothing to put out, or {to} takes nothing in: a \
                                     note has no ports, and a source only puts out"
                                ));
                            }
                            Err(_) => return refused(format!("{from} -> {to} could not be wired")),
                        }
                    }
                }
                said.extend(warned);
            }
            EdgeChange::Set(mode) => {
                let mode = mode_of(mode);
                let Some(edge) = existing.filter(|edge| edge.origin == EdgeOrigin::Wired) else {
                    return refused(format!(
                        "{from} -> {to} is not wired: `edge add {from} {to}` wires it, with the \
                         mode you want"
                    ));
                };
                let was = edge.mode;
                canvas.update(cx, |canvas, cx| {
                    canvas.update_edge(
                        edge.id,
                        |edge| {
                            edge.mode = mode;
                            edge.by = Some(caller);
                        },
                        cx,
                    )
                });
                // Switched on again, it may close a ring that was open.
                if was == EdgeMode::Off && mode != EdgeMode::Off {
                    canvas.update(cx, |canvas, cx| {
                        canvas.update_edge(edge.id, |edge| edge.mode = EdgeMode::Off, cx)
                    });
                    let warned = ring(self, &canvas, cx);
                    canvas.update(cx, |canvas, cx| {
                        canvas.update_edge(edge.id, |edge| edge.mode = mode, cx)
                    });
                    said.extend(warned);
                }
            }
            EdgeChange::Remove => {
                let Some(edge) = existing.filter(|edge| edge.origin == EdgeOrigin::Wired) else {
                    return refused(format!(
                        "{from} -> {to} is not wired. A line that only shows who started or \
                         wrote to whom carries nothing, and is the user's to remove."
                    ));
                };
                // Between a starter and what it started the line stays, as
                // it was before it was wired.
                let related = {
                    let board = canvas.read(cx);
                    let board = board.board();
                    board.node(to_node).and_then(|node| node.parent) == Some(from_node)
                };
                if related {
                    canvas.update(cx, |canvas, cx| {
                        canvas.update_edge(
                            edge.id,
                            |edge| {
                                edge.origin = EdgeOrigin::Spawned;
                                edge.mode = EdgeMode::Auto;
                                edge.by = None;
                            },
                            cx,
                        )
                    });
                } else {
                    canvas.update(cx, |canvas, cx| canvas.disconnect(edge.id, cx));
                    self.traffic.remove(&(tab.clone(), edge.id));
                }
                // What waited on it for the user is gone with it.
                self.held
                    .retain(|held| !(held.tab == tab && held.edge == edge.id));
                self.show_pending(&tab, cx);
            }
        }
        self.save(cx);
        Response::Edges {
            edges: self.tab_edges(&tab, cx),
            said,
        }
    }

    /// How many times what a pane's agent sends or answers now has been
    /// passed on: one more than what started its turn.
    pub(crate) fn hops_from(&self, pane: &str) -> u32 {
        self.hops.get(pane).copied().unwrap_or(0) + 1
    }

    /// `pane ask`: `from` asks `to`, whose next answer goes back to it.
    pub(crate) fn ask_pane(
        &mut self,
        from: &str,
        to: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Response {
        let Some(asker) = self.store.pane(from).cloned() else {
            return refused(format!(
                "pane ask is run from inside a pane of the app, and there is no pane {from}"
            ));
        };
        if self.store.pane(to).is_none() {
            return refused(format!("no pane {to}"));
        }
        if from == to {
            return refused("a pane cannot ask itself");
        }
        // Only a conversation has an answer the app can hand back.
        if self.threads.get(to).is_none_or(|talking| talking.closed()) {
            return refused(format!(
                "{to} is not a native chat that is running, so the app cannot tell what its \
                 answer is: send it with `pane send`, then `pane wait` and `pane read`"
            ));
        }
        let hops = self.hops_from(from);
        if hops > HOP_LIMIT {
            return refused(format!(
                "this has been passed on {HOP_LIMIT} times already, agent to agent: it is the \
                 user's to take further"
            ));
        }
        let question = format!(
            "{}\n(What you answer at the end of this turn goes back to it by itself: no need \
             to send it.)\n{text}",
            canvas_header("question from", &asker.title, from, hops),
        );
        if !self.send_text(to, &question, true, cx) {
            return refused(format!("nothing is running in pane {to}"));
        }
        self.hops.insert(to.to_owned(), hops);
        self.asks.retain(|ask| !(ask.from == from && ask.to == to));
        self.asks.push(Ask {
            from: from.to_owned(),
            to: to.to_owned(),
        });
        self.refresh(to, window, cx);
        Response::Text {
            text: format!(
                "asked {to}: its answer will come to you as a message beginning \
                 `[canvas] answer from`. Nothing was wired. Go on with your own work, or end \
                 your turn, meanwhile."
            ),
        }
    }

    /// A pane's agent ended a turn with `answer`: whoever asked it is told,
    /// once.
    pub(crate) fn answer_asks(
        &mut self,
        pane: &str,
        answer: &str,
        hops: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (owed, rest): (Vec<Ask>, Vec<Ask>) = std::mem::take(&mut self.asks)
            .into_iter()
            .partition(|ask| ask.to == pane);
        self.asks = rest;
        let title = self
            .store
            .pane(pane)
            .map_or_else(String::new, |saved| saved.title.clone());
        for ask in owed {
            if self.store.pane(&ask.from).is_none() {
                continue;
            }
            let text = format!(
                "{}\n{answer}",
                canvas_header("answer from", &title, pane, hops)
            );
            self.hops.insert(ask.from.clone(), hops);
            self.deliver(&ask.from, &text, window, cx);
        }
    }

    /// A pane is gone: whoever waited for its answer is told there will be
    /// none, and what it asked is forgotten.
    pub(crate) fn forget_asks(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .store
            .pane(pane)
            .map_or_else(String::new, |saved| saved.title.clone());
        let (owed, rest): (Vec<Ask>, Vec<Ask>) = std::mem::take(&mut self.asks)
            .into_iter()
            .partition(|ask| ask.to == pane);
        self.asks = rest;
        self.asks.retain(|ask| ask.from != pane);
        self.hops.remove(pane);
        for ask in owed {
            if self.store.pane(&ask.from).is_none() {
                continue;
            }
            let text =
                format!("[canvas] «{title}» ({pane}) closed without answering what you asked.");
            self.deliver(&ask.from, &text, window, cx);
        }
    }

    /// A message that was passed on too many times waits on its target for
    /// the user, on the edge it came along.
    pub(crate) fn hold_at_limit(
        &mut self,
        tab: &str,
        edge: kou_canvas::EdgeId,
        from: &str,
        to: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let traffic = self.traffic.entry((tab.to_owned(), edge)).or_default();
        traffic.waiting += 1;
        let traffic = *traffic;
        self.held.push(Held {
            tab: tab.to_owned(),
            edge,
            from: from.to_owned(),
            to: to.to_owned(),
            text: text.to_owned(),
            // Let through by the user, it starts counting anew.
            hops: 0,
        });
        if let Some(canvas) = self.canvas(tab, cx) {
            canvas.update(cx, |canvas, cx| canvas.set_traffic(edge, traffic, cx));
        }
        self.show_pending(tab, cx);
        let name = |pane: &str| {
            self.store
                .pane(pane)
                .map_or_else(|| pane.to_owned(), |saved| saved.title.clone())
        };
        self.notice = Some(
            format!(
                "A message from {} to {} was passed on {HOP_LIMIT} times, agent to agent: it \
                 waits on {} for you.",
                name(from),
                name(to),
                name(to)
            )
            .into(),
        );
        cx.notify();
    }

    /// The models a harness can run with, if the app knows them freshly
    /// enough; `None` when it has to ask first.
    fn models_known(&self, harness: &str) -> Option<&crate::shell::Offered> {
        self.offered
            .get(harness)
            .filter(|known| known.at.is_some_and(|at| at.elapsed() < MODELS_FRESH))
    }

    /// Asks a harness for its models, for a request that waits for them.
    /// False when it cannot be asked now, and the request is to be answered
    /// from what is known.
    fn ask_models_for(
        &mut self,
        harness: &str,
        cwd: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.probes || self.harness(harness).is_none() {
            return false;
        }
        let already = self
            .offered
            .get(harness)
            .is_some_and(|known| known.at.is_none());
        if !already {
            let cwd = cwd
                .or_else(|| self.store.current().map(|session| session.cwd.clone()))
                .unwrap_or_else(|| self.home.clone());
            let mut asking = self.offered.get(harness).cloned().unwrap_or_default();
            asking.cwd = cwd.clone();
            asking.at = None;
            self.offered.insert(harness.to_owned(), asking);
            self.ask_for_models(harness, cwd, cx);
        }
        true
    }

    fn models_response(&self, harness: &str) -> Response {
        let known = self.offered.get(harness);
        Response::Models {
            harness: harness.to_owned(),
            models: known
                .map(|known| {
                    known
                        .models
                        .iter()
                        .map(|model| kou_ipc::Model {
                            id: model.id.clone(),
                            name: model.name.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            default: known.and_then(|known| known.current.clone()),
        }
    }

    /// `harness models`: answered now from what is known, or once the
    /// harness has said.
    pub(crate) fn harness_models(
        &mut self,
        harness: String,
        reply: Reply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.harness(&harness).is_none() {
            return reply.send(refused(format!(
                "{harness} is not a harness the app found; `kou-desktop harness list` shows them"
            )));
        }
        if self.models_known(&harness).is_none() && self.ask_models_for(&harness, None, cx) {
            self.deferred.push(Deferred::Models { harness, reply });
            self.wait_for_models(window, cx);
            return;
        }
        reply.send(self.models_response(&harness));
    }

    /// Looks a few times a second whether the harnesses asked for their
    /// models have answered, and answers the requests that wait for them;
    /// after half a minute they are answered from what is known. (Not left
    /// to the next frame: a window out of sight draws none.)
    pub(crate) fn wait_for_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.deferred_poll.is_some() {
            return;
        }
        self.deferred_poll = Some(cx.spawn_in(window, async move |this, cx| {
            for _ in 0..400 {
                cx.background_executor()
                    .timer(Duration::from_millis(75))
                    .await;
                let left = this.update_in(cx, |this, window, cx| {
                    this.answer_deferred(window, cx);
                    this.deferred.len()
                });
                if !matches!(left, Ok(left) if left > 0) {
                    break;
                }
            }
            this.update_in(cx, |this, window, cx| {
                this.deferred_poll = None;
                // Whoever still waits is answered from what is known.
                for known in this.offered.values_mut() {
                    known.at.get_or_insert_with(std::time::Instant::now);
                }
                this.answer_deferred(window, cx);
            })
            .ok();
        }));
    }

    /// Whether `pane spawn --model` names a model its harness can run with.
    /// `Err` is the refusal; `Ok(false)` means the harness has to be asked
    /// first, and the spawn waits for it.
    pub(crate) fn check_model(
        &mut self,
        spawn: &kou_ipc::Spawn,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let Some(model) = spawn.model.as_deref().filter(|model| !model.is_empty()) else {
            return Ok(true);
        };
        let harness = spawn.harness.as_str();
        if self.models_known(harness).is_none()
            && self.ask_models_for(harness, spawn.cwd.clone(), cx)
        {
            return Ok(false);
        }
        let Some(known) = self
            .offered
            .get(harness)
            .filter(|known| !known.models.is_empty())
        else {
            // It names no models to the app: nothing to check against.
            return Ok(true);
        };
        if known.models.iter().any(|choice| choice.id == model) {
            return Ok(true);
        }
        let ids: Vec<&str> = known
            .models
            .iter()
            .map(|choice| choice.id.as_str())
            .collect();
        Err(format!(
            "{harness} has no model {model:?}. It runs with: {}. (`kou-desktop harness models \
             {harness}` lists them.)",
            ids.join(", ")
        ))
    }

    /// The requests that waited for a harness's models are answered, once
    /// it has said. Called at the start of a frame, where the window is at
    /// hand.
    pub(crate) fn answer_deferred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.deferred.is_empty() {
            return;
        }
        let waiting = std::mem::take(&mut self.deferred);
        for deferred in waiting {
            let harness = match &deferred {
                Deferred::Models { harness, .. } => harness.clone(),
                Deferred::Spawn { spawn, .. } => spawn.harness.clone(),
            };
            // Still being asked: `at` is set when the answer comes, or fails.
            if self
                .offered
                .get(&harness)
                .is_some_and(|known| known.at.is_none())
            {
                self.deferred.push(deferred);
                continue;
            }
            match deferred {
                Deferred::Models { harness, reply } => reply.send(self.models_response(&harness)),
                Deferred::Spawn { spawn, reply } => self.handle(
                    kou_ipc::Incoming {
                        request: kou_ipc::Request::PaneSpawn(*spawn),
                        reply,
                    },
                    window,
                    cx,
                ),
            }
        }
    }
}

/// What a request that could not be checked yet becomes while it waits.
pub(crate) fn defer_spawn(spawn: kou_ipc::Spawn, reply: Reply) -> Deferred {
    Deferred::Spawn {
        spawn: Box::new(spawn),
        reply,
    }
}
