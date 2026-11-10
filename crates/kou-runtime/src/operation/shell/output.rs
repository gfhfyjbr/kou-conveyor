use super::{Error, Result};

fn first_len(bytes: &[u8]) -> usize {
    let Some(&first) = bytes.first() else {
        return 0;
    };
    let width = match first {
        0..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return 1,
    };
    if bytes.len() >= width && std::str::from_utf8(&bytes[..width]).is_ok() {
        width
    } else {
        1
    }
}
fn last_len(bytes: &[u8]) -> usize {
    if bytes.last().is_some_and(u8::is_ascii) {
        return 1;
    }
    for width in 2..=4.min(bytes.len()) {
        let tail = &bytes[bytes.len() - width..];
        if first_len(tail) == width {
            return width;
        }
    }
    usize::from(!bytes.is_empty())
}
fn sanitize(mut bytes: &[u8]) -> String {
    let mut output = String::new();
    while !bytes.is_empty() {
        let count = first_len(bytes);
        match std::str::from_utf8(&bytes[..count]) {
            Ok(text) => output.push_str(text),
            Err(_) => output.push('\u{fffd}'),
        }
        bytes = &bytes[count..];
    }
    output
}
pub(super) fn bound(
    head: &[u8],
    tail: &[u8],
    full_size: i64,
    limit: usize,
    path: &str,
) -> Result<(String, bool)> {
    let tail = if tail.is_empty() {
        if full_size != head.len() as i64 {
            return Err(Error(
                "bound output: empty tail requires the complete output in head".into(),
            ));
        }
        let mut rest = head;
        let mut count = 0;
        while !rest.is_empty() && count <= limit {
            rest = &rest[first_len(rest)..];
            count += 1;
        }
        if count <= limit {
            return Ok((sanitize(head), false));
        }
        head
    } else {
        tail
    };
    let mut end = 0;
    for _ in 0..limit / 2 {
        if end == head.len() {
            break;
        }
        end += first_len(&head[end..]);
    }
    let mut start = tail.len();
    for _ in 0..limit - limit / 2 {
        if start == 0 {
            break;
        }
        start -= last_len(&tail[..start]);
    }
    let skipped = full_size - end as i64 - (tail.len() - start) as i64;
    let marker = if path.is_empty() {
        String::new()
    } else {
        format!("; complete output in {path}")
    };
    Ok((
        format!(
            "{}...{skipped} bytes truncated{marker}...{}",
            sanitize(&head[..end]),
            sanitize(&tail[start..])
        ),
        true,
    ))
}
