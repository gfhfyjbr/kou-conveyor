//! Portable, versioned colour themes. A share link contains colours only.
use crate::{DARK, LIGHT, Palette, Theme};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

pub const LINK_PREFIX: &str = "kou-theme://v1/";
pub const MAX_LINK: usize = 8192;
pub const MAX_THEMES: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    #[default]
    Charcoal,
    AmoledBlack,
    White,
}

impl Preset {
    pub const ALL: [Self; 3] = [Self::Charcoal, Self::AmoledBlack, Self::White];
    pub fn name(self) -> &'static str {
        match self {
            Self::Charcoal => "Charcoal",
            Self::AmoledBlack => "AMOLED Black",
            Self::White => "White",
        }
    }
    pub fn palette(self) -> Palette {
        match self {
            Self::AmoledBlack => DARK,
            Self::White => LIGHT,
            Self::Charcoal => Palette {
                rail: 0x141412,
                pane: 0x1a1a18,
                raised: 0x1e1e1b,
                code: 0x161614,
                hover: 0x232320,
                selected: 0x292926,
                line: 0x252522,
                line_strong: 0x32322d,
                fg: 0xe9e8e3,
                fg_dim: 0xb9b9b1,
                fg_muted: 0x96968f,
                fg_faint: 0x73736c,
                accent: 0xff7c30,
                accent_ink: 0x151513,
                working: 0xff7c30,
                done: 0x91a8cf,
                blocked: 0xdfb96b,
                error: 0xff5869,
                ok: 0x6cc98f,
                diff_add: 0x1b2b21,
                diff_del: 0x352021,
                primary: 0xeeede7,
                primary_rim: 0xe3e2da,
                primary_ink: 0x171715,
                chip: 0x222220,
                chip_ink: 0xc4c4bc,
            },
        }
    }
    pub fn document(self) -> ThemeDocument {
        ThemeDocument {
            version: 1,
            name: self.name().into(),
            base: self,
            colors: self.palette(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeDocument {
    pub version: u32,
    pub name: String,
    pub base: Preset,
    pub colors: Palette,
}

impl ThemeDocument {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("This theme needs a newer version of kou.".into());
        }
        if self.name.trim().is_empty()
            || self.name.chars().count() > 64
            || self.name.chars().any(char::is_control)
        {
            return Err("Use a theme name of 1–64 characters, without line breaks.".into());
        }
        if Palette::TOKENS
            .iter()
            .any(|key| self.colors.get(key).unwrap() > 0xffffff)
        {
            return Err("A colour is outside the #RRGGBB range.".into());
        }
        Ok(())
    }
    pub fn theme(&self) -> Theme {
        Theme::from_palette(self.base != Preset::White, self.colors)
    }
    pub fn link(&self) -> Result<String, String> {
        self.validate()?;
        let json =
            serde_json::to_vec(self).map_err(|_| "Could not encode this theme.".to_owned())?;
        Ok(format!("{LINK_PREFIX}{}", URL_SAFE_NO_PAD.encode(json)))
    }
    pub fn from_link(link: &str) -> Result<Self, String> {
        let link = link.trim();
        if link.len() > MAX_LINK {
            return Err("This theme link is too long.".into());
        }
        let payload = link
            .strip_prefix(LINK_PREFIX)
            .ok_or("Paste a kou-theme://v1/ theme link.")?;
        let json = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| "This theme link is damaged. Copy the complete link.".to_owned())?;
        let mut theme: Self = serde_json::from_slice(&json)
            .map_err(|_| "This link does not contain a valid kou theme.".to_owned())?;
        theme.name = theme.name.trim().to_owned();
        theme.validate()?;
        Ok(theme)
    }
}

/// None in the store means the old, system-following appearance. Once a
/// preset is selected, OS appearance changes no longer replace it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum Selection {
    #[default]
    System,
    Preset(Preset),
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTheme {
    pub id: String,
    pub theme: ThemeDocument,
}

pub fn parse_hex(text: &str) -> Option<u32> {
    let hex = text.trim().strip_prefix('#').unwrap_or(text.trim());
    (hex.len() == 6 && hex.bytes().all(|c| c.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_round_trip_every_color_and_unicode_name() {
        for base in Preset::ALL {
            let mut theme = base.document();
            theme.name = "Моя тема / 夜 🌑".into();
            theme.colors.raised = 0x123456;
            assert_eq!(
                ThemeDocument::from_link(&theme.link().unwrap()).unwrap(),
                theme
            );
            assert!(theme.link().unwrap().len() < 2048);
        }
    }
    #[test]
    fn rejects_untrusted_or_incomplete_themes() {
        for link in [
            "https://example.com/theme",
            "kou-theme://v2/abc",
            "kou-theme://v1/!",
            "kou-theme://v1/e30",
        ] {
            assert!(ThemeDocument::from_link(link).is_err());
        }
        assert!(ThemeDocument::from_link(&"a".repeat(MAX_LINK + 1)).is_err());
        let mut doc = Preset::Charcoal.document();
        doc.colors.pane = 0xff000000;
        assert!(doc.link().is_err());
        doc.colors.pane = 0;
        doc.version = 2;
        assert!(doc.link().is_err());
        doc.version = 1;
        doc.name = "\n".into();
        assert!(doc.link().is_err());
        let mut json = serde_json::to_value(Preset::White.document()).unwrap();
        json["script"] = "run me".into();
        let link = format!(
            "{LINK_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json).unwrap())
        );
        assert!(ThemeDocument::from_link(&link).is_err());
    }
    #[test]
    fn presets_have_readable_text() {
        for preset in Preset::ALL {
            let p = preset.palette();
            for ground in [p.rail, p.pane, p.raised, p.code, p.hover, p.selected] {
                for ink in [p.fg, p.fg_dim, p.fg_muted, p.accent] {
                    assert!(
                        crate::contrast(ground, ink) >= 4.5,
                        "{preset:?}: {ink:06x} on {ground:06x}"
                    );
                }
            }
            assert!(crate::contrast(p.primary, p.primary_ink) >= 4.5);
        }
    }
    #[test]
    fn hex_and_token_validation() {
        assert_eq!(parse_hex(" #a0B123 "), Some(0xa0b123));
        for bad in ["red", "#123", "#fffffff", "ffffff00", "éffff"] {
            assert_eq!(parse_hex(bad), None);
        }
        let mut p = DARK;
        assert!(!p.set("missing", 0));
        assert!(!p.set("pane", 0x1000000));
        assert_eq!(p, DARK);
    }
}
