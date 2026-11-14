//! How a kou tool call looks in a thread: its one-line title, its kind, the
//! files it touches and what it produced. File edits become diffs, commands
//! become terminals, everything else is text.

use crate::{
    text::{clean, fenced, headline, language_of, parse_time, short_seconds},
    transcript::{Detail, Outcome, Tool, ToolState},
    wire,
};
use kou_thread::{Location, ToolCall, ToolContent, ToolKind, ToolStatus};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The longest title a call gets; the detail view has the rest.
const TITLE_LIMIT: usize = 120;

/// What a call is, by its tool's name or, without one, by what it ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flavor {
    Shell,
    Read,
    Edit,
    Write,
    Patch,
    Search,
    Image,
    Skill,
    Code,
    Other,
}

/// The call as a thread shows it, complete as of now.
pub(crate) fn present(tool: &Tool, cwd: &Path) -> ToolCall {
    let arguments = serde_json::from_str::<Value>(&tool.arguments).ok();
    let flavor = flavor_of(tool);
    let mut call = ToolCall {
        id: tool.call_id.clone(),
        status: match tool.state {
            ToolState::Queued => ToolStatus::Pending,
            ToolState::Running => ToolStatus::Running,
            ToolState::Done => ToolStatus::Done,
            ToolState::Failed => ToolStatus::Failed,
            ToolState::Canceled => ToolStatus::Cancelled,
        },
        kind: match flavor {
            Flavor::Shell | Flavor::Code => ToolKind::Execute,
            Flavor::Read | Flavor::Image => ToolKind::Read,
            Flavor::Edit | Flavor::Write | Flavor::Patch => ToolKind::Edit,
            Flavor::Search => ToolKind::Search,
            Flavor::Skill | Flavor::Other => ToolKind::Other,
        },
        ..ToolCall::default()
    };
    let argument = |names: &[&str]| -> Option<String> {
        let object = arguments.as_ref()?.as_object()?;
        names
            .iter()
            .find_map(|name| object.get(*name)?.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };

    match flavor {
        Flavor::Shell => {
            let command = argument(&["command"])
                .or_else(|| {
                    tool.outcomes.iter().find_map(|o| match &o.detail {
                        Detail::Shell { command, .. } => Some(command.clone()),
                        _ => None,
                    })
                })
                .unwrap_or_default();
            let gist = headline(&clean(script_gist(&command)), TITLE_LIMIT);
            call.title = if tool.verification() {
                format!("Verify: {gist}")
            } else {
                format!("Run {gist}")
            };
            for outcome in &tool.outcomes {
                shell_content(tool, outcome, &mut call);
            }
            if tool.outcomes.is_empty() && !tool.live.is_empty() {
                call.content.push(ToolContent::Terminal {
                    id: tool.call_id.clone(),
                    output: tool.live.clone(),
                    exit: None,
                });
            }
        }
        Flavor::Read | Flavor::Edit | Flavor::Write | Flavor::Patch | Flavor::Search => {
            file_call(tool, flavor, &argument, cwd, &mut call);
        }
        Flavor::Image => {
            let mut path = argument(&["path"]).unwrap_or_default();
            for outcome in &tool.outcomes {
                if let Detail::Image {
                    path: read,
                    described,
                    error,
                } = &outcome.detail
                {
                    path.clone_from(read);
                    if !described.is_empty() {
                        call.content.push(ToolContent::Text(described.clone()));
                    }
                    push_error(&mut call, error);
                }
            }
            call.title = format!("View {}", shown_path(&path, cwd));
            call.locations = locations(&[path], None, cwd);
        }
        Flavor::Skill => {
            let name = argument(&["name"]);
            for outcome in &tool.outcomes {
                if let Detail::Skill { path, error } = &outcome.detail {
                    if error.is_empty() && outcome.status == "completed" {
                        call.content
                            .push(ToolContent::Text(format!("Loaded `{}`", clean(path))));
                    }
                    push_error(&mut call, error);
                    call.locations = locations(std::slice::from_ref(path), None, cwd);
                }
            }
            call.title = match name {
                Some(name) => format!("Use skill {}", headline(&clean(&name), TITLE_LIMIT)),
                None => "Use a skill".to_owned(),
            };
        }
        Flavor::Code => code_call(tool, &argument, &mut call),
        Flavor::Other => {
            let gist = [
                "command",
                "path",
                "file_path",
                "name",
                "query",
                "code",
                "input",
            ]
            .iter()
            .find_map(|name| argument(&[name]))
            .unwrap_or_default();
            let name = if tool.name.is_empty() {
                "Tool call"
            } else {
                &tool.name
            };
            call.title = if gist.is_empty() {
                name.to_owned()
            } else {
                format!("{name}: {}", headline(&clean(&gist), TITLE_LIMIT))
            };
            for outcome in &tool.outcomes {
                if let Detail::Job { result, error } = &outcome.detail {
                    if !result.is_empty() {
                        call.content
                            .push(ToolContent::Text(fenced(&clean(result), "")));
                    }
                    push_error(&mut call, error);
                }
            }
        }
    }
    push_error(&mut call, &tool.call_error);
    call.raw_input = arguments.or_else(|| {
        Some(tool.arguments.as_str())
            .filter(|raw| !raw.is_empty())
            .map(|raw| Value::String(raw.to_owned()))
    });
    call
}

fn flavor_of(tool: &Tool) -> Flavor {
    match tool.name.to_ascii_lowercase().as_str() {
        "bash" => return Flavor::Shell,
        "read" => return Flavor::Read,
        "edit" => return Flavor::Edit,
        "write" => return Flavor::Write,
        "apply_patch" | "applypatch" => return Flavor::Patch,
        "transcriptsearch" => return Flavor::Search,
        "viewimage" => return Flavor::Image,
        "skilluse" => return Flavor::Skill,
        "code" => return Flavor::Code,
        _ => {}
    }
    // A plugin's tool, or a call known only by its status: what it ran says
    // what it is.
    match tool.outcomes.first().map(|outcome| &outcome.detail) {
        Some(Detail::Shell { .. }) => Flavor::Shell,
        Some(Detail::File(state)) => match state.action.as_str() {
            "read" => Flavor::Read,
            "edit" => Flavor::Edit,
            "write" => Flavor::Write,
            "patch" => Flavor::Patch,
            "transcript_search" => Flavor::Search,
            _ => Flavor::Other,
        },
        Some(Detail::Image { .. }) => Flavor::Image,
        Some(Detail::Skill { .. }) => Flavor::Skill,
        Some(Detail::Code(_)) => Flavor::Code,
        _ => Flavor::Other,
    }
}

fn shell_content(tool: &Tool, outcome: &Outcome, call: &mut ToolCall) {
    let Detail::Shell { result, error, .. } = &outcome.detail else {
        return;
    };
    match result {
        Some(result) => {
            let mut output = result.out.clone();
            if !result.err.is_empty() {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(&result.err);
            }
            call.content.push(ToolContent::Terminal {
                id: outcome.id.clone(),
                output,
                exit: Some(result.exit_code),
            });
            if result.timed_out {
                call.content.push(ToolContent::Text(
                    "The command was stopped at its timeout.".to_owned(),
                ));
            }
            call.raw_output = Some(json!({
                "exit_code": result.exit_code,
                "duration_seconds": result.duration,
                "timed_out": result.timed_out,
            }));
        }
        None if !tool.live.is_empty()
            && (tool.live_operation.is_empty() || tool.live_operation == outcome.id) =>
        {
            call.content.push(ToolContent::Terminal {
                id: outcome.id.clone(),
                output: tool.live.clone(),
                exit: None,
            });
        }
        None => {}
    }
    push_error(call, error);
}

fn file_call(
    tool: &Tool,
    flavor: Flavor,
    argument: &dyn Fn(&[&str]) -> Option<String>,
    cwd: &Path,
    call: &mut ToolCall,
) {
    let states: Vec<&wire::FileState> = tool
        .outcomes
        .iter()
        .filter_map(|outcome| match &outcome.detail {
            Detail::File(state) => Some(&**state),
            _ => None,
        })
        .collect();
    let state = states.first().copied();
    let path = state
        .map(|state| state.path.clone())
        .filter(|path| !path.is_empty())
        .or_else(|| argument(&["path", "file_path"]))
        .unwrap_or_default();
    let error = state.map(|state| clean(&state.terminal_error));
    let result = state.and_then(|state| state.result.as_ref());

    match flavor {
        Flavor::Read => {
            call.title = format!("Read {}", shown_path(&path, cwd));
            let line = state
                .map(|state| state.offset)
                .filter(|offset| *offset > 0)
                .and_then(|offset| u32::try_from(offset).ok());
            call.locations = locations(std::slice::from_ref(&path), line, cwd);
            if let Some(result) = result {
                call.content.push(ToolContent::Text(fenced(
                    &clean(&result.text),
                    language_of(&path),
                )));
            }
        }
        Flavor::Edit => {
            call.title = format!("Edit {}", shown_path(&path, cwd));
            let (old, new) = match state {
                Some(state) => (state.old_string.clone(), state.new_string.clone()),
                None => (
                    argument(&["old_string"]).unwrap_or_default(),
                    argument(&["new_string"]).unwrap_or_default(),
                ),
            };
            let line = result.and_then(|result| edit_line(&result.text));
            call.locations = locations(std::slice::from_ref(&path), line, cwd);
            if !old.is_empty() || !new.is_empty() {
                call.content.push(ToolContent::Diff {
                    path: absolute(&path, cwd),
                    old: Some(old),
                    new,
                });
            }
        }
        Flavor::Write => {
            call.title = format!("Write {}", shown_path(&path, cwd));
            call.locations = locations(std::slice::from_ref(&path), None, cwd);
            let content = match state {
                Some(state) => Some(state.content.clone()),
                None => argument(&["content"]),
            };
            if let Some(new) = content {
                call.content.push(ToolContent::Diff {
                    path: absolute(&path, cwd),
                    old: None,
                    new,
                });
            }
        }
        Flavor::Patch => {
            let patch = state
                .map(|state| state.patch.clone())
                .filter(|patch| !patch.is_empty())
                .or_else(|| argument(&["input", "patch"]))
                .unwrap_or_default();
            let files = parse_patch(&patch);
            let named: Vec<String> = files
                .iter()
                .map(|file| file.moved_to.clone().unwrap_or_else(|| file.path.clone()))
                .collect();
            call.title = if named.is_empty() {
                "Apply a patch".to_owned()
            } else {
                headline(
                    &format!(
                        "Patch {}",
                        named
                            .iter()
                            .map(|path| shown_path(path, cwd))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    TITLE_LIMIT,
                )
            };
            call.locations = locations(&named, None, cwd);
            for file in files {
                patch_content(file, cwd, call);
            }
        }
        Flavor::Search => {
            let query = state
                .map(|state| state.query.clone())
                .filter(|query| !query.is_empty())
                .or_else(|| argument(&["query"]))
                .unwrap_or_default();
            call.title = format!(
                "Search the transcript for {}",
                headline(&clean(&query), TITLE_LIMIT)
            );
            if let Some(result) = result {
                call.content
                    .push(ToolContent::Text(fenced(&clean(&result.text), "")));
            }
        }
        _ => {}
    }
    if let Some(error) = error {
        push_error(call, &error);
    }
}

fn code_call(tool: &Tool, argument: &dyn Fn(&[&str]) -> Option<String>, call: &mut ToolCall) {
    let states: Vec<&wire::CodeState> = tool
        .outcomes
        .iter()
        .filter_map(|outcome| match &outcome.detail {
            Detail::Code(state) => Some(&**state),
            _ => None,
        })
        .collect();
    let code = states
        .first()
        .map(|state| state.code.clone())
        .filter(|code| !code.is_empty())
        .or_else(|| argument(&["code"]))
        .unwrap_or_default();
    if !code.is_empty() {
        call.content
            .push(ToolContent::Text(fenced(&clean(&code), "js")));
    }
    let mut made = 0;
    for state in &states {
        let Some(result) = &state.result else {
            push_error(call, &clean(&state.terminal_error));
            continue;
        };
        let calls = result.calls.as_deref().unwrap_or_default();
        made += calls.len();
        if !calls.is_empty() {
            let ended = tool.state.terminal();
            let lines: Vec<String> = calls
                .iter()
                .enumerate()
                .map(|(index, made)| {
                    code_call_line(index + 1, made, &state.config.directory, ended)
                })
                .collect();
            call.content.push(ToolContent::Text(lines.join("\n")));
        }
        if !result.logs.trim().is_empty() {
            call.content.push(ToolContent::Text(format!(
                "Logged:\n\n{}",
                fenced(&clean(&result.logs), "")
            )));
        }
        if !result.value.is_empty() {
            call.content.push(ToolContent::Text(format!(
                "Returned:\n\n{}",
                fenced(&clean(&result.value), "")
            )));
        }
        push_error(call, &clean(&result.error));
        push_error(call, &clean(&state.terminal_error));
    }
    call.title = match made {
        0 => "Run code".to_owned(),
        1 => "Run code: 1 tool call".to_owned(),
        count => format!("Run code: {count} tool calls"),
    };
}

/// One tool call a script made, as a line of a numbered list.
fn code_call_line(number: usize, made: &wire::CodeCall, directory: &str, ended: bool) -> String {
    let name = clean(&made.name);
    let gist = headline(&code_gist(&name, &clean(&made.arguments), directory), 100);
    let mut facts = Vec::new();
    if made.running {
        facts.push(if ended { "canceled" } else { "running" }.to_owned());
    } else if !made.error.is_empty() {
        facts.push("failed".to_owned());
    }
    if let Some(code) = made.exit_code.filter(|code| *code != 0) {
        facts.push(format!("exit {code}"));
    }
    let took = made
        .started
        .as_deref()
        .and_then(parse_time)
        .zip(made.finished.as_deref().and_then(parse_time))
        .and_then(|(started, finished)| finished.duration_since(started).ok());
    if let Some(took) = took {
        facts.push(short_seconds(took.as_secs_f64()));
    }
    let mut line = format!("{number}. `{name}`");
    if !gist.is_empty() {
        line.push_str(&format!(" {gist}"));
    }
    if !facts.is_empty() {
        line.push_str(&format!(" ({})", facts.join(", ")));
    }
    if !made.error.is_empty() {
        line.push_str(&format!(": {}", headline(&clean(&made.error), 200)));
    }
    line
}

/// What a script's call worked on: the files a patch changes, a path as the
/// directory the script ran in names it, the first line of a command that
/// does something, or the first line of what the call was given.
fn code_gist(name: &str, input: &str, directory: &str) -> String {
    match name {
        "bash" => return script_gist(input).to_owned(),
        "applyPatch" => {
            let files: Vec<String> = parse_patch(input)
                .iter()
                .map(|file| {
                    let action = match file.kind {
                        PatchKind::Add => "Add",
                        PatchKind::Update => "Update",
                        PatchKind::Delete => "Delete",
                    };
                    format!("{action} {}", file.path)
                })
                .collect();
            if !files.is_empty() {
                return files.join(", ");
            }
        }
        "read" | "readText" | "write" | "edit" | "viewImage" => {
            if !directory.is_empty()
                && let Ok(relative) = Path::new(input).strip_prefix(directory)
            {
                return relative.display().to_string();
            }
        }
        _ => {}
    }
    input
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// The first line of a script that does something: comments and the options
/// a script sets its shell say nothing of what it does.
fn script_gist(script: &str) -> &str {
    let mut first = "";
    for line in script.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if first.is_empty() {
            first = line;
        }
        let options = (line.starts_with("set -") || line.starts_with("set +"))
            && !line.contains([';', '&', '|']);
        if !line.starts_with('#') && !options {
            return line;
        }
    }
    first
}

/// The line an edit says it changed its file at: "replaced 1 occurrence at
/// line 43", "replaced 4 occurrences, the first at line 43".
fn edit_line(said: &str) -> Option<u32> {
    let first = said.lines().next()?;
    let (_, after) = first.rsplit_once("at line ")?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn push_error(call: &mut ToolCall, error: &str) {
    if error.trim().is_empty() {
        return;
    }
    let text = ToolContent::Text(fenced(error, ""));
    if !call.content.contains(&text) {
        call.content.push(text);
    }
}

/// A path as the workspace names it, for titles.
fn shown_path(path: &str, cwd: &Path) -> String {
    let path = clean(path);
    match Path::new(&path).strip_prefix(cwd) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.display().to_string(),
        _ => headline(&path, TITLE_LIMIT),
    }
}

fn absolute(path: &str, cwd: &Path) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    }
}

fn locations(paths: &[String], line: Option<u32>, cwd: &Path) -> Vec<Location> {
    paths
        .iter()
        .filter(|path| !path.is_empty())
        .map(|path| Location {
            path: absolute(path, cwd),
            line,
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatchKind {
    Add,
    Update,
    Delete,
}

/// One file of an `apply_patch` patch.
#[derive(Debug, PartialEq, Eq)]
struct PatchFile {
    kind: PatchKind,
    path: String,
    moved_to: Option<String>,
    /// The text before and after, per hunk; an added file has one hunk
    /// without a before.
    hunks: Vec<(String, String)>,
}

/// Reads the files of a patch in the `*** Begin Patch` format. What cannot be
/// read is left out: the call's result says whether the patch applied.
fn parse_patch(patch: &str) -> Vec<PatchFile> {
    let mut files: Vec<PatchFile> = Vec::new();
    let mut hunk_open = false;
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            files.push(PatchFile {
                kind: PatchKind::Add,
                path: path.trim().to_owned(),
                moved_to: None,
                hunks: vec![(String::new(), String::new())],
            });
            hunk_open = true;
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            files.push(PatchFile {
                kind: PatchKind::Delete,
                path: path.trim().to_owned(),
                moved_to: None,
                hunks: Vec::new(),
            });
            hunk_open = false;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            files.push(PatchFile {
                kind: PatchKind::Update,
                path: path.trim().to_owned(),
                moved_to: None,
                hunks: Vec::new(),
            });
            hunk_open = false;
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if let Some(file) = files.last_mut() {
                file.moved_to = Some(path.trim().to_owned());
            }
        } else if line.starts_with("*** ") {
            // Begin Patch, End Patch and End of File carry no text.
        } else if line.starts_with("@@") {
            hunk_open = false;
        } else if let Some(file) = files.last_mut() {
            if file.kind == PatchKind::Delete {
                continue;
            }
            if !hunk_open {
                file.hunks.push((String::new(), String::new()));
                hunk_open = true;
            }
            let Some((old, new)) = file.hunks.last_mut() else {
                continue;
            };
            let (sign, rest) = match line.chars().next() {
                Some(sign @ ('+' | '-' | ' ')) => (sign, &line[1..]),
                _ => (' ', line),
            };
            if sign != '+' {
                old.push_str(rest);
                old.push('\n');
            }
            if sign != '-' {
                new.push_str(rest);
                new.push('\n');
            }
        }
    }
    files
}

fn patch_content(file: PatchFile, cwd: &Path, call: &mut ToolCall) {
    let target = file.moved_to.as_deref().unwrap_or(&file.path);
    if let Some(moved) = &file.moved_to {
        call.content.push(ToolContent::Text(format!(
            "Moved `{}` to `{}`",
            clean(&file.path),
            clean(moved)
        )));
    }
    match file.kind {
        PatchKind::Delete => call.content.push(ToolContent::Text(format!(
            "Deleted `{}`",
            clean(&file.path)
        ))),
        PatchKind::Add => {
            for (_, new) in file.hunks {
                call.content.push(ToolContent::Diff {
                    path: absolute(target, cwd),
                    old: None,
                    new,
                });
            }
        }
        PatchKind::Update => {
            for (old, new) in file.hunks {
                call.content.push(ToolContent::Diff {
                    path: absolute(target, cwd),
                    old: Some(old),
                    new,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Body, Transcript};

    fn tool_after(lines: &[&str]) -> ToolCall {
        let mut transcript = Transcript::new();
        for line in lines {
            transcript.apply(line);
        }
        let Body::Tool(tool) = &transcript.entries[0].body else {
            panic!("a tool entry");
        };
        present(tool, Path::new("/w"))
    }

    fn response(name: &str, arguments: &str) -> String {
        let arguments = serde_json::to_string(arguments).expect("a JSON string");
        format!(
            r#"{{"Sequence":1,"Kind":"model_response","Data":{{"TurnID":"t","Response":{{"Stop":"complete","Output":[{{"Type":"tool_call","Data":{{"CallID":"c1","Name":"{name}","Arguments":{arguments}}}}}]}}}}}}"#
        )
    }

    fn status(operation: &str) -> String {
        format!(
            r#"{{"Sequence":2,"Kind":"tool_call_status","Data":{{"CallID":"c1","Status":{{"Error":""}},"Operations":[{operation}]}}}}"#
        )
    }

    #[test]
    fn a_command_is_a_terminal_with_its_exit_status() {
        let call = tool_after(&[&response(
            "Bash",
            r##"{"command":"# build\nset -e\ncargo test --all"}"##,
        )]);
        assert_eq!(call.title, "Run cargo test --all");
        assert_eq!(
            (call.kind, call.status),
            (ToolKind::Execute, ToolStatus::Pending)
        );
        assert!(call.content.is_empty());

        let call = tool_after(&[
            &response("Bash", r#"{"command":"cargo test"}"#),
            &status(
                r#"{"ID":"op1","Type":"shell","Status":"failed","State":{"Input":{"Command":"cargo test"},"Result":{"Out":"running 1 test","Err":"\u001b[31merror\u001b[0m","ExitCode":101,"Duration":1.5}}}"#,
            ),
        ]);
        assert_eq!(call.status, ToolStatus::Failed);
        assert_eq!(
            call.content,
            [ToolContent::Terminal {
                id: "op1".into(),
                output: "running 1 test\n\u{1b}[31merror\u{1b}[0m".into(),
                exit: Some(101),
            }]
        );
        assert_eq!(call.raw_output.expect("a result")["exit_code"], 101);
    }

    #[test]
    fn an_edit_and_a_write_are_diffs_of_their_file() {
        let call = tool_after(&[
            &response(
                "Edit",
                r#"{"path":"src/a.rs","old_string":"one","new_string":"two"}"#,
            ),
            &status(
                r#"{"ID":"op1","Type":"file","Status":"completed","State":{"Action":"edit","Path":"/w/src/a.rs","OldString":"one","NewString":"two","Result":{"Text":"/w/src/a.rs: replaced 1 occurrence at line 43\n    43\ttwo","Files":["/w/src/a.rs"]}}}"#,
            ),
        ]);
        assert_eq!(call.title, "Edit src/a.rs");
        assert_eq!(call.kind, ToolKind::Edit);
        assert_eq!(
            call.locations,
            [Location {
                path: "/w/src/a.rs".into(),
                line: Some(43)
            }]
        );
        assert_eq!(
            call.content,
            [ToolContent::Diff {
                path: "/w/src/a.rs".into(),
                old: Some("one".into()),
                new: "two".into(),
            }]
        );

        let call = tool_after(&[&response("Write", r#"{"path":"b.txt","content":"hi\n"}"#)]);
        assert_eq!(call.title, "Write b.txt");
        assert_eq!(
            call.content,
            [ToolContent::Diff {
                path: "/w/b.txt".into(),
                old: None,
                new: "hi\n".into(),
            }]
        );
    }

    #[test]
    fn a_patch_is_a_diff_per_hunk_and_says_what_it_deleted() {
        let patch = "*** Begin Patch\n*** Add File: new.txt\n+hello\n*** Update File: a.rs\n*** Move to: b.rs\n@@ fn main\n context\n-old\n+new\n@@\n-x\n+y\n*** Delete File: gone.rs\n*** End Patch";
        let arguments = json!({ "input": patch }).to_string();
        let call = tool_after(&[&response("apply_patch", &arguments)]);
        assert_eq!(call.title, "Patch new.txt, b.rs, gone.rs");
        assert_eq!(
            call.content,
            [
                ToolContent::Diff {
                    path: "/w/new.txt".into(),
                    old: None,
                    new: "hello\n".into()
                },
                ToolContent::Text("Moved `a.rs` to `b.rs`".into()),
                ToolContent::Diff {
                    path: "/w/b.rs".into(),
                    old: Some("context\nold\n".into()),
                    new: "context\nnew\n".into()
                },
                ToolContent::Diff {
                    path: "/w/b.rs".into(),
                    old: Some("x\n".into()),
                    new: "y\n".into()
                },
                ToolContent::Text("Deleted `gone.rs`".into()),
            ]
        );
    }

    #[test]
    fn a_read_is_fenced_text_and_a_failure_shows_under_what_was_tried() {
        let call = tool_after(&[
            &response("Read", r#"{"path":"src/a.rs"}"#),
            &status(
                r#"{"ID":"op1","Type":"file","Status":"completed","State":{"Action":"read","Path":"/w/src/a.rs","Offset":10,"Result":{"Text":"    10\tfn a() {}","Lines":1}}}"#,
            ),
        ]);
        assert_eq!(
            (call.title.as_str(), call.kind),
            ("Read src/a.rs", ToolKind::Read)
        );
        assert_eq!(call.locations[0].line, Some(10));
        assert_eq!(
            call.content,
            [ToolContent::Text("```rust\n    10\tfn a() {}\n```".into())]
        );

        let call = tool_after(&[
            &response("Read", r#"{"path":"/etc/missing"}"#),
            &status(
                r#"{"ID":"op1","Type":"file","Status":"failed","State":{"Action":"read","Path":"/etc/missing","TerminalError":"no such file"}}"#,
            ),
        ]);
        assert_eq!(call.title, "Read /etc/missing");
        assert_eq!(call.status, ToolStatus::Failed);
        assert_eq!(
            call.content,
            [ToolContent::Text("```\nno such file\n```".into())]
        );
    }

    #[test]
    fn a_script_lists_the_calls_it_made() {
        let call = tool_after(&[
            &response("Code", r#"{"code":"await bash('go test')"}"#),
            &status(
                r#"{"ID":"op1","Type":"code","Status":"completed","State":{"Code":"await bash('go test')","Config":{"Directory":"/w"},"Result":{"Calls":[{"Index":0,"Name":"bash","Arguments":"set -e\ngo test ./...","Output":"ok","ExitCode":1,"Started":"2026-01-01T00:00:00Z","Finished":"2026-01-01T00:00:02.5Z"},{"Index":1,"Name":"read","Arguments":"/w/a.go","Output":"","Error":"gone","Started":"2026-01-01T00:00:03Z"}],"Logs":"done\n","Value":"3"}}}"#,
            ),
        ]);
        assert_eq!(call.title, "Run code: 2 tool calls");
        assert_eq!(call.kind, ToolKind::Execute);
        assert_eq!(
            call.content,
            [
                ToolContent::Text("```js\nawait bash('go test')\n```".into()),
                ToolContent::Text(
                    "1. `bash` go test ./... (exit 1, 2.5s)\n2. `read` a.go (failed): gone".into()
                ),
                ToolContent::Text("Logged:\n\n```\ndone\n```".into()),
                ToolContent::Text("Returned:\n\n```\n3\n```".into()),
            ]
        );
    }

    #[test]
    fn a_call_known_only_by_its_status_is_named_by_what_it_ran() {
        let call = tool_after(&[&status(
            r#"{"ID":"op1","Type":"shell","Status":"awaiting","Idempotency":{"harness":"verification"},"State":{"Input":{"Command":"make check"}}}"#,
        )]);
        assert_eq!(call.title, "Verify: make check");
        assert_eq!(call.status, ToolStatus::Running);

        let call = tool_after(&[&response("CanvasSend", r#"{"name":"n1","text":"hi"}"#)]);
        assert_eq!(
            (call.title.as_str(), call.kind),
            ("CanvasSend: n1", ToolKind::Other)
        );
    }
}
