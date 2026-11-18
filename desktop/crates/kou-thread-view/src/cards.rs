//! What the view shows of the agents its agent started, in either look: a
//! card for each in the transcript where it was started, the card beside
//! the transcript (the project, its branch and changes, who started whom,
//! the context), and the ticks down the transcript's left edge, one for
//! each prompt.
//!
//! The quieter look's sizes are the user's reference picture's, measured at
//! its 1.7 pixels to the point (see [`crate::studio`]).

use crate::{
    entries::{EDGE, HEAD, MEASURE, chevron, row},
    lights::{Light, Lights, Spot, mark_spot},
    nested::{Look, Nested, NestedState, ran_for_short, tokens, words},
    studio::{COLUMN, SIDE},
    view::{ThreadView, ThreadViewEvent},
};
use gpui::{
    AnyElement, AppContext as _, Context, Div, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    canvas, div, prelude::FluentBuilder as _,
};
use kou_thread::Entry;
use kou_ui::{
    Brand, Icon, Theme, brand, icon, metric, monogram_sized,
    scale::{self, px},
    tip,
};
use std::time::SystemTime;

/// The widest the transcript's column gets in a look, its side padding included.
pub(crate) fn measure(look: Look) -> f32 {
    match look {
        Look::Cockpit => MEASURE,
        Look::Studio => COLUMN + 2. * SIDE,
    }
}

/// A card in the quieter look: its height (the picture's 68 pixels from one
/// card to the next), how far in its avatar stands, the avatar's side, the
/// gap to the title, and the state dot: its side, its ring, where it stands
/// from the avatar's centre.
const CARD: f32 = crate::studio::pitch(40.);
const CARD_PAD: f32 = 6.;
const AVATAR: f32 = 21.;
const AVATAR_GAP: f32 = 8.;
const DOT: f32 = 6.5;
const DOT_RING: f32 = 1.5;
const DOT_AT: (f32, f32) = (-6.8, -6.8);
/// Between the words at a card's right.
const TIME_GAP: f32 = 10.;
/// The size of a card's title (the picture's 413 pixels for
/// "lite-measure: jw cost breakdown and Lightpanda"), and of its muted line.
const TITLE: f32 = crate::studio::sized(10.5);
const DETAIL: f32 = crate::studio::sized(9.6);

/// The card beside: its corners, a row's height, a section's padding, where
/// a row's picture and text start, and how far in from its right the row's
/// end stands.
const BESIDE_RADIUS: f32 = metric::RADIUS_LARGE;
const BESIDE_ROW: f32 = crate::studio::pitch(28.);
const BESIDE_PAD: f32 = 7.;
const BESIDE_INDENT: f32 = 16.5;
const BESIDE_TEXT: f32 = 40.6;
const BESIDE_END: f32 = 17.6;
/// The size of a row's words in the card beside (the picture's 146 pixels
/// for "Previous agents"), and its icons.
const BESIDE_SIZE: f32 = crate::studio::sized(11.5);
const BESIDE_ICON: f32 = 16.;
/// The box a row's picture sits in.
const BESIDE_SLOT: f32 = 18.;
/// Prompt shortcuts fit inside the transcript's side padding at every pane width.
const TICK: f32 = 5.;
const TICK_THICK: f32 = 1.5;
const TICK_PITCH: f32 = 7.;

impl ThreadView {
    /// The cards of `agents`, one under the other, `above` under what is
    /// before them: those of row `row`, from its `first` card on.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn card_group(
        &self,
        agents: &[Nested],
        row: usize,
        first: usize,
        above: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let wall = self.wall_now();
        let now = cx.background_executor().now();
        let still = cx.reduce_motion();
        let studio = self.around.studio();
        div()
            .w_full()
            .pt(px(above))
            .flex()
            .flex_col()
            .when(!studio, |group| group.gap(px(8.)))
            .children(agents.iter().enumerate().map(|(at, agent)| {
                // Where its light goes: round its dot, or round the card.
                let spot = (agent.state == NestedState::Running)
                    .then(|| card_spot(row, first + at))
                    .flatten();
                let card = if studio {
                    self.studio_card(agent, wall, spot, theme, cx)
                } else {
                    self.cockpit_card(agent, wall, spot, theme, cx)
                };
                // A card that came while the view watched rises into place.
                let arrived = (!still)
                    .then(|| self.nest.arrived.get(&agent.id))
                    .flatten()
                    .map(|at| {
                        now.saturating_duration_since(*at).as_secs_f32()
                            / kou_ui::motion::studio::ARRIVE.as_secs_f32()
                    })
                    .filter(|t| *t < 1.);
                match arrived {
                    Some(t) => {
                        let t = kou_ui::motion::studio::arrive(t);
                        div()
                            .relative()
                            .top(px(kou_ui::motion::studio::ARRIVE_BY * (1. - t)))
                            .opacity(t)
                            .child(card)
                            .into_any_element()
                    }
                    None => card,
                }
            }))
            .into_any_element()
    }

    /// Makes the lights of row `row`'s cards of agents that run, the first
    /// time they are needed: a ring going out from the dot in the quieter
    /// look, a light round the card in the cockpit's. Whether one was made
    /// now, to be drawn from the next frame on. None with reduced motion.
    pub(crate) fn card_lights(&mut self, row: usize, cx: &mut Context<Self>) -> bool {
        if cx.reduce_motion() {
            return false;
        }
        let (top, bottom) = self.cards_of(row);
        let running: Vec<Spot> = top
            .iter()
            .chain(bottom.iter())
            .enumerate()
            .filter(|(_, agent)| agent.state == NestedState::Running)
            .filter_map(|(at, _)| card_spot(row, at))
            .collect();
        let studio = self.around.studio();
        let color = kou_ui::theme(cx).working;
        let (epoch, scale, spots) = (self.motion.epoch, self.scale.clone(), self.spots.clone());
        let mut made = false;
        for spot in running {
            self.lights.entry(spot).or_insert_with(|| {
                made = true;
                let light = if studio {
                    Light::Halo { color }
                } else {
                    Light::Run {
                        radius: metric::RADIUS_MEDIUM,
                    }
                };
                cx.new(|_| Lights::over(light, epoch, scale.clone(), spots.clone(), spot))
            });
        }
        made
    }

    /// The lights the cards want now: one for each agent that runs.
    pub(crate) fn card_lights_wanted(&self) -> std::collections::HashSet<Spot> {
        let count = self.thread.entries().len();
        let mut wanted = std::collections::HashSet::new();
        let rows: std::collections::BTreeSet<usize> = self
            .nest
            .places
            .values()
            .map(|after| crate::nested::host_row(*after, count).0)
            .collect();
        for row in rows {
            let (top, bottom) = self.cards_of(row);
            wanted.extend(
                top.iter()
                    .chain(bottom.iter())
                    .enumerate()
                    .filter(|(_, agent)| agent.state == NestedState::Running)
                    .filter_map(|(at, _)| card_spot(row, at)),
            );
        }
        wanted
    }

    /// Notes, as it is painted, that a card whose time counts is on screen.
    pub(crate) fn counting(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let (counting, view) = (self.nest.counting.clone(), cx.weak_entity());
        canvas(
            move |_, _, cx| {
                if !counting.replace(true) {
                    let view = view.clone();
                    cx.defer(move |cx| {
                        view.update(cx, |view, cx| view.keep_ticking(cx)).ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_0()
    }

    /// A card in the quieter look: the avatar with the agent's mark and its
    /// state dot, its title and under it one muted line, at the right its
    /// time and a chevron. A click opens the agent.
    fn studio_card(
        &self,
        agent: &Nested,
        wall: SystemTime,
        spot: Option<Spot>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let said = words(agent, wall);
        let id = agent.id.clone();
        let (hover, ring) = (theme.hover, theme.accent);
        div()
            .id(SharedString::from(format!("nested-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("nested-{id}")
            })
            .relative()
            .h(px(CARD))
            .w_full()
            .pl(px(CARD_PAD))
            .pr(px(CARD_PAD))
            .flex()
            .items_center()
            .gap(px(AVATAR_GAP))
            .rounded(px(metric::RADIUS_MEDIUM))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .tab_index(0)
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener(move |this, click, window, cx| {
                cx.emit(ThreadViewEvent::OpenNested(id.clone()));
                this.rest_focus(click, window, cx);
            }))
            .child(avatar(agent, AVATAR, theme).children(spot.map(|spot| {
                // The dot's place, for its ring.
                let ring = DOT + 2. * DOT_RING;
                div()
                    .absolute()
                    .left(px(AVATAR / 2. + DOT_AT.0 - ring / 2.))
                    .top(px(AVATAR / 2. + DOT_AT.1 - ring / 2.))
                    .size(px(ring))
                    .child(mark_spot(&self.spots, spot))
            })))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(TITLE))
                            .line_height(px(13.2))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.fg)
                            .child(scale::text(agent.title.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(DETAIL))
                            .line_height(px(14.))
                            .text_color(theme.fg_dim)
                            .child(scale::text(said.detail)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(TIME_GAP))
                    .text_size(px(DETAIL + 0.9))
                    .text_color(theme.fg_dim)
                    .children(said.time.into_iter().map(scale::text)),
            )
            .child(
                div()
                    .flex_none()
                    .ml(px(TIME_GAP - AVATAR_GAP))
                    .child(icon(Icon::ChevronRight, theme.fg_muted).size(px(12.))),
            )
            .children(said.counting.then(|| self.counting(cx)))
            .into_any_element()
    }

    /// A cockpit card with the agent's tile, its title, its
    /// state in capitals and its time; its latest words under them.
    fn cockpit_card(
        &self,
        agent: &Nested,
        wall: SystemTime,
        spot: Option<Spot>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let said = words(agent, wall);
        let id = agent.id.clone();
        let (word, tone) = match agent.state {
            NestedState::Running => ("Running", theme.working),
            NestedState::Asking => ("Needs you", theme.blocked),
            NestedState::Finished => ("Done", theme.fg_muted),
            NestedState::Gone => ("Closed", theme.fg_muted),
            NestedState::Failed => ("Failed", theme.error),
        };
        // The time: how long it has run, or when it ended.
        let time = said.time.last().cloned().unwrap_or_default();
        // The app draws the card when it can (`chat-nested`): its frame, its
        // head and the frame of its latest words. Its time and those words,
        // which count up and change as the agent works, stay the view's, as
        // do its light and its coming in.
        if self.chat_face.is_some() {
            let tone = match agent.state {
                NestedState::Running => "working",
                NestedState::Asking => "blocked",
                NestedState::Finished | NestedState::Gone => "fgMuted",
                NestedState::Failed => "error",
            };
            let what = NestedCard {
                id: &agent.id,
                code: if agent.code.is_empty() {
                    &agent.harness
                } else {
                    &agent.code
                },
                title: &agent.title,
                word,
                tone,
                running: agent.state == NestedState::Running,
            };
            let mut slots: Vec<(&'static str, AnyElement)> = vec![
                ("time", scale::text(time.clone()).into_any_element()),
                (
                    "detail",
                    scale::text(said.detail.clone()).into_any_element(),
                ),
            ];
            if said.counting {
                slots.push(("counting", self.counting(cx).into_any_element()));
            }
            if let Some(spot) = spot {
                slots.push(("spot", mark_spot(&self.spots, spot).into_any_element()));
            }
            let memo = crate::face::digest(&what);
            let data = || serde_json::to_value(&what).unwrap_or_default();
            if let Some(card) =
                self.entry_face("chat-nested", &agent.id, Some(memo), data, slots, cx)
            {
                return row(card).into_any_element();
            }
        }
        let (hover, ring) = (theme.hover, theme.accent);
        let card = div()
            .id(SharedString::from(format!("nested-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("nested-{id}")
            })
            .relative()
            .w_full()
            .rounded(px(metric::RADIUS_MEDIUM))
            .border_1()
            .border_color(if agent.state == NestedState::Running {
                crate::entries::lights_mix(theme.working, theme.line, 0.5)
            } else {
                theme.line
            })
            .bg(theme.raised)
            .cursor_pointer()
            .tab_index(0)
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener(move |this, click, window, cx| {
                cx.emit(ThreadViewEvent::OpenNested(id.clone()));
                this.rest_focus(click, window, cx);
            }))
            .child(
                div()
                    .h(px(HEAD))
                    .pl(px(10.))
                    .pr(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(tile(agent, metric::SLOT - 2., theme))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(theme.fg)
                            .child(scale::text(agent.title.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.mono_font.clone())
                            .text_size(px(11.))
                            .text_color(theme.fg_muted)
                            .child(scale::text(time)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.mono_font.clone())
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(10.))
                            .text_color(tone)
                            .child(scale::text(word.to_uppercase())),
                    )
                    .child(chevron(false, theme)),
            )
            .child(
                div()
                    .pl(px(36.))
                    .pr(px(12.))
                    .pb(px(6.))
                    .truncate()
                    .font_family(theme.mono_font.clone())
                    .text_size(px(11.5))
                    .text_color(theme.fg_muted)
                    .child(scale::text(said.detail)),
            )
            .children(said.counting.then(|| self.counting(cx)))
            // Its light goes round it while the agent runs, as a call's does.
            .children(spot.map(|spot| mark_spot(&self.spots, spot)));
        row(card).into_any_element()
    }

    /// The card beside the transcript, at the pane's top right, while the
    /// pane leaves it room: the project and its folder, the branch and its
    /// changes, who started whom, and how full the context is.
    pub(crate) fn beside_card(&self, theme: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let beside = self.nest.beside.as_ref()?;
        // It comes out from the pane's edge and goes back to it over a few
        // frames; away, a handle at the edge pulls it out, room or no room.
        let now = cx.background_executor().now();
        let (level, going) = self.beside_level(now, cx.reduce_motion());
        if level <= 0. && !going {
            return Some(self.beside_handle(theme, cx));
        }
        let studio = self.around.studio();
        let wall = self.wall_now();
        // Who started whom: the agents started in panes, and those the
        // agent started by itself (by a call, whose card is in the
        // transcript).
        let native = self.native_agents(now);
        // (With them, the commands it left running in the background.)
        let commands = self.background_commands(now);
        let own: Vec<&(usize, Nested)> = native
            .iter()
            .filter(|(_, agent)| agent.state.live())
            .chain(commands.iter())
            .collect();
        let running = self
            .nest
            .agents
            .iter()
            .filter(|agent| agent.state.live())
            .count()
            + own.len();
        // Those at work are listed; the others count as previous agents,
        // with the ones gone for good.
        let shown: Vec<&Nested> = self
            .nest
            .agents
            .iter()
            .filter(|agent| agent.state.live())
            .collect();
        let earlier = beside.earlier + self.nest.agents.len() - shown.len()
            + native
                .iter()
                .filter(|(_, agent)| !agent.state.live())
                .count();
        let lineage =
            beside.parent.is_some() || !shown.is_empty() || !own.is_empty() || earlier > 0;
        // How full the context is, from the thread's own figures.
        let usage = self.thread.usage;
        let context = match (usage.context_used, usage.context_limit) {
            (Some(used), Some(limit)) if limit > 0 => Some((used, limit)),
            _ => None,
        };
        self.around.context_beside.set(context.is_some());
        let measured = self.nest.beside_height.clone();
        // Its height, measured as it is drawn, says whether it fits over
        // the field.
        let measure = move || {
            let measured = measured.clone();
            canvas(
                move |bounds, _, _| {
                    measured.set(bounds.size.height.as_f32() / scale::factor().max(0.01));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        // On its way it slides from the pane's edge and fades in.
        let slide = kou_ui::motion::studio::BESIDE_BY * (1. - level);
        let placed = |card: AnyElement| {
            div()
                .absolute()
                .top(px(crate::nested::BESIDE_TOP))
                .right(px(crate::nested::BESIDE_EDGE - slide))
                .opacity(level.clamp(0., 1.))
                .child(card)
                .into_any_element()
        };
        // The app draws the card when it can (`chat-beside`): its sections,
        // rows, buttons and meter. Where it stands, how it comes in, its
        // height measured and the times that count stay the view's.
        if self.chat_face.is_some() {
            let mut agents: Vec<LineageFace> = Vec::new();
            if lineage {
                let face = LineageFace::of;
                agents.extend(
                    beside
                        .parent
                        .iter()
                        .map(|parent| face(parent, true, None, wall)),
                );
                agents.extend(shown.iter().map(|agent| face(agent, false, None, wall)));
                agents.extend(
                    own.iter()
                        .map(|(index, agent)| face(agent, false, Some(*index), wall)),
                );
            }
            let what = BesideFace {
                project: &beside.project,
                branch: beside.branch.as_deref(),
                changes: beside.changes,
                lineage,
                running,
                agents,
                earlier,
                context: context.map(|(used, limit)| ContextFace {
                    used,
                    limit,
                    figures: format!("{} / {}", tokens(used), tokens(limit)),
                }),
                lifted: !self.beside_fits(),
            };
            let counting = what.agents.iter().any(|agent| agent.live);
            let mut slots: Vec<(&'static str, AnyElement)> =
                vec![("measure", measure().into_any_element())];
            if counting {
                slots.push(("counting", self.counting(cx).into_any_element()));
            }
            let memo = crate::face::digest(&what);
            let data = || serde_json::to_value(&what).unwrap_or_default();
            if let Some(card) = self.entry_face("chat-beside", "card", Some(memo), data, slots, cx)
            {
                return Some(placed(card));
            }
        }
        let mut sections: Vec<AnyElement> = Vec::new();
        // The project, and a button that opens its folder.
        let path = beside.path.clone();
        let folder = beside_button("beside-folder", Icon::Folder, theme)
            .tooltip(tip("Open the folder"))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(ThreadViewEvent::OpenFolder(path.clone()));
            }));
        // And one that puts the card away, to the handle at the pane's edge.
        let away = beside_button("beside-hide", Icon::ChevronRight, theme)
            .tooltip(tip("Put this card away"))
            .on_click(cx.listener(|this, _, _, cx| this.pull_beside(false, cx)));
        sections.push(section(
            vec![beside_row(
                Icon::Project,
                beside.project.clone(),
                None,
                Some(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .child(folder)
                        .child(away)
                        .into_any_element(),
                ),
                theme,
            )],
            theme,
        ));
        // The branch and its changes.
        let mut work = Vec::new();
        if let Some(branch) = &beside.branch {
            work.push(beside_row(Icon::Branch, branch.clone(), None, None, theme));
        }
        if let Some((added, removed)) = beside.changes {
            work.push(beside_row(
                Icon::Diff,
                "Changes".into(),
                None,
                Some(changes(added, removed, theme).into_any_element()),
                theme,
            ));
        }
        if !work.is_empty() {
            sections.push(section(work, theme));
        }
        if lineage {
            let mut rows = vec![heading(
                if running > 0 {
                    format!("Lineage · {running} running")
                } else {
                    "Lineage".to_owned()
                },
                studio,
                theme,
            )];
            if let Some(parent) = &beside.parent {
                rows.push(self.lineage_row(parent, true, None, wall, theme, cx));
            }
            for agent in shown {
                rows.push(self.lineage_row(agent, false, None, wall, theme, cx));
            }
            for (index, agent) in own {
                rows.push(self.lineage_row(agent, false, Some(*index), wall, theme, cx));
            }
            if earlier > 0 {
                rows.push(beside_row(
                    Icon::Agents,
                    "Previous agents".into(),
                    None,
                    Some(
                        div()
                            .text_color(theme.fg_muted)
                            .child(scale::text(earlier.to_string()))
                            .into_any_element(),
                    ),
                    theme,
                ));
            }
            sections.push(section(rows, theme));
        }
        if let Some((used, limit)) = context {
            sections.push(context_meter(used, limit, studio, theme));
        }
        let card = div()
            .id("beside")
            .debug_selector(|| "beside".into())
            .occlude()
            .w(px(crate::nested::BESIDE_WIDTH))
            .rounded(px(if studio {
                BESIDE_RADIUS
            } else {
                metric::RADIUS_LARGE
            }))
            .border_1()
            .border_color(theme.line_strong)
            .bg(theme.raised)
            // Pulled out where the pane has no room for it, it lies over
            // the text: lifted off it.
            .when(!self.beside_fits(), |card| {
                card.shadow(crate::picker::shadow(theme))
            })
            .overflow_hidden()
            .flex()
            .flex_col()
            .text_size(px(if studio { BESIDE_SIZE } else { metric::TEXT }))
            .children(sections.into_iter().enumerate().map(|(at, part)| {
                div()
                    .w_full()
                    .when(at > 0, |part| part.border_t_1().border_color(theme.line))
                    .child(part)
            }))
            .child(measure());
        Some(placed(card.into_any_element()))
    }

    /// The handle at the pane's right edge, where the card beside would
    /// stand: it pulls the card out, whether the pane leaves it room or not.
    fn beside_handle(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let placed = |handle: AnyElement| {
            div()
                .absolute()
                .top(px(crate::nested::BESIDE_TOP))
                .right_0()
                .child(handle)
                .into_any_element()
        };
        // The app draws it when it can (`chat-beside-handle`).
        let data = || serde_json::json!({});
        if let Some(handle) = self.entry_face(
            "chat-beside-handle",
            "handle",
            Some(0),
            data,
            Vec::new(),
            cx,
        ) {
            return placed(handle);
        }
        let (hover, ring) = (theme.selected, theme.accent);
        placed(
            div()
                .id("beside-pull")
                .debug_selector(|| "beside-pull".into())
                .occlude()
                .w(px(20.))
                .h(px(BESIDE_ROW + 2. * BESIDE_PAD))
                .rounded_l(px(metric::RADIUS))
                .border_1()
                .border_color(theme.line_strong)
                .bg(theme.raised)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .tab_index(0)
                .hover(move |style| style.bg(hover))
                .focus_visible(move |style| style.border_color(ring))
                .tooltip(tip("Show the project card"))
                .on_click(cx.listener(|this, _, _, cx| this.pull_beside(true, cx)))
                .child(
                    icon(Icon::ChevronRight, theme.fg_muted)
                        .size(px(12.))
                        .with_transformation(gpui::Transformation::rotate(gpui::radians(
                            std::f32::consts::PI,
                        ))),
                )
                .into_any_element(),
        )
    }

    /// One of the agents in the card beside: its mark with its dot, its
    /// title, and how it goes; a click opens it. One the agent started by
    /// itself is the call at entry `call`: a click shows its card, open.
    fn lineage_row(
        &self,
        agent: &Nested,
        parent: bool,
        call: Option<usize>,
        wall: SystemTime,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let state: SharedString = match agent.state {
            NestedState::Running => {
                ran_for_short(wall.duration_since(agent.since).unwrap_or_default()).into()
            }
            NestedState::Asking => "Asking".into(),
            NestedState::Finished | NestedState::Gone => "Done".into(),
            NestedState::Failed => "Failed".into(),
        };
        let tone = match agent.state {
            NestedState::Asking => theme.blocked,
            NestedState::Failed => theme.error,
            _ => theme.fg_muted,
        };
        let id = agent.id.clone();
        let (hover, ring) = (theme.hover, theme.accent);
        let picture: AnyElement = if parent {
            with_dot(
                icon(Icon::Lineage, theme.fg_muted)
                    .size(px(14.))
                    .into_any_element(),
                dot_color(agent.state, theme),
                (5., 5.),
                theme,
            )
        } else {
            with_dot(
                mark(agent, 14., theme),
                dot_color(agent.state, theme),
                (-5., -5.),
                theme,
            )
        };
        let mut row = beside_row_shape(theme)
            .id(SharedString::from(format!("lineage-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("lineage-{id}")
            })
            .cursor_pointer()
            .tab_index(0)
            .border_1()
            .border_color(gpui::transparent_black())
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_color(ring))
            .on_click(cx.listener(move |this, click, window, cx| {
                match call {
                    Some(index) => this.show_agent(index, cx),
                    None => cx.emit(ThreadViewEvent::OpenNested(id.clone())),
                }
                this.rest_focus(click, window, cx);
            }))
            .child(slot(picture))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_color(theme.fg_dim)
                    .child(scale::text(agent.title.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(9.8))
                    .text_color(tone)
                    .child(scale::text(state)),
            );
        if agent.state.live() {
            row = row.child(self.counting(cx));
        }
        row.into_any_element()
    }

    /// The ticks down the transcript's left edge: one for each prompt, the
    /// ones on screen brighter; a click scrolls to the prompt, and the
    /// pointer on one says what it asked.
    pub(crate) fn ticks(&self, theme: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        let entries = self.thread.entries();
        let prompts: Vec<usize> = entries
            .iter()
            .enumerate()
            .skip(self.cleared)
            .filter(|(_, entry)| matches!(entry, Entry::User(_)))
            .map(|(index, _)| index)
            .collect();
        // The cockpit has them once the app tells it of agents started or
        // of what stands beside it: as it was drawn before, otherwise.
        let told = self.nest.beside.is_some() || !self.nest.agents.is_empty();
        if prompts.len() < 2 || !(self.around.studio() || told) {
            return None;
        }
        let (from, to) = self.nest.shown.get();
        // What a tick says under the pointer: the start of its prompt.
        let said = |index: usize| -> String {
            let text = match &entries[index] {
                Entry::User(blocks) => crate::draft::shown(blocks).0,
                _ => String::new(),
            };
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(80)
                .collect()
        };
        // The app draws them when it can (`chat-ticks`); the list they
        // scroll stays the view's.
        if self.chat_face.is_some() {
            let ticks: Vec<TickFace> = prompts
                .iter()
                .map(|&index| TickFace {
                    index,
                    said: said(index),
                    on: index >= from && index < to,
                })
                .collect();
            let memo = crate::face::digest(&ticks);
            let data = || serde_json::json!({ "ticks": ticks });
            if let Some(ticks) =
                self.entry_face("chat-ticks", "ticks", Some(memo), data, Vec::new(), cx)
            {
                return Some(ticks);
            }
        }
        let (bright, quiet, lit) = (theme.fg_faint, theme.fg_faint.opacity(0.5), theme.fg_dim);
        Some(
            div()
                .debug_selector(|| "ticks".into())
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .flex()
                .flex_col()
                .justify_center()
                .children(prompts.into_iter().map(|index| {
                    let first = said(index);
                    let on_screen = index >= from && index < to;
                    let group = SharedString::from(format!("tick-{index}"));
                    div()
                        .id(("tick", index))
                        .debug_selector(move || format!("tick-{index}"))
                        .group(group.clone())
                        .w(px(EDGE))
                        .h(px(TICK_PITCH))
                        .flex()
                        .justify_center()
                        .items_center()
                        .cursor_pointer()
                        .tooltip(tip(first))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.list.scroll_to(gpui::ListOffset {
                                item_ix: index,
                                offset_in_item: gpui::Pixels::ZERO,
                            });
                            this.focus_field(window, cx);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w(px(TICK))
                                .h(px(TICK_THICK))
                                .rounded_full()
                                .bg(if on_screen { bright } else { quiet })
                                .group_hover(group, move |style| style.bg(lit)),
                        )
                }))
                .into_any_element(),
        )
    }
}

/// The card of an agent started in a pane as the app draws it
/// (`chat-nested`): the app's name for the agent, the code its tile shows,
/// its title, how it goes in a word with that word's colour (`tone`), and
/// whether it runs (its border says so).
#[derive(Hash, serde::Serialize)]
struct NestedCard<'a> {
    id: &'a str,
    code: &'a str,
    title: &'a str,
    word: &'static str,
    tone: &'static str,
    running: bool,
}

/// The card beside as the app draws it (`chat-beside`): the project, its
/// branch and its changes (lines added and removed); whether it lists who
/// started whom, with how many of them run, the agents listed and how many
/// went before; how full the context is; and whether it lies over the text
/// (`lifted`: the pane has no room for it).
#[derive(Hash, serde::Serialize)]
struct BesideFace<'a> {
    project: &'a str,
    branch: Option<&'a str>,
    changes: Option<(u32, u32)>,
    lineage: bool,
    running: usize,
    agents: Vec<LineageFace<'a>>,
    earlier: usize,
    context: Option<ContextFace>,
    lifted: bool,
}

/// An agent in the card beside, as the app draws it: the app's name for
/// it, its title, how it goes in a word (`state`: how long it has run while
/// it runs) with that word's colour (`tone`) and its dot's (`dot`); whether
/// it is the agent that started this one (`parent`), or the call at entry
/// `call` started it; and its picture (`mark`: `lineage`, `terminal`,
/// `brand`, the mark of its `harness` or `code`, or `letters`).
#[derive(Hash, serde::Serialize)]
struct LineageFace<'a> {
    id: &'a str,
    title: &'a str,
    state: String,
    tone: &'static str,
    dot: &'static str,
    live: bool,
    parent: bool,
    call: Option<usize>,
    mark: &'static str,
    harness: &'a str,
    code: &'a str,
    letters: String,
}

impl<'a> LineageFace<'a> {
    fn of(agent: &'a Nested, parent: bool, call: Option<usize>, wall: SystemTime) -> Self {
        let state = match agent.state {
            NestedState::Running => {
                ran_for_short(wall.duration_since(agent.since).unwrap_or_default())
            }
            NestedState::Asking => "Asking".into(),
            NestedState::Finished | NestedState::Gone => "Done".into(),
            NestedState::Failed => "Failed".into(),
        };
        let tone = match agent.state {
            NestedState::Asking => "blocked",
            NestedState::Failed => "error",
            _ => "fgMuted",
        };
        let dot = match agent.state {
            NestedState::Running => "working",
            NestedState::Asking => "blocked",
            NestedState::Finished | NestedState::Gone => "done",
            NestedState::Failed => "error",
        };
        let mark = if parent {
            "lineage"
        } else if agent.harness == "shell" {
            "terminal"
        } else if Brand::of_harness(&agent.harness)
            .or_else(|| Brand::of(&agent.code))
            .is_some()
        {
            "brand"
        } else {
            "letters"
        };
        Self {
            id: &agent.id,
            title: &agent.title,
            state,
            tone,
            dot,
            live: agent.state.live(),
            parent,
            call,
            mark,
            harness: &agent.harness,
            code: &agent.code,
            letters: letters(&agent.code),
        }
    }
}

/// How full the context is, as the card beside draws it: the tokens used
/// of the limit, and the two as its figures say them.
#[derive(Hash, serde::Serialize)]
struct ContextFace {
    used: u64,
    limit: u64,
    figures: String,
}

/// A tick down the transcript's edge as the app draws it (`chat-ticks`):
/// the entry of its prompt, what that prompt asked, and whether it is on
/// screen.
#[derive(Hash, serde::Serialize)]
struct TickFace {
    index: usize,
    said: String,
    on: bool,
}

/// The light of the `at`th card of row `row`: kinds from 2 on are cards'.
fn card_spot(row: usize, at: usize) -> Option<Spot> {
    u8::try_from(at + 2).ok().map(|kind| (row, kind))
}

/// The colour of an agent's state dot.
fn dot_color(state: NestedState, theme: &Theme) -> Hsla {
    match state {
        NestedState::Running => theme.working,
        NestedState::Asking => theme.blocked,
        NestedState::Finished | NestedState::Gone => theme.done,
        NestedState::Failed => theme.error,
    }
}

/// An agent's harness mark at `side`, in its maker's colour, or its two
/// letters where it has none.
fn mark(agent: &Nested, side: f32, theme: &Theme) -> AnyElement {
    // A command the agent left running is no agent: a terminal's mark.
    if agent.harness == "shell" {
        return icon(Icon::Terminal, theme.fg_muted)
            .size(px(side))
            .into_any_element();
    }
    match Brand::of_harness(&agent.harness).or_else(|| Brand::of(&agent.code)) {
        Some(found) => brand(found, side, theme.provider(&agent.harness)).into_any_element(),
        None => div()
            .size(px(side))
            .flex()
            .items_center()
            .justify_center()
            .font_family(theme.mono_font.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(side * 0.55))
            .text_color(theme.fg_dim)
            .child(scale::text(letters(&agent.code)))
            .into_any_element(),
    }
}

/// A code's two letters, in capitals.
fn letters(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// `picture` with a state dot ringed in the ground's colour, `at` from its
/// centre.
fn with_dot(picture: AnyElement, color: Hsla, at: (f32, f32), theme: &Theme) -> AnyElement {
    let side = 5.;
    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .child(picture)
        .child(
            div()
                .absolute()
                .left(px(7. + at.0 - side / 2. - 1.))
                .top(px(7. + at.1 - side / 2. - 1.))
                .size(px(side + 2.))
                .rounded_full()
                .border_1()
                .border_color(theme.raised)
                .bg(color),
        )
        .into_any_element()
}

/// The round avatar of a nested agent's card: its harness's mark over its
/// code's letters, and its state dot at the upper left.
fn avatar(agent: &Nested, side: f32, theme: &Theme) -> Div {
    let found = Brand::of_harness(&agent.harness).or_else(|| Brand::of(&agent.code));
    let code = letters(&agent.code);
    let inside = match found {
        Some(found) => div()
            .flex()
            .flex_col()
            .items_center()
            .child(brand(found, side * 0.42, theme.provider(&agent.harness)))
            .children((!code.is_empty()).then(|| {
                div()
                    .mt(px(-0.5))
                    .text_size(px(side * 0.27))
                    .line_height(px(side * 0.32))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.fg_muted)
                    .child(scale::text(code.clone()))
            })),
        None => div()
            .font_family(theme.mono_font.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(side * 0.36))
            .text_color(theme.fg_dim)
            .child(scale::text(code.clone())),
    };
    let ring = DOT + 2. * DOT_RING;
    div()
        .relative()
        .flex_none()
        .size(px(side))
        .rounded_full()
        .bg(theme.hover)
        .flex()
        .items_center()
        .justify_center()
        .child(inside)
        .child(
            div()
                .absolute()
                .left(px(side / 2. + DOT_AT.0 - ring / 2.))
                .top(px(side / 2. + DOT_AT.1 - ring / 2.))
                .size(px(ring))
                .rounded_full()
                .border(px(DOT_RING))
                .border_color(theme.pane)
                .bg(dot_color(agent.state, theme)),
        )
}

/// A harness's tile in the cockpit's manner, for a nested agent.
fn tile(agent: &Nested, side: f32, theme: &Theme) -> Div {
    let code = if agent.code.is_empty() {
        agent.harness.clone()
    } else {
        agent.code.clone()
    };
    monogram_sized(&code, theme.hue(&code), side, theme)
}

/// A section of the card beside: its rows with air above and below.
fn section(rows: Vec<AnyElement>, _theme: &Theme) -> AnyElement {
    div()
        .w_full()
        .py(px(BESIDE_PAD))
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element()
}

/// A row's shape in the card beside, without what is in it.
fn beside_row_shape(_theme: &Theme) -> Div {
    div()
        .h(px(BESIDE_ROW))
        .mx(px(8.))
        .pl(px(BESIDE_INDENT - 8.))
        .pr(px(BESIDE_END - 8.))
        .flex()
        .items_center()
        .gap(px(BESIDE_TEXT - BESIDE_INDENT - BESIDE_SLOT))
        .rounded(px(metric::RADIUS))
}

/// The box a row's picture sits in.
fn slot(picture: AnyElement) -> Div {
    div()
        .flex_none()
        .size(px(BESIDE_SLOT))
        .flex()
        .items_center()
        .justify_center()
        .child(picture)
}

/// A row of the card beside: an icon, its words (and quieter ones after
/// them), and what stands at its end.
fn beside_row(
    glyph: Icon,
    text: String,
    after: Option<String>,
    end: Option<AnyElement>,
    theme: &Theme,
) -> AnyElement {
    beside_row_shape(theme)
        .child(slot(
            icon(glyph, theme.fg_dim)
                .size(px(BESIDE_ICON))
                .into_any_element(),
        ))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.fg_dim)
                        .child(scale::text(text)),
                )
                .children(after.map(|after| {
                    div()
                        .flex_none()
                        .text_color(theme.fg_muted)
                        .child(scale::text(after))
                })),
        )
        .children(end.map(|end| div().flex_none().child(end)))
        .into_any_element()
}

/// The heading over the lineage: the quieter look's muted words, or the
/// cockpit's capitals.
fn heading(text: String, studio: bool, theme: &Theme) -> AnyElement {
    let line = div()
        .h(px(BESIDE_ROW + 4.))
        .px(px(BESIDE_INDENT))
        .flex()
        .items_center()
        .text_color(theme.fg_muted);
    if studio {
        line.text_size(px(10.))
            .child(scale::text(text))
            .into_any_element()
    } else {
        line.font_family(theme.mono_font.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(crate::entries::LABEL))
            .child(scale::text(text.to_uppercase()))
            .into_any_element()
    }
}

/// Lines added and removed, in the monospace.
fn changes(added: u32, removed: u32, theme: &Theme) -> Div {
    div()
        .flex()
        .gap(px(6.))
        .font_family(theme.mono_font.clone())
        .text_size(px(11.5))
        .child(
            div()
                .text_color(if added > 0 { theme.ok } else { theme.fg_muted })
                .child(scale::text(format!("+{added}"))),
        )
        .child(
            div()
                .text_color(if removed > 0 {
                    theme.error
                } else {
                    theme.fg_muted
                })
                .child(scale::text(format!("-{removed}"))),
        )
}

/// A small square button in a row of the card beside.
fn beside_button(id: &'static str, glyph: Icon, theme: &Theme) -> gpui::Stateful<Div> {
    let (hover, ring) = (theme.selected, theme.accent);
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex_none()
        .size(px(22.))
        .mr(px(-4.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(gpui::transparent_black())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tab_index(0)
        .hover(move |style| style.bg(hover))
        .focus_visible(move |style| style.border_color(ring))
        .child(icon(glyph, theme.fg_muted).size(px(14.)))
}

/// The context meter: its name, a track filled as far as the context is,
/// with a mark where the fill ends, and the figures.
fn context_meter(used: u64, limit: u64, studio: bool, theme: &Theme) -> AnyElement {
    let share = (used as f32 / limit as f32).clamp(0., 1.);
    let fill = if share >= 0.9 {
        theme.error
    } else if share >= 0.75 {
        theme.blocked
    } else if studio {
        theme.fg_dim
    } else {
        theme.accent
    };
    let track = 100.;
    let label = if studio {
        div()
            .flex_none()
            .text_size(px(10.5))
            .text_color(theme.fg_muted)
            .child(scale::text("Context"))
    } else {
        div()
            .flex_none()
            .font_family(theme.mono_font.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(crate::entries::LABEL))
            .text_color(theme.fg_muted)
            .child(scale::text("CONTEXT"))
    };
    div()
        .debug_selector(|| "beside-context".into())
        .h(px(37.))
        .pl(px(BESIDE_INDENT))
        .pr(px(BESIDE_END - 4.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(label)
        .child(
            div().flex_1().flex().items_center().child(
                div()
                    .relative()
                    .w(px(track))
                    .max_w_full()
                    .h(px(2.))
                    .rounded_full()
                    .bg(theme.line_strong)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(px(track * share))
                            .rounded_full()
                            .bg(fill),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(-3.))
                            .left(px(track * share - 0.5))
                            .w(px(1.))
                            .h(px(8.))
                            .bg(fill),
                    ),
            ),
        )
        .child(
            div()
                .flex_none()
                .text_color(theme.fg_dim)
                .child(scale::text(format!("{} / {}", tokens(used), tokens(limit)))),
        )
        .into_any_element()
}
