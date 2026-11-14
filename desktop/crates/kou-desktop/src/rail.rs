//! The rail: spaces and their sessions on top, every agent by state below.
//! Structure and density follow the user's reference screenshot.
//!
//! One rhythm runs through it (the numbers are `kou_ui::metric`'s):
//! - every row is `ROW` tall and its hover or selection fill starts `INSET` in
//!   from the rail's edge, whatever its level, so the fills form one column;
//! - every heading and every first-level picture starts on one vertical line,
//!   `INSET + PAD` in; each level of the tree steps `STEP` further, which puts
//!   a child's picture under its parent's label;
//! - sections are split by a hairline and `UNIT` of air, groups inside a
//!   section by `UNIT`.
//!
//! Archived sessions hang one level below their Archive row, on a hairline
//! from it, with an outlined letter and a muted name.
//!
//! The rail is as wide as its parent makes it (208 to 440 pixels). Names
//! truncate, and what sits right of a name gives way in a fixed order (see
//! [`Room`]) rather than being squeezed into an ellipsis.
//!
//! Motion: a folded group opens and shuts over a few frames. Nothing else in
//! the rail moves: rows are there when they exist, the selection is on its row
//! at once, and the state marks hold still.

use crate::{
    shell::{Named, Shell},
    state::{Id, Space, age, name_of},
};
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    WindowControlArea, div, prelude::FluentBuilder as _, px,
};
use kou_ui::{
    Icon, Mark, Theme, grain, icon, icon_button, letter_square, letter_square_quiet, metric,
    monogram, motion::Spring, status_mark, tip,
};

/// Room left for the window's own buttons at the top left on macOS.
#[cfg(target_os = "macos")]
pub const WINDOW_BUTTONS: f32 = 76.;
#[cfg(not(target_os = "macos"))]
pub const WINDOW_BUTTONS: f32 = 8.;

/// Where a row's content starts: its fill's inset plus its padding.
const EDGE: f32 = metric::INSET + metric::PAD;
// An icon button's glyph sits this far inside it; a row of buttons is pulled
// out by as much so their glyphs end on the rows' right edge.
const BUTTON_PAD: f32 = 4.;

/// What fits beside the names at the rail's current width, decided once per
/// frame so every row gives way in the same order: first a session's place
/// (its directory and branch), on archived rows before live ones, then an
/// agent's session name. The names themselves truncate last; counts, ages,
/// chevrons and state marks never go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Room {
    /// A live session's directory and branch.
    pub place: bool,
    /// An archived session's, one level deeper and so with less room.
    pub archived_place: bool,
    /// The session an agent in the Agents list belongs to.
    pub agent_session: bool,
}

impl Room {
    /// What fits in a rail `width` pixels wide.
    pub fn at(width: f32) -> Self {
        Self {
            place: width >= 248.,
            archived_place: width >= 320.,
            agent_session: width >= 248.,
        }
    }
}

/// What the agents under a row are up to, for the row to say at a glance:
/// how many are in the middle of a turn, how many wait for the user (a
/// question, a permission, or a failure to look at), how many finished
/// without having been looked at since.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Busy {
    /// In the middle of a turn.
    pub working: usize,
    /// Waiting for the user's answer.
    pub waiting: usize,
    /// Ended badly.
    pub failed: usize,
    /// Finished, not looked at yet.
    pub done: usize,
}

impl Busy {
    /// Counted from the states of some agents.
    pub fn of(marks: impl Iterator<Item = Mark>) -> Self {
        let mut busy = Self::default();
        for mark in marks {
            match mark {
                Mark::Working => busy.working += 1,
                Mark::Blocked => busy.waiting += 1,
                Mark::Failed => busy.failed += 1,
                Mark::Done => busy.done += 1,
                Mark::Idle => {}
            }
        }
        busy
    }

    /// The marks a row shows for it, in the order they are drawn: work under
    /// way, then whatever wants the user. Work and a wait are two things
    /// to know, so both show; a finished agent shows only when nothing else
    /// does, as the quietest of the three.
    pub fn marks(self) -> Vec<Mark> {
        let mut marks = Vec::new();
        if self.working > 0 {
            marks.push(Mark::Working);
        }
        if self.waiting > 0 {
            marks.push(Mark::Blocked);
        } else if self.failed > 0 {
            marks.push(Mark::Failed);
        }
        if marks.is_empty() && self.done > 0 {
            marks.push(Mark::Done);
        }
        marks
    }

    /// The same in words, for a tooltip.
    pub fn words(self) -> String {
        let mut parts = Vec::new();
        if self.waiting > 0 {
            parts.push(format!("{} waiting for you", self.waiting));
        }
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        if self.working > 0 {
            parts.push(format!("{} working", self.working));
        }
        if self.done > 0 {
            parts.push(format!("{} done", self.done));
        }
        parts.join(", ")
    }
}

/// A row of the rail (and of lists built like it): a fixed height, a fill
/// that starts at the rail's inset, a hover and a focus ring. `selected` fills it.
pub(crate) fn row(id: impl Into<gpui::ElementId>, selected: bool, theme: &Theme) -> Stateful<Div> {
    let (hover, ring) = (theme.hover, theme.accent);
    div()
        .id(id)
        .relative()
        .h(px(metric::ROW))
        .mx(px(metric::INSET))
        // The border is the focus ring and is part of the padding's budget.
        .px(px(metric::PAD - 1.))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(metric::GAP))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .tab_index(0)
        .when(selected, |row| row.bg(theme.selected))
        .when(!selected, |row| row.hover(move |style| style.bg(hover)))
        .focus_visible(move |style| style.border_color(ring))
}

/// A row one or more levels into the tree: the same fill, its content
/// stepped in.
fn tree_row(
    id: impl Into<gpui::ElementId>,
    level: usize,
    selected: bool,
    theme: &Theme,
) -> Stateful<Div> {
    row(id, selected, theme).pl(px(metric::PAD - 1. + level as f32 * metric::STEP))
}

/// The box a row's leading picture sits in, so labels after pictures of
/// different sizes line up.
fn slot(picture: impl IntoElement) -> Div {
    div()
        .flex_none()
        .size(px(metric::SLOT))
        .flex()
        .items_center()
        .justify_center()
        .child(picture)
}

fn label(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(color)
        .child(text.into())
}

fn meta(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .flex_none()
        .max_w(px(140.))
        .truncate()
        .text_size(px(metric::TEXT_SMALL))
        .text_color(theme.fg_muted)
        .child(text.into())
}

/// A section's heading: "Spaces", "Agents". On the rows' left edge, with
/// room on the right for the section's own button.
fn heading(text: &'static str, theme: &Theme) -> Div {
    div()
        .h(px(metric::ROW))
        .pl(px(EDGE))
        .pr(px(EDGE - BUTTON_PAD))
        .flex()
        .flex_none()
        .items_center()
        .text_size(px(metric::TEXT_SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.fg_muted)
        .child(div().flex_1().child(text))
}

fn chevron(open: bool, theme: &Theme) -> impl IntoElement {
    icon(
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        theme.fg_muted,
    )
}

/// An agent's state mark, centred in a fixed box so marks line up in a column.
pub(crate) fn mark(mark: Mark, theme: &Theme) -> Div {
    div()
        .flex_none()
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .child(status_mark(mark, theme))
}

/// A wrapper that is `height * open` tall and clips what does not fit: how a
/// group folds away.
// The marks of what goes on under a row, with the count in words under the
// pointer. Nothing when nothing does.
fn busy_marks(id: impl Into<gpui::ElementId>, busy: Busy, theme: &Theme) -> Option<Stateful<Div>> {
    let marks = busy.marks();
    (!marks.is_empty()).then(|| {
        div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .tooltip(tip(busy.words()))
            .children(marks.into_iter().map(|state| mark(state, theme)))
    })
}

fn folding(height: f32, open: f32) -> Div {
    div()
        .flex_none()
        .w_full()
        .h(px((height * open).max(0.)))
        .overflow_hidden()
        .opacity(open.clamp(0., 1.))
}

impl Shell {
    pub(crate) fn render_rail(
        &self,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let room = Room::at(self.rail_width());
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.rail)
            .border_r_1()
            .border_color(theme.line)
            .child(grain(theme))
            .child(
                div()
                    .h(px(metric::BAR))
                    .pl(px(WINDOW_BUTTONS))
                    .pr(px(EDGE - BUTTON_PAD))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_end()
                    .gap(px(2.))
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        icon_button("rail-seen", Icon::CheckCircle, theme)
                            .tooltip(tip("Mark every finished agent as seen"))
                            .on_click(cx.listener(|this, _, _, cx| this.see_everything(cx))),
                    )
                    .child(
                        icon_button("rail-hide", Icon::Sidebar, theme)
                            .tooltip(tip("Hide the rail"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_rail(cx))),
                    ),
            )
            .child(
                div()
                    .id("rail-spaces")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.spaces(room, theme, cx)),
            )
            .child(self.agents(room, theme, cx))
            .child(
                div()
                    .flex_none()
                    .py(px(metric::UNIT))
                    .border_t_1()
                    .border_color(theme.line)
                    .child(
                        // A page, as the settings are: this shows it, and
                        // goes back from it.
                        row("rail-usage", self.usage_open, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_usage(cx)))
                            .child(slot(icon(Icon::Flame, theme.fg_muted)))
                            .child(label("Usage", theme.fg)),
                    )
                    // The pages plugins add, each a row of its own.
                    .children(
                        self.plugin_pages
                            .iter()
                            .filter(|page| page.id != "usage")
                            .enumerate()
                            .map(|(at, page)| {
                                let id = page.id.clone();
                                let open = self.plugin_page.as_deref() == Some(id.as_str());
                                let glyph = page
                                    .icon
                                    .as_deref()
                                    .and_then(|name| {
                                        let path = format!("icons/{name}.svg");
                                        Icon::ALL.iter().copied().find(|icon| icon.path() == path)
                                    })
                                    .unwrap_or(Icon::Note);
                                row(("rail-plugin-page", at), open, theme)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.show_plugin_page(&id, cx)
                                    }))
                                    .child(slot(icon(glyph, theme.fg_muted)))
                                    .child(label(page.title.clone(), theme.fg))
                            }),
                    )
                    .child(
                        row("rail-settings", self.settings_open, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx)))
                            .child(slot(icon(Icon::Sliders, theme.fg_muted)))
                            .child(label("Settings", theme.fg)),
                    ),
            )
    }

    fn spaces(&self, room: Room, theme: &Theme, cx: &Context<Self>) -> Div {
        let mut list = div().flex().flex_col().pb(px(metric::UNIT * 2.)).child(
            heading("Spaces", theme).child(
                icon_button("space-add", Icon::FolderPlus, theme)
                    .tooltip(tip("Add a project folder as a space"))
                    .on_click(cx.listener(|this, _, window, cx| this.add_space(window, cx))),
            ),
        );
        if self.store.spaces.is_empty() {
            return list.child(
                div()
                    .pl(px(EDGE))
                    .pr(px(EDGE))
                    .text_size(px(metric::TEXT_SMALL))
                    .text_color(theme.fg_muted)
                    .child("No spaces yet. Add a project folder to start."),
            );
        }
        for (index, space) in self.store.spaces.iter().enumerate() {
            list = list.child(
                self.space(index, space, room, theme, cx)
                    // Spaces are told apart by a little air as well as by their folders.
                    .when(index > 0, |block| block.mt(px(metric::UNIT))),
            );
        }
        list
    }

    // A space's row, and under it, folding as one block, its sessions, its
    // Archive row and, when that is open, its archived sessions.
    fn space(
        &self,
        index: usize,
        space: &Space,
        room: Room,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let id = space.id.clone();
        let open: Vec<_> = self.store.sessions_of(&space.id, false).collect();
        let archived: Vec<_> = self.store.sessions_of(&space.id, true).collect();
        let renaming = self.renaming(&Named::Space(id.clone()));
        // What its sessions' agents are up to: seen on the space itself, so
        // a folded one still says that something in it works or waits.
        let busy = self.busy(open.iter().map(|session| session.id.as_str()));
        let header = row(("space", index), false, theme)
            .on_click(cx.listener({
                let id = id.clone();
                move |this, event: &gpui::ClickEvent, window, cx| {
                    if this.renaming(&Named::Space(id.clone())) {
                        return;
                    }
                    // A click folds it or unfolds it; the second of two puts
                    // it back as it was and opens its name instead.
                    if let Some(space) = this.store.space_mut(&id) {
                        space.collapsed = !space.collapsed;
                    }
                    if event.click_count() >= 2 {
                        this.edit_name(Named::Space(id.clone()), window, cx);
                    }
                    this.save(cx);
                }
            }))
            .child(slot(icon(Icon::FolderFill, theme.hue(&space.name))))
            .child(if renaming {
                self.name_field(theme, cx)
            } else {
                label(space.name.clone(), theme.fg)
            })
            .children(busy_marks(("space-busy", index), busy, theme))
            .child(
                icon_button(("space-new", index), Icon::Plus, theme)
                    .tooltip(tip("Start a session in this space"))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.add_session(&id, cx);
                        }
                    })),
            )
            .child(meta(open.len().to_string(), theme))
            .child(chevron(!space.collapsed, theme));
        let block = div().flex().flex_col().child(header);

        let unfolded = self.spring(
            format!("fold-{id}"),
            if space.collapsed { 0. } else { 1. },
            Spring::SNAPPY,
        );
        if unfolded <= 0.001 {
            return block;
        }
        let mut inner = div().flex().flex_col();
        let mut natural = 0.;
        for session in &open {
            inner = inner.child(self.session_row(&session.id, space, false, room, theme, cx));
            natural += metric::ROW;
        }
        if !archived.is_empty() {
            inner = inner.child(
                tree_row(("archive", index), 1, false, theme)
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            if let Some(space) = this.store.space_mut(&id) {
                                space.archive_open = !space.archive_open;
                            }
                            this.save(cx);
                        }
                    }))
                    .child(slot(icon(Icon::Archive, theme.fg_muted)))
                    .child(label("Archive", theme.fg_muted))
                    .child(meta(archived.len().to_string(), theme))
                    .child(chevron(space.archive_open, theme)),
            );
            natural += metric::ROW;
            let archive_unfolded = self.spring(
                format!("fold-archive-{id}"),
                if space.archive_open { 1. } else { 0. },
                Spring::SNAPPY,
            );
            if archive_unfolded > 0.001 {
                let height = archived.len() as f32 * metric::ROW;
                // The hairline the archived rows hang from runs down from the
                // middle of the Archive row's picture. It is drawn first, so a
                // hovered row's fill covers it.
                let guide = div()
                    .absolute()
                    .top_0()
                    .bottom(px(metric::UNIT))
                    .left(px(EDGE + metric::STEP + metric::SLOT / 2. - 0.5))
                    .w(px(1.))
                    .bg(theme.line_strong);
                inner =
                    inner.child(folding(height, archive_unfolded).child(
                        div().relative().flex().flex_col().child(guide).children(
                            archived.iter().map(|session| {
                                self.session_row(&session.id, space, true, room, theme, cx)
                            }),
                        ),
                    ));
                natural += height * archive_unfolded;
            }
        }
        block.child(folding(natural, unfolded).child(inner))
    }

    // What the agents of these sessions are up to, all told.
    fn busy<'a>(&self, sessions: impl Iterator<Item = &'a str>) -> Busy {
        let sessions: Vec<&str> = sessions.collect();
        Busy::of(
            self.store
                .panes
                .iter()
                .filter(|pane| sessions.contains(&pane.session.as_str()))
                .filter_map(|pane| self.live.get(&pane.id))
                .map(|live| live.shown()),
        )
    }

    // Whether this is what is being renamed.
    fn renaming(&self, what: &Named) -> bool {
        self.name_edit
            .as_ref()
            .is_some_and(|edit| edit.what == *what)
    }

    // The field a name is typed in, in the place of the name.
    fn name_field(&self, theme: &Theme, cx: &Context<Self>) -> Div {
        let Some(edit) = self.name_edit.as_ref() else {
            return div();
        };
        div()
            .flex_1()
            .min_w_0()
            .h(px(metric::ROW - 6.))
            .mx(px(-5.))
            .px(px(4.))
            .flex()
            .items_center()
            .overflow_hidden()
            .rounded(px(metric::RADIUS_SMALL))
            .border_1()
            .border_color(theme.accent)
            .bg(theme.raised)
            .text_color(theme.fg)
            .track_focus(&self.tab_focus)
            .on_key_down(cx.listener(Self::name_edit_key))
            // A press in the field is not a press on its row.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(kou_canvas::field_lines(
                "rail-name-field",
                &edit.field,
                theme,
                px(16.),
            ))
    }

    fn session_row(
        &self,
        id: &Id,
        space: &Space,
        archived: bool,
        room: Room,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(session) = self.store.session(id) else {
            return div().into_any_element();
        };
        let selected = !self.page_open() && self.store.selected.as_deref() == Some(id.as_str());
        // Where a session works, when it is not the space's own folder (a
        // worktree, a subfolder): that directory and the branch checked out
        // there. A session in the space's folder says nothing, rather than
        // repeat the same branch on every row of the space.
        let shown = if archived {
            room.archived_place
        } else {
            room.place
        };
        let place = (shown && session.cwd != space.path).then(|| {
            let directory = name_of(&session.cwd);
            match self.branch(&session.cwd) {
                Some(branch) => format!("{directory} · {branch}"),
                None => directory,
            }
        });
        let key = SharedString::from(format!("session-{id}"));
        let group = SharedString::from(format!("session-row-{id}"));
        let (picture, name_color) = if archived {
            (letter_square_quiet(&session.name, theme), theme.fg_muted)
        } else {
            (
                letter_square(&session.name, theme.hue(&session.name), theme),
                theme.fg,
            )
        };
        let action = if archived {
            icon_button(
                SharedString::from(format!("{key}-delete")),
                Icon::Close,
                theme,
            )
            .tooltip(tip("Delete this session"))
            .tab_stop(selected)
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.delete_session(&id, cx);
                }
            }))
        } else {
            icon_button(
                SharedString::from(format!("{key}-archive")),
                Icon::Archive,
                theme,
            )
            .tooltip(tip("Archive this session"))
            .tab_stop(selected)
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.set_archived(&id, true, window, cx);
                }
            }))
        };
        // The row's one action: on the selected row it takes its place at the
        // end; on the others it appears under the pointer, laid over the end
        // of the row on the hover fill while the place hides, so a quiet row
        // keeps all its room. A hidden one is not a stop for the Tab key.
        let action = if selected {
            div().flex_none().mr(px(-BUTTON_PAD)).child(action)
        } else {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(metric::PAD - 1. - BUTTON_PAD))
                .pl(px(metric::GAP))
                .flex()
                .items_center()
                .bg(theme.hover)
                .invisible()
                .group_hover(group.clone(), |style| style.visible())
                .child(action)
        };
        let name = if self.renaming(&Named::Session(id.clone())) {
            self.name_field(theme, cx)
        } else {
            div()
                .flex_auto()
                .min_w_0()
                .truncate()
                .text_color(name_color)
                .child(SharedString::from(session.name.clone()))
        };
        // An archived session runs nothing.
        let busy = if archived {
            Busy::default()
        } else {
            self.busy(std::iter::once(id.as_str()))
        };
        let busy = busy_marks(SharedString::from(format!("{key}-busy")), busy, theme);
        tree_row(key, if archived { 2 } else { 1 }, selected, theme)
            .group(group.clone())
            .on_click(cx.listener({
                let id = id.clone();
                move |this, event: &gpui::ClickEvent, window, cx| {
                    if this.renaming(&Named::Session(id.clone())) {
                        return;
                    }
                    this.select_session(&id, cx);
                    // Twice: its name.
                    if event.click_count() >= 2 {
                        this.edit_name(Named::Session(id.clone()), window, cx);
                    }
                }
            }))
            .child(slot(picture))
            .child(name)
            .when_some(place, |row, place| {
                // The place gives way long before the name does.
                row.child(
                    meta(place, theme)
                        .flex_shrink(64.)
                        .min_w_0()
                        .max_w(gpui::relative(0.6))
                        .when(!selected, |place| {
                            place.group_hover(group, |style| style.invisible())
                        }),
                )
            })
            .children(busy)
            .child(action)
            .into_any_element()
    }

    fn agents(&self, room: Room, theme: &Theme, cx: &Context<Self>) -> impl IntoElement + use<> {
        // Most urgent first: what waits on the user, what failed, what
        // finished, what runs, what rests.
        let groups: [(&'static str, Mark, Hsla); 5] = [
            ("Needs you", Mark::Blocked, theme.blocked),
            ("Failed", Mark::Failed, theme.error),
            ("Done", Mark::Done, theme.done),
            ("Working", Mark::Working, theme.working),
            ("Idle", Mark::Idle, theme.fg_muted),
        ];
        let mut section = div()
            .id("rail-agents")
            .flex_none()
            .max_h(gpui::relative(0.5))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .pt(px(metric::UNIT))
            .pb(px(metric::UNIT * 2.))
            .border_t_1()
            .border_color(theme.line)
            .child(heading("Agents", theme));
        if self.store.panes.is_empty() {
            return section.child(
                div()
                    .pl(px(EDGE))
                    .pr(px(EDGE))
                    .text_size(px(metric::TEXT_SMALL))
                    .text_color(theme.fg_muted)
                    .child("Agents you start appear here, grouped by what they are doing."),
            );
        }
        let mut first = true;
        for (name, group_mark, color) in groups {
            let mut members: Vec<_> = self
                .store
                .panes
                .iter()
                .filter(|pane| {
                    self.live
                        .get(&pane.id)
                        .is_some_and(|live| live.shown() == group_mark)
                })
                .collect();
            if members.is_empty() {
                continue;
            }
            // Latest change on top.
            members.sort_by_key(|pane| {
                std::cmp::Reverse(self.live.get(&pane.id).map(|live| live.changed))
            });
            // The group's heading carries its state's mark where its rows
            // carry their harness's, and its name in the state's colour.
            let title = [
                slot(status_mark(group_mark, theme)).into_any_element(),
                div()
                    .flex_none()
                    .text_color(color)
                    .child(name)
                    .into_any_element(),
                div()
                    .flex_1()
                    .text_color(theme.fg_muted)
                    .child(SharedString::from(members.len().to_string()))
                    .into_any_element(),
            ];
            let foldable = group_mark == Mark::Idle;
            let heading = if foldable {
                row("agents-idle", false, theme)
                    .h(px(metric::HEADING))
                    .text_size(px(metric::TEXT_SMALL))
                    .font_weight(FontWeight::MEDIUM)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.idle_open = !this.idle_open;
                        cx.notify();
                    }))
                    .children(title)
                    .child(chevron(self.idle_open, theme))
                    .into_any_element()
            } else {
                div()
                    .h(px(metric::HEADING))
                    .mx(px(metric::INSET))
                    .px(px(metric::PAD))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(metric::GAP))
                    .text_size(px(metric::TEXT_SMALL))
                    .font_weight(FontWeight::MEDIUM)
                    .children(title)
                    .into_any_element()
            };
            section = section.child(
                div()
                    .flex()
                    .flex_col()
                    .when(!first, |group| group.mt(px(metric::UNIT)))
                    .child(heading),
            );
            first = false;
            let unfolded = if foldable {
                self.spring(
                    "fold-idle",
                    if self.idle_open { 1. } else { 0. },
                    Spring::SNAPPY,
                )
            } else {
                1.
            };
            if unfolded <= 0.001 {
                continue;
            }
            let mut block = div().flex().flex_col();
            for pane in &members {
                let Some(live) = self.live.get(&pane.id) else {
                    continue;
                };
                let session = self.store.session(&pane.session);
                let focused = !self.page_open()
                    && !self.pending_in_front()
                    && self.store.selected.as_deref() == Some(pane.session.as_str())
                    && session.is_some_and(|session| session.focused.as_ref() == Some(&pane.id));
                let code = self
                    .harness(&pane.harness)
                    .map_or_else(|| pane.harness.clone(), |harness| harness.code.clone());
                let id = pane.id.clone();
                let session_name = room
                    .agent_session
                    .then(|| session.map(|session| session.name.clone()))
                    .flatten();
                block = block.child(
                    row(SharedString::from(format!("agent-{id}")), focused, theme)
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| this.focus_pane(&id, cx)
                        }))
                        .child(slot(monogram(&code, theme.hue(&code), theme)))
                        .child(
                            // The agent's name gives way after its session's.
                            div()
                                .flex_shrink(1.)
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.fg)
                                .child(SharedString::from(pane.title.clone())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(metric::TEXT_SMALL))
                                .text_color(theme.fg_muted)
                                .when_some(session_name, |place, name| {
                                    place.child(SharedString::from(name))
                                }),
                        )
                        .child(meta(age(live.changed.elapsed().as_millis() as u64), theme))
                        .child(mark(live.shown(), theme)),
                );
            }
            let natural = members.len() as f32 * metric::ROW;
            section = section.child(folding(natural, unfolded).child(block));
        }
        section
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RAIL_WIDTHS;

    #[test]
    fn a_row_says_that_work_is_under_way_and_that_someone_waits() {
        use Mark::{Blocked, Done, Failed, Idle, Working};
        let busy = |marks: &[Mark]| Busy::of(marks.iter().copied());
        // Nothing going on: nothing shown.
        assert!(busy(&[]).marks().is_empty());
        assert!(busy(&[Idle, Idle]).marks().is_empty());
        // Work and a wait are both said, work first.
        assert_eq!(busy(&[Working, Idle, Working]).marks(), [Working]);
        assert_eq!(busy(&[Blocked]).marks(), [Blocked]);
        assert_eq!(busy(&[Idle, Blocked, Working]).marks(), [Working, Blocked]);
        // A failure wants the user as a wait does; a wait is the more pressing.
        assert_eq!(busy(&[Failed, Working]).marks(), [Working, Failed]);
        assert_eq!(busy(&[Failed, Blocked]).marks(), [Blocked]);
        // Finished and not looked at shows only when it is all there is.
        assert_eq!(busy(&[Done, Idle]).marks(), [Done]);
        assert_eq!(busy(&[Done, Working]).marks(), [Working]);
        assert_eq!(
            busy(&[Blocked, Working, Working, Done, Failed]).words(),
            "1 waiting for you, 1 failed, 2 working, 1 done"
        );
    }

    #[test]
    fn details_give_way_in_order_as_the_rail_narrows() {
        let (narrowest, widest) = RAIL_WIDTHS;
        assert_eq!(
            Room::at(widest),
            Room {
                place: true,
                archived_place: true,
                agent_session: true,
            }
        );
        assert_eq!(
            Room::at(narrowest),
            Room {
                place: false,
                archived_place: false,
                agent_session: false,
            }
        );
        // The default width shows a live session's place but not an
        // archived one's, which sits a level deeper.
        let default = Room::at(metric::RAIL);
        assert!(default.place && default.agent_session && !default.archived_place);
        // Narrowing never brings a detail back.
        let mut last = Room::at(widest);
        for width in (narrowest as u32..=widest as u32).rev() {
            let now = Room::at(width as f32);
            assert!(!now.place || last.place);
            assert!(!now.archived_place || last.archived_place);
            assert!(!now.agent_session || last.agent_session);
            // An archived place is never shown where a live one is not.
            assert!(!now.archived_place || now.place);
            last = now;
        }
    }
}
