use crate::lock::with_paths;
use memchr::{memchr, memmem};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::{self, DirBuilder, File, Metadata, Permissions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
};

pub const DEFAULT_READ_LIMIT: usize = 2_000;
pub const MAX_READ_LIMIT: usize = 20_000;
pub const DEFAULT_MAX_LINE_BYTES: usize = 2_000;
pub const DEFAULT_MAX_READ_BYTES: usize = 40_000;
pub const MAX_EDIT_BYTES: usize = 32 << 20;
// The input cap alone does not bound replace_all expansion. Reject before
// allocating or changing the file; callers can explicitly use Write instead.
pub const MAX_EDIT_RESULT_BYTES: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileError(pub String);
impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for FileError {}
pub type Result<T> = std::result::Result<T, FileError>;

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ReadOptions {
    pub offset: i64,
    pub limit: i64,
    pub max_line_bytes: i64,
    pub max_bytes: i64,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ReadResult {
    pub text: String,
    pub lines: usize,
    pub from: usize,
    pub to: usize,
    pub size: u64,
    pub truncated: bool,
}

pub fn read(name: impl AsRef<Path>, options: ReadOptions) -> Result<ReadResult> {
    read_interruptible(name, options, || false)
}

pub fn read_interruptible(
    name: impl AsRef<Path>,
    options: ReadOptions,
    canceled: impl Fn() -> bool,
) -> Result<ReadResult> {
    if canceled() {
        return fail("context canceled");
    }
    let name = name.as_ref();
    let info = metadata(name)?;
    if info.is_dir() {
        return fail(format!(
            "{} is a directory; list it with Bash (ls -la {})",
            name.display(),
            shell_quote(&name.to_string_lossy())
        ));
    }
    if options.offset < 0 {
        return fail("offset must not be negative");
    }
    if options.limit < 0 {
        return fail("limit must not be negative");
    }
    let limit = positive(options.limit, DEFAULT_READ_LIMIT).min(MAX_READ_LIMIT);
    let max_line = positive(options.max_line_bytes, DEFAULT_MAX_LINE_BYTES);
    let max_bytes = positive(options.max_bytes, DEFAULT_MAX_READ_BYTES);
    let mut file = File::open(name).map_err(|err| describe(err, name))?;
    let mut head = [0; 8192];
    let n = read_head(&mut file, &mut head).map_err(|err| describe(err, name))?;
    if head[..n].contains(&0) {
        return fail(format!(
            "{} is a binary file ({}){}",
            name.display(),
            format_bytes(info.len()),
            if is_image_path(name) {
                "; ViewImage shows images"
            } else {
                ""
            }
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|err| describe(err, name))?;
    let mut result = ReadResult {
        size: info.len(),
        ..Default::default()
    };
    if info.len() == 0 {
        result.text = "(empty file)".into();
        return Ok(result);
    }
    let from = options.offset.max(1) as usize;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut prefix = Vec::new();
    let mut text = Vec::new();
    let mut shown = 0;
    loop {
        let show = result.lines + 1 >= from && shown < limit && text.len() < max_bytes;
        let cap = if show { max_line.saturating_add(1) } else { 0 };
        let Some(length) = read_line(&mut reader, &mut prefix, cap, &canceled)
            .map_err(|err| describe(err, name))?
        else {
            break;
        };
        result.lines += 1;
        if !show {
            continue;
        }
        write!(text, "{:6}\t", result.lines).expect("Vec writes cannot fail");
        if length > max_line {
            let mut cut = max_line;
            while cut > 0 && prefix[cut] & 0xc0 == 0x80 {
                cut -= 1;
            }
            text.extend_from_slice(&prefix[..cut]);
            write!(text, " … [line cut: {} more bytes]", length - cut)
                .expect("Vec writes cannot fail");
        } else {
            text.extend_from_slice(&prefix);
        }
        text.push(b'\n');
        shown += 1;
        if result.from == 0 {
            result.from = result.lines;
        }
        result.to = result.lines;
    }
    if result.lines == 0 {
        result.text = "(empty file)".into();
        return Ok(result);
    }
    if result.from == 0 {
        return fail(format!(
            "offset {from} is past the end of {}, which has {}",
            name.display(),
            count(result.lines as u64, "line", "lines")
        ));
    }
    result.truncated = result.to < result.lines;
    text.pop();
    if result.truncated && shown < limit {
        write!(text, "\n[Lines {}-{} of {} shown, about {max_bytes} bytes, the most one read shows; read on with offset={}, or find what you need with rg -n.]", result.from, result.to, result.lines, result.to + 1).expect("Vec writes cannot fail");
    } else if result.truncated {
        write!(
            text,
            "\n[Lines {}-{} of {} shown; read on with offset={}.]",
            result.from,
            result.to,
            result.lines,
            result.to + 1
        )
        .expect("Vec writes cannot fail");
    } else if result.from > 1 {
        write!(
            text,
            "\n[Lines {}-{} of {} shown.]",
            result.from, result.to, result.lines
        )
        .expect("Vec writes cannot fail");
    }
    result.text = display_bytes(&text);
    Ok(result)
}

// Retain only the requested prefix even for a multi-gigabyte physical line.
// The final byte is tracked separately to strip CR without retaining the tail.
fn read_line(
    reader: &mut impl BufRead,
    prefix: &mut Vec<u8>,
    cap: usize,
    canceled: &impl Fn() -> bool,
) -> io::Result<Option<usize>> {
    prefix.clear();
    let mut length = 0usize;
    let mut last = None;
    loop {
        if canceled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "context canceled",
            ));
        }
        let part = reader.fill_buf()?;
        if part.is_empty() {
            if last.is_none() {
                return Ok(None);
            }
            break;
        }
        let newline = memchr(b'\n', part);
        let used = newline.unwrap_or(part.len());
        if used > 0 {
            let saved = used.min(cap.saturating_sub(prefix.len()));
            prefix.extend_from_slice(&part[..saved]);
            length = length
                .checked_add(used)
                .ok_or_else(|| io::Error::other("line is too long"))?;
            last = Some(part[used - 1]);
        }
        reader.consume(used + usize::from(newline.is_some()));
        if newline.is_some() {
            break;
        }
    }
    if last == Some(b'\r') {
        if prefix.len() == length {
            prefix.pop();
        }
        length -= 1;
    }
    Ok(Some(length))
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct EditOptions {
    pub replace_all: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct EditResult {
    pub replacements: usize,
    pub line: usize,
    pub snippet: String,
    pub lines: usize,
}

pub fn edit(
    name: impl AsRef<Path>,
    old: &str,
    replacement: &str,
    options: EditOptions,
) -> Result<EditResult> {
    let name = name.as_ref();
    if old.is_empty() {
        return fail("old_string must not be empty; Write creates or replaces a file whole");
    }
    if old == replacement {
        return fail("old_string and new_string are the same; nothing to change");
    }
    with_paths(&[name], || {
        edit_locked(name, old.as_bytes(), replacement.as_bytes(), options)
    })
}

fn edit_locked(
    name: &Path,
    old: &[u8],
    replacement: &[u8],
    options: EditOptions,
) -> Result<EditResult> {
    let info = metadata(name)?;
    if info.is_dir() {
        return fail(format!("{} is a directory", name.display()));
    }
    if info.len() > MAX_EDIT_BYTES as u64 {
        return too_large(name, info.len());
    }
    // A concurrent external writer may grow the file after stat.
    let content = read_bounded(name, MAX_EDIT_BYTES)?;
    if content[..content.len().min(8192)].contains(&0) {
        return fail(format!("{} is a binary file", name.display()));
    }
    let converted;
    let (old, replacement) = if memmem::find(&content, old).is_none()
        && memmem::find(&content, b"\r\n").is_some()
        && !old.contains(&b'\r')
    {
        converted = (crlf(old), crlf(replacement));
        (converted.0.as_slice(), converted.1.as_slice())
    } else {
        (old, replacement)
    };
    let mut positions = memmem::find_iter(&content, old);
    let Some(first) = positions.next() else {
        return Err(not_found(name, &content, old));
    };
    let occurrences = 1 + positions.count();
    if occurrences > 1 && !options.replace_all {
        return fail(format!(
            "old_string appears {occurrences} times in {}; include more of the surrounding lines so it appears once, or set replace_all to change every occurrence",
            name.display()
        ));
    }
    let replacements = if options.replace_all { occurrences } else { 1 };
    let new_len = replacement
        .len()
        .checked_mul(replacements)
        .and_then(|added| (content.len() - old.len() * replacements).checked_add(added));
    let Some(new_len) = new_len.filter(|len| *len <= MAX_EDIT_RESULT_BYTES) else {
        return fail(format!(
            "editing {} would exceed the {} result limit; use Write or Bash",
            name.display(),
            format_bytes(MAX_EDIT_RESULT_BYTES as u64)
        ));
    };
    let mut updated = Vec::with_capacity(new_len);
    let mut after = 0;
    for position in memmem::find_iter(&content, old).take(replacements) {
        updated.extend_from_slice(&content[after..position]);
        updated.extend_from_slice(replacement);
        after = position + old.len();
    }
    updated.extend_from_slice(&content[after..]);
    replace_file(name, &updated, info.permissions().mode() & 0o777)?;
    let line = 1 + memchr::memchr_iter(b'\n', &content[..first]).count();
    let changed = 1 + memchr::memchr_iter(b'\n', replacement).count();
    Ok(EditResult {
        replacements,
        line,
        snippet: snippet(&updated, line, changed),
        lines: line_count(&updated),
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct WriteResult {
    pub bytes: usize,
    pub lines: usize,
    pub created: bool,
    pub previous_lines: usize,
}

pub fn write(name: impl AsRef<Path>, content: impl AsRef<[u8]>) -> Result<WriteResult> {
    let name = name.as_ref();
    let content = content.as_ref();
    with_paths(&[name], || {
        let mut result = WriteResult {
            bytes: content.len(),
            lines: line_count(content),
            created: true,
            previous_lines: 0,
        };
        let mode = match fs::metadata(name) {
            Ok(info) if info.is_dir() => return fail(format!("{} is a directory", name.display())),
            Ok(info) => {
                result.created = false;
                if info.len() <= MAX_EDIT_BYTES as u64
                    && let Ok(previous) = read_bounded(name, MAX_EDIT_BYTES)
                {
                    result.previous_lines = line_count(&previous);
                }
                info.permissions().mode() & 0o777
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => 0o644,
            Err(err) => return Err(describe(err, name)),
        };
        DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(parent(name))
            .map_err(|err| {
                FileError(format!("create the directory of {}: {err}", name.display()))
            })?;
        replace_file(name, content, mode)?;
        Ok(result)
    })
}

pub(crate) fn replace_file(name: &Path, content: &[u8], mode: u32) -> Result<()> {
    let target = name.canonicalize().unwrap_or_else(|_| name.to_owned());
    let name = target.as_path();
    let action = || -> io::Result<()> {
        let mut file = tempfile::Builder::new()
            .prefix(&format!(
                ".{}.",
                name.file_name().unwrap_or_default().to_string_lossy()
            ))
            .suffix(".tmp")
            .tempfile_in(parent(name))?;
        file.write_all(content)?;
        file.as_file()
            .set_permissions(Permissions::from_mode(mode))?;
        file.persist(name).map_err(|err| err.error)?;
        Ok(())
    };
    action().map_err(|err| FileError(format!("write {}: {err}", name.display())))
}

fn parent(name: &Path) -> &Path {
    name.parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub(crate) fn read_bounded(name: &Path, cap: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(name)
        .and_then(|file| file.take(cap as u64 + 1).read_to_end(&mut data))
        .map_err(|err| describe(err, name))?;
    if data.len() > cap {
        return too_large(name, data.len() as u64);
    }
    Ok(data)
}

fn read_head(reader: &mut impl Read, bytes: &mut [u8]) -> io::Result<usize> {
    let mut used = 0;
    while used < bytes.len() {
        match reader.read(&mut bytes[used..]) {
            Ok(0) => break,
            Ok(n) => used += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(used)
}

fn too_large<T>(name: &Path, size: u64) -> Result<T> {
    fail(format!(
        "{} is {}, too large to edit; use Bash",
        name.display(),
        format_bytes(size)
    ))
}
fn metadata(name: &Path) -> Result<Metadata> {
    fs::metadata(name).map_err(|err| describe(err, name))
}
fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(FileError(message.into()))
}
fn describe(err: io::Error, name: &Path) -> FileError {
    FileError(match err.kind() {
        io::ErrorKind::NotFound => format!("{} does not exist", name.display()),
        io::ErrorKind::PermissionDenied => format!("{}: permission denied", name.display()),
        _ => format!("{}: {err}", name.display()),
    })
}
fn positive(value: i64, default: usize) -> usize {
    if value <= 0 { default } else { value as usize }
}

pub fn is_image_path(name: impl AsRef<Path>) -> bool {
    name.as_ref().extension().is_some_and(|ext| {
        matches!(
            ext.to_string_lossy().to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff"
        )
    })
}

fn not_found(name: &Path, content: &[u8], old: &[u8]) -> FileError {
    let content = display_bytes(content);
    let old = display_bytes(old);
    let squeezed = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    if squeezed(&content).contains(&squeezed(&old)) {
        return FileError(format!(
            "old_string was not found in {} as written, but matches when whitespace is ignored: check indentation (tabs against spaces), trailing spaces and blank lines, and copy the lines exactly as Read shows them",
            name.display()
        ));
    }
    let mut lines = old.trim().split('\n');
    let first = lines.next().unwrap_or_default().trim();
    if lines.next().is_some() && content.contains(first) {
        return FileError(format!(
            "old_string was not found in {}; its first line is there, so a later line differs: copy the lines exactly as Read shows them",
            name.display()
        ));
    }
    FileError(format!(
        "old_string was not found in {}; copy the lines exactly as Read shows them, without the line numbers",
        name.display()
    ))
}

fn crlf(bytes: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        if byte == b'\n' {
            result.push(b'\r');
        }
        result.push(byte);
    }
    result
}

fn snippet(content: &[u8], line: usize, changed: usize) -> String {
    let from = line.saturating_sub(4).max(1);
    let to = line
        .saturating_add(changed - 1)
        .saturating_add(4)
        .min(line_count(content));
    let mut text = Vec::new();
    for (index, value) in content
        .split(|b| *b == b'\n')
        .enumerate()
        .take(to)
        .skip(from - 1)
    {
        write!(text, "{:6}\t", index + 1).expect("Vec writes cannot fail");
        text.extend_from_slice(value);
        text.push(b'\n');
    }
    text.pop();
    display_bytes(&text)
}

pub(crate) fn line_count(content: &[u8]) -> usize {
    memchr::memchr_iter(b'\n', content).count()
        + usize::from(!content.is_empty() && !content.ends_with(b"\n"))
}

// Match Go's rune-by-rune display (and json/v1 replacement), while edits
// preserve the original bytes. This is not json/v2's strict string codec.
fn display_bytes(mut bytes: &[u8]) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(bytes) {
            Ok(rest) => {
                text.push_str(rest);
                break;
            }
            Err(err) => {
                let good = err.valid_up_to();
                text.push_str(std::str::from_utf8(&bytes[..good]).expect("validated UTF-8 prefix"));
                text.push('\u{fffd}');
                bytes = &bytes[good + 1..];
            }
        }
    }
    text
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
fn format_bytes(n: u64) -> String {
    if n < 1 << 10 {
        count(n, "byte", "bytes")
    } else if n < 1 << 20 {
        format!("{:.1} KB", n as f64 / (1 << 10) as f64)
    } else if n < 1 << 30 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else {
        format!("{:.1} GB", n as f64 / (1u64 << 30) as f64)
    }
}
fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_./@%+=:,-".contains(&c))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
