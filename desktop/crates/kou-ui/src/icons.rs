//! The icon set: Lucide's (ISC, see `icons/LICENSE-lucide`), on its 24px grid
//! with round caps and joins, drawn at 16px so that its 2px line comes out at
//! the 1.3px hairline of the rest of the app. `icons/README.md` says which of
//! Lucide's each one is; the few that Lucide has no drawing for (`stop`,
//! `cursor`, `zoom-actual`, `appearance`, `folder-fill`) are drawn here in the
//! same line.

use crate::scale::px;
use gpui::{AssetSource, SharedString, Svg, svg};
use std::borrow::Cow;

macro_rules! icons {
    ($($(#[$doc:meta])* $name:ident => $file:literal),* $(,)?) => {
        /// One of the app's icons.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon { $($(#[$doc])* $name,)* }
        impl Icon {
            /// Every icon, for tests and galleries.
            pub const ALL: &'static [Icon] = &[$(Icon::$name,)*];
            /// The asset path GPUI loads it by.
            pub fn path(self) -> &'static str {
                match self { $(Icon::$name => concat!("icons/", $file, ".svg"),)* }
            }
            fn bytes(path: &str) -> Option<&'static [u8]> {
                match path {
                    $(concat!("icons/", $file, ".svg") =>
                        Some(include_bytes!(concat!("../icons/", $file, ".svg")).as_slice()),)*
                    _ => None,
                }
            }
        }
    };
}

icons! {
    /// Create something here.
    Plus => "plus",
    /// Zoom out, or take one away.
    Minus => "minus",
    /// Close a tab or dismiss.
    Close => "close",
    /// An open group.
    ChevronDown => "chevron-down",
    /// A closed group, or "go into".
    ChevronRight => "chevron-right",
    /// A space: a folder of sessions.
    Folder => "folder",
    /// Add a space.
    FolderPlus => "folder-plus",
    /// Archived sessions.
    Archive => "archive",
    /// Recent activity.
    Clock => "clock",
    /// Settings: things you adjust.
    Sliders => "sliders",
    /// Show or hide the rail.
    Sidebar => "sidebar",
    /// Mark everything seen.
    CheckCircle => "check-circle",
    /// The canvas view: boxes joined by edges.
    Canvas => "canvas",
    /// A terminal pane.
    Terminal => "terminal",
    /// One pane filling the window.
    Pane => "pane",
    /// Divide a pane: a new one to its right.
    Split => "split",
    /// Divide a pane: a new one below it.
    SplitDown => "split-down",
    /// A native agent thread.
    Thread => "thread",
    /// A note node.
    Note => "note",
    /// Search.
    Search => "search",
    /// Stop a running agent.
    Stop => "stop",
    /// Send a prompt.
    ArrowUp => "arrow-up",
    /// More actions for this row.
    More => "more",
    /// Sound cues are on.
    Sound => "sound",
    /// Sound cues are off.
    SoundOff => "sound-off",
    /// Fit the canvas to the window.
    Fit => "fit",
    /// A git branch.
    Branch => "branch",
    /// Something agents read from: a canvas source node.
    Source => "source",
    /// An edge between two nodes of the canvas.
    Wire => "wire",
    /// Make a copy.
    Duplicate => "duplicate",
    /// Delete.
    Trash => "trash",
    /// Rename or edit text.
    Pencil => "pencil",
    /// Bring one thing into focus.
    Target => "target",
    /// Inspect: what it is and its settings.
    Info => "info",
    /// Take the last change back.
    Undo => "undo",
    /// Force a message into the running turn.
    Bolt => "bolt",
    /// Queue a message for when the agent finishes.
    Queue => "queue",
    /// A picture.
    Image => "image",
    /// A file.
    File => "file",
    /// Copy to the clipboard.
    Copy => "copy",
    /// Attach files to a prompt.
    Paperclip => "paperclip",
    /// Hold what waits.
    Pause => "pause",
    /// Let what waits go on.
    Play => "play",
    /// Done: copied, saved.
    Check => "check",
    /// A handle to drag a row by.
    Grip => "grip",
    /// Zoom to 100%: one to one.
    ZoomActual => "zoom-actual",
    /// A space in the rail: the folder filled in its hue, so a space reads
    /// as heavier than the sessions under it.
    FolderFill => "folder-fill",
    /// The terminal's cursor: a prompt and a block.
    Cursor => "cursor",
    /// Light or dark: a disc half filled.
    Appearance => "appearance",
    /// Keys and shortcuts.
    Keyboard => "keyboard",
    /// Code: angle brackets.
    Code => "code",
    /// A bug: something being debugged.
    Bug => "bug",
    /// A flask: tests and experiments.
    Flask => "flask",
    /// An open book: docs.
    Book => "book",
    /// A database: stacked discs.
    Database => "database",
    /// A globe: the web.
    Globe => "globe",
    /// An eye: review.
    Eye => "eye",
    /// A star: the one that matters.
    Star => "star",
    /// A rocket: a release.
    Rocket => "rocket",
    /// A wrench: tooling.
    Wrench => "wrench",
    /// A box: a package or a build.
    Package => "package",
    /// A server: two racks.
    Server => "server",
    /// A flame: the hot one.
    Flame => "flame",
    /// A project: the window a folder is worked on in.
    Project => "project",
    /// Lines changed: a page with a plus and a minus.
    Diff => "diff",
    /// Who started whom: two agents joined to the one that started them.
    Lineage => "lineage",
    /// Agents: a head with an antenna.
    Agents => "agents",
}

/// The icons a tab can be given, each with the name it is saved under. The
/// names are part of the app's saved state: add, never rename.
pub const TAB_ICONS: &[(&str, Icon)] = &[
    ("terminal", Icon::Terminal),
    ("code", Icon::Code),
    ("bug", Icon::Bug),
    ("flask", Icon::Flask),
    ("eye", Icon::Eye),
    ("book", Icon::Book),
    ("branch", Icon::Branch),
    ("package", Icon::Package),
    ("database", Icon::Database),
    ("server", Icon::Server),
    ("globe", Icon::Globe),
    ("wrench", Icon::Wrench),
    ("bolt", Icon::Bolt),
    ("rocket", Icon::Rocket),
    ("flame", Icon::Flame),
    ("star", Icon::Star),
];

/// The icon saved under a name, if there is one.
pub fn tab_icon(name: &str) -> Option<Icon> {
    TAB_ICONS
        .iter()
        .find(|(saved, _)| *saved == name)
        .map(|(_, icon)| *icon)
}

macro_rules! pictures {
    ($($(#[$doc:meta])* $name:ident => $file:literal),* $(,)?) => {
        /// A drawing for a screen that has nothing on it yet: the same hairline,
        /// round caps and joins as the icons, on a 48px grid, each of the
        /// thing the screen is about rather than a mood.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Picture { $($(#[$doc])* $name,)* }
        impl Picture {
            /// Every picture, for tests and galleries.
            pub const ALL: &'static [Picture] = &[$(Picture::$name,)*];
            /// The asset path GPUI loads it by.
            pub fn path(self) -> &'static str {
                match self { $(Picture::$name => concat!("pictures/", $file, ".svg"),)* }
            }
            fn bytes(path: &str) -> Option<&'static [u8]> {
                match path {
                    $(concat!("pictures/", $file, ".svg") =>
                        Some(include_bytes!(concat!("../pictures/", $file, ".svg")).as_slice()),)*
                    _ => None,
                }
            }
        }
    };
}

pictures! {
    /// No space yet: folders, and a plus on the front one.
    Spaces => "spaces",
    /// No session open: the window with its rail, and an arrow toward it.
    Rail => "rail",
    /// A pane whose process is gone: a terminal whose cursor is hollow.
    Stopped => "stopped",
    /// No earlier conversation: one empty speech bubble.
    Conversations => "conversations",
    /// Nothing found on this machine: a lens over a prompt.
    Search => "search",
}

/// The size a [`Picture`] is drawn at.
pub const PICTURE: f32 = 48.;

/// The app's embedded assets. Pass to `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::bytes(path)
            .or_else(|| Picture::bytes(path))
            .or_else(|| crate::Brand::bytes(path))
            .map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Icon::ALL
            .iter()
            .map(|icon| icon.path())
            .chain(Picture::ALL.iter().map(|picture| picture.path()))
            .chain(crate::Brand::ALL.iter().map(|brand| brand.path()))
            .filter(|icon| icon.starts_with(path))
            .map(SharedString::new_static)
            .collect())
    }
}

/// A [`Picture`] at its size, in one colour (an SVG takes no colour from its
/// parent). Its size follows [`crate::scale::px`].
pub fn picture(picture: Picture, color: impl Into<gpui::Hsla>) -> Svg {
    use gpui::Styled as _;
    svg()
        .path(picture.path())
        .size(px(PICTURE))
        .flex_none()
        .text_color(color.into())
}

/// An icon at the standard size. GPUI paints an SVG in the colour set on the
/// element itself, never an inherited one, so the colour is an argument. Its
/// size follows [`crate::scale::px`].
pub fn icon(icon: Icon, color: impl Into<gpui::Hsla>) -> Svg {
    use gpui::Styled as _;
    svg()
        .path(icon.path())
        .size(px(crate::metric::ICON))
        .flex_none()
        .text_color(color.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_embedded_and_is_an_svg() {
        for icon in Icon::ALL {
            let Some(bytes) = Assets.load(icon.path()).unwrap() else {
                panic!("{} is not embedded", icon.path());
            };
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.starts_with("<svg "), "{}", icon.path());
            assert!(text.contains("viewBox=\"0 0 24 24\""), "{}", icon.path());
            // One line for all of them: round caps and joins.
            assert!(text.contains("stroke-width=\"2\""), "{}", icon.path());
            assert!(
                text.contains("stroke-linecap=\"round\"")
                    && text.contains("stroke-linejoin=\"round\""),
                "{}",
                icon.path()
            );
        }
        assert_eq!(Assets.list("icons/").unwrap().len(), Icon::ALL.len());
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
    }

    #[test]
    fn every_picture_is_embedded_on_its_own_grid() {
        for picture in Picture::ALL {
            let Some(bytes) = Assets.load(picture.path()).unwrap() else {
                panic!("{} is not embedded", picture.path());
            };
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.starts_with("<svg "), "{}", picture.path());
            assert!(text.contains("viewBox=\"0 0 48 48\""), "{}", picture.path());
            // The icons' line: one hairline weight, round caps and joins.
            assert!(text.contains("stroke-width=\"1.3\""), "{}", picture.path());
            assert!(
                text.contains("stroke-linecap=\"round\""),
                "{}",
                picture.path()
            );
        }
        assert_eq!(Assets.list("pictures/").unwrap().len(), Picture::ALL.len());
    }
}
