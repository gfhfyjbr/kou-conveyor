//! What a plugin draws, as the app's own elements.
//!
//! A plugin's page is a tree of [`Node`]s (see `kou-plugins`): boxes laid
//! out as flex, text, and a few things of the app's own (an icon, a button,
//! a harness's tile, a state's mark). This turns the tree into GPUI
//! elements, reading the properties it knows and passing the rest by:
//!
//! - layout: `dir` (`row`, `col`), `wrap`, `gap`, `gapX`, `gapY`, `items`,
//!   `justify`, `grow`, `shrink`, `position` (`absolute`, `relative`) with
//!   `inset`, `top`, `right`, `bottom`, `left`;
//! - size and room: `w`, `h`, `minW`, `minH`, `maxW`, `maxH` (pixels,
//!   `"full"`, `"40%"`), `p`, `px`, `py`, `pt`, `pr`, `pb`, `pl` and the
//!   same with `m`;
//! - look: `bg` (a colour, or `{ hatch, width, gap }`), `border` and
//!   `borderT`..., `borderColor`, `dashed`, `radius` and `radiusT`...
//!   (pixels or `"full"`), `overflow` (`hidden`, `scroll`), `opacity`;
//! - text: `color`, `size`, `weight`, `font` (`ui`, `mono`), `lineHeight`,
//!   `truncate`, `italic`, `nowrap`, `align`;
//! - what it answers to: `onClick`, `hover` (`{ bg }`), `tooltip`,
//!   `cursor`, `focusable`; and `id`, which tests find it by.
//!
//! A colour is a name of the theme's (`fg`, `fgMuted`, `accent`, `line`,
//! ...) or `#rrggbb` with or without an alpha.

use crate::shell::Shell;
use gpui::{
    AnyElement, Context, DefiniteLength, Div, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, Length, ParentElement as _, Rgba, SharedString, StatefulInteractiveElement as _,
    Styled as _, div, pattern_slash, px, relative,
};
use kou_plugins::Node;
use kou_ui::{Icon, Mark, Theme, button, button_with_icon, icon, metric, monogram_sized, tip};
use serde_json::{Value, json};
use std::cell::Cell;

/// What a page is drawn with: whose it is, for the handlers, the theme,
/// and how many elements that answer to something were made so far (each
/// is known to the window by its number).
pub(crate) struct Drawing<'a> {
    pub page: SharedString,
    pub theme: &'a Theme,
    pub made: Cell<u64>,
}

/// The theme as a plugin's script is given it: its colours as `#rrggbb`,
/// by the names scripts use, and the sizes the app's own parts keep to.
pub(crate) fn theme_json(theme: &Theme) -> Value {
    let palette = &theme.palette;
    let hex = |rgb: u32| format!("#{:06x}", rgb & 0xff_ffff);
    json!({
        "dark": theme.dark,
        "rail": hex(palette.rail),
        "pane": hex(palette.pane),
        "raised": hex(palette.raised),
        "code": hex(palette.code),
        "hover": hex(palette.hover),
        "selected": hex(palette.selected),
        "line": hex(palette.line),
        "lineStrong": hex(palette.line_strong),
        "fg": hex(palette.fg),
        "fgDim": hex(palette.fg_dim),
        "fgMuted": hex(palette.fg_muted),
        "fgFaint": hex(palette.fg_faint),
        "accent": hex(palette.accent),
        "accentInk": hex(palette.accent_ink),
        "working": hex(palette.working),
        "done": hex(palette.done),
        "blocked": hex(palette.blocked),
        "error": hex(palette.error),
        "ok": hex(palette.ok),
        "chip": hex(palette.chip),
        "chipInk": hex(palette.chip_ink),
        "metric": {
            "text": metric::TEXT,
            "textSmall": metric::TEXT_SMALL,
            "row": metric::ROW,
            "control": metric::CONTROL,
            "controlSmall": metric::CONTROL_SMALL,
            "unit": metric::UNIT,
            "gap": metric::GAP,
            "icon": metric::ICON,
            "radiusSmall": metric::RADIUS_SMALL,
            "radius": metric::RADIUS,
            "radiusMedium": metric::RADIUS_MEDIUM,
            "radiusLarge": metric::RADIUS_LARGE,
        },
    })
}

/// A colour a script names: one of the theme's, or `#rrggbb[aa]`.
fn color(value: &Value, theme: &Theme) -> Option<Hsla> {
    let name = value.as_str()?.trim();
    if let Some(hex) = name.strip_prefix('#') {
        let digits = u32::from_str_radix(hex, 16).ok()?;
        let (rgb, alpha) = match hex.len() {
            6 => (digits, 255),
            8 => (digits >> 8, digits & 0xff),
            _ => return None,
        };
        let part = |shift: u32| ((rgb >> shift) & 0xff) as f32 / 255.;
        return Some(
            Rgba {
                r: part(16),
                g: part(8),
                b: part(0),
                a: alpha as f32 / 255.,
            }
            .into(),
        );
    }
    Some(match name {
        "transparent" => gpui::transparent_black(),
        "rail" => theme.rail,
        "pane" => theme.pane,
        "raised" => theme.raised,
        "code" => theme.code,
        "hover" => theme.hover,
        "selected" => theme.selected,
        "line" => theme.line,
        "lineStrong" => theme.line_strong,
        "fg" => theme.fg,
        "fgDim" => theme.fg_dim,
        "fgMuted" => theme.fg_muted,
        "fgFaint" => theme.fg_faint,
        "accent" => theme.accent,
        "accentInk" => theme.accent_ink,
        "working" => theme.working,
        "done" => theme.done,
        "blocked" => theme.blocked,
        "error" => theme.error,
        "ok" => theme.ok,
        "chip" => theme.chip,
        "chipInk" => theme.chip_ink,
        _ => return None,
    })
}

/// A length a script gives: pixels, `"full"`, or a share as `"40%"`.
fn length(value: &Value) -> Option<DefiniteLength> {
    match value {
        Value::Number(pixels) => Some(px(pixels.as_f64()? as f32).into()),
        Value::String(text) if text == "full" => Some(relative(1.)),
        Value::String(text) => {
            let share: f32 = text.strip_suffix('%')?.trim().parse().ok()?;
            Some(relative(share / 100.))
        }
        _ => None,
    }
}

/// An icon by the name of its file: `clock`, `info`, `chevron-right`.
fn icon_named(name: &str) -> Option<Icon> {
    let path = format!("icons/{name}.svg");
    Icon::ALL.iter().copied().find(|icon| icon.path() == path)
}

/// The box of `node` with everything about how it lies and looks.
fn styled(node: &Node, theme: &Theme) -> Div {
    let mut el = div();
    let number = |name: &str| node.number(name);
    let size = |name: &str| node.props.get(name).and_then(length);
    let tint = |name: &str| node.props.get(name).and_then(|value| color(value, theme));
    match node.text("dir") {
        Some("row") => el = el.flex().flex_row(),
        Some("col") => el = el.flex().flex_col(),
        _ => {}
    }
    if node.flag("wrap") {
        el = el.flex_wrap();
    }
    if let Some(gap) = number("gap") {
        el = el.gap(px(gap));
    }
    if let Some(gap) = number("gapX") {
        el = el.gap_x(px(gap));
    }
    if let Some(gap) = number("gapY") {
        el = el.gap_y(px(gap));
    }
    el = match node.text("items") {
        Some("start") => el.items_start(),
        Some("center") => el.items_center(),
        Some("end") => el.items_end(),
        Some("baseline") => el.items_baseline(),
        _ => el,
    };
    el = match node.text("justify") {
        Some("start") => el.justify_start(),
        Some("center") => el.justify_center(),
        Some("end") => el.justify_end(),
        Some("between") => el.justify_between(),
        Some("around") => el.justify_around(),
        _ => el,
    };
    // Room inside and outside: the general first, the particular over it.
    for (name, set) in [
        ("p", Div::p as fn(Div, gpui::Pixels) -> Div),
        ("px", Div::px),
        ("py", Div::py),
        ("pt", Div::pt),
        ("pr", Div::pr),
        ("pb", Div::pb),
        ("pl", Div::pl),
        ("m", Div::m),
        ("mx", Div::mx),
        ("my", Div::my),
        ("mt", Div::mt),
        ("mr", Div::mr),
        ("mb", Div::mb),
        ("ml", Div::ml),
    ] {
        if let Some(room) = number(name) {
            el = set(el, px(room));
        }
    }
    if let Some(width) = size("w") {
        el = el.w(width);
    }
    if let Some(height) = size("h") {
        el = el.h(height);
    }
    if let Some(width) = size("minW") {
        el = el.min_w(width);
    }
    if let Some(height) = size("minH") {
        el = el.min_h(height);
    }
    if let Some(width) = size("maxW") {
        el = el.max_w(width);
    }
    if let Some(height) = size("maxH") {
        el = el.max_h(height);
    }
    // `grow: 1` is "take what room is left", as `flex: 1` is.
    let grow = match node.props.get("grow") {
        Some(Value::Bool(true)) => Some(1.),
        Some(value) => value.as_f64().map(|grow| grow as f32),
        None => None,
    };
    if let Some(grow) = grow {
        let style = el.style();
        style.flex_grow = Some(grow);
        style.flex_basis = Some(Length::Definite(relative(0.)));
    }
    if let Some(shrink) = number("shrink") {
        el.style().flex_shrink = Some(shrink);
    }
    match node.text("position") {
        Some("absolute") => el = el.absolute(),
        Some("relative") => el = el.relative(),
        _ => {}
    }
    if let Some(inset) = size("inset") {
        el = el.top(inset).right(inset).bottom(inset).left(inset);
    }
    if let Some(at) = size("top") {
        el = el.top(at);
    }
    if let Some(at) = size("right") {
        el = el.right(at);
    }
    if let Some(at) = size("bottom") {
        el = el.bottom(at);
    }
    if let Some(at) = size("left") {
        el = el.left(at);
    }
    match node.props.get("bg") {
        Some(Value::Object(pattern)) => {
            if let Some(stripe) = pattern.get("hatch").and_then(|value| color(value, theme)) {
                let width = pattern.get("width").and_then(Value::as_f64).unwrap_or(1.5);
                let gap = pattern.get("gap").and_then(Value::as_f64).unwrap_or(5.);
                el = el.bg(pattern_slash(stripe, width as f32, gap as f32));
            }
        }
        Some(value) => {
            if let Some(fill) = color(value, theme) {
                el = el.bg(fill);
            }
        }
        None => {}
    }
    if let Some(width) = number("border") {
        el = el.border(px(width));
    }
    if let Some(width) = number("borderT") {
        el = el.border_t(px(width));
    }
    if let Some(width) = number("borderR") {
        el = el.border_r(px(width));
    }
    if let Some(width) = number("borderB") {
        el = el.border_b(px(width));
    }
    if let Some(width) = number("borderL") {
        el = el.border_l(px(width));
    }
    if let Some(line) = tint("borderColor") {
        el = el.border_color(line);
    }
    if node.flag("dashed") {
        el = el.border_dashed();
    }
    let corner = |name: &str| match node.props.get(name) {
        Some(Value::String(text)) if text == "full" => Some(px(9999.)),
        Some(value) => value.as_f64().map(|radius| px(radius as f32)),
        None => None,
    };
    if let Some(radius) = corner("radius") {
        el = el.rounded(radius);
    }
    if let Some(radius) = corner("radiusT") {
        el = el.rounded_t(radius);
    }
    if let Some(radius) = corner("radiusR") {
        el = el.rounded_r(radius);
    }
    if let Some(radius) = corner("radiusB") {
        el = el.rounded_b(radius);
    }
    if let Some(radius) = corner("radiusL") {
        el = el.rounded_l(radius);
    }
    if node.text("overflow") == Some("hidden") {
        el = el.overflow_hidden();
    }
    if let Some(opacity) = number("opacity") {
        el = el.opacity(opacity.clamp(0., 1.));
    }
    // Text: what is said here and in everything inside.
    if let Some(ink) = tint("color") {
        el = el.text_color(ink);
    }
    if let Some(size) = number("size") {
        el = el.text_size(px(size));
    }
    if let Some(height) = number("lineHeight") {
        el = el.line_height(px(height));
    }
    el = match node.text("weight") {
        Some("normal") => el.font_weight(FontWeight::NORMAL),
        Some("medium") => el.font_weight(FontWeight::MEDIUM),
        Some("semibold") => el.font_weight(FontWeight::SEMIBOLD),
        Some("bold") => el.font_weight(FontWeight::BOLD),
        _ => el,
    };
    el = match node.text("font") {
        Some("mono") => el.font_family(theme.mono_font.clone()),
        Some("ui") => el.font_family(theme.ui_font.clone()),
        _ => el,
    };
    if node.flag("italic") {
        el = el.italic();
    }
    if node.flag("truncate") {
        el = el.truncate();
    }
    if node.flag("nowrap") {
        el = el.whitespace_nowrap();
    }
    el = match node.text("align") {
        Some("center") => el.text_center(),
        Some("right") => el.text_right(),
        _ => el,
    };
    if node.text("cursor") == Some("pointer") {
        el = el.cursor_pointer();
    }
    if let Some(fill) = node
        .props
        .get("hover")
        .and_then(|hover| hover.get("bg"))
        .and_then(|value| color(value, theme))
    {
        el = el.hover(move |style| style.bg(fill));
    }
    el
}

/// `node` and everything in it, as the window draws it.
pub(crate) fn element(node: &Node, drawing: &Drawing, cx: &Context<Shell>) -> AnyElement {
    let theme = drawing.theme;
    let named = node.text("id").map(str::to_owned);
    let click = node.handler("onClick");
    let said = node.text("tooltip").map(str::to_owned);
    // What it holds: its text, the app's own thing it stands for, or its
    // children.
    let mut el = styled(node, theme);
    match node.kind.as_str() {
        "text" => {
            el = el.child(SharedString::from(
                node.text("value").unwrap_or_default().to_owned(),
            ));
        }
        "icon" => {
            let tint = node
                .props
                .get("color")
                .and_then(|value| color(value, theme))
                .unwrap_or(theme.fg_muted);
            if let Some(found) = node.text("name").and_then(icon_named) {
                let side = node.number("size").unwrap_or(metric::ICON);
                el = el.flex_none().child(icon(found, tint).size(px(side)));
            }
        }
        "monogram" => {
            let code = node.text("code").unwrap_or_default();
            let side = node.number("size").unwrap_or(kou_ui::MONOGRAM_LARGE);
            el = el
                .flex_none()
                .child(monogram_sized(code, theme.hue(code), side, theme));
        }
        "mark" => {
            let mark = match node.text("state") {
                Some("working") => Mark::Working,
                Some("done") => Mark::Done,
                Some("blocked") => Mark::Blocked,
                Some("failed") => Mark::Failed,
                _ => Mark::Idle,
            };
            el = el.flex_none().child(kou_ui::status_mark(mark, theme));
        }
        "button" => {
            // The app's own button, which answers to the click itself.
            let number = drawing.made.replace(drawing.made.get() + 1);
            let id = gpui::ElementId::NamedInteger(drawing.page.clone(), number);
            let label = node.text("label").unwrap_or_default().to_owned();
            let primary = node.flag("primary");
            let mut made = match node.text("icon").and_then(icon_named) {
                Some(glyph) => button_with_icon(id, glyph, label, primary, theme),
                None => button(id, label, primary, theme),
            };
            if node.flag("small") {
                made = made
                    .h(px(metric::CONTROL_SMALL))
                    .text_size(px(metric::TEXT_SMALL));
            }
            if let Some(said) = said {
                made = made.tooltip(tip(said));
            }
            if let Some(handler) = click {
                let page = drawing.page.clone();
                made = made.on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_event(&page, handler, cx);
                }));
            }
            let made = match named {
                Some(name) => made.debug_selector(move || name.clone()).into_any_element(),
                None => made.into_any_element(),
            };
            return el.flex_none().child(made).into_any_element();
        }
        _ => {
            el = el.children(
                node.children
                    .iter()
                    .map(|child| element(child, drawing, cx)),
            );
        }
    }
    let el = match named {
        Some(name) => el.debug_selector(move || name.clone()),
        None => el,
    };
    let scrolls = node.text("overflow") == Some("scroll");
    let focusable = node.flag("focusable");
    if click.is_none() && said.is_none() && !scrolls && !focusable {
        return el.into_any_element();
    }
    // It answers to something: the window knows it by a number.
    let number = drawing.made.replace(drawing.made.get() + 1);
    let mut el = el.id(gpui::ElementId::NamedInteger(drawing.page.clone(), number));
    if scrolls {
        el = el.overflow_y_scroll();
    }
    if focusable {
        let ring = theme.accent;
        el = el
            .tab_index(0)
            .border_1()
            .border_color(gpui::transparent_black())
            .focus_visible(move |style| style.border_color(ring));
    }
    if let Some(said) = said {
        el = el.tooltip(tip(said));
    }
    if let Some(handler) = click {
        let page = drawing.page.clone();
        el = el.on_click(cx.listener(move |this, _, _, cx| {
            this.plugin_event(&page, handler, cx);
        }));
    }
    el.into_any_element()
}
