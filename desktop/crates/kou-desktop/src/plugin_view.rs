//! What a plugin draws, as the app's own elements.
//!
//! A plugin's page is a tree of [`Node`]s (see `kou-plugins`): boxes laid
//! out as flex, text, and a few things of the app's own (an icon, a button,
//! a harness's tile, a state's mark). This turns the tree into GPUI
//! elements, reading the properties it knows and passing the rest by:
//!
//! - layout: `dir` (`row`, `col`, or `grid` with `cols`; a child's
//!   `colSpan`), `wrap`, `gap`, `gapX`, `gapY`, `items`, `justify`, `grow`,
//!   `shrink`, `position` (`absolute`, `relative`) with `inset`, `top`,
//!   `right`, `bottom`, `left`;
//! - size and room: `w`, `h`, `minW`, `minH`, `maxW`, `maxH` (pixels,
//!   `"full"`, `"40%"`), `p`, `px`, `py`, `pt`, `pr`, `pb`, `pl` and the
//!   same with `m`;
//! - look: `bg` (a colour, or `{ hatch, width, gap }`), `border` and
//!   `borderT`... (pixels at the zoom, or `"1px"`: the screen's pixels
//!   whatever the zoom), `borderColor`, `dashed`, `radius` and `radiusT`...
//!   (pixels or `"full"`), `overflow` (`hidden`, `scroll`), `opacity`;
//! - text: `color`, `size`, `weight`, `font` (`ui`, `mono`), `lineHeight`,
//!   `truncate` (`"start"` cuts the start off instead of the end), `strike`,
//!   `italic`, `nowrap`, `align`;
//! - what it answers to: `onClick` (given `{ count, keyboard }`: 2 for a
//!   double click; `keyboard` when a key pressed it), `onPoint` (a press, or a drag, with where in the box it is:
//!   `{ x, y }` from 0 to 1), `hover` (`{ bg, color, borderColor,
//!   opacity }`), `tooltip`, `cursor` (`pointer`, `text`, `grab`),
//!   `focusable`, `tabStop` (`false`: not
//!   reached by Tab), `stop` (a click on it goes no further, to the boxes
//!   it is in); and `id`, which tests find it by;
//! - `key`: what the window knows the box by from one drawing to the next
//!   (where it is scrolled to, what has the keys, where what moves in it
//!   stands), whether or not it answers to anything; without one a box
//!   that answers to something is known by its place among those that do;
//! - `group` names a box, and `groupHover` (`{ group, bg, color, visible }`)
//!   is how a box inside it looks while the pointer is over it;
//!   `invisible` keeps a box's room but does not draw it;
//! - `windowArea: "drag"`: the window is moved by dragging the box;
//! - `basis` (a length, or `"auto"`): where a box that grows starts from;
//! - `keyContext`: the keys bound to that context work inside the box;
//! - `overflow: "scrollX"` scrolls sideways; `shadow` (`sm`, `md`, `lg`,
//!   `xl`, or `float`, the app's own under what floats); `occlude`: what is under the box gets no pointer; `stopPress`:
//!   a press in it goes no further; `onOutside`: a press anywhere else;
//! - `float` (`{ anchor, margin, priority }`): the box is drawn over
//!   everything else, hanging from its `anchor` corner (`topLeft`,
//!   `topRight`, ...) where it stands, kept `margin` inside the window: a
//!   menu;
//! - `spring` (`{ on, borderColor, bg, opacity }`): while `on`, the box goes
//!   to these over a short spring, and back when not;
//! - `ring` (`{ borderColor, color, bg }`): Tab reaches the box, and this is
//!   how it looks while the keys are on it (its own border kept);
//! - `active` (`{ top, opacity, bg }`): how it looks while it is pressed,
//!   `top` moving it down so far;
//! - an `icon`'s `enter` (`{ key, ms, easing, size, opacity }`): it grows
//!   from `size` and comes in from `opacity` when first drawn under `key`
//!   (`easing`: `ease-out`, `cubic-out` or `cubic-in`);
//! - `drop` (`{ borderColor, bg }`): the box takes files dropped on it,
//!   where the app has a use for them, and looks so while they are over it.
//!
//! A colour is a name of the theme's (`fg`, `fgMuted`, `accent`, `line`,
//! ...) or `#rrggbb` with or without an alpha; or `mix(line, error, 0.6)`
//! (the second laid on the first at that share) or `alpha(accent, 0.12)`,
//! which come out exactly as the app's own parts mix theirs; or
//! `provider(openai)`, a provider's or a harness's own colour.
//!
//! Besides boxes and `text`, `icon` (`rotate`: turned by so many degrees),
//! `monogram` (`absent`: outlined, for a harness that cannot run here),
//! `brand` (`harness`, `code`: the mark of a harness in its maker's colour,
//! none for one without), `mark` and `button` (`selected`: the one chosen
//! among buttons side by side; `size` and `color` for its text):
//!
//! - `picture` (`name`, `color`) is one of the app's larger drawings, the
//!   ones its empty states lead with (`spaces`, `stopped`, ...);
//! - `iconButton` (`name`, `size`) is a button that is an icon alone;
//! - `slot` (`name`) is a part the app draws itself, placed here: a view
//!   that is given such parts is told their names;
//! - `grain` is the film grain of a large surface, over all of the box it
//!   is in (which is `relative`) and under what comes after it;
//! - `markdown` and `code` (`value`; a code's `lang`) are drawn as the
//!   transcript draws them, where the drawing stands in one;
//! - `image` (`file`, a path in the drawing's folder);
//! - `plot` is a chart's marks (see [`crate::plot`]): `kind` (`line`,
//!   `bar`, `points`), `series` (`{ name, color, values, shown }`),
//!   `labels`, `lo`, `hi`, `grid`, `h`;
//! - `input` is a field to type in: `key` (which field it is), `value`,
//!   `placeholder`, `multiline`, `disabled`, and `onChange`, which is given
//!   `{ value }` when what was typed is kept.
//!
//! Sizes go through the zoom in effect ([`kou_ui::scale`]), so a drawing in
//! a chat on the canvas is as large as the chat around it.

use crate::plot::{Inks, Kind, Plot, Series};
use gpui::{
    AnimationExt as _, AnyElement, App, Bounds, DefiniteLength, Div, ElementId, ExternalPaths,
    FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, Length,
    MouseButton, ParentElement as _, Pixels, Rgba, SharedString, SpringAnimation, SpringConfig,
    StatefulInteractiveElement as _, Styled, StyledImage as _, Window, canvas, div, img,
    pattern_slash, relative,
};
use kou_canvas::field::Field;
use kou_plugins::Node;
use kou_thread_view::ToolAids;
use kou_ui::{
    Icon, Mark, Theme, button, button_with_icon, icon, metric, monogram_sized,
    scale::{self, px},
    tip,
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    path::{Component, Path},
    rc::Rc,
};

/// The spring a box goes to its `spring` over: it settles with a little
/// give, as the app's own parts do.
const SETTLE: SpringConfig = SpringConfig::new(420., 36., 1.);

/// Takes the files dropped on a box that says it takes them.
pub(crate) type Dropped = Rc<dyn Fn(&ExternalPaths, &mut Window, &mut App)>;

/// What something drawn does when it is used: calls the script's handler
/// of this number with what happened.
pub(crate) type On = Rc<dyn Fn(u64, Value, &mut Window, &mut App)>;

/// Where the keys of a field being typed in go.
pub(crate) type Keys = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>;

/// The field of a drawing that is being typed in: which one, the text as
/// it stands, and where its keys go.
pub(crate) struct Typing<'a> {
    pub key: &'a str,
    pub field: &'a Field,
    pub focus: &'a FocusHandle,
    pub keys: Keys,
}

/// What a page is drawn with: whose it is (the window knows its elements
/// that answer to something by this name and a number, counted in `made`),
/// the theme, and what its elements do when used.
pub(crate) struct Drawing<'a> {
    pub page: SharedString,
    pub theme: &'a Theme,
    pub made: Cell<u64>,
    pub on: On,
    /// Text as the transcript draws it, for a drawing that stands in one.
    pub aids: Option<&'a ToolAids<'a>>,
    /// The folder its files are in: its pictures.
    pub root: Option<&'a Path>,
    pub typing: Option<Typing<'a>>,
    /// The parts the app draws itself, by name, for the drawing to place:
    /// each is taken where it is placed first.
    pub slots: RefCell<Vec<(&'static str, AnyElement)>>,
    pub dropped: Option<Dropped>,
}

impl Drawing<'_> {
    /// The next element that answers to something, as the window knows it.
    fn next(&self) -> gpui::ElementId {
        let number = self.made.replace(self.made.get() + 1);
        gpui::ElementId::NamedInteger(self.page.clone(), number)
    }
}

/// The theme as a plugin's script is given it: its colours as `#rrggbb`,
/// by the names scripts use, and the sizes the app's own parts keep to.
pub(crate) fn theme_json(theme: &Theme) -> Value {
    // Worked out once for each palette: it is part of every drawing's input.
    thread_local! {
        static MADE: RefCell<Option<(kou_ui::Palette, Value)>> = const { RefCell::new(None) };
    }
    if let Some(made) = MADE.with_borrow(|made| {
        made.as_ref()
            .filter(|(palette, _)| *palette == theme.palette)
            .map(|(_, json)| json.clone())
    }) {
        return made;
    }
    let json = make_theme_json(theme);
    MADE.with_borrow_mut(|made| *made = Some((theme.palette, json.clone())));
    json
}

fn make_theme_json(theme: &Theme) -> Value {
    let palette = &theme.palette;
    let hex = |rgb: u32| format!("#{:06x}", rgb & 0xff_ffff);
    // (In two parts: one would be more than the macro can take.)
    let metric = json!({
        "text": metric::TEXT,
        "textSmall": metric::TEXT_SMALL,
        "row": metric::ROW,
        "control": metric::CONTROL,
        "controlSmall": metric::CONTROL_SMALL,
        "unit": metric::UNIT,
        "gap": metric::GAP,
        "icon": metric::ICON,
        "iconSmall": metric::ICON_SMALL,
        "radiusSmall": metric::RADIUS_SMALL,
        "radius": metric::RADIUS,
        "radiusMedium": metric::RADIUS_MEDIUM,
        "radiusLarge": metric::RADIUS_LARGE,
        "radiusMark": metric::RADIUS_MARK,
        "inset": metric::INSET,
        "pad": metric::PAD,
        "slot": metric::SLOT,
        "step": metric::STEP,
        "heading": metric::HEADING,
        "bar": metric::BAR,
    });
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
        "primary": hex(palette.primary),
        "primaryRim": hex(palette.primary_rim),
        "primaryInk": hex(palette.primary_ink),
        "hueInk": hex(kou_ui::HUE_INK),
        // The fills that tell names apart: a name's is given with it.
        "hues": kou_ui::HUES.map(hex),
        "metric": metric,
    })
}

/// A colour a script names: one of the theme's, or `#rrggbb[aa]`.
fn color(value: &Value, theme: &Theme) -> Option<Hsla> {
    let name = value.as_str()?.trim();
    // `mix(under, over, share)` and `alpha(color, share)`, worked out here
    // as the app's own parts work theirs out: not rounded to whole steps
    // first, as what `ui.mix` and `ui.alpha` give is.
    let call = |function: &str| {
        let inner = name.strip_prefix(function)?.strip_prefix('(')?;
        let parts: Vec<&str> = inner.strip_suffix(')')?.split(',').map(str::trim).collect();
        let share: f32 = parts.last()?.parse().ok()?;
        Some((parts, share.clamp(0., 1.)))
    };
    if let Some((parts, share)) = call("mix") {
        let under = Rgba::from(color(&Value::from(*parts.first()?), theme)?);
        let over = Rgba::from(color(&Value::from(*parts.get(1)?), theme)?);
        let at = |from: f32, to: f32| from + (to - from) * share;
        return Some(
            Rgba {
                r: at(under.r, over.r),
                g: at(under.g, over.g),
                b: at(under.b, over.b),
                a: at(under.a, over.a),
            }
            .into(),
        );
    }
    // A provider's or a harness's own colour, by its id.
    if let Some(id) = name
        .strip_prefix("provider(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return Some(theme.provider(id.trim()));
    }
    if let Some((parts, share)) = call("alpha") {
        return Some(color(&Value::from(*parts.first()?), theme)?.opacity(share));
    }
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
        "primary" => theme.primary,
        "primaryRim" => theme.primary_rim,
        "primaryInk" => theme.primary_ink,
        "hueInk" => theme.hue_ink,
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
    // Most boxes have none of most of these: each group of them is looked
    // at only when the box may have one of the group (see `Props::any`).
    // A property read in a group must be named in the group's list.
    const LAYOUT: u64 = kou_plugins::tags(&[
        "dir", "cols", "colSpan", "wrap", "gap", "gapX", "gapY", "items", "justify",
    ]);
    const ROOM: u64 = kou_plugins::tags(&[
        "p", "px", "py", "pt", "pr", "pb", "pl", "m", "mx", "my", "mt", "mr", "mb", "ml",
    ]);
    const PLACE: u64 = kou_plugins::tags(&[
        "w", "h", "minW", "minH", "maxW", "maxH", "grow", "basis", "shrink", "position", "inset",
        "top", "right", "bottom", "left",
    ]);
    const LOOK: u64 = kou_plugins::tags(&[
        "bg",
        "border",
        "borderT",
        "borderR",
        "borderB",
        "borderL",
        "borderColor",
        "dashed",
        "radius",
        "radiusT",
        "radiusR",
        "radiusB",
        "radiusL",
        "overflow",
        "opacity",
    ]);
    const TEXT: u64 = kou_plugins::tags(&[
        "color",
        "size",
        "lineHeight",
        "weight",
        "font",
        "italic",
        "truncate",
        "strike",
        "nowrap",
        "align",
        "keyContext",
        "cursor",
        "invisible",
        "group",
        "hover",
        "groupHover",
        "windowArea",
        "shadow",
        "occlude",
        "stopPress",
    ]);
    if node.props.any(LAYOUT) {
        match node.text("dir") {
            Some("row") => el = el.flex().flex_row(),
            Some("col") => el = el.flex().flex_col(),
            Some("grid") => {
                let cols = number("cols").unwrap_or(1.).clamp(1., 24.) as u16;
                el = el.grid().grid_cols(cols);
            }
            _ => {}
        }
        if let Some(span) = number("colSpan") {
            el = el.col_span(span.clamp(1., 24.) as u16);
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
    }
    if node.props.any(ROOM) {
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
    }
    if node.props.any(PLACE) {
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
        match node.props.get("basis") {
            Some(Value::String(text)) if text == "auto" => {
                el.style().flex_basis = Some(Length::Auto)
            }
            Some(value) => {
                if let Some(basis) = length(value) {
                    el.style().flex_basis = Some(Length::Definite(basis));
                }
            }
            None => {}
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
    }
    if node.props.any(LOOK) {
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
        // A border's width: pixels at the zoom in effect, or `"1px"`, the
        // screen's pixels whatever the zoom (a hairline stays a hairline).
        let width = |name: &str| match node.props.get(name)? {
            Value::String(text) => {
                let pixels: f32 = text.strip_suffix("px")?.trim().parse().ok()?;
                Some(gpui::px(pixels))
            }
            value => value.as_f64().map(|width| px(width as f32)),
        };
        if let Some(width) = width("border") {
            el = el.border(width);
        }
        if let Some(width) = width("borderT") {
            el = el.border_t(width);
        }
        if let Some(width) = width("borderR") {
            el = el.border_r(width);
        }
        if let Some(width) = width("borderB") {
            el = el.border_b(width);
        }
        if let Some(width) = width("borderL") {
            el = el.border_l(width);
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
    }
    if node.props.any(TEXT) {
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
        if node.text("truncate") == Some("start") {
            el = el
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis_start();
        }
        if node.flag("strike") {
            el = el.line_through();
        }
        if node.flag("nowrap") {
            el = el.whitespace_nowrap();
        }
        el = match node.text("align") {
            Some("center") => el.text_center(),
            Some("right") => el.text_right(),
            Some("left") => el.text_left(),
            _ => el,
        };
        if let Some(context) = node.text("keyContext") {
            el = el.key_context(context);
        }
        el = match node.text("cursor") {
            Some("pointer") => el.cursor_pointer(),
            Some("text") => el.cursor_text(),
            Some("grab") => el.cursor(gpui::CursorStyle::OpenHand),
            _ => el,
        };
        if node.flag("invisible") {
            el = el.invisible();
        }
        if let Some(group) = node.text("group") {
            el = el.group(SharedString::from(group.to_owned()));
        }
        if let Some(looks) = node
            .props
            .get("hover")
            .and_then(|given| Looks::of(given, theme))
        {
            el = el.hover(move |style| looks.over(style));
        }
        if let Some(given) = node.props.get("groupHover")
            && let Some(group) = given.get("group").and_then(Value::as_str)
            && let Some(looks) = Looks::of(given, theme)
        {
            el = el.group_hover(SharedString::from(group.to_owned()), move |style| {
                looks.over(style)
            });
        }
        if node.text("windowArea") == Some("drag") {
            el = el.window_control_area(gpui::WindowControlArea::Drag);
        }
        el = match node.text("shadow") {
            Some("sm") => el.shadow_sm(),
            Some("md") => el.shadow_md(),
            Some("lg") => el.shadow_lg(),
            Some("xl") => el.shadow_xl(),
            // The app's own, under what floats over the window: a menu, a
            // picker, a card of help (`0 24px 64px`, half black in the dark).
            Some("float") => el.shadow(vec![gpui::BoxShadow {
                color: gpui::black().opacity(if theme.dark { 0.5 } else { 0.18 }),
                offset: gpui::point(px(0.), px(24.)),
                blur_radius: px(64.),
                spread_radius: px(0.),
                inset: false,
            }]),
            _ => el,
        };
        if node.flag("occlude") {
            el = el.occlude();
        }
        if node.flag("stopPress") {
            el = el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        }
    }
    el
}

/// How a box looks, over how it looks at rest, while something holds (the
/// pointer is over it, or over its group).
struct Looks {
    bg: Option<Hsla>,
    color: Option<Hsla>,
    border: Option<Hsla>,
    opacity: Option<f32>,
    visible: Option<bool>,
}

impl Looks {
    /// `{ bg, color, borderColor, opacity, visible }`, when it says anything.
    fn of(given: &Value, theme: &Theme) -> Option<Self> {
        let tint = |name: &str| given.get(name).and_then(|value| color(value, theme));
        let looks = Self {
            bg: tint("bg"),
            color: tint("color"),
            border: tint("borderColor"),
            opacity: given
                .get("opacity")
                .and_then(Value::as_f64)
                .map(|opacity| (opacity as f32).clamp(0., 1.)),
            visible: given.get("visible").and_then(Value::as_bool),
        };
        let said = looks.bg.is_some()
            || looks.color.is_some()
            || looks.border.is_some()
            || looks.opacity.is_some()
            || looks.visible.is_some();
        said.then_some(looks)
    }

    fn over(&self, mut style: gpui::StyleRefinement) -> gpui::StyleRefinement {
        if let Some(fill) = self.bg {
            style = style.bg(fill);
        }
        if let Some(ink) = self.color {
            style = style.text_color(ink);
        }
        if let Some(line) = self.border {
            style = style.border_color(line);
        }
        if let Some(opacity) = self.opacity {
            style = style.opacity(opacity);
        }
        match self.visible {
            Some(true) => style.visible(),
            Some(false) => style.invisible(),
            None => style,
        }
    }
}

/// A file a drawing names, in its folder: nothing for a path that leads
/// out of the folder.
fn inside(root: &Path, file: &str) -> Option<std::path::PathBuf> {
    let file = Path::new(file);
    file.components()
        .all(|part| matches!(part, Component::Normal(_)))
        .then(|| root.join(file))
}

/// A chart's marks, as a script gives them.
fn plot(node: &Node, drawing: &Drawing) -> Option<Plot> {
    let theme = drawing.theme;
    let series: Vec<Series> = node
        .props
        .get("series")?
        .as_array()?
        .iter()
        .map(|given| Series {
            name: given
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
                .into(),
            color: given
                .get("color")
                .and_then(|value| color(value, theme))
                .unwrap_or(theme.accent),
            values: given
                .get("values")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_f64().map(|value| value as f32))
                        .collect()
                })
                .unwrap_or_default(),
            shown: strings(given.get("shown")),
        })
        .collect();
    Some(Plot {
        id: drawing.next(),
        kind: match node.text("kind") {
            Some("bar") => Kind::Bar,
            Some("points") => Kind::Points,
            _ => Kind::Line,
        },
        series,
        labels: strings(node.props.get("labels")),
        lo: node.number("lo").unwrap_or(0.),
        hi: node.number("hi").unwrap_or(1.),
        grid: node
            .props
            .get("grid")
            .and_then(Value::as_array)
            .map(|grid| {
                grid.iter()
                    .filter_map(|value| value.as_f64().map(|value| value as f32))
                    .collect()
            })
            .unwrap_or_default(),
        height: node.number("h").unwrap_or(160.).clamp(40., 600.),
        inks: Inks {
            ground: theme.raised,
            grid: theme.line,
            zero: theme.line_strong,
            text: theme.fg,
            quiet: theme.fg_muted,
            // (A step up from the card it stands in, never the pane's black.)
            tip: theme.selected,
            rim: theme.line_strong,
            font: theme.ui_font.clone(),
        },
    })
}

fn strings(given: Option<&Value>) -> Vec<SharedString> {
    given
        .and_then(Value::as_array)
        .map(|texts| {
            texts
                .iter()
                .map(|text| SharedString::from(text.as_str().unwrap_or_default().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// A field to type in (`input`): what it holds, or what it asks for while
/// it holds nothing; pressed, it is typed in where it stands.
fn input(node: &Node, drawing: &Drawing) -> AnyElement {
    let theme = drawing.theme;
    let key = node.text("key").unwrap_or_default();
    let value = node.text("value").unwrap_or_default();
    let multiline = node.flag("multiline");
    let disabled = node.flag("disabled");
    let typing = drawing.typing.as_ref().filter(|typing| typing.key == key);
    let mut el = div()
        .id(drawing.next())
        .w_full()
        .min_h(px(if multiline {
            metric::CONTROL * 2.
        } else {
            metric::CONTROL
        }))
        .px(px(8.))
        .py(px(5.))
        .flex()
        .flex_col()
        .justify_center()
        .overflow_hidden()
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(if typing.is_some() {
            theme.accent
        } else {
            theme.line_strong
        })
        .bg(theme.code)
        .text_size(px(metric::TEXT))
        .line_height(px(18.))
        .text_color(theme.fg);
    if let Some(name) = node.text("id").map(str::to_owned) {
        el = el.debug_selector(move || name.clone());
    }
    match typing {
        Some(typing) => {
            let keys = typing.keys.clone();
            el.track_focus(typing.focus)
                .on_key_down(move |event, window, cx| keys(event, window, cx))
                // A press in the field is the field's own.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(kou_canvas::field_lines(
                    SharedString::from(format!("{}-field", drawing.page)),
                    typing.field,
                    theme,
                    px(16.),
                ))
                .into_any_element()
        }
        None => {
            let shown = if value.is_empty() {
                el = el.text_color(theme.fg_faint);
                node.text("placeholder").unwrap_or_default()
            } else {
                value
            };
            el = el.children(
                shown
                    .split('\n')
                    .map(|line| div().truncate().child(scale::text(line.to_owned()))),
            );
            if disabled {
                return el.opacity(0.6).into_any_element();
            }
            let Some(handler) = node.handler("onChange") else {
                return el.into_any_element();
            };
            let (on, rim) = (drawing.on.clone(), theme.fg_faint);
            let start = json!({ "$edit": { "key": key, "value": value, "multiline": multiline } });
            el.cursor_text()
                .hover(move |style| style.border_color(rim))
                .on_click(move |_, window, cx| on(handler, start.clone(), window, cx))
                .into_any_element()
        }
    }
}

/// What a box goes to while its `spring` is on: its border and its fill,
/// each from what it has at rest.
struct Sprung {
    id: ElementId,
    on: bool,
    border: Option<(Rgba, Rgba)>,
    fill: Option<(Rgba, Rgba)>,
    /// From its `opacity` (or whole) to the spring's.
    opacity: Option<(f32, f32)>,
}

fn sprung(node: &Node, drawing: &Drawing) -> Option<Sprung> {
    let given = node.props.get("spring")?;
    let theme = drawing.theme;
    let pair = |name: &str| {
        let to = given.get(name).and_then(|value| color(value, theme))?;
        let from = node
            .props
            .get(name)
            .and_then(|value| color(value, theme))
            .unwrap_or_else(gpui::transparent_black);
        Some((Rgba::from(from), Rgba::from(to)))
    };
    Some(Sprung {
        // By its name where it has one: the spring goes on from where it
        // was, whatever else of the drawing came or went.
        id: match node.text("id") {
            Some(name) => ElementId::Name(format!("{}-{name}-spring", drawing.page).into()),
            None => drawing.next(),
        },
        on: given.get("on").and_then(Value::as_bool) == Some(true),
        border: pair("borderColor"),
        fill: pair("bg"),
        opacity: given.get("opacity").and_then(Value::as_f64).map(|to| {
            let from = node.number("opacity").unwrap_or(1.);
            (from.clamp(0., 1.), (to as f32).clamp(0., 1.))
        }),
    })
}

/// `el` as it is, or going to its spring's colours.
fn settled<E>(el: E, spring: Option<Sprung>) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    let Some(spring) = spring else {
        return el.into_any_element();
    };
    let (border, fill, opacity) = (spring.border, spring.fill, spring.opacity);
    el.with_spring(
        spring.id,
        SpringAnimation::new(SETTLE).to(spring.on),
        move |mut el, phase| {
            if let Some((from, to)) = border {
                el = el.border_color(phase.interpolate_clamped(from, to));
            }
            if let Some((from, to)) = fill {
                el = el.bg(phase.interpolate_clamped(from, to));
            }
            if let Some((from, to)) = opacity {
                el = el.opacity(from + (to - from) * phase.0.clamp(0., 1.));
            }
            el
        },
    )
    .into_any_element()
}

/// How a thing comes in when it is first drawn under its key (`enter`):
/// over how long, eased how, from what size and what opacity.
struct Entering {
    id: ElementId,
    duration: std::time::Duration,
    easing: fn(f32) -> f32,
    size: Option<f32>,
    opacity: Option<f32>,
}

fn entering(node: &Node, drawing: &Drawing) -> Option<Entering> {
    let given = node.props.get("enter")?;
    let key = given.get("key").and_then(Value::as_str)?;
    let number = |name: &str| {
        given
            .get(name)
            .and_then(Value::as_f64)
            .map(|got| got as f32)
    };
    let linear: fn(f32) -> f32 = |t| t;
    Some(Entering {
        id: ElementId::Name(format!("{}:{key}-enter", drawing.page).into()),
        duration: std::time::Duration::from_millis(
            number("ms").unwrap_or(170.).clamp(1., 2000.) as u64
        ),
        easing: match given.get("easing").and_then(Value::as_str) {
            Some("ease-out") => kou_ui::motion::cockpit::ease_out,
            Some("cubic-out") => kou_ui::motion::ease_out,
            Some("cubic-in") => kou_ui::motion::ease_in,
            _ => linear,
        },
        size: number("size"),
        opacity: number("opacity").map(|opacity| opacity.clamp(0., 1.)),
    })
}

impl Entering {
    /// An icon `side` across, coming in.
    fn icon(self, glyph: gpui::Svg, side: f32) -> AnyElement {
        let (from, opacity, unit) = (self.size.unwrap_or(side), self.opacity, px(1.));
        glyph
            .with_animation(
                self.id,
                gpui::Animation::new(self.duration).with_easing(self.easing),
                move |glyph, t| {
                    let glyph = glyph.size(unit * (from + (side - from) * t));
                    match opacity {
                        Some(start) => glyph.opacity(start + (1. - start) * t),
                        None => glyph,
                    }
                },
            )
            .into_any_element()
    }
}

/// What the window knows `node` by: its `key`, or its place.
fn known_as(node: &Node, drawing: &Drawing) -> ElementId {
    match node.text("key") {
        Some(key) => ElementId::Name(format!("{}:{key}", drawing.page).into()),
        None => drawing.next(),
    }
}

/// What a click on `node` does: its handler is told how many clicks it was,
/// and a click on a box that says `stop` goes no further.
fn clicked(
    node: &Node,
    handler: u64,
    drawing: &Drawing,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let (on, stop) = (drawing.on.clone(), node.flag("stop"));
    move |event, window, cx| {
        if stop {
            cx.stop_propagation();
        }
        let keyboard = matches!(event, gpui::ClickEvent::Keyboard(_));
        on(
            handler,
            json!({ "count": event.click_count(), "keyboard": keyboard }),
            window,
            cx,
        )
    }
}

/// `node` and everything in it, as the window draws it.
pub(crate) fn element(node: &Node, drawing: &Drawing) -> AnyElement {
    let theme = drawing.theme;
    // A part of the app's own, placed as it is: nothing around it.
    if node.kind == "slot" {
        let name = node.text("name").unwrap_or_default();
        let mut slots = drawing.slots.borrow_mut();
        return match slots.iter().position(|(slot, _)| *slot == name) {
            Some(at) => slots.remove(at).1,
            None => gpui::Empty.into_any_element(),
        };
    }
    // The film grain of a large surface, over all of the box it is in and
    // under what comes after it.
    if node.kind == "grain" {
        return kou_ui::grain(theme).into_any_element();
    }
    let named = node.text("id").map(str::to_owned);
    let click = node.handler("onClick");
    let point = node.handler("onPoint");
    let said = node.text("tooltip").map(str::to_owned);
    // What it holds: its text, the app's own thing it stands for, or its
    // children.
    let mut el = styled(node, theme);
    if let (Some(over), Some(dropped)) = (node.props.get("drop"), drawing.dropped.clone()) {
        let border = over
            .get("borderColor")
            .and_then(|value| color(value, theme));
        let fill = over.get("bg").and_then(|value| color(value, theme));
        el = el
            .drag_over::<ExternalPaths>(move |mut style, _, _, _| {
                if let Some(border) = border {
                    style = style.border_color(border);
                }
                if let Some(fill) = fill {
                    style = style.bg(fill);
                }
                style
            })
            .on_drop(move |paths: &ExternalPaths, window, cx| dropped(paths, window, cx));
    }
    match node.kind.as_str() {
        "iconButton" => {
            let Some(glyph) = node.text("name").and_then(icon_named) else {
                return el.into_any_element();
            };
            // Its square (`size`) and its glyph in it (`glyph`): the small
            // icon in a square smaller than the usual, unless it says.
            let side = node.number("size").unwrap_or(metric::ICON_BUTTON);
            let mut made = match node.number("glyph") {
                Some(glyph_side) => {
                    kou_ui::icon_button_of(known_as(node, drawing), glyph, side, glyph_side, theme)
                }
                None => kou_ui::icon_button_sized(known_as(node, drawing), glyph, side, theme),
            };
            if node.props.get("tabStop") == Some(&Value::Bool(false)) {
                made = made.tab_stop(false);
            }
            if let Some(fill) = node.props.get("bg").and_then(|value| color(value, theme)) {
                made = made.bg(fill);
            }
            if let Some(said) = said {
                made = made.tooltip(tip(said));
            }
            if let Some(handler) = click {
                made = made.on_click(clicked(node, handler, drawing));
            }
            let made = match named {
                Some(name) => made.debug_selector(move || name.clone()).into_any_element(),
                None => made.into_any_element(),
            };
            return div().flex_none().child(made).into_any_element();
        }
        "text" => {
            el = el.child(scale::text(
                node.text("value").unwrap_or_default().to_owned(),
            ));
        }
        "markdown" => {
            let value = node.text("value").unwrap_or_default();
            el = match drawing.aids {
                Some(aids) => el.child((aids.markdown)(value)),
                None => el.child(scale::text(value.to_owned())),
            };
        }
        "code" => {
            let value = node.text("value").unwrap_or_default();
            el = match drawing.aids {
                Some(aids) => el.child((aids.code)(value, node.text("lang"))),
                None => el
                    .font_family(theme.mono_font.clone())
                    .children(value.lines().map(|line| scale::text(line.to_owned()))),
            };
        }
        "image" => {
            let file = node.text("file").unwrap_or_default();
            if let Some(path) = drawing.root.and_then(|root| inside(root, file)) {
                el = el.child(
                    img(path)
                        .max_w_full()
                        .max_h(px(node.number("maxH").unwrap_or(360.)))
                        .object_fit(gpui::ObjectFit::Contain),
                );
            }
        }
        "plot" => {
            if let Some(plot) = plot(node, drawing) {
                el = el.w_full().child(plot);
            }
        }
        "input" => return el.child(input(node, drawing)).into_any_element(),
        "icon" => {
            let tint = node
                .props
                .get("color")
                .and_then(|value| color(value, theme))
                .unwrap_or(theme.fg_muted);
            if let Some(found) = node.text("name").and_then(icon_named) {
                let side = node.number("size").unwrap_or(metric::ICON);
                let mut glyph = icon(found, tint).size(px(side));
                // Turned by so many degrees: a chevron that points back.
                if let Some(turn) = node.number("rotate") {
                    glyph = glyph.with_transformation(gpui::Transformation::rotate(gpui::radians(
                        turn.to_radians(),
                    )));
                }
                // An icon takes no colour from the box it is in: its group's
                // pointer colours it itself.
                if let Some(given) = node.props.get("groupHover")
                    && let Some(group) = given.get("group").and_then(Value::as_str)
                    && let Some(ink) = given.get("color").and_then(|value| color(value, theme))
                {
                    glyph = glyph.group_hover(SharedString::from(group.to_owned()), move |style| {
                        style.text_color(ink)
                    });
                }
                el = el.flex_none().child(match entering(node, drawing) {
                    Some(enter) => enter.icon(glyph, side),
                    None => glyph.into_any_element(),
                });
            }
        }
        "brand" => {
            // A harness's mark, in its maker's colour; nothing for one that
            // has none.
            let harness = node.text("harness").unwrap_or_default();
            let found = kou_ui::Brand::of_harness(harness)
                .or_else(|| kou_ui::Brand::of(node.text("code").unwrap_or_default()));
            if let Some(found) = found {
                let side = node.number("size").unwrap_or(metric::ICON);
                el = el
                    .flex_none()
                    .child(kou_ui::brand(found, side, theme.provider(harness)));
            }
        }
        "monogram" => {
            let code = node.text("code").unwrap_or_default();
            let side = node.number("size").unwrap_or(kou_ui::MONOGRAM_LARGE);
            // A harness that cannot run here: its tile outlined.
            let tile = if node.flag("absent") {
                kou_ui::monogram_absent(code, side, theme)
            } else {
                monogram_sized(code, theme.hue(code), side, theme)
            };
            el = el.flex_none().child(tile);
        }
        "picture" => {
            let path = format!("pictures/{}.svg", node.text("name").unwrap_or_default());
            if let Some(found) = kou_ui::Picture::ALL
                .iter()
                .find(|found| found.path() == path)
            {
                let tint = node
                    .props
                    .get("color")
                    .and_then(|value| color(value, theme))
                    .unwrap_or(theme.fg_faint);
                el = el.flex_none().child(kou_ui::picture(*found, tint));
            }
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
            let label = node.text("label").unwrap_or_default().to_owned();
            let primary = node.flag("primary");
            let mut made = match node.text("icon").and_then(icon_named) {
                Some(glyph) => {
                    button_with_icon(known_as(node, drawing), glyph, label, primary, theme)
                }
                None => button(known_as(node, drawing), label, primary, theme),
            };
            if node.flag("small") {
                made = made
                    .h(px(metric::CONTROL_SMALL))
                    .text_size(px(metric::TEXT_SMALL));
            }
            // One that warns of what it does says so in its colour.
            if node.flag("danger") {
                made = made
                    .text_color(theme.error)
                    .border_color(theme.error.opacity(0.5));
            }
            // The one chosen among buttons side by side.
            if node.flag("selected") {
                made = made.bg(theme.selected).border_color(theme.accent);
            }
            if let Some(size) = node.number("size") {
                made = made.text_size(px(size));
            }
            if let Some(ink) = node
                .props
                .get("color")
                .and_then(|value| color(value, theme))
            {
                made = made.text_color(ink);
            }
            if let Some(line) = node
                .props
                .get("borderColor")
                .and_then(|value| color(value, theme))
            {
                made = made.border_color(line);
            }
            if let Some(opacity) = node.number("opacity") {
                made = made.opacity(opacity.clamp(0., 1.));
            }
            if let Some(said) = said {
                made = made.tooltip(tip(said));
            }
            if node.flag("disabled") {
                made = made.opacity(0.5).cursor_default();
            } else if let Some(handler) = click {
                made = made.on_click(clicked(node, handler, drawing));
            }
            let made = match named {
                Some(name) => made.debug_selector(move || name.clone()).into_any_element(),
                None => made.into_any_element(),
            };
            return el.flex_none().child(made).into_any_element();
        }
        _ => {
            el = el.children(node.children.iter().map(|child| element(child, drawing)));
        }
    }
    let mut el = match named {
        Some(name) => el.debug_selector(move || name.clone()),
        None => el,
    };
    if let Some(handler) = node.handler("onOutside") {
        let on = drawing.on.clone();
        el = el.on_mouse_down_out(move |_, window, cx| on(handler, json!({}), window, cx));
    }
    // A press, as it goes down (where a click waits for it to come up),
    // which goes no further.
    if let Some(handler) = node.handler("onPress") {
        let on = drawing.on.clone();
        el = el.on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            on(handler, json!({ "count": event.click_count }), window, cx)
        });
    }
    let hovered = node.handler("onHover");
    let scrolls = matches!(node.text("overflow"), Some("scroll" | "scrollX"));
    let focusable = node.flag("focusable");
    let spring = sprung(node, drawing);
    // A ring of its own while the keys are on it, and a look while pressed.
    let ring = node
        .props
        .get("ring")
        .and_then(|given| Looks::of(given, theme));
    let pressed = node.props.get("active").map(|given| {
        let top = given.get("top").and_then(Value::as_f64);
        (Looks::of(given, theme), top.map(|top| px(top as f32)))
    });
    if click.is_none()
        && point.is_none()
        && said.is_none()
        && !scrolls
        && !focusable
        && ring.is_none()
        && pressed.is_none()
        && hovered.is_none()
        // (A box with a key is known by it all the same: what moves inside
        // it goes on from where it was when it starts to answer.)
        && node.text("key").is_none()
    {
        return floated(node, settled(el, spring));
    }
    // It answers to something: the window knows it by its key or a number.
    let mut el = el.id(known_as(node, drawing));
    if scrolls {
        el = match node.text("overflow") {
            Some("scrollX") => el.overflow_x_scroll(),
            _ => el.overflow_y_scroll(),
        };
    }
    if focusable {
        let ring = theme.accent;
        // Supply a focus border only when the script has not styled one.
        // A selected theme's border must survive becoming a tab stop.
        if ["border", "borderT", "borderR", "borderB", "borderL"]
            .iter()
            .all(|key| node.props.get(key).is_none())
        {
            el = el.border_1();
        }
        if node.props.get("borderColor").is_none() {
            el = el.border_color(gpui::transparent_black());
        }
        el = el
            .tab_index(0)
            .focus_visible(move |style| style.border_color(ring));
        if node.props.get("tabStop") == Some(&Value::Bool(false)) {
            el = el.tab_stop(false);
        }
    }
    if let Some(looks) = ring {
        el = el
            .tab_index(0)
            .focus_visible(move |style| looks.over(style));
        if node.props.get("tabStop") == Some(&Value::Bool(false)) {
            el = el.tab_stop(false);
        }
    }
    if let Some((looks, top)) = pressed {
        el = el.active(move |style| {
            let style = match &looks {
                Some(looks) => looks.over(style),
                None => style,
            };
            match top {
                Some(top) => style.top(top),
                None => style,
            }
        });
    }
    if let Some(said) = said {
        el = el.tooltip(tip(said));
    }
    if let Some(handler) = click {
        el = el.on_click(clicked(node, handler, drawing));
    }
    if let Some(handler) = hovered {
        let on = drawing.on.clone();
        el = el.on_hover(move |hovered: &bool, window, cx| {
            on(handler, json!({ "hovered": *hovered }), window, cx)
        });
    }
    if let Some(handler) = point {
        // Where in the box the pointer is: the box's place is noted as it
        // is drawn, and a press or a drag is told as shares of it.
        let place: Rc<Cell<Bounds<Pixels>>> = Rc::default();
        let noted = place.clone();
        let told = move |at: gpui::Point<Pixels>| {
            let place = place.get();
            let share = |at: Pixels, from: Pixels, room: Pixels| {
                if room <= Pixels::ZERO {
                    0.
                } else {
                    ((at - from) / room).clamp(0., 1.)
                }
            };
            json!({
                "x": share(at.x, place.origin.x, place.size.width),
                "y": share(at.y, place.origin.y, place.size.height),
            })
        };
        let (press, drag) = (drawing.on.clone(), drawing.on.clone());
        let (pressed, dragged) = (told.clone(), told);
        el = el
            .child(
                canvas(move |bounds, _, _| noted.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                press(handler, pressed(event.position), window, cx);
            })
            .on_mouse_move(move |event, window, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    drag(handler, dragged(event.position), window, cx);
                }
            });
    }
    floated(node, settled(el, spring))
}

/// `el` where it is, or, for a box that says `float` (`{ anchor, margin,
/// priority }`), over everything else from there: a menu, a popover. It
/// hangs from its `anchor` corner (`topLeft`, `topRight`, `bottomLeft`,
/// `bottomRight`) and is kept `margin` inside the window.
fn floated(node: &Node, el: AnyElement) -> AnyElement {
    let Some(float) = node.props.get("float") else {
        return el;
    };
    let anchor = match float.get("anchor").and_then(Value::as_str) {
        Some("topRight") => gpui::Anchor::TopRight,
        Some("bottomLeft") => gpui::Anchor::BottomLeft,
        Some("bottomRight") => gpui::Anchor::BottomRight,
        _ => gpui::Anchor::TopLeft,
    };
    let mut anchored = gpui::anchored().anchor(anchor);
    if let Some(margin) = float.get("margin").and_then(Value::as_f64) {
        anchored = anchored.snap_to_window_with_margin(px(margin as f32));
    }
    let priority = float.get("priority").and_then(Value::as_u64).unwrap_or(1);
    gpui::deferred(anchored.child(el))
        .with_priority(priority as usize)
        .into_any_element()
}
