//! Small controls every view shares, so a button or a status mark looks and
//! behaves the same wherever it appears.

use crate::{Icon, Theme, icon, metric, scale::px};
use gpui::{
    Div, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div,
};

/// What an agent is doing, as the rail and a pane header show it. The shape
/// differs per state as well as the colour, so it reads without colour vision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mark {
    /// A turn is running: an open ring in the colour of work.
    Working,
    /// Finished and not looked at yet: a filled dot.
    Done,
    /// Waiting for the user: a filled square.
    Blocked,
    /// Nothing happening: a faint open ring.
    Idle,
    /// The process failed or exited with an error: a filled square in red.
    Failed,
}

impl Mark {
    /// The word a screen reader and a tooltip get for this state.
    pub fn label(self) -> &'static str {
        match self {
            Mark::Working => "working",
            Mark::Done => "done",
            Mark::Blocked => "needs you",
            Mark::Idle => "idle",
            Mark::Failed => "failed",
        }
    }

    /// The name a plugin's script knows this state by: `working`, `done`,
    /// `blocked`, `idle`, `failed`.
    pub fn name(self) -> &'static str {
        match self {
            Mark::Working => "working",
            Mark::Done => "done",
            Mark::Blocked => "blocked",
            Mark::Idle => "idle",
            Mark::Failed => "failed",
        }
    }
}

/// The 8px status mark for an agent.
pub fn status_mark(mark: Mark, theme: &Theme) -> Div {
    let base = div().flex_none().size(px(8.));
    match mark {
        Mark::Working => base.rounded_full().border_2().border_color(theme.working),
        Mark::Done => base.rounded_full().bg(theme.done),
        Mark::Blocked => base.rounded(px(metric::RADIUS_MARK)).bg(theme.blocked),
        Mark::Idle => base.rounded_full().border_1().border_color(theme.fg_faint),
        Mark::Failed => base.rounded(px(metric::RADIUS_MARK)).bg(theme.error),
    }
}

/// The letter a name's square shows: its first letter or digit, in capitals.
pub fn initial(name: &str) -> SharedString {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "·".to_owned())
        .into()
}

/// A coloured square with one letter: how a session or a space is told apart at a glance.
pub fn letter_square(name: &str, fill: Hsla, theme: &Theme) -> Div {
    let letter = initial(name);
    div()
        .flex_none()
        .size(px(18.))
        .rounded(px(metric::RADIUS_SMALL))
        .bg(fill)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .text_color(theme.hue_ink)
        .child(crate::scale::text(letter))
}

/// A session's letter square, quieter: an outline with a muted letter, for a
/// session that is archived and should not compete with the live ones.
pub fn letter_square_quiet(name: &str, theme: &Theme) -> Div {
    let letter = initial(name);
    div()
        .flex_none()
        .size(px(18.))
        .rounded(px(metric::RADIUS_SMALL))
        .border_1()
        .border_color(theme.fg_faint)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.fg_muted)
        .child(crate::scale::text(letter))
}

/// The side of a harness's mark in rows, tabs and pane headers.
pub const MONOGRAM: f32 = 16.;
/// The side of a harness's mark where it heads a larger row: the picker, the settings.
pub const MONOGRAM_LARGE: f32 = 20.;

/// A harness's mark at [`MONOGRAM`] size: its one or two letter code on a
/// tile of its hue. The app's own mark for the harness, not the vendor's logo.
pub fn monogram(code: &str, tint: Hsla, theme: &Theme) -> Div {
    monogram_sized(code, tint, MONOGRAM, theme)
}

/// A harness's mark at any size; drawn for [`MONOGRAM`] and [`MONOGRAM_LARGE`].
///
/// Its shape tells it from a session's letter square at a glance: three
/// corners rounded, the lower left one almost square, like a speech tile (a
/// harness is what talks), and two letters in the monospace (a harness is a
/// program) where a session has one in the interface font. The fill is the
/// categorical hue and the letters are [`crate::HUE_INK`], which the theme
/// tests hold to 4.5:1 on every hue.
pub fn monogram_sized(code: &str, tint: Hsla, side: f32, theme: &Theme) -> Div {
    harness_tile(code, side, theme.hue_ink, theme).bg(tint)
}

/// A harness's mark for one that cannot run here (not installed, broken): the
/// same tile, outlined, with muted letters.
pub fn monogram_absent(code: &str, side: f32, theme: &Theme) -> Div {
    harness_tile(code, side, theme.fg_muted, theme)
        .border_1()
        .border_color(theme.fg_faint)
}

// What is on the tile, in `ink`: the product's own mark when it has one, a
// prompt for a plain shell, else the two letters of its code.
fn harness_tile(code: &str, side: f32, ink: Hsla, theme: &Theme) -> Div {
    let corner = px((side / 4.).round());
    let tile = div()
        .flex_none()
        .size(px(side))
        .rounded_tl(corner)
        .rounded_tr(corner)
        .rounded_br(corner)
        .rounded_bl(px((side / 12.).round().max(1.)))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden();
    // A mark fills more of the tile than letters do, and keeps a rim of the hue.
    let glyph = ((side * 0.68) * 2.).round() / 2.;
    if let Some(mark) = crate::Brand::of(code) {
        return tile.child(crate::brand(mark, glyph, ink));
    }
    if code.eq_ignore_ascii_case("sh") {
        return tile.child(crate::icon(crate::Icon::Terminal, ink).size(px(glyph + 1.)));
    }
    let letters: String = code
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    tile.font_family(theme.mono_font.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px((side * 0.55 * 2.).round() / 2.))
        .line_height(px(side))
        .text_color(ink)
        .child(crate::scale::text(letters))
}

/// A square icon button: reachable by Tab, activated by click, Enter or Space,
/// with a visible ring when focused from the keyboard. Add `.on_click`.
pub fn icon_button(id: impl Into<ElementId>, glyph: Icon, theme: &Theme) -> Stateful<Div> {
    icon_button_sized(id, glyph, metric::ICON_BUTTON, theme)
}

/// An icon button of another side: 20 in a tab, 22 in a pane's header, more
/// beside a field. A smaller one than the usual holds the small icon, so
/// that its fill has room around the glyph.
pub fn icon_button_sized(
    id: impl Into<ElementId>,
    glyph: Icon,
    side: f32,
    theme: &Theme,
) -> Stateful<Div> {
    let glyph_side = if side < metric::ICON_BUTTON {
        metric::ICON_SMALL
    } else {
        metric::ICON
    };
    icon_button_of(id, glyph, side, glyph_side, theme)
}

/// A row's own action (start one here, archive this one): the usual square,
/// for the pointer, with the small icon, which is a detail of its row.
pub fn icon_button_small(id: impl Into<ElementId>, glyph: Icon, theme: &Theme) -> Stateful<Div> {
    icon_button_of(id, glyph, metric::ICON_BUTTON, metric::ICON_SMALL, theme)
}

/// An icon button of `side` whose glyph is `glyph_side` wide.
pub fn icon_button_of(
    id: impl Into<ElementId>,
    glyph: Icon,
    side: f32,
    glyph_side: f32,
    theme: &Theme,
) -> Stateful<Div> {
    let id = id.into();
    // The icon brightens with its button; an SVG takes no colour from its parent.
    let group = SharedString::from(format!("icon-button-{id:?}"));
    let (hover, ring, fg) = (theme.selected, theme.accent, theme.fg);
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .size(px(side))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(gpui::transparent_black())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tab_index(0)
        .hover(move |style| style.bg(hover))
        // Pressed, it gives a little: the answer to a click before anything else happens.
        .active(|style| style.opacity(0.6))
        .focus_visible(move |style| style.border_color(ring))
        .child(
            icon(glyph, theme.fg_muted)
                .size(px(glyph_side))
                .group_hover(group, move |style| style.text_color(fg)),
        )
}

/// What a control without a label is for, shown when the pointer rests on it:
/// `icon_button(..).tooltip(tip("Hide the rail"))`. Asked for inside a scaled
/// subtree (see [`crate::scale`]), the tip is drawn at that subtree's factor,
/// though GPUI draws it over everything else, later.
pub fn tip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let text = text.into();
    let factor = crate::scale::factor();
    move |_, cx| {
        let text = text.clone();
        gpui::AppContext::new(cx, |_| Tip(text, factor)).into()
    }
}

struct Tip(SharedString, f32);

/// What draws a tooltip's body in place of the app's own, when the app has
/// one: given what it says, at the factor in effect. `None` leaves it to
/// [`tip`]'s own.
pub type TipFace = std::rc::Rc<dyn Fn(&SharedString, &gpui::App) -> Option<gpui::AnyElement>>;

struct TipFaced(TipFace);

impl gpui::Global for TipFaced {}

/// Gives the drawing of tooltips' bodies to `face`, or back with `None`.
pub fn set_tip_face(face: Option<TipFace>, cx: &mut gpui::App) {
    match face {
        Some(face) => cx.set_global(TipFaced(face)),
        None => {
            if cx.has_global::<TipFaced>() {
                cx.remove_global::<TipFaced>();
            }
        }
    }
}

impl gpui::Render for Tip {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = crate::theme(cx);
        let face = cx.try_global::<TipFaced>().map(|faced| faced.0.clone());
        let drawn = face.and_then(|face| crate::scale::with_factor(self.1, || face(&self.0, cx)));
        if let Some(tip) = drawn {
            return if self.1 == 1. {
                tip
            } else {
                crate::scale::scaled(self.1, tip).into_any_element()
            };
        }
        let tip = crate::scale::with_factor(self.1, || {
            div()
                .debug_selector(|| "tip-native".into())
                .px(px(8.))
                .py(px(4.))
                .rounded(px(metric::RADIUS))
                .bg(theme.selected)
                .border_1()
                .border_color(theme.line_strong)
                .font_family(theme.ui_font.clone())
                .text_size(px(metric::TEXT_SMALL))
                .text_color(theme.fg)
                .child(crate::scale::text(self.0.clone()))
        });
        if self.1 == 1. {
            tip.into_any_element()
        } else {
            crate::scale::scaled(self.1, tip).into_any_element()
        }
    }
}

/// A text button. `primary` is the fill of the one primary action, as the
/// send button has it: at most one per surface.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    theme: &Theme,
) -> Stateful<Div> {
    button_base(id, primary, theme).child(crate::scale::text(label))
}

/// A text button led by an icon, for an action whose picture finds it faster
/// than its words: "In its terminal", "As a thread", "Check again".
pub fn button_with_icon(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    primary: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let ink = if primary {
        theme.primary_ink
    } else {
        theme.fg_muted
    };
    button_base(id, primary, theme)
        .pl(px(8.))
        .child(icon(glyph, ink).size(px(metric::ICON_SMALL)))
        .child(crate::scale::text(label))
}

fn button_base(id: impl Into<ElementId>, primary: bool, theme: &Theme) -> Stateful<Div> {
    let ring = theme.accent;
    let base = div()
        .id(id)
        .flex_none()
        .h(px(metric::CONTROL))
        .px(px(10.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(metric::TEXT))
        .cursor_pointer()
        .tab_index(0);
    if primary {
        let ink = theme.primary_ink;
        base.bg(theme.primary)
            .border_color(theme.primary_rim)
            .text_color(ink)
            .font_weight(FontWeight::MEDIUM)
            .hover(|style| style.opacity(0.9))
            .active(|style| style.opacity(0.7))
            .focus_visible(move |style| style.border_color(ink))
    } else {
        let hover = theme.selected;
        base.border_color(theme.line_strong)
            .text_color(theme.fg)
            .hover(move |style| style.bg(hover))
            .active(|style| style.opacity(0.7))
            .focus_visible(move |style| style.border_color(ring))
    }
}

/// The key a control answers to, shown beside its label.
pub fn key_hint(keys: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .flex_none()
        .px(px(4.))
        .rounded(px(metric::RADIUS_SMALL))
        .border_1()
        .border_color(theme.line_strong)
        .font_family(theme.mono_font.clone())
        .text_size(px(10.5))
        .text_color(theme.fg_muted)
        .child(crate::scale::text(keys))
}
