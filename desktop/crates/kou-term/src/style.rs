//! The look of a terminal view and the colour arithmetic behind it.

use gpui::{Hsla, Pixels, Rgba, SharedString, px, rgb};

/// Fonts, colours and spacing of a [`TerminalView`](crate::TerminalView).
///
/// The crate knows nothing about the application's theme: the application fills this from
/// its own tokens and calls [`TerminalView::set_style`](crate::TerminalView::set_style)
/// when the theme changes.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalStyle {
    /// The monospace family cells are drawn in.
    pub font_family: SharedString,
    /// The font size.
    pub font_size: Pixels,
    /// The height of a row as a multiple of the font size.
    pub line_height: f32,
    /// The colour of text that sets none.
    pub foreground: Hsla,
    /// The colour behind cells that set none, and of the padding.
    pub background: Hsla,
    /// The colour of the cursor.
    pub cursor: Hsla,
    /// The colour laid over selected cells; text keeps its own colour on top of it.
    pub selection: Hsla,
    /// The sixteen ANSI colours: black to white, then their bright variants.
    pub ansi: [Hsla; 16],
    /// Space between the view's edge and the grid.
    pub padding: Pixels,
    /// How the cursor travels between cells: stretched and sprung, as in
    /// Neovide. `None` moves it in one frame.
    pub cursor_motion: Option<crate::CursorMotion>,
}

impl Default for TerminalStyle {
    /// A dark style in the system's monospace font, for tests and examples, in the
    /// colours of the app's dark theme (`kou_ui::DARK`, `kou_ui::ANSI_DARK`); an
    /// application passes its own.
    fn default() -> Self {
        let ansi = [
            0x1c1c1c, 0xf0717b, 0x8fd08a, 0xe8c26a, 0x7fa6f0, 0xc59af2, 0x6fcfd3, 0xc8c8c8,
            0x707070, 0xff8f97, 0xa9e5a3, 0xf5d68d, 0xa3c1f7, 0xd8b7f7, 0x93e1e4, 0xfafafa,
        ]
        .map(|hex| Hsla::from(rgb(hex)));
        Self {
            font_family: "Menlo".into(),
            font_size: px(13.),
            line_height: 1.4,
            foreground: rgb(0xfafafa).into(),
            background: rgb(0x000000).into(),
            cursor: rgb(0xfafafa).into(),
            selection: Hsla::from(rgb(0xff5b1f)).opacity(0.3),
            ansi,
            padding: px(8.),
            cursor_motion: Some(crate::CursorMotion::default()),
        }
    }
}

/// A colour as 8-bit red, green and blue.
pub(crate) fn rgb_bytes(color: Hsla) -> (u8, u8, u8) {
    let rgba = Rgba::from(color);
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    (channel(rgba.r), channel(rgba.g), channel(rgba.b))
}

/// An 8-bit colour as a GPUI colour.
pub(crate) fn from_bytes(r: u8, g: u8, b: u8) -> Hsla {
    Rgba {
        r: f32::from(r) / 255.,
        g: f32::from(g) / 255.,
        b: f32::from(b) / 255.,
        a: 1.,
    }
    .into()
}

/// Whether a background is dark, for answering a program that asks which colour scheme
/// the terminal has (CSI ? 996 n).
pub(crate) fn is_dark(background: Hsla) -> bool {
    let (r, g, b) = rgb_bytes(background);
    // Rec. 709 luma on the 8-bit values: below the middle is dark.
    0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b) < 128.
}

/// Colours 16 to 255 of the 256-colour table as 8-bit red, green and blue: a 6x6x6 cube,
/// then a grey ramp. The first sixteen come from the style, not from a formula.
pub(crate) fn indexed_rgb_bytes(index: u8) -> (u8, u8, u8) {
    match index {
        16..=231 => {
            let index = index - 16;
            let level = |step: u8| if step == 0 { 0 } else { 55 + 40 * step };
            (level(index / 36), level(index / 6 % 6), level(index % 6))
        }
        232..=255 => {
            let grey = 8 + 10 * (index - 232);
            (grey, grey, grey)
        }
        _ => (0, 0, 0),
    }
}

/// `amount` of `over` mixed into `base`, channel by channel.
pub(crate) fn mix(base: Hsla, over: Hsla, amount: f32) -> Hsla {
    let base = Rgba::from(base);
    let over = Rgba::from(over);
    let lerp = |from: f32, to: f32| from + (to - from) * amount;
    Rgba {
        r: lerp(base.r, over.r),
        g: lerp(base.g, over.g),
        b: lerp(base.b, over.b),
        a: lerp(base.a, over.a),
    }
    .into()
}

/// How much of the background shows through faint text.
pub(crate) const DIM_AMOUNT: f32 = 0.4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_cube_and_grey_ramp() {
        assert_eq!(indexed_rgb_bytes(16), (0, 0, 0));
        assert_eq!(indexed_rgb_bytes(231), (255, 255, 255));
        // 196 is the cube's pure red.
        assert_eq!(indexed_rgb_bytes(196), (255, 0, 0));
        assert_eq!(indexed_rgb_bytes(110), (135, 175, 215));
        assert_eq!(indexed_rgb_bytes(232), (8, 8, 8));
        assert_eq!(indexed_rgb_bytes(255), (238, 238, 238));
    }

    #[test]
    fn bytes_round_trip_and_darkness() {
        let style = TerminalStyle::default();
        assert_eq!(rgb_bytes(style.background), (0, 0, 0));
        let (r, g, b) = rgb_bytes(style.foreground);
        assert_eq!(rgb_bytes(from_bytes(r, g, b)), (r, g, b));
        assert!(is_dark(style.background));
        assert!(!is_dark(style.foreground));
    }

    #[test]
    fn faint_text_leans_towards_the_background() {
        let black = from_bytes(0, 0, 0);
        let white = from_bytes(255, 255, 255);
        assert_eq!(rgb_bytes(mix(white, black, DIM_AMOUNT)), (153, 153, 153));
        assert_eq!(rgb_bytes(mix(white, black, 0.)), (255, 255, 255));
    }
}
