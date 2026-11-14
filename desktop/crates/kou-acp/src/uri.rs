//! `file://` URIs, the form in which the protocol names files inside content blocks.

use std::path::{Path, PathBuf};

/// The `file://` URI of an absolute path, percent-encoded.
pub(crate) fn from_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut uri = String::with_capacity(text.len() + 8);
    uri.push_str("file://");
    // A Windows path starts with a drive letter, which a URI puts after a third slash.
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'\\' if cfg!(windows) => uri.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(byte as char)
            }
            _ => {
                uri.push('%');
                uri.push(HEX[(byte >> 4) as usize] as char);
                uri.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    uri
}

/// The path a `file://` URI names, or `None` for any other kind of URI.
pub(crate) fn to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Only local files: an empty authority or `localhost`.
    let path = match rest.strip_prefix("localhost") {
        Some(path) if path.starts_with('/') => path,
        _ => rest,
    };
    if !path.starts_with('/') {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let decoded = percent_decode(path);
    // `/C:/Users/..` is how a URI spells a Windows drive path.
    let bytes = decoded.as_bytes();
    if cfg!(windows) && bytes.len() >= 3 && bytes[2] == b':' && bytes[1].is_ascii_alphabetic() {
        return Some(PathBuf::from(&decoded[1..]));
    }
    Some(PathBuf::from(decoded))
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let pair = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| Some((hex_value(hex[0])?, hex_value(hex[1])?)));
        match pair {
            Some((high, low)) => {
                out.push(high << 4 | low);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_path_survives_the_round_trip_with_spaces_and_unicode() {
        for path in ["/home/user/src/main.rs", "/tmp/a b/ф#1?.txt", "/x/100%/y"] {
            let uri = from_path(Path::new(path));
            assert!(uri.starts_with("file:///"), "{uri}");
            assert!(uri.is_ascii(), "{uri}");
            assert_eq!(to_path(&uri), Some(PathBuf::from(path)), "{uri}");
        }
        assert_eq!(
            from_path(Path::new("/tmp/a b/ф.txt")),
            "file:///tmp/a%20b/%D1%84.txt"
        );
    }

    #[cfg(unix)]
    #[test]
    fn only_local_file_uris_name_a_path() {
        assert_eq!(
            to_path("file://localhost/etc/hosts"),
            Some(PathBuf::from("/etc/hosts"))
        );
        assert_eq!(to_path("file:///a%2"), Some(PathBuf::from("/a%2")));
        assert_eq!(to_path("https://example.com/a"), None);
        assert_eq!(to_path("file://server/share/a"), None);
        assert_eq!(to_path("zed://buffer/1"), None);
    }
}
