//! Files in prompts, shared by the views that write prompts and the backends
//! that read them.
//!
//! A prompt links a file or a folder by a `$` reference in its text, as in
//! kou-conveyor's cockpits: `$cmd/main.go`, `$cmd/main.go:120-160` for those
//! lines, `$cmd/` for a folder's entries, `$"a b.txt"` for a path with
//! spaces, `$~/notes.md` and `$/etc/hosts` outside the project. A reference
//! names something that exists, or it is no link: `$HOME` and `$1` stay text.
//!
//! [`file_links`] reads the references of a text, [`link_query`] finds the
//! one being typed at the cursor, and [`FileIndex`] lists a project's files
//! (as git sees them, or by a walk that keeps to `.gitignore`) off the
//! calling thread, to complete a reference from.

use serde::Serialize;
use std::{
    io::Read as _,
    ops::Range,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The most links one prompt brings; the runner takes no more.
pub const MAX_FILE_LINKS: usize = 20;
/// The most bytes a reference may take.
const MAX_REFERENCE: usize = 1024;
/// Punctuation that ends a sentence or a clause after a path.
const TRAILING: &str = ".,;:!?)]}>'\"…»";
/// The most files an index lists, and how long it lists them for.
const MAX_INDEXED: usize = 200_000;
const INDEX_TIME: Duration = Duration::from_secs(5);
/// How long an index keeps a listing at least before it lists anew.
const FRESH: Duration = Duration::from_secs(3);
/// Folders a walk leaves out: version control, the harness's own,
/// dependencies and caches.
const SKIPPED_FOLDERS: [&str; 18] = [
    ".git",
    ".hg",
    ".svn",
    ".harness",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".tox",
    ".gradle",
    ".next",
    ".nuxt",
    ".turbo",
    ".terraform",
    ".cache",
    ".Trash",
];

// ---------------------------------------------------------------- links

/// A file or folder a prompt links to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileLink {
    /// The reference as the text has it: `$cmd/main.go:10-40`.
    pub label: String,
    /// The path it names: relative to the workspace, absolute, or in `~/`.
    pub path: String,
    /// The first line it asks for; 0 where it asks for none.
    #[serde(skip_serializing_if = "is_zero")]
    pub start_line: u32,
    /// The last line it asks for; 0 where it does not say.
    #[serde(skip_serializing_if = "is_zero")]
    pub end_line: u32,
    /// It names a folder.
    #[serde(skip)]
    pub directory: bool,
    /// The file's size in bytes; 0 for a folder.
    #[serde(skip)]
    pub size: u64,
    /// Where the reference is in the text, `$` included.
    #[serde(skip)]
    pub range: Range<usize>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// Where a linked path is: in the workspace, unless it is absolute or in the
/// home folder.
pub fn link_target(workspace: &Path, name: &str) -> PathBuf {
    if let Some(rest) = name.strip_prefix("~/").or((name == "~").then_some(""))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    let path = Path::new(name);
    if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    }
}

/// How a prompt links a path: `$cmd/main.go`, quoted where it has spaces.
pub fn link_label(name: &str) -> String {
    if name.contains(char::is_whitespace) && !name.contains('"') {
        format!("$\"{name}\"")
    } else {
        format!("${name}")
    }
}

/// The links of a text: its references to files and folders that exist, each
/// label once, in the order of the text, at most [`MAX_FILE_LINKS`].
pub fn file_links(text: &str, workspace: &Path) -> Vec<FileLink> {
    let mut links: Vec<FileLink> = Vec::new();
    for reference in references(text) {
        let Some(link) = reference.resolve(workspace) else {
            continue;
        };
        if links.iter().any(|other| other.label == link.label) {
            continue;
        }
        links.push(link);
        if links.len() == MAX_FILE_LINKS {
            break;
        }
    }
    links
}

/// A `$` of a text that may start a link: the text it may take, or its
/// quoted path and what follows the quote.
struct Reference<'a> {
    /// Offset of the `$`.
    start: usize,
    raw: &'a str,
    quoted: bool,
}

fn references(text: &str) -> Vec<Reference<'_>> {
    let code = code_ranges(text);
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(next) = text[at..].find('$') {
        let start = at + next;
        at = start + 1;
        if !opens_reference(text, start) || in_ranges(&code, start) {
            continue;
        }
        let rest = &text[start + 1..];
        if let Some(quoted) = rest.strip_prefix('"') {
            let Some(end) = quoted.find(['"', '\n']) else {
                continue;
            };
            if end == 0 || !quoted[end..].starts_with('"') || end > MAX_REFERENCE {
                continue;
            }
            // The quoted path, and what follows it up to a space.
            let after = &quoted[end + 1..];
            let after_end = after.find(char::is_whitespace).unwrap_or(after.len());
            found.push(Reference {
                start,
                raw: &quoted[..end + 1 + after_end],
                quoted: true,
            });
            continue;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if end == 0 || end > MAX_REFERENCE {
            continue;
        }
        found.push(Reference {
            start,
            raw: &rest[..end],
            quoted: false,
        });
    }
    found
}

/// Whether the `$` at `index` may start a reference: it follows the start of
/// the text, a space, a comma, or an opening bracket or quote.
fn opens_reference(text: &str, index: usize) -> bool {
    match text[..index].chars().next_back() {
        None => true,
        Some(before) => before.is_whitespace() || "([{<\"',".contains(before),
    }
}

impl Reference<'_> {
    fn resolve(&self, workspace: &Path) -> Option<FileLink> {
        let found = |mut link: FileLink, label: String| {
            link.range = self.start..self.start + label.len();
            link.label = label;
            link
        };
        if self.quoted {
            let end = self.raw.find('"')?;
            let (name, mut after) = (&self.raw[..end], &self.raw[end + 1..]);
            loop {
                if let Some(link) = link_to(name, after, workspace) {
                    return Some(found(link, format!("$\"{name}\"{after}")));
                }
                let last = after.chars().next_back()?;
                if !TRAILING.contains(last) {
                    return None;
                }
                after = &after[..after.len() - last.len_utf8()];
            }
        }
        let mut token = self.raw;
        while !token.is_empty() {
            if let Some(link) = link_to(token, "", workspace) {
                return Some(found(link, format!("${token}")));
            }
            if let Some(at) = token.rfind(':').filter(|at| *at > 0)
                && let Some(link) = link_to(&token[..at], &token[at..], workspace)
            {
                return Some(found(link, format!("${token}")));
            }
            let last = token.chars().next_back()?;
            if !TRAILING.contains(last) {
                break;
            }
            token = &token[..token.len() - last.len_utf8()];
        }
        None
    }
}

/// Links `name`, with the lines a suffix such as `:10-40` asks for, if it is
/// a file or folder that exists.
fn link_to(name: &str, suffix: &str, workspace: &Path) -> Option<FileLink> {
    if name.trim_matches(['.', '/', '~']).is_empty() || name.contains(['\0', '\n', '\r']) {
        return None;
    }
    let (mut start_line, mut end_line) = (0, 0);
    if !suffix.is_empty() {
        let lines = suffix.strip_prefix(':')?;
        let (first, last) = match lines.split_once('-') {
            Some((first, last)) => (first, Some(last)),
            None => (lines, None),
        };
        let number = |digits: &str| -> Option<u32> {
            ((1..=9).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| digits.parse().ok())
                .flatten()
        };
        start_line = number(first)?;
        if let Some(last) = last {
            end_line = number(last)?;
        }
        if start_line < 1 || end_line != 0 && end_line < start_line {
            return None;
        }
    }
    let metadata = std::fs::metadata(link_target(workspace, name)).ok()?;
    if !suffix.is_empty() && metadata.is_dir() {
        return None;
    }
    Some(FileLink {
        label: String::new(),
        path: name.to_owned(),
        start_line,
        end_line,
        directory: metadata.is_dir(),
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        range: 0..0,
    })
}

/// Where a text has code: fenced blocks and code spans, as byte ranges.
fn code_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    // A line of three or more backticks or tildes opens a block, which a
    // line of as many of the same closes, or the end of the text.
    let mut fence: Option<(&str, usize)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() <= 3 {
            let marker = fence_marker(trimmed);
            match fence {
                None if !marker.is_empty() => fence = Some((marker, offset)),
                Some((open, start))
                    if marker.starts_with(open) && trimmed[marker.len()..].trim().is_empty() =>
                {
                    ranges.push((start, offset + line.len()));
                    fence = None;
                }
                _ => {}
            }
        }
        offset += line.len();
    }
    if let Some((_, start)) = fence {
        ranges.push((start, text.len()));
    }
    // A code span is a run of backticks, up to the next run of as many.
    let run_at = |from: usize| text[from..].len() - text[from..].trim_start_matches('`').len();
    let mut at = 0;
    while let Some(next) = text[at..].find('`') {
        let open = at + next;
        if in_ranges(&ranges, open) {
            at = open + 1;
            continue;
        }
        let run = run_at(open);
        let mut search = open + run;
        at = open + run;
        while let Some(next) = text[search..].find('`') {
            let close = search + next;
            let length = run_at(close);
            if length == run {
                ranges.push((open, close + length));
                at = close + length;
                break;
            }
            search = close + length;
        }
    }
    ranges
}

/// The backticks or tildes that start a line, three or more, or nothing.
fn fence_marker(line: &str) -> &str {
    for mark in ['`', '~'] {
        let marker = &line[..line.len() - line.trim_start_matches(mark).len()];
        if marker.len() >= 3 {
            return marker;
        }
    }
    ""
}

fn in_ranges(ranges: &[(usize, usize)], index: usize) -> bool {
    ranges
        .iter()
        .any(|(from, to)| index >= *from && index < *to)
}

// ---------------------------------------------------------------- completing

/// The reference being typed at the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkQuery {
    /// Offset of its `$` in the text.
    pub start: usize,
    /// The path typed so far.
    pub query: String,
    /// It opened with `$"`, so the path may hold spaces.
    pub quoted: bool,
}

/// The reference being typed at the end of `before`, the text before the
/// cursor; `None` where none is, or where its lines are being typed.
pub fn link_query(before: &str) -> Option<LinkQuery> {
    let code = code_ranges(before);
    // A quoted path still open, which may hold spaces: $"a b
    if let Some(at) = before.rfind("$\"")
        && opens_reference(before, at)
        && !in_ranges(&code, at)
    {
        let name = &before[at + 2..];
        if !name.contains(['"', '\n']) && name.len() <= MAX_REFERENCE {
            return Some(LinkQuery {
                start: at,
                query: name.to_owned(),
                quoted: true,
            });
        }
    }
    // Otherwise the last word, from its first $ that opens a reference.
    let word = before
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(0, |(at, ch)| at + ch.len_utf8());
    for (offset, ch) in before[word..].char_indices() {
        let at = word + offset;
        if ch != '$' || !opens_reference(before, at) || in_ranges(&code, at) {
            continue;
        }
        let rest = &before[at + 1..];
        if rest.len() > MAX_REFERENCE || rest.starts_with('"') {
            return None;
        }
        if let Some(colon) = rest.rfind(':')
            && colon + 1 < rest.len()
            && rest[colon + 1..]
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-')
        {
            // Lines are being typed: the path is complete.
            return None;
        }
        return Some(LinkQuery {
            start: at,
            query: rest.to_owned(),
            quoted: false,
        });
    }
    None
}

/// A file or folder a reference being typed can complete to. A folder's
/// path ends in a slash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatch {
    /// As the reference would name it.
    pub path: String,
    /// It is a folder.
    pub directory: bool,
}

/// What is in the folder a path typed from `/` or `~/` ends in, whose names
/// start with what follows its last slash, written the way it was typed:
/// folders first, each with a slash at the end. Hidden names show once a
/// dot is typed.
pub fn complete_path(typed: &str, limit: usize) -> Vec<FileMatch> {
    let cut = typed.rfind('/').map_or(0, |at| at + 1);
    let (dir, prefix) = (&typed[..cut], typed[cut..].to_lowercase());
    let Ok(entries) = std::fs::read_dir(link_target(Path::new(""), dir)) else {
        return Vec::new();
    };
    let mut matches: Vec<FileMatch> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            if !lower.starts_with(&prefix) || name.starts_with('.') && !prefix.starts_with('.') {
                return None;
            }
            // A link to a folder is a folder.
            let directory = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir());
            let mut path = format!("{dir}{name}");
            if directory {
                path.push('/');
            }
            Some(FileMatch { path, directory })
        })
        .collect();
    matches.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    matches.truncate(limit);
    matches
}

// ---------------------------------------------------------------- the index

/// A project's files and folders as one listing found them: relative to the
/// project, `/`-separated, folders with a slash at the end, sorted.
#[derive(Debug, Default)]
pub struct Listing {
    entries: Vec<String>,
    /// The entries in lower case, kept so that matching does not lower them
    /// at every key.
    lower: Vec<String>,
    /// Where each entry's last part starts.
    names: Vec<u32>,
    /// Why the listing is not whole: it timed out, a folder could not be read.
    pub error: Option<String>,
    /// It stopped at its limit of entries.
    pub truncated: bool,
    /// The listing came from git, which knows what the project ignores.
    pub from_git: bool,
}

impl Listing {
    /// A listing of these files, with the folders they are in added.
    pub fn of(files: Vec<String>) -> Self {
        let mut entries = files;
        let mut folders = std::collections::HashSet::new();
        for file in &entries {
            let mut rest = file.trim_end_matches('/');
            while let Some(at) = rest.rfind('/') {
                rest = &rest[..at];
                if !folders.insert(format!("{rest}/")) {
                    break;
                }
            }
        }
        entries.extend(folders);
        entries.sort();
        entries.dedup();
        let lower = entries.iter().map(|entry| entry.to_lowercase()).collect();
        let names = entries
            .iter()
            .map(|entry| {
                let trimmed = entry.trim_end_matches('/');
                trimmed.rfind('/').map_or(0, |at| at + 1) as u32
            })
            .collect();
        Self {
            entries,
            lower,
            names,
            ..Self::default()
        }
    }

    /// Every entry.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// How many entries it has.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it found nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries a reference that has `query` so far can complete to, the
    /// best first, at most `limit`. Nothing typed lists the top level, and a
    /// folder typed with its slash its entries; otherwise paths that start
    /// with the query come first, then names that start with it, then names
    /// and paths that hold it, and then those that hold its letters in order
    /// close together, shorter paths before longer ones. A path from `/` or
    /// `~/` lists what is in that folder of the disk instead.
    pub fn matches(&self, query: &str, limit: usize) -> Vec<FileMatch> {
        if query.starts_with('/') || query.starts_with("~/") {
            return complete_path(query, limit);
        }
        let folder = query.ends_with('/')
            && self
                .entries
                .binary_search_by(|e| e.as_str().cmp(query))
                .is_ok();
        let lower = query.to_lowercase();
        let mut found: Vec<(u32, usize)> = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if query.is_empty() || folder {
                // The entries right inside the folder, or the top level.
                if let Some(rest) = entry.strip_prefix(query)
                    && !rest.is_empty()
                    && !rest.trim_end_matches('/').contains('/')
                {
                    found.push((if rest.ends_with('/') { 3 } else { 2 }, index));
                }
                continue;
            }
            let score = score(&self.lower[index], self.names[index] as usize, &lower);
            if score > 0 {
                found.push((score, index));
            }
        }
        let order = |a: &(u32, usize), b: &(u32, usize)| {
            b.0.cmp(&a.0)
                .then_with(|| self.entries[a.1].len().cmp(&self.entries[b.1].len()))
                .then_with(|| self.entries[a.1].cmp(&self.entries[b.1]))
        };
        // The best `limit` without sorting every match of a large project.
        if found.len() > limit && limit > 0 {
            found.select_nth_unstable_by(limit - 1, order);
            found.truncate(limit);
        }
        found.sort_by(order);
        found.truncate(limit);
        found
            .into_iter()
            .map(|(_, index)| FileMatch {
                path: self.entries[index].clone(),
                directory: self.entries[index].ends_with('/'),
            })
            .collect()
    }
}

/// How well an entry matches a query, both lower case: 0 for not at all.
/// `name` is where the entry's last part starts.
fn score(entry: &str, name: usize, query: &str) -> u32 {
    let base = entry[name..].trim_end_matches('/');
    if entry.starts_with(query) {
        return 5000;
    }
    if base.starts_with(query) {
        return 4000;
    }
    if let Some(at) = base.find(query) {
        return 3000 - at.min(100) as u32;
    }
    if let Some(at) = entry.find(query) {
        let bonus = if at == 0 || "/._-".contains(char_before(entry, at)) {
            200
        } else {
            0
        };
        return 2000 + bonus - at.min(100) as u32;
    }
    // The query's letters in order, close together: fewer gaps and more at
    // the starts of words better. Letters strewn along a long path match
    // nothing that was meant.
    let mut score: i64 = 1000;
    let (mut from, mut first, mut last) = (0, None, None::<usize>);
    for ch in query.chars() {
        let Some(next) = entry[from..].find(ch) else {
            return 0;
        };
        let next = from + next;
        first.get_or_insert(next);
        if let Some(last) = last {
            score -= (next - last - 1).min(20) as i64;
        }
        if next == 0 || "/._-".contains(char_before(entry, next)) {
            score += 10;
        }
        last = Some(next);
        from = next + ch.len_utf8();
    }
    let (Some(first), Some(last)) = (first, last) else {
        return 0;
    };
    let span = last - first + 1;
    let length = query.chars().count();
    if span > (3 * length).max(length + 8) {
        return 0;
    }
    score.clamp(1, 1999) as u32
}

fn char_before(text: &str, at: usize) -> char {
    text[..at].chars().next_back().unwrap_or(' ')
}

#[derive(Default)]
struct State {
    listing: Option<Arc<Listing>>,
    /// When the listing was made, and how long it took.
    at: Option<Instant>,
    took: Duration,
    listing_now: bool,
    /// Told, by being dropped, once the first listing is there.
    waiting: Vec<async_channel::Sender<()>>,
}

struct Shared {
    root: PathBuf,
    state: Mutex<State>,
}

/// A project's files and folders, for a `$` being typed to complete from.
///
/// The listing is made on a thread of its own as soon as the index exists,
/// and made again in the background once it is a few seconds old (longer
/// where listing takes long), so a key never waits for the disk. In a work
/// tree of git it holds what git tracks or would take; elsewhere a walk that
/// keeps to the `.gitignore` files it meets and leaves out version control,
/// dependencies and caches. Clones share one listing.
#[derive(Clone)]
pub struct FileIndex {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for FileIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileIndex")
            .field("root", &self.shared.root)
            .finish_non_exhaustive()
    }
}

impl FileIndex {
    /// An index of the project at `root`, which starts listing it at once.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let index = Self {
            shared: Arc::new(Shared {
                root: root.into(),
                state: Mutex::new(State::default()),
            }),
        };
        index.relist();
        index
    }

    /// The project's folder.
    pub fn root(&self) -> &Path {
        &self.shared.root
    }

    /// The latest listing, `None` until the first is done. A listing grown
    /// old is returned while a new one is made.
    pub fn listing(&self) -> Option<Arc<Listing>> {
        let (listing, stale) = {
            let state = self.lock();
            let stale = state
                .at
                .is_some_and(|at| at.elapsed() > FRESH.max(state.took * 10))
                && !state.listing_now;
            (state.listing.clone(), stale)
        };
        if stale {
            self.relist();
        }
        listing
    }

    /// A channel that closes once the first listing is done: `recv` returns
    /// an error then, at once if it is done already.
    pub fn listed(&self) -> async_channel::Receiver<()> {
        let (sender, receiver) = async_channel::bounded(1);
        let mut state = self.lock();
        if state.listing.is_none() {
            state.waiting.push(sender);
        }
        receiver
    }

    /// Waits for the first listing on the calling thread, at most `timeout`.
    pub fn wait(&self, timeout: Duration) -> Option<Arc<Listing>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(listing) = self.lock().listing.clone() {
                return Some(listing);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn relist(&self) {
        {
            let mut state = self.lock();
            if state.listing_now {
                return;
            }
            state.listing_now = true;
        }
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("kou-file-index".into())
            .spawn(move || {
                let started = Instant::now();
                let listing = Arc::new(list(&shared.root));
                let mut state = shared
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.listing = Some(listing);
                state.at = Some(Instant::now());
                state.took = started.elapsed();
                state.listing_now = false;
                state.waiting.clear();
            });
        if spawned.is_err() {
            let mut state = self.lock();
            state.listing_now = false;
            state.listing = Some(Arc::new(Listing {
                error: Some("no thread could be started to list the files".into()),
                ..Listing::default()
            }));
            state.waiting.clear();
        }
    }
}

/// Lists a project: through git where it is a work tree, else by a walk.
fn list(root: &Path) -> Listing {
    let deadline = Instant::now() + INDEX_TIME;
    if let Some(files) = git_files(root, deadline) {
        let truncated = files.len() >= MAX_INDEXED;
        let mut listing = Listing::of(files);
        listing.truncated = truncated;
        listing.from_git = true;
        return listing;
    }
    let mut walk = Walk {
        files: Vec::new(),
        deadline,
        error: None,
    };
    walk.folder(root, "", &Rules::default());
    let truncated = walk.files.len() >= MAX_INDEXED;
    let mut listing = Listing::of(walk.files);
    listing.truncated = truncated;
    listing.error = walk.error;
    listing
}

/// The files of a work tree of git that are tracked or not ignored, or
/// `None` where `root` is not in one or git is not there.
fn git_files(root: &Path, deadline: Instant) -> Option<Vec<String>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = reader.join().ok()?;
    if !status?.success() {
        return None;
    }
    let mut files = Vec::new();
    for name in out.split(|byte| *byte == 0) {
        if name.is_empty() || name.starts_with(b".harness/") {
            continue;
        }
        files.push(String::from_utf8_lossy(name).into_owned());
        if files.len() == MAX_INDEXED {
            break;
        }
    }
    Some(files)
}

/// A walk of a folder no git keeps.
struct Walk {
    files: Vec<String>,
    deadline: Instant,
    error: Option<String>,
}

impl Walk {
    /// Lists `dir`, whose path in the project is `prefix` (empty, or ending
    /// in a slash), under the rules of the folders above it.
    fn folder(&mut self, dir: &Path, prefix: &str, above: &Rules) -> bool {
        if self.files.len() >= MAX_INDEXED {
            return false;
        }
        if Instant::now() > self.deadline {
            self.error = Some("listing the files took too long; some are left out".into());
            return false;
        }
        let rules = match std::fs::read_to_string(dir.join(".gitignore")) {
            Ok(text) => above.with(&text, prefix),
            Err(_) => above.clone(),
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return true;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // A link to a folder is not followed: it may lead in circles.
            let is_dir = kind.is_dir();
            let path = format!("{prefix}{name}");
            if is_dir && SKIPPED_FOLDERS.contains(&name.as_str()) {
                continue;
            }
            if rules.ignores(&path, is_dir) {
                continue;
            }
            if is_dir {
                if !self.folder(&entry.path(), &format!("{path}/"), &rules) {
                    return false;
                }
            } else {
                self.files.push(path);
                if self.files.len() >= MAX_INDEXED {
                    return false;
                }
            }
        }
        true
    }
}

/// The ignore rules in force in a folder: those of the `.gitignore` files
/// above it and its own, in that order.
#[derive(Clone, Default)]
struct Rules(Arc<Vec<Rule>>);

/// One line of a `.gitignore`.
struct Rule {
    /// The folder of the `.gitignore`, as a prefix of the project's paths.
    base: String,
    glob: Vec<char>,
    /// `!`: the rule takes a path back.
    negated: bool,
    /// A trailing slash: it applies to folders only.
    folders: bool,
    /// A slash at its start or in its middle: it is matched against the
    /// path from its `.gitignore`'s folder, not against a name at any depth.
    anchored: bool,
}

impl Rules {
    fn with(&self, text: &str, base: &str) -> Self {
        let mut rules: Vec<Rule> = Vec::new();
        for line in text.lines() {
            let line = line.trim_end_matches(['\r']);
            // Trailing spaces go unless escaped; the rest of the syntax keeps them.
            let line = line.trim_end_matches(' ');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negated, line) = match line.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, line.strip_prefix('\\').unwrap_or(line)),
            };
            let (folders, line) = match line.strip_suffix('/') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            if line.is_empty() {
                continue;
            }
            let anchored = line.trim_start_matches("**/").contains('/');
            let line = line.strip_prefix('/').unwrap_or(line);
            rules.push(Rule {
                base: base.to_owned(),
                glob: line.chars().collect(),
                negated,
                folders,
                anchored,
            });
        }
        if rules.is_empty() {
            return self.clone();
        }
        let mut all: Vec<Rule> = self
            .0
            .iter()
            .map(|rule| Rule {
                base: rule.base.clone(),
                glob: rule.glob.clone(),
                negated: rule.negated,
                folders: rule.folders,
                anchored: rule.anchored,
            })
            .collect();
        all.extend(rules);
        Self(Arc::new(all))
    }

    /// Whether the project's path `path` is ignored: the last rule that
    /// matches it says.
    fn ignores(&self, path: &str, is_dir: bool) -> bool {
        let mut ignored = false;
        for rule in self.0.iter() {
            if rule.folders && !is_dir {
                continue;
            }
            let Some(local) = path.strip_prefix(&rule.base) else {
                continue;
            };
            let subject: Vec<char> = if rule.anchored {
                local.chars().collect()
            } else {
                local.rsplit('/').next().unwrap_or(local).chars().collect()
            };
            if glob(&rule.glob, &subject) {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

/// Whether `text` matches a `.gitignore` glob: `*` and `?` within one part of
/// a path, `**` across parts, `[...]` a class of characters.
fn glob(pattern: &[char], text: &[char]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some('*') if pattern.get(1) == Some(&'*') => {
            // `**/` matches no folder or any number of them; `**` at the
            // end, everything.
            let rest = &pattern[2..];
            let rest = rest.strip_prefix(&['/']).unwrap_or(rest);
            if rest.is_empty() {
                return true;
            }
            (0..=text.len()).any(|at| (at == 0 || text[at - 1] == '/') && glob(rest, &text[at..]))
        }
        Some('*') => {
            let rest = &pattern[1..];
            for at in 0..=text.len() {
                if glob(rest, &text[at..]) {
                    return true;
                }
                if at < text.len() && text[at] == '/' {
                    break;
                }
            }
            false
        }
        Some('?') => !text.is_empty() && text[0] != '/' && glob(&pattern[1..], &text[1..]),
        Some('[') => {
            let Some(close) = pattern.iter().skip(2).position(|ch| *ch == ']') else {
                return text.first() == Some(&'[') && glob(&pattern[1..], &text[1..]);
            };
            let class = &pattern[1..close + 2];
            let Some(&ch) = text.first() else {
                return false;
            };
            let (negate, class) = match class.first() {
                Some('!' | '^') => (true, &class[1..]),
                _ => (false, class),
            };
            let mut hit = false;
            let mut at = 0;
            while at < class.len() {
                if at + 2 < class.len() && class[at + 1] == '-' {
                    hit |= (class[at]..=class[at + 2]).contains(&ch);
                    at += 3;
                } else {
                    hit |= class[at] == ch;
                    at += 1;
                }
            }
            hit != negate && ch != '/' && glob(&pattern[close + 3..], &text[1..])
        }
        Some('\\') if pattern.len() > 1 => {
            text.first() == Some(&pattern[1]) && glob(&pattern[2..], &text[1..])
        }
        Some(ch) => text.first() == Some(ch) && glob(&pattern[1..], &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
        std::fs::write(path, text).expect("a file");
    }

    #[test]
    fn references_link_what_exists_and_leave_the_rest_as_text() {
        let workspace = tempfile::tempdir().expect("a directory");
        let root = workspace.path();
        write(root, "cmd/main.go", "package main\n");
        write(root, "a b.txt", "x");

        let text = "Why does $cmd/main.go:40-60 panic? See $cmd/, $\"a b.txt\". \
                    Not $HOME, not $1, not `$cmd/main.go`, nor cost$cmd/main.go; \
                    again $cmd/main.go:40-60 and $cmd/main.go.";
        let links = file_links(text, root);
        let labels: Vec<&str> = links.iter().map(|link| link.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "$cmd/main.go:40-60",
                "$cmd/",
                "$\"a b.txt\"",
                "$cmd/main.go"
            ]
        );
        assert_eq!(
            (
                links[0].path.as_str(),
                links[0].start_line,
                links[0].end_line
            ),
            ("cmd/main.go", 40, 60)
        );
        // Each link knows where it is in the text, what it is and how large.
        assert_eq!(&text[links[0].range.clone()], "$cmd/main.go:40-60");
        assert_eq!(&text[links[2].range.clone()], "$\"a b.txt\"");
        assert!(links[1].directory && !links[0].directory);
        assert_eq!(links[2].size, 1);
        // What the runner reads of a link is its label, path and lines.
        assert_eq!(
            serde_json::to_string(&links[3]).expect("JSON"),
            r#"{"label":"$cmd/main.go","path":"cmd/main.go"}"#
        );
        assert_eq!(
            serde_json::to_string(&links[0]).expect("JSON"),
            r#"{"label":"$cmd/main.go:40-60","path":"cmd/main.go","start_line":40,"end_line":60}"#
        );
        assert!(file_links("```\n$cmd/main.go\n```", root).is_empty());
        assert!(
            file_links("$cmd/:3", root).is_empty(),
            "a folder has no lines"
        );
        assert!(file_links("$cmd/main.go:60-40", root).is_empty());
    }

    #[test]
    fn labels_quote_paths_with_spaces() {
        assert_eq!(link_label("src/a.rs"), "$src/a.rs");
        assert_eq!(link_label("a b.txt"), "$\"a b.txt\"");
        assert_eq!(
            link_target(Path::new("/w"), "src/a.rs"),
            PathBuf::from("/w/src/a.rs")
        );
        assert_eq!(
            link_target(Path::new("/w"), "/etc/hosts"),
            PathBuf::from("/etc/hosts")
        );
    }

    #[test]
    fn the_reference_at_the_cursor_is_found_until_its_lines_are_typed() {
        let query = |before: &str| link_query(before).map(|q| (q.start, q.query, q.quoted));
        assert_eq!(query("look at $"), Some((8, String::new(), false)));
        assert_eq!(query("look at $src/ma"), Some((8, "src/ma".into(), false)));
        assert_eq!(query("($src"), Some((1, "src".into(), false)));
        assert_eq!(query("$\"a b"), Some((0, "a b".into(), true)));
        assert_eq!(query("cost$src"), None, "a $ inside a word is text");
        assert_eq!(query("$src/main.go:12"), None, "lines are being typed");
        assert_eq!(query("$src/main.go:12-1"), None);
        assert_eq!(query("`$src"), None, "a $ in code is text");
        assert_eq!(query("$src done"), None, "the reference ended");
        assert_eq!(query("пишу $фа"), Some((9, "фа".into(), false)));
        assert_eq!(query(""), None);
    }

    #[test]
    fn matches_rank_prefixes_names_and_letters_in_order() {
        let listing = Listing::of(
            [
                "src/main.rs",
                "src/lib.rs",
                "src/composer/view.rs",
                "docs/main-notes.md",
                "README.md",
                "Cargo.toml",
                "crates/kou-thread/src/files.rs",
                "a b/c d.txt",
            ]
            .map(String::from)
            .to_vec(),
        );
        let paths = |query: &str| -> Vec<String> {
            listing
                .matches(query, 60)
                .into_iter()
                .map(|found| found.path)
                .collect()
        };
        // Nothing typed: the top level, folders first, shorter names first,
        // as the cockpits list it.
        assert_eq!(
            paths(""),
            [
                "a b/",
                "src/",
                "docs/",
                "crates/",
                "README.md",
                "Cargo.toml"
            ]
        );
        // A folder with its slash: what is right inside it.
        assert_eq!(
            paths("src/"),
            ["src/composer/", "src/lib.rs", "src/main.rs"]
        );
        // A path that starts with it, then a name that does.
        assert_eq!(paths("src/ma"), ["src/main.rs"]);
        assert_eq!(paths("main")[..2], ["src/main.rs", "docs/main-notes.md"]);
        // Case does not matter.
        assert_eq!(paths("readme"), ["README.md"]);
        // Letters in order, near each other.
        assert_eq!(paths("cmpvw"), ["src/composer/view.rs"]);
        assert!(paths("zzz").is_empty());
        // Folders are entries too, with the slash.
        let found = listing.matches("compos", 60);
        assert_eq!(found[0].path, "src/composer/");
        assert!(found[0].directory);
        assert_eq!(paths("c d"), ["a b/c d.txt"]);
    }

    #[test]
    fn paths_from_the_root_or_home_list_that_folder() {
        let folder = tempfile::tempdir().expect("a directory");
        let root = folder.path().canonicalize().expect("a path");
        write(&root, "inner/x.txt", "x");
        write(&root, "notes.md", "n");
        write(&root, ".hidden", "h");
        let typed = format!("{}/", root.display());
        let found = complete_path(&typed, 10);
        let names: Vec<&str> = found
            .iter()
            .map(|found| &found.path[typed.len()..])
            .collect();
        assert_eq!(names, ["inner/", "notes.md"]);
        let dotted = complete_path(&format!("{typed}."), 10);
        assert_eq!(dotted.len(), 1);
        assert!(dotted[0].path.ends_with(".hidden"));
    }

    #[test]
    fn a_walk_keeps_to_gitignore_and_leaves_out_dependencies() {
        let folder = tempfile::tempdir().expect("a directory");
        let root = folder.path();
        write(
            root,
            ".gitignore",
            "# build output\n/target/\n*.log\n!keep.log\nsecret?.txt\n",
        );
        write(root, "src/main.rs", "fn main() {}");
        write(root, "src/debug.log", "x");
        write(root, "src/keep.log", "x");
        write(root, "src/.gitignore", "generated/\n");
        write(root, "src/generated/out.rs", "x");
        write(root, "target/debug/app", "x");
        write(
            root,
            "deep/target/kept.rs",
            "anchored rules match from their folder only",
        );
        write(root, "secret1.txt", "x");
        write(root, "secret12.txt", "x");
        write(root, "node_modules/left/index.js", "x");
        write(root, ".harness/sessions/a.jsonl", "x");
        let listing = list(root);
        assert!(!listing.from_git);
        assert_eq!(
            listing.entries(),
            [
                ".gitignore",
                "deep/",
                "deep/target/",
                "deep/target/kept.rs",
                "secret12.txt",
                "src/",
                "src/.gitignore",
                "src/keep.log",
                "src/main.rs",
            ]
        );
    }

    #[test]
    fn globs_follow_gitignore() {
        let matches = |pattern: &str, text: &str| {
            glob(
                &pattern.chars().collect::<Vec<_>>(),
                &text.chars().collect::<Vec<_>>(),
            )
        };
        assert!(matches("*.rs", "main.rs"));
        assert!(!matches("*.rs", "src/main.rs"), "* stays in one part");
        assert!(matches("**/*.rs", "src/a/main.rs"));
        assert!(matches("**/*.rs", "main.rs"));
        assert!(matches("src/**", "src/a/b"));
        assert!(matches("a/**/b", "a/b"));
        assert!(matches("a/**/b", "a/x/y/b"));
        assert!(matches("file[0-9].txt", "file7.txt"));
        assert!(!matches("file[!0-9].txt", "file7.txt"));
        assert!(matches("\\#notes", "#notes"));
        assert!(matches("?.md", "a.md"));
        assert!(!matches("?.md", "ab.md"));
    }

    #[test]
    fn a_work_tree_of_git_is_listed_as_git_sees_it() {
        let folder = tempfile::tempdir().expect("a directory");
        let root = folder.path();
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        if !git(&["init", "-q"]) {
            eprintln!("skipped: git is not available");
            return;
        }
        write(root, ".gitignore", "build/\n");
        write(root, "src/lib.rs", "x");
        write(root, "build/out.o", "x");
        write(root, ".harness/ui-history.json", "[]");
        write(root, "untracked.md", "x");
        let listing = list(root);
        assert!(listing.from_git);
        assert_eq!(
            listing.entries(),
            [".gitignore", "src/", "src/lib.rs", "untracked.md"]
        );
    }

    #[test]
    fn the_index_lists_off_the_calling_thread_and_says_when_it_is_done() {
        let folder = tempfile::tempdir().expect("a directory");
        write(folder.path(), "a.txt", "x");
        let index = FileIndex::new(folder.path());
        let listed = index.listed();
        let listing = index.wait(Duration::from_secs(10)).expect("listed");
        assert_eq!(listing.entries(), ["a.txt"]);
        // The channel closed when the listing was done, and one asked for
        // after is closed already.
        assert!(listed.recv_blocking().is_err());
        assert!(index.listed().recv_blocking().is_err());
        assert!(
            index.clone().listing().is_some(),
            "clones share the listing"
        );
    }

    #[test]
    fn a_hundred_thousand_files_match_quickly() {
        let files: Vec<String> = (0..100_000)
            .map(|n| format!("crates/crate-{}/src/module_{}/file_{n}.rs", n % 97, n % 13))
            .collect();
        let started = Instant::now();
        let listing = Listing::of(files);
        let built = started.elapsed();
        assert!(listing.len() > 100_000);
        let started = Instant::now();
        let found = listing.matches("file_4242", 60);
        let exact = started.elapsed();
        let started = Instant::now();
        let fuzzy = listing.matches("m3/f42", 60);
        let scattered = started.elapsed();
        assert_eq!(found[0].path, "crates/crate-71/src/module_4/file_4242.rs");
        assert!(!fuzzy.is_empty());
        // Unoptimised test builds are several times slower than the app; the
        // bound is for them.
        assert!(
            exact < Duration::from_millis(400) && scattered < Duration::from_millis(400),
            "matching took {exact:?} and {scattered:?} (listing {built:?})"
        );
    }
}
