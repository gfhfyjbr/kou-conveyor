use bumpalo::Bump;
use serde::{Deserialize, Serialize};
use std::rc::Rc;

type Text = Rc<str>;
fn empty_text<T: AsRef<str>>(s: &T) -> bool {
    s.as_ref().is_empty()
}
fn no_items<T>(s: &[T]) -> bool {
    s.is_empty()
}
fn zero(n: &i64) -> bool {
    *n == 0
}
fn no(b: &bool) -> bool {
    !*b
}

// The wire/display projection only: private fold state and Rev are not ported.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Entry {
    pub id: Text,
    pub kind: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub at: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub text: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub detail: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub phase: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub state: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub model: Text,
    pub forced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<Box<Tool>>,
    #[serde(skip_serializing_if = "no_items")]
    pub images: Vec<Image>,
    #[serde(skip_serializing_if = "no_items")]
    pub files: Vec<LinkedFile>,
    #[serde(skip_serializing_if = "empty_text")]
    pub replaces: Text,
    pub gone: bool,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Tool {
    pub call_id: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub name: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub input: Text,
    pub state: Text,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "empty_text")]
    pub output: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub stderr: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub error: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub started: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub finished: Text,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<Box<Image>>,
    #[serde(skip_serializing_if = "no_items")]
    pub syntax: Vec<i32>,
    #[serde(skip_serializing_if = "no_items")]
    pub calls: Vec<CodeCall>,
    #[serde(skip_serializing_if = "empty_text")]
    pub logs: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub value: Text,
    #[serde(skip_serializing_if = "no_items")]
    pub files: Vec<Text>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Box<FileView>>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Image {
    pub label: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub media_type: Text,
    pub width: i64,
    pub height: i64,
    pub size: i64,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LinkedFile {
    pub label: Text,
    pub path: Text,
    #[serde(skip_serializing_if = "no")]
    pub directory: bool,
    #[serde(skip_serializing_if = "zero")]
    pub from: i64,
    #[serde(skip_serializing_if = "zero")]
    pub to: i64,
    #[serde(skip_serializing_if = "zero")]
    pub lines: i64,
    #[serde(skip_serializing_if = "zero")]
    pub size: i64,
    #[serde(skip_serializing_if = "no")]
    pub binary: bool,
    #[serde(skip_serializing_if = "empty_text")]
    pub error: Text,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CodeCall {
    pub name: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub input: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub gist: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub output: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub error: Text,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub state: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub started: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub finished: Text,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FileView {
    pub kind: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub content: Text,
    #[serde(skip_serializing_if = "empty_text")]
    pub diff: Text,
    #[serde(skip_serializing_if = "zero")]
    pub line: i64,
    #[serde(skip_serializing_if = "no_items")]
    pub syntax: Vec<i32>,
    #[serde(skip_serializing_if = "zero")]
    pub replacements: i64,
}

#[derive(Serialize)]
pub struct ArenaEntry<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub at: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub text: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub detail: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub phase: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub state: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub model: &'a str,
    pub forced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<&'a mut ArenaTool<'a>>,
    #[serde(skip_serializing_if = "no_items")]
    pub images: &'a mut [ArenaImage<'a>],
    #[serde(skip_serializing_if = "no_items")]
    pub files: &'a mut [ArenaLinkedFile<'a>],
    #[serde(skip_serializing_if = "empty_text")]
    pub replaces: &'a str,
    pub gone: bool,
}

#[derive(Serialize)]
pub struct ArenaTool<'a> {
    pub call_id: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub name: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub input: &'a str,
    pub state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "empty_text")]
    pub output: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub stderr: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub error: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub started: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub finished: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<&'a mut ArenaImage<'a>>,
    #[serde(skip_serializing_if = "no_items")]
    pub syntax: &'a mut [i32],
    #[serde(skip_serializing_if = "no_items")]
    pub calls: &'a mut [ArenaCodeCall<'a>],
    #[serde(skip_serializing_if = "empty_text")]
    pub logs: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub value: &'a str,
    #[serde(skip_serializing_if = "no_items")]
    pub files: &'a mut [&'a str],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<&'a mut ArenaFileView<'a>>,
}

#[derive(Serialize)]
pub struct ArenaImage<'a> {
    pub label: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub media_type: &'a str,
    pub width: i64,
    pub height: i64,
    pub size: i64,
}

#[derive(Serialize)]
pub struct ArenaLinkedFile<'a> {
    pub label: &'a str,
    pub path: &'a str,
    #[serde(skip_serializing_if = "no")]
    pub directory: bool,
    #[serde(skip_serializing_if = "zero")]
    pub from: i64,
    #[serde(skip_serializing_if = "zero")]
    pub to: i64,
    #[serde(skip_serializing_if = "zero")]
    pub lines: i64,
    #[serde(skip_serializing_if = "zero")]
    pub size: i64,
    #[serde(skip_serializing_if = "no")]
    pub binary: bool,
    #[serde(skip_serializing_if = "empty_text")]
    pub error: &'a str,
}

#[derive(Serialize)]
pub struct ArenaCodeCall<'a> {
    pub name: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub input: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub gist: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub output: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub error: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub state: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub started: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub finished: &'a str,
}

#[derive(Serialize)]
pub struct ArenaFileView<'a> {
    pub kind: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub content: &'a str,
    #[serde(skip_serializing_if = "empty_text")]
    pub diff: &'a str,
    #[serde(skip_serializing_if = "zero")]
    pub line: i64,
    #[serde(skip_serializing_if = "no_items")]
    pub syntax: &'a mut [i32],
    #[serde(skip_serializing_if = "zero")]
    pub replacements: i64,
}

impl Image {
    fn arena(&self) -> ArenaImage<'_> {
        ArenaImage {
            label: &self.label,
            media_type: &self.media_type,
            width: self.width,
            height: self.height,
            size: self.size,
        }
    }
}

impl LinkedFile {
    fn arena(&self) -> ArenaLinkedFile<'_> {
        ArenaLinkedFile {
            label: &self.label,
            path: &self.path,
            directory: self.directory,
            from: self.from,
            to: self.to,
            lines: self.lines,
            size: self.size,
            binary: self.binary,
            error: &self.error,
        }
    }
}

impl CodeCall {
    fn arena(&self) -> ArenaCodeCall<'_> {
        ArenaCodeCall {
            name: &self.name,
            input: &self.input,
            gist: &self.gist,
            output: &self.output,
            error: &self.error,
            exit_code: self.exit_code,
            state: &self.state,
            started: &self.started,
            finished: &self.finished,
        }
    }
}

impl FileView {
    fn arena<'a>(&'a self, arena: &'a Bump) -> ArenaFileView<'a> {
        ArenaFileView {
            kind: &self.kind,
            content: &self.content,
            diff: &self.diff,
            line: self.line,
            syntax: arena.alloc_slice_copy(&self.syntax),
            replacements: self.replacements,
        }
    }
}

impl Tool {
    fn arena<'a>(&'a self, arena: &'a Bump) -> ArenaTool<'a> {
        ArenaTool {
            call_id: &self.call_id,
            name: &self.name,
            input: &self.input,
            state: &self.state,
            exit_code: self.exit_code,
            output: &self.output,
            stderr: &self.stderr,
            error: &self.error,
            started: &self.started,
            finished: &self.finished,
            image: self.image.as_ref().map(|v| arena.alloc(v.arena())),
            syntax: arena.alloc_slice_copy(&self.syntax),
            calls: arena.alloc_slice_fill_iter(self.calls.iter().map(CodeCall::arena)),
            logs: &self.logs,
            value: &self.value,
            files: arena.alloc_slice_fill_iter(self.files.iter().map(|v| v.as_ref())),
            change: self.change.as_ref().map(|v| arena.alloc(v.arena(arena))),
        }
    }
}

impl Entry {
    fn arena<'a>(&'a self, arena: &'a Bump) -> ArenaEntry<'a> {
        ArenaEntry {
            id: &self.id,
            kind: &self.kind,
            at: &self.at,
            text: &self.text,
            detail: &self.detail,
            phase: &self.phase,
            state: &self.state,
            model: &self.model,
            forced: self.forced,
            tool: self.tool.as_ref().map(|v| arena.alloc(v.arena(arena))),
            images: arena.alloc_slice_fill_iter(self.images.iter().map(Image::arena)),
            files: arena.alloc_slice_fill_iter(self.files.iter().map(LinkedFile::arena)),
            replaces: &self.replaces,
            gone: self.gone,
        }
    }
}

// Arena values own no Rc/String/Vec, so resetting the arena does not skip resource drops.
pub fn arena_clone<'a>(source: &'a [Entry], arena: &'a Bump) -> &'a mut [ArenaEntry<'a>] {
    arena.alloc_slice_fill_iter(source.iter().map(|entry| entry.arena(arena)))
}

pub fn summary(source: &[Entry]) -> [u64; 3] {
    let mut out = [source.len() as u64, 0, 0];
    for entry in source {
        out[1] += entry.id.len() as u64 + entry.text.len() as u64;
        if let Some(tool) = &entry.tool {
            out[2] += tool.syntax.len() as u64 + tool.calls.len() as u64 + tool.output.len() as u64;
        }
    }
    out
}

pub fn validate(source: &[Entry]) {
    let expected = serde_json::to_value(source).unwrap();
    let mut owned = source.to_vec();
    assert_eq!(serde_json::to_value(&owned).unwrap(), expected);
    let arena = Bump::new();
    let borrowed = arena_clone(source, &arena);
    assert_eq!(serde_json::to_value(&borrowed).unwrap(), expected);
    if let Some(entry) = owned.first_mut() {
        entry.text = "mutated".into();
    }
    if let Some(entry) = borrowed.first_mut() {
        entry.text = "mutated";
    }
    for entry in &mut owned {
        if let Some(tool) = &mut entry.tool {
            if let Some(run) = tool.syntax.first_mut() {
                *run ^= 1;
            }
            if let Some(call) = tool.calls.first_mut() {
                call.exit_code = Some(99);
            }
            if let Some(image) = &mut tool.image {
                image.width += 1;
            }
            if let Some(change) = &mut tool.change
                && let Some(run) = change.syntax.first_mut()
            {
                *run ^= 1;
            }
        }
    }
    for entry in borrowed.iter_mut() {
        if let Some(tool) = &mut entry.tool {
            if let Some(run) = tool.syntax.first_mut() {
                *run ^= 1;
            }
            if let Some(call) = tool.calls.first_mut() {
                call.exit_code = Some(99);
            }
            if let Some(image) = &mut tool.image {
                image.width += 1;
            }
            if let Some(change) = &mut tool.change
                && let Some(run) = change.syntax.first_mut()
            {
                *run ^= 1;
            }
        }
    }
    assert_eq!(
        serde_json::to_value(&owned).unwrap(),
        serde_json::to_value(&borrowed).unwrap()
    );
    assert_eq!(
        serde_json::to_value(source).unwrap(),
        expected,
        "clone mutation changed source"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_mutable_buffers_are_independent() {
        let source: Vec<Entry> = serde_json::from_str(r#"[{"id":"e","kind":"tool","tool":{"call_id":"c","state":"done","syntax":[1,2],"calls":[{"name":"Bash","state":"done","exit_code":0}],"image":{"label":"i","width":10},"change":{"kind":"edit","syntax":[3,4]}}}]"#).unwrap();
        validate(&source);
    }

    #[test]
    fn go_json_v2_omitempty_keeps_false_and_zero_numbers() {
        let entry = Entry {
            images: vec![Image::default()],
            ..Entry::default()
        };
        let value = serde_json::to_value(entry).unwrap();
        assert_eq!(value["forced"], false);
        assert_eq!(value["gone"], false);
        assert_eq!(value["images"][0]["width"], 0);
        assert_eq!(value["images"][0]["height"], 0);
        assert_eq!(value["images"][0]["size"], 0);
    }
}
