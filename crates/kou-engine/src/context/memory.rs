use super::*;
use std::fmt::Write;
pub const NOTES_LIMIT: usize = 32_000;
pub const NOTES_UPDATE_PREFIX: &str =
    "[harness] Your notes changed since the conversation last showed them.";
const NOTES_LEFT_OUT: &str = "from the middle of the notes are left out here";
type NotesReader = Arc<dyn Fn() -> std::io::Result<String> + Send + Sync>;
#[derive(Default)]
pub struct Memory {
    pub notes: String,
    pub read_notes: Option<NotesReader>,
    pub summaries: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(super) struct TouchedFile {
    pub path: String,
    pub read: bool,
    pub changed: bool,
}

pub fn shown_notes(notes: &str, path: &str) -> String {
    let notes = notes.trim();
    if notes.len() <= NOTES_LIMIT {
        return notes.into();
    }
    let mut head = NOTES_LIMIT * 2 / 3;
    let mut tail = notes.len() - NOTES_LIMIT / 3;
    // Find byte line breaks before correcting cuts to UTF-8 boundaries.
    if let Some(at) = notes.as_bytes()[..head]
        .iter()
        .rposition(|byte| *byte == b'\n')
        && at > head - 2000
    {
        head = at;
    }
    if let Some(at) = notes.as_bytes()[tail..]
        .iter()
        .position(|byte| *byte == b'\n')
        && at < 2000
    {
        tail += at + 1;
    }
    while !notes.is_char_boundary(head) {
        head -= 1;
    }
    while !notes.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n[… {} {NOTES_LEFT_OUT}, of {} in all: the whole file is {path} …]\n{}",
        &notes[..head],
        kilobytes(tail - head),
        kilobytes(notes.len()),
        &notes[tail..]
    )
}
pub fn shows_whole_notes(shown: &str) -> bool {
    !shown.contains(NOTES_LEFT_OUT)
}
pub fn notes_shown_by(message: &str) -> Option<&str> {
    if !message.starts_with(NOTES_UPDATE_PREFIX) {
        return None;
    }
    if let Some(start) = message.find("\n<notes>\n")
        && let Some(end) = message.rfind("\n</notes>")
        && end >= start + "\n<notes>\n".len()
    {
        return Some(&message[start + "\n<notes>\n".len()..end]);
    }
    Some("")
}
fn kilobytes(size: usize) -> String {
    if size < 10_000 {
        format!("{:.1} KB", size as f64 / 1000.0)
    } else {
        format!("{} KB", (size + 500) / 1000)
    }
}
impl Builder {
    pub(super) fn read_notes(&self) -> Option<String> {
        if self.memory.notes.is_empty() {
            return None;
        }
        Some(
            self.memory
                .read_notes
                .as_ref()
                .and_then(|read| read().ok())
                .map_or_else(String::new, |notes| shown_notes(&notes, &self.memory.notes)),
        )
    }
    pub fn notes_update(&self) -> Option<String> {
        let shown = self.shown_notes.as_ref()?;
        let notes = self.read_notes()?;
        if &notes == shown {
            return None;
        }
        if notes.is_empty() {
            Some(format!(
                "{NOTES_UPDATE_PREFIX} The file {} is empty now.",
                self.memory.notes
            ))
        } else {
            Some(format!(
                "{NOTES_UPDATE_PREFIX} They are in {}, and read now:\n<notes>\n{notes}\n</notes>",
                self.memory.notes
            ))
        }
    }
    pub(super) fn memory_text(&self, notes: Option<&str>) -> String {
        let mut text = String::new();
        if let Some(notes) = notes {
            if notes.is_empty() {
                write!(text, "\n\nYou keep notes across compactions in {}, which is empty so far. Write there what the summary may lose and you must not: the hypotheses you are testing, the evidence for and against them, the negative results, what you deferred and the next step. Each compaction shows the notes here, up to {}; past that, only their beginning and end.", self.memory.notes, kilobytes(NOTES_LIMIT)).unwrap();
            } else {
                write!(text, "\n\nYour notes, which you keep in {} across compactions; keep them current, since no compaction changes them:\n<notes>\n{notes}\n</notes>", self.memory.notes).unwrap();
            }
        }
        if !self.touched.is_empty() {
            text.push_str("\n\nFiles read or changed before the compaction, the latest first; read a file again before you change it:\n<files>\n");
            for file in &self.touched {
                let mut how = Vec::new();
                if file.read {
                    how.push("read");
                }
                if file.changed {
                    how.push("changed");
                }
                writeln!(text, "- {} ({})", file.path, how.join(", ")).unwrap();
            }
            text.push_str("</files>");
        }
        let details = "For anything exact the summary does not hold — code, an error message, a command's output, your own wording — ";
        let mut pointers = Vec::new();
        if self.offers("TranscriptSearch") {
            pointers.push(format!("{details}search the session's transcript with TranscriptSearch: it covers the whole conversation, compacted parts included."));
        } else if self.offers("Code") {
            pointers.push(format!("{details}search the session's transcript with transcriptSearch() in Code: it covers the whole conversation, compacted parts included."));
        } else if !self.transcript.is_empty() {
            pointers.push(format!("{details}search the session's transcript at {} with rg; it is JSON Lines, one session record per line, too large to read whole.", self.transcript));
        }
        if !self.memory.summaries.is_empty() {
            pointers.push(format!(
                "The summaries of this session's compactions, oldest first, are in {}.",
                self.memory.summaries
            ));
        }
        if !pointers.is_empty() {
            text.push_str("\n\n");
            text.push_str(&pointers.join(" "));
        }
        text
    }
    fn offers(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool.name == name)
    }
}
pub(super) fn remember_failures(items: &[Item], failed: &mut HashSet<String>) {
    for item in items {
        if let LlmData::ToolResult(result) = &item.data
            && result
                .output
                .first()
                .is_some_and(|part| part.value.starts_with("Error:"))
        {
            failed.insert(result.call_id.clone());
        }
    }
}
pub(super) fn touched_files(
    items: &[Item],
    earlier: &[TouchedFile],
    failures: &HashSet<String>,
) -> Vec<TouchedFile> {
    let mut failed = failures.clone();
    remember_failures(items, &mut failed);
    let mut files: Vec<TouchedFile> = Vec::new();
    let mut touch = |path: &str, changed: bool| {
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        let mut file = if let Some(index) = files.iter().position(|file| file.path == path) {
            files.remove(index)
        } else {
            TouchedFile {
                path: path.into(),
                ..TouchedFile::default()
            }
        };
        file.read |= !changed;
        file.changed |= changed;
        files.push(file);
    };
    const NAMES: &[&str] = &["Read", "Edit", "Write", "apply_patch"];
    for item in items {
        let LlmData::ToolCall(call) = &item.data else {
            continue;
        };
        if failed.contains(&call.call_id) {
            continue;
        }
        let name = if NAMES.contains(&call.name.as_str()) {
            call.name.as_str()
        } else {
            let aliases: Vec<&str> = NAMES
                .iter()
                .copied()
                .filter(|name| {
                    call.name
                        .match_indices('_')
                        .any(|(at, _)| at > 0 && call.name.get(at + 1..) == Some(name))
                })
                .collect();
            if aliases.len() != 1 {
                continue;
            }
            aliases[0]
        };
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Arguments {
            #[serde(deserialize_with = "input::zero")]
            path: String,
            #[serde(deserialize_with = "input::zero")]
            file_path: String,
            #[serde(deserialize_with = "input::zero")]
            input: String,
            #[serde(deserialize_with = "input::zero")]
            patch: String,
        }
        let Ok(args) = serde_json::from_str::<Arguments>(&call.arguments) else {
            continue;
        };
        let path = if args.path.is_empty() {
            &args.file_path
        } else {
            &args.path
        };
        match name {
            "Read" => touch(path, false),
            "Edit" | "Write" => touch(path, true),
            "apply_patch" => {
                let patch = if args.input.trim().is_empty() {
                    &args.patch
                } else {
                    &args.input
                };
                for line in patch.lines() {
                    for marker in [
                        "*** Add File: ",
                        "*** Update File: ",
                        "*** Delete File: ",
                        "*** Move to: ",
                    ] {
                        if let Some(rest) = line.trim().strip_prefix(marker) {
                            touch(rest, true);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    files.reverse();
    for file in earlier {
        if files.len() >= 20 {
            break;
        }
        if !files.iter().any(|value| value.path == file.path) {
            files.push(file.clone());
        }
    }
    files.truncate(20);
    files
}
