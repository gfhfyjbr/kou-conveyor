//! Destinations in agent-authored Markdown, resolved without launching a URI scheme.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    Web(String),
    File(PathBuf),
    Theme(String),
}

pub(crate) fn destination(href: &str, root: Option<&Path>) -> Option<Destination> {
    let href = href.trim();
    if href.is_empty() || href.chars().any(char::is_control) {
        return None;
    }
    let lower = href.to_ascii_lowercase();
    if ["http://", "https://", "mailto:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return Some(Destination::Web(href.into()));
    }
    if href.starts_with("kou-theme://") {
        return Some(Destination::Theme(href.into()));
    }
    if lower.starts_with("file:") {
        let mut url = url::Url::parse(href).ok()?;
        if url
            .host_str()
            .is_some_and(|host| !host.is_empty() && host != "localhost")
        {
            return None;
        }
        let path = without_position(url.path()).to_owned();
        url.set_path(&path);
        let path = url.to_file_path().ok()?;
        if path.to_string_lossy().chars().any(char::is_control) {
            return None;
        }
        return Some(Destination::File(path));
    }
    if href.starts_with('#') || href.starts_with('?') || href.starts_with("//") {
        return None;
    }
    // Position suffixes are editor hints, not part of the filename. Decode
    // after splitting, so an encoded '#' or ':12' can still name a real file.
    let path = href.split_once('#').map_or(href, |(path, _)| path);
    let path = without_position(path);
    let path = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    if path.is_empty() || path.chars().any(char::is_control) {
        return None;
    }
    let file = Path::new(path.as_ref());
    // Unknown protocols stay inert; relative files are resolved against the
    // conversation's project, never the desktop process's working directory.
    if !file.is_absolute() && path.split(['/', '\\']).next()?.contains(':') {
        return None;
    }
    if file.is_absolute() {
        Some(Destination::File(file.into()))
    } else {
        root.map(|root| Destination::File(root.join(file)))
    }
}

fn without_position(path: &str) -> &str {
    let mut path = path;
    for _ in 0..2 {
        if let Some((file, position)) = path.rsplit_once(':')
            && !position.is_empty()
            && position.bytes().all(|b| b.is_ascii_digit())
            && position.parse::<u32>().is_ok_and(|n| n > 0)
        {
            path = file;
        } else {
            break;
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_markdown_targets_resolve_paths_and_source_positions() {
        let root = Path::new("/work/project");
        for (href, file) in [
            (
                "/Users/me/Project/docs/js-ui.md",
                "/Users/me/Project/docs/js-ui.md",
            ),
            (
                "/Users/me/My Project/file.rs:12:3",
                "/Users/me/My Project/file.rs",
            ),
            ("src/main.rs:42", "/work/project/src/main.rs"),
            (
                "./docs/My%20Notes.md#L12-L20",
                "/work/project/./docs/My Notes.md",
            ),
            ("docs/тема.md#colours", "/work/project/docs/тема.md"),
            (
                "docs/name%23part%3A12.md",
                "/work/project/docs/name#part:12.md",
            ),
            ("README.md", "/work/project/README.md"),
        ] {
            assert_eq!(
                destination(href, Some(root)),
                Some(Destination::File(file.into())),
                "{href}"
            );
        }
        assert!(destination("README.md", None).is_none());
    }

    #[test]
    fn file_urls_are_decoded_as_local_paths() {
        let file = std::env::temp_dir().join("kou docs/русский.md");
        let mut url = url::Url::from_file_path(&file).unwrap();
        url.set_fragment(Some("L42"));
        assert_eq!(
            destination(url.as_str(), None),
            Some(Destination::File(file))
        );
    }

    #[test]
    fn web_and_theme_links_keep_their_own_routes() {
        assert_eq!(
            destination("https://example.com/docs:12#L1", None),
            Some(Destination::Web("https://example.com/docs:12#L1".into()))
        );
        assert_eq!(
            destination("HTTP://example.com", None),
            Some(Destination::Web("HTTP://example.com".into()))
        );
        assert_eq!(
            destination("mailto:me@example.com", None),
            Some(Destination::Web("mailto:me@example.com".into()))
        );
        let link = kou_ui::themes::Preset::Charcoal.document().link().unwrap();
        assert_eq!(destination(&link, None), Some(Destination::Theme(link)));
    }

    #[test]
    fn unsupported_protocols_and_nonlocal_file_urls_are_not_file_links() {
        for href in [
            "javascript:alert(1)",
            "vscode://open?file=x",
            "data:text/html,x",
            "command:run",
            "//example.com/a",
            "file://server/private.txt",
            "#heading",
            "?query",
            "\0",
            "docs/%00.txt",
            "file:///tmp/%00.txt",
        ] {
            assert!(
                destination(href, Some(Path::new("/work/project"))).is_none(),
                "{href}"
            );
        }
    }
}
