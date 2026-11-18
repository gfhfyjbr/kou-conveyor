//! How the field's surroundings look and move: the list a `$`, a `/` or
//! Ctrl+R opens over it, growing from the cursor with a highlight that glides
//! between lines; the pictures' strip and the links' chips, which slide in
//! and out while the area above the text grows and shrinks with them; the
//! large picture; the short notices.

use super::{Composer, LEAVING, Leaving, MENU_ROWS, Menu, MenuKind, PictureState, What};
use crate::pictures;
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, Context, Div, ElementId, FontWeight,
    ImageSource, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    SharedString, SpringAnimation, SpringConfig, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, anchored, canvas, deferred, div, img, point,
    prelude::FluentBuilder as _,
};
use kou_thread::files::FileLink;
use kou_ui::{
    Icon, Theme, icon, key_hint, metric, motion,
    scale::{self, px},
};
use std::{cell::Cell, rc::Rc, time::Instant};

/// Height of a line of a list.
const ROW: f32 = 28.;

/// The list over the field as the app draws it (`chat-menu`): its height,
/// whether the files are still being listed, its lines and
/// its foot.
#[derive(Hash, serde::Serialize)]
struct MenuData {
    height: u32,
    loading: bool,
    lines: Vec<MenuLine>,
    footer: Option<MenuFoot>,
}

/// A line of the list: `kind` is `file`, `command`, `value`, `hint` or
/// `prompt`; `on`, the keys are on it.
#[derive(Default, Hash, serde::Serialize)]
struct MenuLine {
    kind: &'static str,
    on: bool,
    name: String,
    detail: String,
    /// A file's folder, before its name.
    dir: Option<String>,
    directory: bool,
    /// A command's other name, and what it takes.
    alias: Option<String>,
    args: Option<String>,
    /// A value that is the one in force.
    current: bool,
}

/// The list's foot: what Tab (or Enter, `enter`) does, whether Enter runs
/// (`run`), and its note.
#[derive(Hash, serde::Serialize)]
struct MenuFoot {
    take: &'static str,
    enter: bool,
    run: bool,
    note: String,
}
/// Height of a list's footer.
const FOOTER: f32 = 28.;
/// A spring that settles with a little give: what grows and what moves.
pub(crate) const SETTLE: SpringConfig = SpringConfig::new(420., 36., 1.);
/// A quicker one, for a highlight following the keys.
pub(crate) const GLIDE: SpringConfig = SpringConfig::new(900., 54., 1.);
/// Where the text starts, from the field's left edge.
const TEXT_LEFT: f32 = 0.;
/// From the field's text to the frame's outer edges: its padding
/// (`.composer`: `12px 10px 10px 14px`).
const FRAME_LEFT: f32 = 14.;
const FRAME_RIGHT: f32 = 10.;
const FRAME_TOP: f32 = 12.;

/// A block whose height follows its content's over time: it grows from
/// nothing as its content arrives and shrinks as it leaves. The content is
/// measured as drawn, in unscaled pixels so that a step of the zoom is not a
/// change; a change of it springs the height there.
pub(crate) fn grow(
    id: impl Into<ElementId>,
    measured: Rc<Cell<f32>>,
    content: impl IntoElement,
    cx: &App,
) -> AnyElement {
    // Without motion the block simply takes its content's height.
    if cx.reduce_motion() {
        return div().w_full().child(content).into_any_element();
    }
    let target = measured.get();
    let (factor, unit) = (scale::factor(), px(1.));
    let inner = div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .child(content)
        .child(
            canvas(
                move |bounds, window, _| {
                    let height = bounds.size.height.as_f32() / factor;
                    if (height - measured.get()).abs() > 0.5 {
                        measured.set(height);
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
    div()
        .relative()
        .w_full()
        .overflow_hidden()
        .child(inner)
        .with_spring(
            id,
            SpringAnimation::new(SETTLE)
                .to(target)
                .from(0.)
                .with_epsilon(0.25),
            move |this, height: f32| this.h(unit * height.max(0.)),
        )
        .into_any_element()
}

impl Composer {
    /// Notes what the strip and the tray stopped showing, so it can leave
    /// rather than vanish, and forgets what has left.
    pub(super) fn track_presence(&mut self, now: Instant, cx: &mut Context<Self>) {
        let pictures: Vec<_> = self.referenced().into_iter().cloned().collect();
        for gone in self
            .shown_pictures
            .iter()
            .filter(|old| !pictures.iter().any(|new| new.id == old.id))
        {
            self.leaving_pictures.push(Leaving {
                item: gone.clone(),
                since: now,
            });
        }
        self.leaving_pictures
            .retain(|leaving| !pictures.iter().any(|new| new.id == leaving.item.id));
        self.shown_pictures = pictures;

        let links = self.links.clone();
        for gone in self
            .shown_links
            .iter()
            .filter(|old| !links.iter().any(|new| new.label == old.label))
        {
            self.leaving_links.push(Leaving {
                item: gone.clone(),
                since: now,
            });
        }
        self.leaving_links
            .retain(|leaving| !links.iter().any(|new| new.label == leaving.item.label));
        self.shown_links = links;

        let done = |since: Instant| now.saturating_duration_since(since) >= LEAVING;
        self.leaving_pictures.retain(|leaving| !done(leaving.since));
        self.leaving_links.retain(|leaving| !done(leaving.since));
        if !self.leaving_pictures.is_empty() || !self.leaving_links.is_empty() {
            // Look again once they have gone.
            let timer = cx.background_executor().timer(LEAVING);
            cx.spawn(async move |this, cx| {
                timer.await;
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        }
        let focused: std::collections::HashSet<u64> = self
            .shown_pictures
            .iter()
            .map(|picture| picture.id)
            .collect();
        self.picture_focus.retain(|id, _| focused.contains(id));
        let labels: std::collections::HashSet<String> = self
            .shown_links
            .iter()
            .map(|link| link.label.clone())
            .collect();
        self.chip_focus.retain(|label, _| labels.contains(label));
    }

    /// The list over the field (`.commands`): as wide as the field's frame,
    /// over it, rising into place.
    pub(super) fn menu_element(&self, _: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        if menu.items.is_empty() && !menu.loading {
            return None;
        }
        let theme = self.chrome_themed(cx);
        // The field as drawn, in the unscaled sizes written here.
        let factor = scale::factor();
        let field = self.bounds.get();
        let width = field.size.width.as_f32() / factor + FRAME_LEFT + FRAME_RIGHT;
        // The list is laid out after everything else, outside the scaled
        // subtree: what moves in it takes its sizes now.
        let unit = px(1.);

        let shown = menu.items.len().min(MENU_ROWS);
        let first = menu
            .highlighted
            .saturating_sub(MENU_ROWS - 1)
            .min(menu.items.len().saturating_sub(shown));
        let loading_row = menu.loading && menu.items.is_empty();
        let list_height = if loading_row { ROW } else { shown as f32 * ROW } + 8.;
        let footer =
            matches!(menu.kind, MenuKind::Files { .. } | MenuKind::History) || menu.choosable();

        // The line the keys are on: a rounded block under it (round corners).
        let highlight = || {
            (!loading_row && menu.choosable()).then(|| {
                let top = (menu.highlighted - first) as f32 * ROW + 4.;
                div()
                    .absolute()
                    .left(px(4.))
                    .right(px(4.))
                    .top(unit * top)
                    .h(px(ROW))
                    .rounded(px(metric::RADIUS))
                    .bg(theme.hover)
            })
        };

        let prefix = match menu.kind {
            MenuKind::Files { .. } => "file-match",
            MenuKind::Commands => "slash-command",
            MenuKind::History => "history-item",
        };
        let more = menu.items.len().saturating_sub(first + shown);
        let still = cx.reduce_motion();
        let duration = if still {
            std::time::Duration::from_millis(1)
        } else {
            motion::cockpit::MENU
        };
        // The app draws the card, its lines and its foot when it can
        // (`chat-menu`); the highlight that follows the keys and where each
        // line takes the pointer stay the field's, over the lines.
        let drawn =
            self.chat_face
                .as_ref()
                .filter(|_| !self.around.studio())
                .and_then(|face| {
                    let hits = div().absolute().top_0().left_0().size_full().children(
                        menu.items.iter().enumerate().skip(first).take(shown).map(
                            |(index, item)| {
                                let top = (index - first) as f32 * ROW + 4.;
                                div()
                                    .id((prefix, index))
                                    .debug_selector(move || format!("{prefix}-{index}"))
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .top(unit * top)
                                    .h(px(ROW))
                                    .when(item.what != What::Hint, |row| {
                                        row.cursor_pointer()
                                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                                this.hover_menu(index, cx)
                                            }))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.accept(index, true, cx)
                                            }))
                                    })
                            },
                        ),
                    );
                    let mut slots = vec![("hits", hits.into_any_element())];
                    slots.extend(
                        highlight().map(|highlight| ("highlight", highlight.into_any_element())),
                    );
                    let lines: Vec<MenuLine> = menu
                        .items
                        .iter()
                        .enumerate()
                        .skip(first)
                        .take(shown)
                        .map(|(index, item)| self.menu_line(menu, index, item))
                        .collect();
                    let (take, note) = match &menu.kind {
                        MenuKind::Files { .. } => ("complete", "$path:10-40 links lines"),
                        MenuKind::Commands => ("complete", ""),
                        MenuKind::History => ("use", ""),
                    };
                    let footer = footer.then(|| MenuFoot {
                        take,
                        enter: matches!(menu.kind, MenuKind::History),
                        run: matches!(menu.kind, MenuKind::Commands),
                        note: if more > 0 {
                            format!("{more} more below")
                        } else {
                            note.to_owned()
                        },
                    });
                    let what = MenuData {
                        // (A whole number of pixels: lines of 28 and 8 around.)
                        height: list_height as u32,
                        loading: loading_row,
                        lines,
                        footer,
                    };
                    let key = format!("menu-{}", cx.entity_id().as_u64());
                    let part = crate::face::Handed::of("chat-menu", key, what);
                    let asked = |_: &mut Self,
                                 _: &str,
                                 _: &serde_json::Value,
                                 _: &mut Window,
                                 _: &mut Context<Self>| {};
                    crate::face::drawn(face, part, slots, cx.weak_entity(), asked, &self.trace, cx)
                });
        if let Some(drawn) = drawn {
            let card = div()
                .id(("menu", menu.id))
                .occlude()
                .w(px(width))
                .child(drawn)
                // `.commands`: `rise 0.1s ease-out`.
                .with_animation(
                    ("menu-in", menu.id),
                    Animation::new(duration).with_easing(motion::cockpit::ease_out),
                    move |this, t| this.opacity(t).mt(unit * (6. * (1. - t))),
                );
            return Some(Self::menu_placed(card.into_any_element()));
        }
        let highlight = highlight();
        let rows = menu
            .items
            .iter()
            .enumerate()
            .skip(first)
            .take(shown)
            .map(|(index, item)| self.menu_row(menu, index, item, prefix, theme, cx));
        let list = div()
            .relative()
            .flex_none()
            .h(px(list_height))
            .py(px(4.))
            .children(highlight)
            .when(loading_row, |list| {
                list.child(
                    div()
                        .h(px(ROW))
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .font_family(theme.mono_font.clone())
                        .text_size(px(11.5))
                        .text_color(theme.fg_muted)
                        .child(scale::text("Listing the project's files")),
                )
            })
            .children(rows);
        let footer = footer.then(|| self.menu_footer(menu, more, theme));

        let card = div()
            .id(("menu", menu.id))
            .occlude()
            .w(px(width))
            .rounded(px(metric::RADIUS_LARGE))
            .border_1()
            .border_color(theme.line_strong)
            .bg(theme.raised)
            .shadow(crate::picker::shadow(theme))
            .overflow_hidden()
            .flex()
            .flex_col()
            .justify_end()
            .text_size(px(13.))
            .text_color(theme.fg)
            .child(list)
            .children(footer)
            // `.commands`: `rise 0.1s ease-out`.
            .with_animation(
                ("menu-in", menu.id),
                Animation::new(duration).with_easing(motion::cockpit::ease_out),
                move |this, t| this.opacity(t).mt(unit * (6. * (1. - t))),
            );
        Some(Self::menu_placed(card.into_any_element()))
    }

    /// The list's card where it stands: over the field, from its frame's
    /// left edge, laid out after everything else.
    fn menu_placed(card: AnyElement) -> AnyElement {
        div()
            .absolute()
            .top_0()
            .left(px(-FRAME_LEFT))
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .offset(point(px(0.), px(-(FRAME_TOP + 6.))))
                        .snap_to_window_with_margin(px(8.))
                        .child(card),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    /// A line of the list as the app draws it (see [`MenuLine`]).
    fn menu_line(&self, menu: &Menu, index: usize, item: &super::MenuItem) -> MenuLine {
        let on = index == menu.highlighted && item.what != What::Hint;
        let mut line = MenuLine {
            kind: "hint",
            on,
            name: item.name.clone(),
            detail: item.detail.clone(),
            ..MenuLine::default()
        };
        match &item.what {
            What::File { directory } => {
                let trimmed = item.name.trim_end_matches('/');
                let cut = trimmed.rfind('/').map_or(0, |at| at + 1);
                let (dir, name) = item.name.split_at(cut);
                line.kind = "file";
                line.dir = Some(dir.to_owned());
                line.name = name.to_owned();
                line.directory = *directory;
            }
            What::Command { spec, alias } => {
                let spec = &self.catalog.specs[*spec];
                line.kind = "command";
                line.name = format!("/{}", spec.name);
                line.alias = alias.map(|alias| format!("/{alias}"));
                line.args = spec.args.clone();
            }
            What::Value { current, .. } => {
                line.kind = "value";
                line.current = *current;
            }
            What::Hint => {}
            What::Prompt => {
                line.kind = "prompt";
                line.name = item.name.split_whitespace().collect::<Vec<_>>().join(" ");
            }
        }
        line
    }

    fn menu_row(
        &self,
        menu: &Menu,
        index: usize,
        item: &super::MenuItem,
        prefix: &'static str,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let on = index == menu.highlighted && item.what != What::Hint;
        let row = div()
            .id((prefix, index))
            .debug_selector(move || format!("{prefix}-{index}"))
            .relative()
            .h(px(ROW))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(14.))
            .when(item.what != What::Hint, |row| {
                row.cursor_pointer()
                    .on_mouse_move(cx.listener(move |this, _, _, cx| this.hover_menu(index, cx)))
                    .on_click(cx.listener(move |this, _, _, cx| this.accept(index, true, cx)))
            });
        let mono = |text: SharedString| {
            div()
                .flex_none()
                .font_family(theme.mono_font.clone())
                .text_size(px(12.5))
                .child(scale::text(text))
        };
        // `.commands small`: what it does, quieter, brighter on the line chosen.
        let detail = |text: &str| {
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(px(12.5))
                .text_color(if on { theme.fg_dim } else { theme.fg_muted })
                .child(scale::text(text.to_owned()))
        };
        match &item.what {
            What::File { directory } => {
                let trimmed = item.name.trim_end_matches('/');
                let cut = trimmed.rfind('/').map_or(0, |at| at + 1);
                let (dir, name) = item.name.split_at(cut);
                row.child(
                    icon(
                        if *directory { Icon::Folder } else { Icon::File },
                        if on { theme.accent } else { theme.fg_muted },
                    )
                    .size(px(13.)),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .items_center()
                        .font_family(theme.mono_font.clone())
                        .text_size(px(metric::TEXT - 0.5))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis_start()
                                .text_color(theme.fg_muted)
                                .child(scale::text(dir.to_owned())),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if on { theme.accent } else { theme.fg })
                                .child(scale::text(name.to_owned())),
                        ),
                )
                .children((!item.detail.is_empty()).then(|| {
                    div()
                        .flex_none()
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_muted)
                        .child(scale::text(item.detail.clone()))
                }))
            }
            What::Command { spec, alias } => {
                let spec = &self.catalog.specs[*spec];
                row.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_baseline()
                        .gap(px(5.))
                        .child(
                            mono(format!("/{}", spec.name).into())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if on { theme.accent } else { theme.fg }),
                        )
                        .children(alias.map(|alias| {
                            mono(format!("/{alias}").into()).text_color(theme.fg_muted)
                        }))
                        .children(
                            spec.args
                                .clone()
                                .map(|args| mono(args.into()).text_color(theme.fg_muted)),
                        ),
                )
                .child(detail(&item.detail))
            }
            What::Value { current, .. } => row
                .child(
                    mono(item.name.clone().into())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if on { theme.accent } else { theme.fg }),
                )
                .child(detail(&item.detail))
                .children(current.then(|| {
                    div()
                        .flex_none()
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_muted)
                        .child(scale::text("current"))
                })),
            What::Hint => row
                .child(mono(item.name.clone().into()).text_color(theme.fg_dim))
                .child(detail(&item.detail)),
            What::Prompt => row
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_color(if on { theme.fg } else { theme.fg_dim })
                        .child(scale::text(
                            item.name.split_whitespace().collect::<Vec<_>>().join(" "),
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(metric::TEXT_SMALL))
                        .text_color(theme.fg_muted)
                        .child(scale::text(item.detail.clone())),
                ),
        }
    }

    fn menu_footer(&self, menu: &Menu, more: usize, theme: &Theme) -> Div {
        let hint = |keys: &'static str, what: &'static str| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(key_hint(keys, theme))
                .child(scale::text(what))
        };
        let (take, note) = match &menu.kind {
            MenuKind::Files { .. } => ("complete", "$path:10-40 links lines"),
            MenuKind::Commands => ("complete", ""),
            MenuKind::History => ("use", ""),
        };
        let note: SharedString = if more > 0 {
            format!("{more} more below").into()
        } else {
            note.into()
        };
        div()
            .flex_none()
            .h(px(FOOTER))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(16.))
            .border_t_1()
            .border_color(theme.line)
            .font_family(theme.mono_font.clone())
            .text_size(px(10.5))
            .text_color(theme.fg_muted)
            .child(hint("↑↓", "choose"))
            .child(hint(
                if matches!(menu.kind, MenuKind::History) {
                    "↵"
                } else {
                    "tab"
                },
                take,
            ))
            .when(matches!(menu.kind, MenuKind::Commands), |footer| {
                footer.child(hint("↵", "run"))
            })
            .child(hint("esc", "close"))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_right()
                    .font_family(theme.mono_font.clone())
                    .child(scale::text(note)),
            )
    }

    /// The picture shown large over the transcript, above the field.
    pub(super) fn preview_element(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (id, pinned) = self.preview?;
        let picture = self.pictures.iter().find(|picture| picture.id == id)?;
        let PictureState::Ready(prepared) = &picture.state else {
            return None;
        };
        let theme = self.themed(cx);
        let factor = scale::factor();
        let field = self.bounds.get();
        let large = prepared.large.clone();
        let size = large.size(0);
        let (width, height) = (size.width.0.max(1) as f32, size.height.0.max(1) as f32);
        // Fit it to the field's width and a height that leaves the transcript in view;
        // never larger than it is, on a display of two pixels a point. The field
        // as drawn, in the unscaled sizes written here.
        let room_w = (field.size.width.as_f32() / factor - 32.).max(160.);
        let scale = (room_w / width).min(380. / height).min(0.5);
        let (shown_w, shown_h) = (width * scale, height * scale);
        // Laid out after everything else, outside the scaled subtree.
        let lift = px(10.);
        let mut caption = vec![prepared.describe()];
        caption.extend(prepared.note.clone());
        let hint = if pinned {
            "esc or a click closes it"
        } else {
            "⌫ removes it · the cursor elsewhere closes it"
        };
        let card = div()
            .id(("preview", id))
            .occlude()
            .max_w(field.size.width)
            .p(px(8.))
            .rounded(px(metric::RADIUS_LARGE))
            .border_1()
            .border_color(theme.line_strong)
            .bg(theme.raised)
            .shadow(crate::picker::shadow(theme))
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(pinned, |card| {
                card.cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.close_preview(cx)))
            })
            .child(
                div().flex().justify_center().child(
                    div()
                        .flex_none()
                        .rounded(px(metric::RADIUS))
                        .overflow_hidden()
                        .child(
                            img(ImageSource::Render(large))
                                .w(px(shown_w))
                                .h(px(shown_h)),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .font_family(theme.mono_font.clone())
                    .text_size(px(metric::TEXT_SMALL))
                    .text_color(theme.fg_dim)
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.accent)
                            .child(scale::text(picture.label.clone())),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(scale::text(caption.join(" · "))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.fg_muted)
                            .child(scale::text(hint)),
                    ),
            )
            .with_animation(
                ("preview-in", id),
                motion::enter(motion::BASE),
                move |this, t| this.opacity(t).mb(lift * (1. - t)),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left(px(8.))
                .child(
                    deferred(
                        anchored()
                            .anchor(Anchor::BottomLeft)
                            .offset(point(px(0.), px(-10.)))
                            .snap_to_window_with_margin(px(8.))
                            .child(card),
                    )
                    .with_priority(1),
                )
                .into_any_element(),
        )
    }

    /// Above the text: a notice, what the field edits, the pictures and the links.
    pub(super) fn above_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = self.themed(cx);
        let notice = self.notice.as_ref().map(|notice| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(metric::TEXT_SMALL))
                .text_color(if notice.error {
                    theme.error
                } else {
                    theme.fg_dim
                })
                .child(
                    icon(
                        Icon::Info,
                        if notice.error {
                            theme.error
                        } else {
                            theme.fg_muted
                        },
                    )
                    .size(px(12.)),
                )
                .child(div().min_w_0().child(scale::text(notice.text.clone())))
                .with_animation(
                    ("notice", notice.id),
                    motion::enter(motion::QUICK),
                    |this, t| this.opacity(t),
                )
        });
        let banner = self.banner.clone().map(|banner| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(metric::TEXT_SMALL))
                .text_color(theme.fg_dim)
                .child(icon(Icon::Pencil, theme.accent).size(px(12.)))
                .child(scale::text(banner))
        });
        let strip = self.picture_strip(cx);
        let tray = self.link_tray(cx);
        let any = notice.is_some() || banner.is_some() || strip.is_some() || tray.is_some();
        // The quieter look's frame stands off its content at the top by
        // itself: what is above the text starts where the text would.
        let over = if self.around.studio() { 0. } else { 9. };
        let content = div()
            .w_full()
            .when(any, |content| {
                content.pl(px(TEXT_LEFT)).pr(px(4.)).pt(px(over)).pb(px(1.))
            })
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(notice)
            .children(banner)
            .children(strip)
            .children(tray);
        grow("above", self.above_height.clone(), content, cx)
    }

    fn picture_strip(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        if self.shown_pictures.is_empty() && self.leaving_pictures.is_empty() {
            return None;
        }
        let theme = self.themed(cx);
        let unit = px(1.);
        let active = self.picture_before_cursor().map(|picture| picture.id);
        let mut cards: Vec<AnyElement> = Vec::new();
        for (index, picture) in self.shown_pictures.clone().into_iter().enumerate() {
            let focus = self
                .picture_focus
                .entry(picture.id)
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let id = picture.id;
            let on = active == Some(id) || self.preview.is_some_and(|(shown, _)| shown == id);
            let ring = theme.accent;
            let card = picture_card(&picture, on, theme)
                .id(("picture", id))
                .debug_selector(move || format!("picture-{index}"))
                .track_focus(&focus)
                .cursor_pointer()
                .focus_visible(move |style| style.border_color(ring))
                .on_click(cx.listener(move |this, _, _, cx| this.open_preview(id, cx)))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "backspace" | "delete" => {
                            this.remove_picture(id, cx);
                            window.focus(&this.focus, cx);
                            cx.stop_propagation();
                        }
                        "escape" => {
                            this.close_preview(cx);
                            window.focus(&this.focus, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }))
                .child(
                    div()
                        .id(("picture-remove", id))
                        .debug_selector(move || format!("picture-remove-{index}"))
                        .absolute()
                        .top(px(3.))
                        .right(px(3.))
                        .size(px(20.))
                        .rounded(px(metric::RADIUS_SMALL))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.selected))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.remove_picture(id, cx);
                            window.focus(&this.focus, cx);
                            cx.stop_propagation();
                        }))
                        .child(icon(Icon::Close, theme.fg_muted).size(px(12.))),
                )
                .with_animation(
                    ("picture-in", id),
                    motion::enter(motion::BASE),
                    move |this, t| {
                        this.opacity(t)
                            .max_w(unit * motion::mix(40., 260., t))
                            .mt(unit * ((1. - t) * 8.))
                    },
                );
            cards.push(card.into_any_element());
        }
        for leaving in &self.leaving_pictures {
            let card = picture_card(&leaving.item, false, theme)
                .id(("picture-out", leaving.item.id))
                .with_animation(
                    ("picture-out", leaving.item.id),
                    Animation::new(LEAVING).with_easing(motion::ease_in),
                    move |this, t| this.opacity(1. - t).max_w(unit * motion::mix(260., 0., t)),
                );
            cards.push(card.into_any_element());
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .children(cards),
        )
    }

    fn link_tray(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        if self.shown_links.is_empty() && self.leaving_links.is_empty() {
            return None;
        }
        let theme = self.themed(cx);
        let unit = px(1.);
        let mut chips: Vec<AnyElement> = Vec::new();
        for (index, link) in self.shown_links.clone().into_iter().enumerate() {
            let focus = self
                .chip_focus
                .entry(link.label.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let (ring, faint) = (theme.accent, theme.fg_faint);
            let (label, remove_label) = (link.label.clone(), link.label.clone());
            let key_label = link.label.clone();
            let chip = link_chip(&link, theme)
                .id(("link-chip", index))
                .debug_selector(move || format!("link-chip-{index}"))
                .track_focus(&focus)
                .cursor_pointer()
                .hover(move |style| style.border_color(faint))
                .focus_visible(move |style| style.border_color(ring))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.cursor_after(&label, window, cx)),
                )
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "backspace" | "delete" => {
                            this.remove_link(&key_label, cx);
                            window.focus(&this.focus, cx);
                            cx.stop_propagation();
                        }
                        "escape" => {
                            window.focus(&this.focus, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }))
                .child(
                    div()
                        .id(("link-remove", index))
                        .debug_selector(move || format!("link-remove-{index}"))
                        .flex_none()
                        .size(px(16.))
                        .rounded(px(metric::RADIUS_SMALL))
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|style| style.bg(theme.selected))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.remove_link(&remove_label, cx);
                            window.focus(&this.focus, cx);
                            cx.stop_propagation();
                        }))
                        .child(icon(Icon::Close, theme.fg_muted).size(px(10.))),
                )
                .with_animation(
                    ElementId::Name(format!("link-in-{}", link.label).into()),
                    motion::enter(motion::BASE),
                    move |this, t| {
                        this.opacity(t)
                            .max_w(unit * motion::mix(30., 320., t))
                            .ml(unit * ((1. - t) * -6.))
                    },
                );
            chips.push(chip.into_any_element());
        }
        for leaving in &self.leaving_links {
            let chip = link_chip(&leaving.item, theme)
                .id(ElementId::Name(
                    format!("link-out-{}", leaving.item.label).into(),
                ))
                .with_animation(
                    ElementId::Name(format!("link-out-anim-{}", leaving.item.label).into()),
                    Animation::new(LEAVING).with_easing(motion::ease_in),
                    move |this, t| this.opacity(1. - t).max_w(unit * motion::mix(320., 0., t)),
                );
            chips.push(chip.into_any_element());
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .children(chips),
        )
    }
}

/// A picture's card in the strip: the thumbnail, its label and what it is.
fn picture_card(picture: &super::Picture, on: bool, theme: &Theme) -> Div {
    let (thumb, caption): (AnyElement, String) = match &picture.state {
        PictureState::Ready(prepared) => (
            // Zoomed in, the thumbnail would be drawn larger than it is.
            img(ImageSource::Render(if scale::factor() > 1. {
                prepared.large.clone()
            } else {
                prepared.thumb.clone()
            }))
            .size_full()
            .object_fit(gpui::ObjectFit::Cover)
            .into_any_element(),
            prepared.describe(),
        ),
        PictureState::Reading => (div().into_any_element(), "Reading".to_owned()),
    };
    div()
        .relative()
        .flex_none()
        .max_w(px(260.))
        .p(px(4.))
        .pr(px(28.))
        .flex()
        .items_center()
        .gap(px(10.))
        .overflow_hidden()
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(if on { theme.accent } else { theme.line_strong })
        .bg(theme.pane)
        .child(
            div()
                .flex_none()
                .size(px(52.))
                .rounded(px(metric::RADIUS_MARK))
                .overflow_hidden()
                .bg(theme.selected)
                .child(thumb),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .font_family(theme.mono_font.clone())
                .text_size(px(11.))
                .line_height(px(14.))
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if on { theme.accent } else { theme.fg })
                        .child(scale::text(picture.label.clone())),
                )
                .child(
                    div()
                        .truncate()
                        .text_color(theme.fg_muted)
                        .child(scale::text(caption)),
                ),
        )
}

/// A link's chip in the tray: what it links, and what of it the model sees.
fn link_chip(link: &FileLink, theme: &Theme) -> Div {
    let detail = if link.directory {
        "folder".to_owned()
    } else if link.start_line > 0 && link.end_line > 0 {
        if link.start_line == link.end_line {
            format!("line {}", link.start_line)
        } else {
            format!("lines {}-{}", link.start_line, link.end_line)
        }
    } else if link.start_line > 0 {
        format!("from line {}", link.start_line)
    } else {
        pictures::size(link.size as usize)
    };
    div()
        .flex_none()
        .max_w(px(320.))
        .h(px(24.))
        .pl(px(7.))
        .pr(px(4.))
        .flex()
        .items_center()
        .gap(px(6.))
        .overflow_hidden()
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(theme.line_strong)
        .bg(theme.pane)
        .font_family(theme.mono_font.clone())
        .text_size(px(11.5))
        .child(
            icon(
                if link.directory {
                    Icon::Folder
                } else {
                    Icon::File
                },
                theme.accent,
            )
            .size(px(12.)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg)
                .child(scale::text(link.label.clone())),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(10.5))
                .text_color(theme.fg_muted)
                .child(scale::text(detail)),
        )
}
