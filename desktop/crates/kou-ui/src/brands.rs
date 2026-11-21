//! The marks of the products the app runs: their own logos, so that a tab, a
//! row or a node is recognised at a glance.
//!
//! They come from Simple Icons, unchanged (see `brands/README.md` for the
//! version and the licence). A product without a mark there keeps the two
//! letters of its code; nothing is drawn to look like a logo.

use crate::scale::px;
use gpui::{Hsla, Styled as _, Svg, svg};

macro_rules! brands {
    ($($(#[$doc:meta])* $name:ident => $file:literal for [$($code:literal),+]),* $(,)?) => {
        /// A product with a mark of its own.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Brand { $($(#[$doc])* $name,)* }
        impl Brand {
            /// Every mark, for tests and galleries.
            pub const ALL: &'static [Brand] = &[$(Brand::$name,)*];
            /// The asset path GPUI loads it by.
            pub fn path(self) -> &'static str {
                match self { $(Brand::$name => concat!("brands/", $file, ".svg"),)* }
            }
            /// The mark of the harness with this code (`CC`, `cx`, ...), if it has one.
            pub fn of(code: &str) -> Option<Brand> {
                $(if [$($code),+].iter().any(|known| known.eq_ignore_ascii_case(code)) {
                    return Some(Brand::$name);
                })*
                None
            }
            pub(crate) fn bytes(path: &str) -> Option<&'static [u8]> {
                match path {
                    $(concat!("brands/", $file, ".svg") =>
                        Some(include_bytes!(concat!("../brands/", $file, ".svg")).as_slice()),)*
                    _ => None,
                }
            }
        }
    };
}

brands! {
    /// Claude Code.
    Claude => "claude" for ["CC"],
    /// Codex, by OpenAI.
    OpenAi => "openai" for ["CX"],
    /// Gemini CLI.
    Gemini => "googlegemini" for ["G", "GM"],
    /// OpenCode.
    OpenCode => "opencode" for ["OC"],
    /// Cursor's agent.
    Cursor => "cursor" for ["CU"],
    /// GitHub Copilot CLI.
    Copilot => "githubcopilot" for ["CP"],
    /// Qwen Code.
    Qwen => "qwen" for ["Q"],
}

impl Brand {
    /// The mark of a harness by the name the app knows it by (`claude`,
    /// `codex`, `gemini`, ...), or by its code; `None` for one without a mark.
    pub fn of_harness(name: &str) -> Option<Brand> {
        let name = name.trim();
        const NAMES: &[(&str, Brand)] = &[
            ("claude", Brand::Claude),
            ("claude-code", Brand::Claude),
            ("codex", Brand::OpenAi),
            ("openai", Brand::OpenAi),
            ("gemini", Brand::Gemini),
            ("opencode", Brand::OpenCode),
            ("cursor", Brand::Cursor),
            ("copilot", Brand::Copilot),
            ("qwen", Brand::Qwen),
        ];
        NAMES
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, mark)| *mark)
            .or_else(|| Brand::of(name))
    }
}

/// A mark at `side` pixels (through [`crate::scale::px`]), in one colour (an
/// SVG takes none from its parent).
pub fn brand(mark: Brand, side: f32, color: impl Into<Hsla>) -> Svg {
    svg()
        .path(mark.path())
        .size(px(side))
        .flex_none()
        .text_color(color.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mark_is_embedded_and_found_by_its_harness_code() {
        for mark in Brand::ALL {
            let bytes = Brand::bytes(mark.path()).expect("embedded");
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(
                text.starts_with("<svg") && text.contains("viewBox=\"0 0 24 24\""),
                "{mark:?}"
            );
        }
        assert_eq!(Brand::of("CC"), Some(Brand::Claude));
        assert_eq!(Brand::of("cx"), Some(Brand::OpenAi));
        assert_eq!(Brand::of("gm"), Some(Brand::Gemini));
        assert_eq!(Brand::of_harness("claude"), Some(Brand::Claude));
        assert_eq!(Brand::of_harness("Codex"), Some(Brand::OpenAi));
        assert_eq!(Brand::of_harness("CC"), Some(Brand::Claude));
        assert_eq!(Brand::of_harness("amp"), None);
        // No mark is borrowed for a product that has none: Amp is not AMP.
        for code in ["A", "D", "pi", "ai", "GO", "KI", "GK", "DV", "ko", "sh", ""] {
            assert_eq!(Brand::of(code), None, "{code}");
        }
    }
}
