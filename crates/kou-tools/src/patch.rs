use crate::{
    fileops::{
        FileError, MAX_EDIT_BYTES, MAX_EDIT_RESULT_BYTES, Result, read_bounded, replace_file,
    },
    lock::with_paths,
};
use memchr::memmem;
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::{self, DirBuilder, Metadata},
    io,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

pub const MAX_PATCH_BYTES: usize = 4 << 20;
pub const MAX_PATCH_VIEW_BYTES: usize = 64 << 20;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PatchResult {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub moved: Vec<String>,
}
impl PatchResult {
    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        for (prefix, values) in [
            ("A", &self.added),
            ("M", &self.modified),
            ("R", &self.moved),
            ("D", &self.deleted),
        ] {
            lines.extend(values.iter().map(|name| format!("{prefix} {name}")));
        }
        if lines.is_empty() {
            "The patch changed nothing.".into()
        } else {
            format!("Applied the patch:\n{}", lines.join("\n"))
        }
    }
    pub fn files(&self) -> Vec<String> {
        let mut files = self.added.clone();
        files.extend_from_slice(&self.modified);
        files.extend_from_slice(&self.deleted);
        files.extend(
            self.moved
                .iter()
                .filter_map(|value| value.split_once(" -> ").map(|(_, to)| to.to_owned())),
        );
        files
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Add,
    Delete,
    Update,
}
struct Change {
    kind: Kind,
    path: String,
    moved: String,
    lines: Vec<Vec<u8>>,
    sections: Vec<Section>,
}
#[derive(Clone, Default)]
struct Section {
    context: Vec<Vec<u8>>,
    old: Vec<Vec<u8>>,
    chunks: Vec<Chunk>,
    eof: bool,
}
#[derive(Clone)]
struct Chunk {
    offset: usize,
    removed: usize,
    added: Vec<Vec<u8>>,
}

/// All parsing and context validation precede publication. Like the Go tool,
/// this is not an all-or-nothing multi-file transaction on an I/O failure.
pub fn apply_patch(patch: &str, root: impl AsRef<Path>) -> Result<PatchResult> {
    if patch.len() > MAX_PATCH_BYTES {
        return fail("the patch exceeds the 4 MiB input limit");
    }
    let changes = parse(patch)?;
    if changes.is_empty() {
        return fail("the patch holds no file");
    }
    let root = root.as_ref();
    let resolve = |name: &str| clean(&root.join(name));
    let mut named = Vec::new();
    for change in &changes {
        named.push(resolve(&change.path));
        if !change.moved.is_empty() {
            named.push(resolve(&change.moved));
        }
    }
    let references: Vec<_> = named.iter().map(PathBuf::as_path).collect();
    with_paths(&references, || apply(&changes, &resolve))
}

struct Entry {
    path: PathBuf,
    exists: bool,
    info: Option<Metadata>,
    stat_error: Option<io::Error>,
    data: Option<Vec<u8>>,
    mode: u32,
    write: bool,
}
#[derive(Default)]
struct View {
    indices: HashMap<PathBuf, usize>,
    entries: Vec<Entry>,
    bytes: usize,
}
impl View {
    fn get(&mut self, path: PathBuf) -> usize {
        if let Some(&index) = self.indices.get(&path) {
            return index;
        }
        let mut value = Entry {
            path: path.clone(),
            exists: false,
            info: None,
            stat_error: None,
            data: None,
            mode: 0o644,
            write: false,
        };
        match fs::metadata(&path) {
            Ok(info) => {
                value.exists = true;
                value.mode = info.permissions().mode() & 0o777;
                value.info = Some(info);
            }
            Err(e) => value.stat_error = Some(e),
        }
        let index = self.entries.len();
        self.indices.insert(path, index);
        self.entries.push(value);
        index
    }
    fn data(&mut self, index: usize, data: Option<Vec<u8>>) -> Result<()> {
        let old = self.entries[index].data.as_ref().map_or(0, Vec::len);
        let size = data.as_ref().map_or(0, Vec::len);
        if size > MAX_EDIT_RESULT_BYTES
            || size > MAX_PATCH_VIEW_BYTES.saturating_sub(self.bytes - old)
        {
            return fail("the patch exceeds the 64 MiB staged-content limit");
        }
        self.bytes = self.bytes - old + size;
        self.entries[index].data = data;
        Ok(())
    }
}

fn apply(changes: &[Change], resolve: &impl Fn(&str) -> PathBuf) -> Result<PatchResult> {
    let mut result = PatchResult::default();
    let mut view = View::default();
    for change in changes {
        let path = resolve(&change.path);
        let index = view.get(path.clone());
        let current = &view.entries[index];
        let directory =
            current.info.as_ref().is_some_and(Metadata::is_dir) && current.data.is_none();
        match change.kind {
            Kind::Add => {
                if current.exists {
                    return fail(format!(
                        "cannot add {}: it exists; use Update File to change it",
                        change.path
                    ));
                }
                let mut content = change.lines.join(&b'\n');
                if !change.lines.is_empty() {
                    content.push(b'\n');
                }
                view.data(index, Some(content))?;
                let current = &mut view.entries[index];
                current.exists = true;
                current.write = true;
                result.added.push(change.path.clone());
            }
            Kind::Delete => {
                if !current.exists {
                    return fail(format!(
                        "cannot delete {}: {}",
                        change.path,
                        missing(current, &change.path)
                    ));
                }
                if directory {
                    return fail(format!("cannot delete {}: it is a directory", change.path));
                }
                view.data(index, None)?;
                view.entries[index].exists = false;
                view.entries[index].write = false;
                result.deleted.push(change.path.clone());
            }
            Kind::Update => {
                if !current.exists {
                    return fail(format!(
                        "cannot update {}: {}",
                        change.path,
                        missing(current, &change.path)
                    ));
                }
                if directory {
                    return fail(format!("cannot update {}: it is a directory", change.path));
                }
                if current.data.is_none() {
                    let size = current
                        .info
                        .as_ref()
                        .expect("existing entry has disk metadata or data")
                        .len();
                    if size > MAX_EDIT_BYTES as u64 {
                        return fail(format!(
                            "cannot update {}: {:.1} MB is too large",
                            change.path,
                            size as f64 / (1 << 20) as f64
                        ));
                    }
                    let data = read_bounded(&path, MAX_EDIT_BYTES)?;
                    view.data(index, Some(data))?;
                }
                let updated = apply_sections(
                    view.entries[index].data.as_ref().expect("loaded content"),
                    &change.sections,
                )
                .map_err(|e| FileError(format!("{}: {e}", change.path)))?;
                if change.moved.is_empty() {
                    view.data(index, Some(updated))?;
                    view.entries[index].write = true;
                    if !result.added.contains(&change.path)
                        && !result.modified.contains(&change.path)
                    {
                        result.modified.push(change.path.clone());
                    }
                    continue;
                }
                let destination = resolve(&change.moved);
                let moved = view.get(destination.clone());
                if destination != path {
                    if view.entries[moved].exists {
                        return fail(format!(
                            "cannot move {} to {}: it exists",
                            change.path, change.moved
                        ));
                    }
                    view.entries[index].exists = false;
                    view.entries[index].write = false;
                    view.data(index, None)?;
                }
                let mode = view.entries[index].mode;
                view.data(moved, Some(updated))?;
                let moved = &mut view.entries[moved];
                moved.exists = true;
                moved.write = true;
                moved.mode = mode;
                result
                    .moved
                    .push(format!("{} -> {}", change.path, change.moved));
            }
        }
    }
    for current in &view.entries {
        if current.exists && current.write {
            let parent = current
                .path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            DirBuilder::new()
                .recursive(true)
                .mode(0o755)
                .create(parent)
                .map_err(|e| {
                    FileError(format!(
                        "create the directory of {}: {e}",
                        current.path.display()
                    ))
                })?;
            replace_file(
                &current.path,
                current.data.as_ref().expect("dirty content"),
                current.mode,
            )?;
        }
    }
    for current in &view.entries {
        if !current.exists && current.info.is_some() {
            match fs::remove_file(&current.path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return fail(format!("delete {}: {e}", current.path.display())),
            }
        }
    }
    Ok(result)
}
fn missing(entry: &Entry, name: &str) -> String {
    match entry.stat_error.as_ref().map(io::Error::kind) {
        Some(io::ErrorKind::PermissionDenied) => format!("{name}: permission denied"),
        _ => format!("{name} does not exist"),
    }
}

fn parse(patch: &str) -> Result<Vec<Change>> {
    let normalized = patch.replace("\r\n", "\n");
    let all: Vec<&str> = normalized.split('\n').collect();
    let Some(start) = all.iter().position(|line| line.trim() == "*** Begin Patch") else {
        return fail("the patch must start with *** Begin Patch");
    };
    let Some(end) = (start + 1..all.len())
        .rev()
        .find(|&i| all[i].trim() == "*** End Patch")
    else {
        return fail("the patch must end with *** End Patch");
    };
    let lines = &all[start + 1..end];
    let mut changes = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        let (kind, path) = if let Some(path) = line.strip_prefix("*** Add File: ") {
            (Kind::Add, path)
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            (Kind::Delete, path)
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            (Kind::Update, path)
        } else {
            return fail(format!(
                "line {}: expected *** Add File:, *** Update File: or *** Delete File:, got {line:?}",
                index + 1
            ));
        };
        let mut change = Change {
            kind,
            path: path.trim().into(),
            moved: String::new(),
            lines: Vec::new(),
            sections: Vec::new(),
        };
        index += 1;
        match kind {
            Kind::Add => {
                while index < lines.len() && !lines[index].starts_with("*** ") {
                    let Some(added) = lines[index].strip_prefix('+') else {
                        return fail(format!(
                            "line {} of the added file {} must start with +: {:?}",
                            index + 1,
                            change.path,
                            lines[index]
                        ));
                    };
                    change.lines.push(added.as_bytes().to_owned());
                    index += 1;
                }
            }
            Kind::Update => {
                if let Some(moved) = lines
                    .get(index)
                    .and_then(|line| line.strip_prefix("*** Move to: "))
                {
                    change.moved = moved.trim().into();
                    index += 1;
                }
                change.sections = parse_sections(lines, &mut index, &change.path)?;
                if change.sections.is_empty() && change.moved.is_empty() {
                    return fail(format!("the update of {} changes nothing", change.path));
                }
            }
            Kind::Delete => {}
        }
        if change.path.is_empty() {
            return fail("a file line names no path");
        }
        changes.push(change);
    }
    Ok(changes)
}

fn parse_sections(lines: &[&str], index: &mut usize, path: &str) -> Result<Vec<Section>> {
    let mut sections = Vec::new();
    while *index < lines.len() {
        let line = lines[*index];
        if line.starts_with("*** ") && !line.starts_with("*** End of File") {
            break;
        }
        let mut section = Section::default();
        if line.starts_with("@@") {
            while *index < lines.len() && lines[*index].starts_with("@@") {
                let header = lines[*index].strip_prefix("@@").unwrap().trim();
                if !header.is_empty() {
                    section.context.push(header.as_bytes().to_owned());
                }
                *index += 1;
            }
        } else if !sections.is_empty() {
            return fail(format!(
                "{path}: line {}: a section after the first must start with @@",
                *index + 1
            ));
        }
        let mut chunk = None;
        while *index < lines.len() {
            let line = lines[*index];
            if line.starts_with("@@")
                || line.starts_with("*** ") && !line.starts_with("*** End of File")
            {
                break;
            }
            *index += 1;
            if line == "*** End of File" {
                section.eof = true;
            } else if line.starts_with(['+', '-']) {
                let at = *chunk.get_or_insert_with(|| {
                    section.chunks.push(Chunk {
                        offset: section.old.len(),
                        removed: 0,
                        added: Vec::new(),
                    });
                    section.chunks.len() - 1
                });
                if let Some(added) = line.strip_prefix('+') {
                    section.chunks[at].added.push(added.as_bytes().to_owned());
                } else {
                    section.chunks[at].removed += 1;
                    section.old.push(line.as_bytes()[1..].to_owned());
                }
            } else {
                chunk = None;
                section
                    .old
                    .push(line.strip_prefix(' ').unwrap_or(line).as_bytes().to_owned());
            }
        }
        if section.chunks.is_empty() {
            if section.context.is_empty()
                && section.old.iter().all(|line| trim_space(line).is_empty())
            {
                continue;
            }
            return fail(format!("{path}: a section adds or removes nothing"));
        }
        sections.push(section);
    }
    Ok(sections)
}

struct Edit<'a> {
    at: usize,
    remove: usize,
    added: &'a [Vec<u8>],
    section: usize,
}
fn apply_sections(content: &[u8], sections: &[Section]) -> Result<Vec<u8>> {
    let windows = memmem::find_iter(content, b"\r\n").count()
        > memchr::memchr_iter(b'\n', content).count() / 2;
    let normalized;
    let content = if windows {
        normalized = normalize_crlf(content);
        normalized.as_slice()
    } else {
        content
    };
    let trailing = content.ends_with(b"\n");
    let mut lines: Vec<&[u8]> = content.split(|b| *b == b'\n').collect();
    if trailing {
        lines.pop();
    }
    let mut cursor = 0;
    let mut edits = Vec::new();
    for (number, section) in sections.iter().enumerate() {
        for header in &section.context {
            let at = (cursor..lines.len())
                .find(|&n| lines[n] == header)
                .or_else(|| {
                    (cursor..lines.len()).find(|&n| trim_space(lines[n]) == trim_space(header))
                });
            let Some(at) = at else {
                return fail(format!(
                    "the @@ context {:?} of section {} was not found after line {cursor}",
                    String::from_utf8_lossy(header),
                    number + 1
                ));
            };
            cursor = at + 1;
        }
        let mut old = section.old.as_slice();
        let found = find_context(&lines, old, cursor, section.eof)
            .or_else(|error| {
                let end = section
                    .chunks
                    .iter()
                    .map(|chunk| chunk.offset + chunk.removed)
                    .max()
                    .unwrap_or(0);
                while old.len() > end && trim_space(old.last().unwrap()).is_empty() {
                    old = &old[..old.len() - 1];
                }
                if old.len() < section.old.len() {
                    find_context(&lines, old, cursor, section.eof).map_err(|_| error)
                } else {
                    Err(error)
                }
            })
            .map_err(|e| FileError(format!("section {}: {e}", number + 1)))?;
        for chunk in &section.chunks {
            edits.push(Edit {
                at: found + chunk.offset,
                remove: chunk.removed,
                added: &chunk.added,
                section: number,
            });
        }
        cursor = found + old.len();
    }
    let mut updated = Vec::new();
    let mut position = 0;
    for edit in edits {
        if edit.at < position {
            return fail(format!(
                "section {} overlaps the one before it",
                edit.section + 1
            ));
        }
        updated.extend_from_slice(&lines[position..edit.at]);
        updated.extend(edit.added.iter().map(Vec::as_slice));
        position = edit.at + edit.remove;
    }
    updated.extend_from_slice(&lines[position..]);
    let body = updated
        .iter()
        .try_fold(0usize, |total, line| total.checked_add(line.len()));
    let separators = updated.len().saturating_sub(1);
    let ending = trailing || !updated.is_empty() && (body != Some(0) || separators != 0);
    let newlines = separators.checked_add(usize::from(ending));
    let length = body.and_then(|body| {
        newlines?
            .checked_mul(if windows { 2 } else { 1 })?
            .checked_add(body)
    });
    if !length.is_some_and(|length| length <= MAX_EDIT_RESULT_BYTES) {
        return fail("the updated file exceeds the 64 MiB result limit");
    }
    let mut result = updated.join(&b'\n');
    if trailing || !updated.is_empty() && !result.is_empty() {
        result.push(b'\n');
    }
    if windows {
        let mut crlf =
            Vec::with_capacity(result.len() + memchr::memchr_iter(b'\n', &result).count());
        for byte in result {
            if byte == b'\n' {
                crlf.push(b'\r');
            }
            crlf.push(byte);
        }
        result = crlf;
    }
    Ok(result)
}
fn normalize_crlf(content: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(content.len());
    let mut after = 0;
    for at in memmem::find_iter(content, b"\r\n") {
        output.extend_from_slice(&content[after..at]);
        output.push(b'\n');
        after = at + 2;
    }
    output.extend_from_slice(&content[after..]);
    output
}
fn find_context(lines: &[&[u8]], old: &[Vec<u8>], from: usize, eof: bool) -> Result<usize> {
    if old.is_empty() {
        return Ok(if eof { lines.len() } else { from });
    }
    let matches = |at: usize, mode| {
        at <= lines.len()
            && old.len() <= lines.len() - at
            && old.iter().enumerate().all(|(n, want)| match mode {
                0 => lines[at + n] == want,
                1 => trim_end(lines[at + n]) == trim_end(want),
                _ => trim_space(lines[at + n]) == trim_space(want),
            })
    };
    for mode in 0..3 {
        if eof && old.len() <= lines.len() && matches(lines.len() - old.len(), mode) {
            return Ok(lines.len() - old.len());
        }
        if let Some(at) =
            (from..=lines.len().saturating_sub(old.len())).find(|&at| matches(at, mode))
        {
            return Ok(at);
        }
    }
    fail(format!(
        "its context was not found after line {from}; the file does not hold these lines as the patch gives them:\n{}\nRead the file again and copy the lines exactly",
        old.iter()
            .take(3)
            .map(|line| String::from_utf8_lossy(line))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}
fn trim_end(mut bytes: &[u8]) -> &[u8] {
    while bytes.last().is_some_and(|b| matches!(b, b' ' | b'\t')) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}
fn trim_space(mut bytes: &[u8]) -> &[u8] {
    // Decode only edge characters. Invalid interior UTF-8 is neither changed
    // nor treated as whitespace, matching strings.TrimSpace on a Go string.
    while !bytes.is_empty() {
        let width = match bytes[0] {
            0..=127 => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => break,
        };
        if width > bytes.len()
            || !std::str::from_utf8(&bytes[..width])
                .ok()
                .and_then(|s| s.chars().next())
                .is_some_and(char::is_whitespace)
        {
            break;
        }
        bytes = &bytes[width..];
    }
    while !bytes.is_empty() {
        let mut at = bytes.len() - 1;
        while at > 0 && bytes[at] & 0xc0 == 0x80 {
            at -= 1;
        }
        if !std::str::from_utf8(&bytes[at..])
            .ok()
            .and_then(|s| s.chars().next())
            .is_some_and(char::is_whitespace)
        {
            break;
        }
        bytes = &bytes[..at];
    }
    bytes
}
fn clean(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if result.file_name().is_some_and(|name| name != "..") => {
                result.pop();
            }
            Component::ParentDir if result.has_root() => {}
            part => result.push(part.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}
fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(FileError(message.into()))
}
