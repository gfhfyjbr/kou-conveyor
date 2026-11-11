//! Go `strconv.Quote` (`%q`) rendering, used wherever Go diagnostics are part of a contract.
use crate::printable::PRINTABLE;
use std::fmt::Write;

fn printable(scalar: u32) -> bool {
    PRINTABLE
        .binary_search_by(|&(first, last)| {
            if last < scalar {
                std::cmp::Ordering::Less
            } else if first > scalar {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}
fn push(result: &mut String, ch: char) {
    match ch {
        '"' => result.push_str("\\\""),
        '\\' => result.push_str("\\\\"),
        _ if printable(ch as u32) => result.push(ch),
        '\x07' => result.push_str("\\a"),
        '\x08' => result.push_str("\\b"),
        '\x0c' => result.push_str("\\f"),
        '\n' => result.push_str("\\n"),
        '\r' => result.push_str("\\r"),
        '\t' => result.push_str("\\t"),
        '\x0b' => result.push_str("\\v"),
        '\0'..='\x1f' | '\x7f' => write!(result, "\\x{:02x}", ch as u32).unwrap(),
        _ if (ch as u32) < 0x10000 => write!(result, "\\u{:04x}", ch as u32).unwrap(),
        _ => write!(result, "\\U{:08x}", ch as u32).unwrap(),
    }
}
/// Quotes text exactly as Go's `strconv.Quote` does.
pub fn go_quote(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 2);
    result.push('"');
    text.chars().for_each(|ch| push(&mut result, ch));
    result.push('"');
    result
}
/// Like [`go_quote`] for a Go string that may hold invalid UTF-8: each bad byte becomes `\xNN`.
pub fn go_quote_bytes(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() + 2);
    result.push('"');
    for chunk in bytes.utf8_chunks() {
        chunk.valid().chars().for_each(|ch| push(&mut result, ch));
        for byte in chunk.invalid() {
            write!(result, "\\x{byte:02x}").unwrap();
        }
    }
    result.push('"');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_follows_go_escapes_and_printable_ranges() {
        // `~` stands for a backslash in the expected Go output.
        for (text, want) in [
            ("", "@@"),
            ("a@b~c'd", "@a~@b~~c'd@"),
            ("BEL", "@~a~b~f~n~r~t~v@"),
            ("CTL", "@~x00~x01~x1f~x7f@"),
            ("LATIN", "@~u0085~u00a0~u00ad@"),
            ("MARK", "@eMARK@"),
            ("FORMAT", "@~u200b~u2028~ue000@"),
            ("ASTRAL", "@~U000e0001~U0010ffff@"),
        ] {
            let text = match text {
                "BEL" => String::from_utf8(vec![7, 8, 12, 10, 13, 9, 11]).unwrap(),
                "CTL" => String::from_utf8(vec![0, 1, 0x1f, 0x7f]).unwrap(),
                "LATIN" => [0x85, 0xa0, 0xad].map(scalar).iter().collect(),
                "MARK" => [0x65, 0x301, 0x754c, 0x1f603, 0xfffd]
                    .map(scalar)
                    .iter()
                    .collect(),
                "FORMAT" => [0x200b, 0x2028, 0xe000].map(scalar).iter().collect(),
                "ASTRAL" => [0xe0001, 0x10ffff].map(scalar).iter().collect(),
                other => other.replace('@', "QUOTE").replace('~', "SLASH"),
            }
            .replace("QUOTE", &scalar(0x22).to_string())
            .replace("SLASH", &scalar(0x5c).to_string());
            let marks: String = [0x301, 0x754c, 0x1f603, 0xfffd]
                .map(scalar)
                .iter()
                .collect();
            let want = want
                .replace("MARK", &marks)
                .replace('@', &scalar(0x22).to_string())
                .replace('~', &scalar(0x5c).to_string());
            assert_eq!(go_quote(&text), want);
            assert_eq!(go_quote_bytes(text.as_bytes()), want);
        }
        let want = "@a~xff~xc0~x80b@"
            .replace('@', &scalar(0x22).to_string())
            .replace('~', &scalar(0x5c).to_string());
        assert_eq!(go_quote_bytes(&[0x61, 0xff, 0xc0, 0x80, 0x62]), want);
    }
    fn scalar(code: u32) -> char {
        char::from_u32(code).unwrap()
    }
}
