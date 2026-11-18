//! Every colour, size and font the desktop views use.
//!
//! Colour presets and portable custom palettes live in `themes`. These
//! tokens are shared by native views, plugin drawings and terminal styles.

use gpui::{App, Global, Hsla, SharedString, WindowAppearance, rgb};

macro_rules! palette {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        /// A theme's colours as `0xRRGGBB`, so they can be checked without a window.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct Palette { $($(#[$doc])* pub $name: u32,)* }
        impl Palette {
            pub const TOKENS: &'static [&'static str] = &[$(stringify!($name)),*];
            pub fn get(&self, token: &str) -> Option<u32> {
                match token { $(stringify!($name) => Some(self.$name),)* _ => None }
            }
            pub fn set(&mut self, token: &str, color: u32) -> bool {
                if color > 0xffffff { return false; }
                match token { $(stringify!($name) => self.$name = color,)* _ => return false }
                true
            }
        }
        /// The same colours, ready to paint.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Colors { $($(#[$doc])* pub $name: Hsla,)* }
        impl From<&Palette> for Colors {
            fn from(p: &Palette) -> Self {
                Self { $($name: rgb(p.$name).into(),)* }
            }
        }
    };
}

palette! {
    /// The rail and the tab strip.
    rail,
    /// The working surface: terminals, threads, the canvas.
    pane,
    /// A surface raised a step off the pane: the prompt field, a card, a menu.
    raised,
    /// Behind code and the output of commands.
    code,
    /// A row under the pointer.
    hover,
    /// The selected row or tab; a prompt's bubble.
    selected,
    /// Hairline between regions.
    line,
    /// Border of something that floats: a card, a menu, a node.
    line_strong,
    /// Primary text.
    fg,
    /// Secondary text: pane paths, quiet terminal output.
    fg_dim,
    /// Meta text: counts, branches, ages. The dimmest colour still legible as text.
    fg_muted,
    /// Idle marks and disabled icons. Never text.
    fg_faint,
    /// Links, the focus ring, what is selected on the canvas.
    accent,
    /// Text on an accent fill.
    accent_ink,
    /// An agent whose turn runs: its mark, its dot, what waits on it.
    working,
    /// Finished and not yet looked at.
    done,
    /// Waiting for the user: a permission prompt, a question.
    blocked,
    /// Failed.
    error,
    /// Succeeded, additions.
    ok,
    /// Background of an added line in a diff.
    diff_add,
    /// Background of a removed line in a diff.
    diff_del,
    /// The one primary action on a surface: the send button, a primary button.
    primary,
    /// The rim of a primary action.
    primary_rim,
    /// Text and icons on a primary action.
    primary_ink,
    /// Behind inline code.
    chip,
    /// Inline code.
    chip_ink,
}

/// The reference picture's colours, sampled: the ground `#000000`, the prompt
/// field and the side card `#0e0e0e`, the field's hairline `#1a1a1a` and the
/// card's `#252525`, prose `#fafafa`, green `#00cc7c`, inline
/// code `#d4d4d4` on `#1a1a1a`. The picture's send button runs from `#121331`
/// to `#30366f` at its foot; ours is one flat colour, the middle of the two,
/// with the lighter one as its rim. `fg_muted` and `fg_faint` are lifted a
/// little from the picture's greys so that meta text clears 4.5:1 and marks
/// 3:1 on the selected fill. The accent is the app's orange, not the
/// picture's teal, and an agent that runs wears it; one that finished is
/// blue. Amber and red, which the picture does not show, keep their earlier
/// hues.
pub const DARK: Palette = Palette {
    rail: 0x000000,
    pane: 0x000000,
    raised: 0x0e0e0e,
    code: 0x0e0e0e,
    hover: 0x121212,
    selected: 0x1c1c1c,
    line: 0x1a1a1a,
    line_strong: 0x252525,
    fg: 0xfafafa,
    fg_dim: 0xb4b4b4,
    fg_muted: 0x858585,
    fg_faint: 0x686868,
    accent: 0xff5b1f,
    accent_ink: 0x000000,
    working: 0xff5b1f,
    done: 0x87a8ed,
    blocked: 0xefb443,
    error: 0xff5468,
    ok: 0x00cc7c,
    diff_add: 0x022800,
    diff_del: 0x3d0100,
    primary: 0x212550,
    primary_rim: 0x30366f,
    primary_ink: 0xfafafa,
    chip: 0x1a1a1a,
    chip_ink: 0xd4d4d4,
};

/// [`DARK`] on white, each colour darkened until it clears the same contrast
/// there.
pub const LIGHT: Palette = Palette {
    rail: 0xf5f5f5,
    pane: 0xffffff,
    raised: 0xf7f7f7,
    code: 0xf5f5f5,
    hover: 0xf0f0f0,
    selected: 0xe8e8e8,
    line: 0xe5e5e5,
    line_strong: 0xd6d6d6,
    fg: 0x0d0d0d,
    fg_dim: 0x454545,
    fg_muted: 0x666666,
    fg_faint: 0x828282,
    accent: 0xa83205,
    accent_ink: 0xffffff,
    working: 0xa83205,
    done: 0x2a55bd,
    blocked: 0x7a4f00,
    error: 0xb01834,
    ok: 0x08703f,
    diff_add: 0xd9f2dc,
    diff_del: 0xf9dada,
    primary: 0x32386c,
    primary_rim: 0x3d4585,
    primary_ink: 0xffffff,
    chip: 0xefefef,
    chip_ink: 0x2b2b2b,
};

/// Fills of the letter squares that tell sessions, spaces and harnesses apart.
/// Categorical, not decorative: one hue per name, the same in both themes.
pub const HUES: [u32; 8] = [
    0x39ab63, 0x2aa7a0, 0x4d9fe0, 0x7d9bf0, 0xa58bf0, 0xd86397, 0xe98051, 0xcfa43a,
];
/// The letter on a hue square: the dark theme's ground.
pub const HUE_INK: u32 = 0x000000;

/// The accents the settings offer, as they are on the dark ground; the first
/// is the palette's own. On the light ground they are the same hues, darkened
/// until they read there ([`accent_in`]).
pub const ACCENTS: [(&str, u32); 6] = [
    ("Orange", 0xff5b1f),
    ("Teal", 0x009f8b),
    ("Green", 0x39ab63),
    ("Blue", 0x4d9fe0),
    ("Violet", 0xa58bf0),
    ("Pink", 0xd86397),
];

/// The accent a theme draws with: the palette's own without a choice; with
/// one, that colour moved toward white on the dark ground, or toward black on
/// the light one, until it reads as text (4.5:1) on every surface.
pub fn accent_in(dark: bool, chosen: Option<u32>) -> u32 {
    accent_on(if dark { DARK } else { LIGHT }, dark, chosen)
}

fn accent_on(palette: Palette, dark: bool, chosen: Option<u32>) -> u32 {
    let Some(chosen) = chosen else {
        return palette.accent;
    };
    let chosen = chosen & 0xff_ffff;
    let grounds = [
        palette.rail,
        palette.pane,
        palette.raised,
        palette.code,
        palette.hover,
        palette.selected,
    ];
    let toward = if dark { 0xff_ffff } else { 0x00_0000 };
    (0..=50)
        .map(|step| blend(chosen, toward, step as f32 / 50.))
        .find(|color| {
            grounds
                .iter()
                .all(|ground| contrast(*color, *ground) >= 4.5)
        })
        .unwrap_or(toward)
}

/// The colours of the providers that make models, the web cockpit's
/// (`theme.css`, `[data-provider]`): a provider's dot in the model picker, on
/// a prompt's model tag. The same in both themes; xAI takes the text colour.
pub const PROVIDERS: [(&[&str], u32); 8] = [
    (&["anthropic", "claude"], 0xd9774f),
    (&["openai", "codex"], 0x8f8aff),
    (&["google", "antigravity", "gemini"], 0x3fb6a8),
    (&["moonshot", "kimi"], 0x4f95ff),
    (&["meta", "cognition", "devin"], 0x6f8cff),
    (&["deepseek"], 0x5b8def),
    (&["qwen"], 0xa38cf4),
    (&["mistral"], 0xff8a3d),
];

/// The sixteen ANSI colours a terminal pane maps to, normal then bright.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnsiPalette(pub [u32; 16]);

/// On the black ground: black is the selected fill, so that a bar a program
/// paints black still shows; the greys are neutral, as the palette's are.
pub const ANSI_DARK: AnsiPalette = AnsiPalette([
    0x1c1c1c, 0xf0717b, 0x8fd08a, 0xe8c26a, 0x7fa6f0, 0xc59af2, 0x6fcfd3, 0xc8c8c8, 0x707070,
    0xff8f97, 0xa9e5a3, 0xf5d68d, 0xa3c1f7, 0xd8b7f7, 0x93e1e4, 0xfafafa,
]);
/// On white: black is text, white a grey that still reads.
pub const ANSI_LIGHT: AnsiPalette = AnsiPalette([
    0x171717, 0xc0273d, 0x2a7d36, 0x8a5a00, 0x2757c4, 0x8a3fc0, 0x157a80, 0x6b6b6b, 0x454545,
    0xd9304a, 0x35913f, 0xa66d00, 0x3a6be0, 0xa052d8, 0x1c9198, 0x828282,
]);

/// Sizes in logical pixels. One scale, so regions line up without per-view numbers.
pub mod metric {
    /// The rail's width until the user drags its edge: the reference's, at 1x.
    pub const RAIL: f32 = 264.;
    /// Height of the tab strip and of the rail's top bar, which share a line.
    pub const BAR: f32 = 38.;
    /// A list row.
    pub const ROW: f32 = 28.;
    /// Body text.
    pub const TEXT: f32 = 13.;
    /// Meta text beside a row's name.
    pub const TEXT_SMALL: f32 = 11.5;

    // Corners. The reference picture has three, measured at its 1.7 pixels to
    // the point, and circles; the app has those and one between them. Every
    // corner in the app is one of these or `rounded_full`.

    /// A mark of a few pixels: a state's square, a meter's bar, a bullet,
    /// the letters of a mention. Half the small corner.
    pub const RADIUS_MARK: f32 = RADIUS_SMALL / 2.;
    /// What is smaller than a control: chips, tags and badges under 22 pixels
    /// tall, letter squares, key hints, inline code (6 of the picture's
    /// pixels), a field inside a row.
    pub const RADIUS_SMALL: f32 = 4.;
    /// Whatever is pressed and is 22 pixels tall or more: buttons, tabs, the
    /// rows of a list or a menu, icon buttons, words that act; and tooltips.
    /// The picture's toolbar button (10 of its pixels). A control with a
    /// word on it never has more: no pills.
    pub const RADIUS: f32 = 6.;
    /// What is framed inside a column or a card: code, a tool's output, a
    /// picture, a notice; and a floating bar one row tall (a toolbar, a
    /// toast), which a large corner would turn into a pill. A large frame's
    /// corner one unit in, so a frame set one unit inside a large one runs
    /// parallel to it; in the same way a frame inside any other takes that
    /// one's corner less the padding between them.
    pub const RADIUS_MEDIUM: f32 = RADIUS_LARGE - UNIT;
    /// What holds input or floats: the prompt field, cards, menus, popovers,
    /// canvas nodes, the inspector, a screen's groups. The picture's prompt
    /// field and the card beside the transcript (24 of its pixels).
    pub const RADIUS_LARGE: f32 = 14.;
    /// An icon's box: in a toolbar's button, and where it leads a row.
    pub const ICON: f32 = 16.;
    /// The box of an icon that is a detail of a row or a header: what closes
    /// a tab, what opens a group, a row's own action. Beside 13px text the
    /// full icon reads as heavier than the words it serves.
    pub const ICON_SMALL: f32 = 14.;
    /// An icon button: its square, which fills under the pointer.
    pub const ICON_BUTTON: f32 = 24.;

    // The rhythm of lists. Every list in the app (the rail, the picker, the
    // settings) puts its first picture and its headings on the same vertical
    // line, `INSET + PAD` in from its region's edge, and each level of a tree
    // one `STEP` further, so a child's picture sits under its parent's label.

    /// The unit every vertical gap is a multiple of.
    pub const UNIT: f32 = 4.;
    /// How far a row's fill (hover, selection) sits in from its region's edge.
    pub const INSET: f32 = 6.;
    /// From a row's fill to its first picture; includes the row's 1px focus border.
    pub const PAD: f32 = 8.;
    /// The box every leading picture sits in (an icon, a letter square, a
    /// harness mark, a state mark), so the labels after them line up.
    pub const SLOT: f32 = 18.;
    /// Between a picture and its label, and between a row's details.
    pub const GAP: f32 = 6.;
    /// One level of a tree.
    pub const STEP: f32 = SLOT + GAP;
    /// A heading over a group of rows.
    pub const HEADING: f32 = 24.;
    /// A text button.
    pub const CONTROL: f32 = 26.;
    /// A text button inside a row or a header.
    pub const CONTROL_SMALL: f32 = 22.;
}

/// How much of a surface the film grain may cover at its strongest speck, in
/// the dark theme. The grain is the theme's `fg` laid over the surface through
/// a fixed, sparse noise (about two pixels in three are left untouched), so
/// on average it moves a surface by about one level in 255. The strongest
/// speck is what the contrast tests hold every text colour against; 0 turns
/// the grain off. See `kou_ui::grain`. Off: the reference's ground is flat,
/// and on black any grain reads as dust.
pub const GRAIN_DARK: f32 = 0.;
/// The same for the light theme.
pub const GRAIN_LIGHT: f32 = 0.;

// Rows are taller than the controls that sit in them.
const _: () = assert!(metric::CONTROL_SMALL < metric::CONTROL && metric::CONTROL < metric::ROW);
// The corners, each larger than the last.
const _: () = assert!(
    metric::RADIUS_MARK < metric::RADIUS_SMALL
        && metric::RADIUS_SMALL < metric::RADIUS
        && metric::RADIUS < metric::RADIUS_MEDIUM
        && metric::RADIUS_MEDIUM < metric::RADIUS_LARGE
);

/// The active theme, held as a GPUI global.
#[derive(Clone)]
pub struct Theme {
    /// Whether this is the dark palette.
    pub dark: bool,
    colors: Colors,
    /// See [`HUES`].
    pub hues: [Hsla; 8],
    /// See [`HUE_INK`].
    pub hue_ink: Hsla,
    /// Terminal colours as `0xRRGGBB`; the terminal crate has its own colour type.
    pub ansi: AnsiPalette,
    /// The palette the colours came from.
    pub palette: Palette,
    /// Interface labels and prose.
    pub ui_font: SharedString,
    /// Code, commands, terminal cells and numeric metadata.
    pub mono_font: SharedString,
}

impl std::ops::Deref for Theme {
    type Target = Colors;
    fn deref(&self) -> &Colors {
        &self.colors
    }
}

impl Global for Theme {}

impl Theme {
    /// The theme for a system appearance.
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        let dark = matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        Self::new(dark)
    }

    /// The dark or the light theme.
    pub fn new(dark: bool) -> Self {
        let palette = if dark { DARK } else { LIGHT };
        Self::from_palette(dark, palette)
    }

    pub fn from_palette(dark: bool, palette: Palette) -> Self {
        Self {
            dark,
            colors: Colors::from(&palette),
            hues: HUES.map(|hue| rgb(hue).into()),
            hue_ink: rgb(HUE_INK).into(),
            ansi: if dark { ANSI_DARK } else { ANSI_LIGHT },
            palette,
            ui_font: SharedString::new_static("Onest"),
            mono_font: SharedString::new_static("Martian Mono"),
        }
    }

    /// This theme with the accent the user chose in place of the palette's
    /// (see [`accent_in`]); `None` leaves it as it is. The accent, the ink on
    /// it and the colour of an agent at work change, which is the accent's:
    /// the app's orange is one colour to its user, wherever it shows.
    pub fn accented(mut self, accent: Option<u32>) -> Self {
        if accent.is_none() {
            return self;
        }
        let accent = accent_on(self.palette, self.dark, accent);
        self.palette.accent = accent;
        self.palette.working = accent;
        self.palette.accent_ink = if contrast(0x00_0000, accent) >= contrast(0xff_ffff, accent) {
            0x00_0000
        } else {
            0xff_ffff
        };
        self.colors = Colors::from(&self.palette);
        self
    }

    /// The hue square colour for a name: stable across runs, spread over the eight hues.
    pub fn hue(&self, name: &str) -> Hsla {
        self.hues[hue_index(name)]
    }

    /// A model provider's colour (see [`PROVIDERS`]): the text colour for
    /// xAI, and `fg_faint` for a provider without one.
    pub fn provider(&self, id: &str) -> Hsla {
        let id = id.trim().to_ascii_lowercase();
        if id == "xai" {
            return self.colors.fg;
        }
        PROVIDERS
            .iter()
            .find(|(names, _)| names.contains(&id.as_str()))
            .map_or(self.colors.fg_faint, |(_, hue)| rgb(*hue).into())
    }

    /// The colour the film grain is painted in: `fg` at the theme's grain
    /// strength, so it lightens a dark surface and darkens a light one.
    pub fn grain_ink(&self) -> Hsla {
        self.colors.fg.opacity(grain_strength(self.dark))
    }
}

/// The grain strength of a theme, as [`GRAIN_DARK`] or [`GRAIN_LIGHT`].
pub fn grain_strength(dark: bool) -> f32 {
    if dark { GRAIN_DARK } else { GRAIN_LIGHT }
}

/// `under` with `over` laid on it at `alpha`, as `0xRRGGBB`: what a surface
/// looks like under its strongest grain speck, for checking contrast.
pub fn blend(under: u32, over: u32, alpha: f32) -> u32 {
    let channel = |shift: u32| {
        let a = ((under >> shift) & 0xff) as f32;
        let b = ((over >> shift) & 0xff) as f32;
        ((a + (b - a) * alpha.clamp(0., 1.)).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// Which of the eight hues a name gets. FNV-1a, so the choice survives restarts.
pub fn hue_index(name: &str) -> usize {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in name.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    (hash % HUES.len() as u64) as usize
}

const FONT_FILES: &[&[u8]] = &[
    include_bytes!("../fonts/onest/Onest-Regular.ttf"),
    include_bytes!("../fonts/onest/Onest-Medium.ttf"),
    include_bytes!("../fonts/onest/Onest-SemiBold.ttf"),
    include_bytes!("../fonts/onest/Onest-Bold.ttf"),
    include_bytes!("../fonts/martian-mono/MartianMono-Regular.ttf"),
    include_bytes!("../fonts/martian-mono/MartianMono-Medium.ttf"),
    include_bytes!("../fonts/martian-mono/MartianMono-SemiBold.ttf"),
];

struct FontsLoaded;
impl Global for FontsLoaded {}

/// Register the bundled fonts and install the theme for the current appearance.
pub fn init(cx: &mut App) {
    if !cx.has_global::<FontsLoaded>() {
        let faces = FONT_FILES
            .iter()
            .copied()
            .map(std::borrow::Cow::Borrowed)
            .collect();
        match cx.text_system().add_fonts(faces) {
            Ok(()) => cx.set_global(FontsLoaded),
            Err(err) => eprintln!("kou-ui: could not register bundled fonts: {err}"),
        }
    }
    let appearance = cx.window_appearance();
    cx.set_global(Theme::for_appearance(appearance));
}

/// The active theme.
pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

/// WCAG relative luminance of `0xRRGGBB`.
pub fn luminance(color: u32) -> f32 {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 0xff) as f32 / 255.;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// WCAG contrast ratio of two `0xRRGGBB` colours.
pub fn contrast(a: u32, b: u32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surfaces(p: &Palette) -> [(&'static str, u32); 6] {
        [
            ("rail", p.rail),
            ("pane", p.pane),
            ("raised", p.raised),
            ("code", p.code),
            ("hover", p.hover),
            ("selected", p.selected),
        ]
    }

    // Every colour that is drawn as text on a surface.
    fn text(p: &Palette) -> [(&'static str, u32); 9] {
        [
            ("fg", p.fg),
            ("fg_dim", p.fg_dim),
            ("fg_muted", p.fg_muted),
            ("accent", p.accent),
            ("working", p.working),
            ("done", p.done),
            ("blocked", p.blocked),
            ("error", p.error),
            ("ok", p.ok),
        ]
    }

    #[test]
    fn text_clears_aa_on_every_surface_in_both_themes() {
        for (theme, p) in [("dark", DARK), ("light", LIGHT)] {
            for (surface, bg) in surfaces(&p) {
                for (name, fg) in text(&p) {
                    let ratio = contrast(fg, bg);
                    assert!(ratio >= 4.5, "{theme}: {name} on {surface} is {ratio:.2}:1");
                }
            }
        }
    }

    #[test]
    fn text_still_clears_aa_under_the_strongest_grain_speck() {
        for (theme, p, dark) in [("dark", DARK, true), ("light", LIGHT, false)] {
            let strength = grain_strength(dark);
            assert!(
                (0. ..=0.1).contains(&strength),
                "{theme}: grain is film, not fog"
            );
            // The grain lies on the two large surfaces; a hovered or selected
            // row's fill is opaque and drawn over it, so it has none.
            for (surface, bg) in [("rail", p.rail), ("pane", p.pane)] {
                let grained = blend(bg, p.fg, strength);
                for (name, fg) in text(&p) {
                    let ratio = contrast(fg, grained);
                    assert!(
                        ratio >= 4.5,
                        "{theme}: {name} on grained {surface} is {ratio:.2}:1"
                    );
                }
                let ratio = contrast(p.fg_faint, grained);
                assert!(
                    ratio >= 3.,
                    "{theme}: marks on grained {surface} are {ratio:.2}:1"
                );
            }
        }
        assert_eq!(blend(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(blend(0x102030, 0xffffff, 0.), 0x102030);
    }

    #[test]
    fn a_focus_ring_shows_on_every_surface_and_on_the_accent() {
        for (theme, p) in [("dark", DARK), ("light", LIGHT)] {
            for (surface, bg) in surfaces(&p) {
                let ratio = contrast(p.accent, bg);
                assert!(ratio >= 3., "{theme}: ring on {surface} is {ratio:.2}:1");
            }
            // A primary button's ring is its ink, drawn on its own fill.
            let ratio = contrast(p.primary_ink, p.primary);
            assert!(ratio >= 3., "{theme}: ring on a primary is {ratio:.2}:1");
        }
    }

    #[test]
    fn an_accent_the_user_chose_reads_on_every_surface_and_moves_nothing_else() {
        for (name, dark, base) in [("dark", true, DARK), ("light", false, LIGHT)] {
            // Without a choice, the palette's own.
            assert_eq!(Theme::new(dark).accented(None).palette, base);
            assert_eq!(accent_in(dark, None), base.accent);
            let odd = [("black", 0x000000), ("white", 0xffffff), ("mud", 0x3a3a2a)];
            for (accent, hex) in ACCENTS.into_iter().chain(odd) {
                let theme = Theme::new(dark).accented(Some(hex));
                let p = theme.palette;
                for (surface, bg) in surfaces(&base) {
                    let ratio = contrast(p.accent, bg);
                    assert!(
                        ratio >= 4.5,
                        "{name}: {accent} on {surface} is {ratio:.2}:1"
                    );
                }
                let ratio = contrast(p.accent_ink, p.accent);
                assert!(ratio >= 4.5, "{name}: ink on {accent} is {ratio:.2}:1");
                assert_eq!(theme.accent, Hsla::from(rgb(p.accent)), "{name}: {accent}");
                // An agent at work wears the accent.
                assert_eq!(p.working, p.accent, "{name}: {accent}");
                // The other states, the grounds and the text stay the palette's.
                let rest = Palette {
                    accent: base.accent,
                    accent_ink: base.accent_ink,
                    working: base.working,
                    ..p
                };
                assert_eq!(rest, base, "{name}: {accent}");
            }
        }
        // On the dark ground an accent on offer is drawn as it is listed.
        for (accent, hex) in ACCENTS {
            assert_eq!(accent_in(true, Some(hex)), hex, "{accent}");
        }
        assert_eq!(ACCENTS[0].1, DARK.accent);
        // Without a choice the two are one colour as well.
        assert_eq!((DARK.working, LIGHT.working), (DARK.accent, LIGHT.accent));
    }

    #[test]
    fn the_rhythm_adds_up() {
        use metric::*;
        // A child's picture sits under its parent's label.
        assert_eq!(STEP, SLOT + GAP);
        // Rows, headings and controls are whole multiples of half the unit.
        for size in [ROW, HEADING, CONTROL, CONTROL_SMALL, BAR] {
            assert_eq!(size % (UNIT / 2.), 0., "{size}");
        }
        // A frame one unit inside the large one runs parallel to it.
        assert_eq!(RADIUS_LARGE - RADIUS_MEDIUM, UNIT);
    }

    #[test]
    fn marks_and_borders_that_carry_meaning_clear_three_to_one() {
        for (theme, p) in [("dark", DARK), ("light", LIGHT)] {
            for (surface, bg) in surfaces(&p) {
                let ratio = contrast(p.fg_faint, bg);
                assert!(
                    ratio >= 3.,
                    "{theme}: fg_faint on {surface} is {ratio:.2}:1"
                );
            }
        }
    }

    #[test]
    fn ink_is_legible_on_fills() {
        for (theme, p) in [("dark", DARK), ("light", LIGHT)] {
            for (name, fg, bg) in [
                ("accent ink", p.accent_ink, p.accent),
                ("text on diff add", p.fg, p.diff_add),
                ("text on diff del", p.fg, p.diff_del),
                // A tab's count of panes sits on a `line` badge.
                ("a count on its badge", p.fg_muted, p.line),
                ("inline code on its chip", p.chip_ink, p.chip),
                ("a primary's label", p.primary_ink, p.primary),
                // A prompt's bubble is the selected fill.
                ("a prompt on its bubble", p.fg, p.selected),
            ] {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 4.5, "{theme}: {name} is {ratio:.2}:1");
            }
        }
        // A session's letter and a harness's monogram are drawn in the ink on
        // every hue; a monogram's letters are text at 9 to 11px.
        for hue in HUES {
            let ratio = contrast(HUE_INK, hue);
            assert!(ratio >= 4.5, "letter on {hue:06x} is {ratio:.2}:1");
        }
    }

    #[test]
    fn terminal_colours_are_readable_on_the_pane() {
        for (theme, p, ansi) in [("dark", DARK, ANSI_DARK), ("light", LIGHT, ANSI_LIGHT)] {
            // Index 0 is the terminal's own black (or white): a background, not text.
            for (index, color) in ansi.0.iter().enumerate().skip(1) {
                let ratio = contrast(*color, p.pane);
                assert!(
                    ratio >= 3.,
                    "{theme}: ansi {index} on the pane is {ratio:.2}:1"
                );
            }
        }
        // Text in the default colour on a cell a program painted black.
        let ratio = contrast(DARK.fg, ANSI_DARK.0[0]);
        assert!(ratio >= 4.5, "text on black is {ratio:.2}:1");
    }

    #[test]
    fn providers_have_their_web_colours_and_others_a_quiet_one() {
        let theme = Theme::new(true);
        assert_eq!(theme.provider("Anthropic"), rgb(0xd9774f).into());
        assert_eq!(theme.provider("codex"), theme.provider("openai"));
        assert_eq!(theme.provider("xai"), theme.fg);
        assert_eq!(theme.provider("somebody"), theme.fg_faint);
        // A dot is seen beside the provider's name, which says it: it only
        // has to stand out of the menu it is drawn on. Mistral's orange on
        // the light theme's menus (`#f7f7f7`) is the least, at 2.19:1.
        for (theme, p) in [("dark", DARK), ("light", LIGHT)] {
            for (_, hue) in PROVIDERS {
                let ratio = contrast(hue, p.raised);
                assert!(ratio >= 2.15, "{theme}: {hue:06x} is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn a_name_keeps_its_hue() {
        assert_eq!(hue_index("herdr"), hue_index("herdr"));
        let spread: std::collections::HashSet<_> = [
            "herdr",
            "readme",
            "tasks",
            "review",
            "collector",
            "user-error",
            "Aliyoop",
        ]
        .iter()
        .map(|name| hue_index(name))
        .collect();
        assert!(
            spread.len() >= 4,
            "seven names landed on {} hues",
            spread.len()
        );
    }
}
