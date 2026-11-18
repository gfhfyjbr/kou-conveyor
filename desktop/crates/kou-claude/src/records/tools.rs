//! A tool's call and its result as the conversation shows them. One
//! conversion for a call as it comes live and for one read back from a
//! record: both are Claude Code's `tool_use` and `tool_result` blocks.
//!
//! What a call looks like follows what Claude's ACP adapter made of it, which
//! is what the app showed until now: a command is a terminal, an edit a diff
//! of its file, a read the lines it read, a subagent the card of its agent.
//! The adapter worked from the same blocks and from the result Claude Code
//! keeps beside them (`toolUseResult`), which says more than the text the
//! model was given: the patch an edit made, a command's own output, that a
//! command or an agent went on in the background.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use kou_thread::{
    AgentWork, Location, PlanStatus, PlanStep, ToolCall, ToolContent, ToolKind, ToolPatch,
    ToolStatus,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The call a `tool_use` block makes: `id`, the tool's `name` and its
/// `input` (which may still be growing while the block is being written:
/// whatever of it is there is shown), and `parent`, the call of the agent
/// that started the subagent making it, when one did.
///
/// The call is said to run: Claude Code runs a call as soon as the model
/// has made it, unless it asks first, and a question makes it wait
/// ([`kou_thread::Update::Permission`]).
pub fn call(id: &str, name: &str, input: &Value, parent: Option<&str>) -> ToolCall {
    let name = own_name(name);
    let (title, kind) = title_and_kind(name, input);
    let mut call = ToolCall {
        id: id.to_owned(),
        title,
        kind,
        status: ToolStatus::Running,
        content: asked(id, name, input),
        locations: locations(name, input),
        raw_input: Some(input.clone()),
        parent: parent.map(str::to_owned),
        ..ToolCall::default()
    };
    if is_agent(name) {
        call.agent = Some(text(input, "subagent_type").unwrap_or_default().to_owned());
    }
    // Asked to run in the background: its call will be over once it is
    // started. (What its result says settles it; see `result`.)
    if (is_agent(name) || is_shell(name))
        && input.get("run_in_background") == Some(&Value::Bool(true))
    {
        call.work = AgentWork {
            background: true,
            ..AgentWork::default()
        };
    }
    call
}

/// What a `tool_result` block changes of its call: how it ended and what it
/// gave. `name` and `input` are the call's, `content` the block's `content`
/// (a string, or blocks of text and pictures), `structured` the result as
/// Claude Code keeps it beside the block (`tool_use_result` of the `user`
/// message: a patch for an edit, the output and exit code of a command,
/// `isAsync` for an agent started in the background), `failed` the block's
/// `is_error`.
pub fn result(
    id: &str,
    name: &str,
    input: &Value,
    content: &Value,
    structured: Option<&Value>,
    failed: bool,
) -> ToolPatch {
    let name = own_name(name);
    // A tool the API ran itself says a failure in what it gives back.
    let failed = failed || server_error(content).is_some();
    // Only an object says anything: a failure keeps a string there.
    let structured = structured.filter(|value| value.is_object());
    let said = plain_text(content);
    let mut patch = ToolPatch {
        id: id.to_owned(),
        status: Some(if failed {
            ToolStatus::Failed
        } else {
            ToolStatus::Done
        }),
        ..ToolPatch::default()
    };
    // The user said no to it, or stopped it: it did not fail of itself.
    if failed && refused(&said) {
        patch.status = Some(ToolStatus::Cancelled);
    }
    if is_shell(name) {
        shell_result(&mut patch, id, content, structured, failed, &said);
        return patch;
    }
    if failed {
        // What was asked stays in view, with why it did not happen.
        let mut shown = asked(id, name, input);
        if !said.is_empty() {
            shown.push(ToolContent::Text(fenced(&said, "")));
        }
        shown.extend(pictures(content));
        patch.content = Some(shown);
        return patch;
    }
    match name {
        "Read" | "NotebookRead" => read_result(&mut patch, input, content, structured, &said),
        "Edit" | "MultiEdit" | "Write" => {
            if let Some(structured) = structured {
                file_change(&mut patch, structured);
            }
        }
        "NotebookEdit" => {
            if let Some(structured) = structured {
                let path = text(structured, "notebook_path").or(text(input, "notebook_path"));
                let new = text(structured, "new_source").unwrap_or_default();
                let old = text(structured, "old_source");
                if let Some(path) = path
                    && (old.is_some() || !new.is_empty())
                {
                    patch.content = Some(vec![ToolContent::Diff {
                        path: PathBuf::from(path),
                        old: old.map(str::to_owned),
                        new: new.to_owned(),
                    }]);
                }
            }
        }
        "Glob" | "Grep" | "LS" | "ToolSearch" => {
            let listed = structured
                .filter(|_| name == "Glob")
                .and_then(|structured| structured.get("filenames")?.as_array())
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|names| !names.is_empty());
            let shown = listed.unwrap_or_else(|| said.clone());
            if !shown.trim().is_empty() {
                patch.content = Some(vec![ToolContent::Text(fenced(&shown, ""))]);
            }
        }
        "WebFetch" => {
            let answer = structured
                .and_then(|structured| text(structured, "result"))
                .map(str::to_owned)
                .unwrap_or_else(|| said.clone());
            if !answer.trim().is_empty() {
                patch.content = Some(vec![ToolContent::Text(answer)]);
            }
        }
        "WebSearch" => {
            let hits = structured
                .map(search_hits)
                .filter(|hits| !hits.is_empty())
                .or_else(|| server_hits(content));
            let shown = hits.unwrap_or_else(|| said.clone());
            if !shown.trim().is_empty() {
                patch.content = Some(vec![ToolContent::Text(shown)]);
            }
        }
        "code_execution" | "bash_code_execution" | "text_editor_code_execution" => {
            code_result(&mut patch, id, content, &said);
        }
        "Agent" | "Task" => agent_result(&mut patch, content, structured, &said),
        "AskUserQuestion" => {
            let answered = structured.map(answers).filter(|text| !text.is_empty());
            let shown = answered.unwrap_or_else(|| said.clone());
            if !shown.trim().is_empty() {
                patch.content = Some(vec![ToolContent::Text(shown)]);
            }
        }
        "ExitPlanMode" => patch.title = Some("Plan approved".to_owned()),
        // A confirmation for the model; the call says it all.
        "Skill" | "EnterPlanMode" | "TodoWrite" => {}
        "Monitor" | "Workflow" => {
            // Both are over once what they started runs, and say so.
            let started = structured.is_some_and(|structured| {
                structured.get("taskId").is_some()
                    || matches!(
                        text(structured, "status"),
                        Some("async_launched" | "remote_launched")
                    )
            });
            if started {
                patch.background = Some(true);
            }
            if !said.trim().is_empty() {
                patch.content = Some(vec![ToolContent::Text(fenced(&said, ""))]);
            }
        }
        _ => {
            let mut shown = asked(id, name, input);
            shown.extend(shown_result(content));
            patch.content = Some(shown);
        }
    }
    if patch.content.as_ref().is_none_or(Vec::is_empty) && !said.is_empty() {
        patch.raw_output = Some(Value::String(said));
    }
    patch
}

/// The plan a call of Claude Code's to-do tool states, when `name` is that
/// tool: it is shown as the conversation's plan, not as a call. The newer
/// task tools change a list the conversation keeps: [`Tasks`] follows it.
pub fn plan(name: &str, input: &Value) -> Option<Vec<PlanStep>> {
    if name != "TodoWrite" {
        return None;
    }
    let todos = input.get("todos")?.as_array()?;
    Some(
        todos
            .iter()
            .filter_map(|todo| {
                let content = text(todo, "content")?;
                let status = plan_status(text(todo, "status"));
                // What is being done is said as it is being done.
                let shown = match (status, text(todo, "activeForm")) {
                    (PlanStatus::Running, Some(active)) => active,
                    _ => content,
                };
                Some(PlanStep {
                    text: shown.to_owned(),
                    status,
                })
            })
            .collect(),
    )
}

/// Whether a call of the tool `name` is the plan's: Claude Code's to-do tool
/// and its task tools, which the conversation shows as its plan ([`plan`],
/// [`Tasks`]) and not as calls, as the adapter did.
pub fn is_plan(name: &str) -> bool {
    matches!(
        name,
        "TodoWrite" | "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet"
    )
}

/// The task list of a conversation, as Claude Code's task tools change it
/// (`TaskCreate`, `TaskUpdate`, `TaskList`): the plan it shows. A task is
/// known by the id its creation answered with, so the list follows the
/// tools' results and not their calls.
#[derive(Clone, Debug, Default)]
pub struct Tasks {
    tasks: Vec<Task>,
}

#[derive(Clone, Debug)]
struct Task {
    id: String,
    subject: String,
    status: PlanStatus,
    active: Option<String>,
}

impl Tasks {
    /// The plan after the result of a call of the task tool `name` (the
    /// arguments of [`result`]), when that result changed it.
    pub fn apply(
        &mut self,
        name: &str,
        input: &Value,
        content: &Value,
        structured: Option<&Value>,
        failed: bool,
    ) -> Option<Vec<PlanStep>> {
        if failed {
            return None;
        }
        let structured = structured.filter(|value| value.is_object());
        let said = plain_text(content);
        match name {
            "TaskCreate" => {
                let task = structured.and_then(|structured| structured.get("task"));
                let (id, subject) = match task.and_then(|task| text(task, "id")) {
                    Some(id) => (
                        id.to_owned(),
                        task.and_then(|task| text(task, "subject"))
                            .or(text(input, "subject"))?
                            .to_owned(),
                    ),
                    // What older versions answered: `Task #3 created successfully: Fix it`.
                    None => {
                        let rest = said.trim().strip_prefix("Task #")?;
                        let (id, subject) = rest.split_once(" created successfully: ")?;
                        (id.to_owned(), subject.to_owned())
                    }
                };
                self.tasks.retain(|task| task.id != id);
                self.tasks.push(Task {
                    id,
                    subject,
                    status: PlanStatus::Pending,
                    active: text(input, "activeForm").map(str::to_owned),
                });
            }
            "TaskUpdate" => {
                let id = text(input, "taskId")?;
                if let Some(structured) = structured {
                    let confirmed = structured.get("success") == Some(&Value::Bool(true))
                        && text(structured, "taskId") == Some(id);
                    if !confirmed {
                        return None;
                    }
                }
                if text(input, "status") == Some("deleted") {
                    self.tasks.retain(|task| task.id != id);
                } else {
                    let at = match self.tasks.iter().position(|task| task.id == id) {
                        Some(at) => at,
                        None => {
                            // A task created before what was read: known now.
                            let subject = text(input, "subject")?;
                            self.tasks.push(Task {
                                id: id.to_owned(),
                                subject: subject.to_owned(),
                                status: PlanStatus::Pending,
                                active: None,
                            });
                            self.tasks.len() - 1
                        }
                    };
                    let task = &mut self.tasks[at];
                    if let Some(subject) = text(input, "subject") {
                        subject.clone_into(&mut task.subject);
                    }
                    if let Some(status) = text(input, "status") {
                        task.status = plan_status(Some(status));
                    }
                    if let Some(active) = text(input, "activeForm") {
                        task.active = Some(active.to_owned());
                    }
                }
            }
            "TaskList" => {
                let listed =
                    match structured.and_then(|structured| structured.get("tasks")?.as_array()) {
                        Some(tasks) => tasks
                            .iter()
                            .filter_map(|task| {
                                Some((
                                    text(task, "id")?.to_owned(),
                                    text(task, "subject")?.to_owned(),
                                    plan_status(text(task, "status")),
                                ))
                            })
                            .collect(),
                        None => listed_tasks(&said)?,
                    };
                let before = std::mem::take(&mut self.tasks);
                for (id, subject, status) in listed {
                    let active = before
                        .iter()
                        .find(|task| task.id == id)
                        .and_then(|task| task.active.clone());
                    self.tasks.push(Task {
                        id,
                        subject,
                        status,
                        active,
                    });
                }
            }
            _ => return None,
        }
        Some(self.steps())
    }

    /// The plan the list is now.
    pub fn steps(&self) -> Vec<PlanStep> {
        self.tasks
            .iter()
            .map(|task| PlanStep {
                text: match (&task.active, task.status) {
                    (Some(active), PlanStatus::Running) => active.clone(),
                    _ => task.subject.clone(),
                },
                status: task.status,
            })
            .collect()
    }
}

/// The tasks a `TaskList` answered in text, as older versions did: a line
/// `#3 [in_progress] Fix it` each.
fn listed_tasks(said: &str) -> Option<Vec<(String, String, PlanStatus)>> {
    if said.trim() == "No tasks found" {
        return Some(Vec::new());
    }
    said.trim()
        .lines()
        .map(|line| {
            let rest = line.strip_prefix('#')?;
            let (id, rest) = rest.split_once(" [")?;
            let (status, subject) = rest.split_once("] ")?;
            if !matches!(status, "pending" | "in_progress" | "completed") {
                return None;
            }
            // `(owner)` and `[blocked by #1]` are said after the subject.
            let subject = match subject.rfind(" [blocked by ") {
                Some(at) if subject.ends_with(']') => &subject[..at],
                _ => subject,
            };
            Some((id.to_owned(), subject.to_owned(), plan_status(Some(status))))
        })
        .collect::<Option<Vec<_>>>()
        .filter(|tasks| !tasks.is_empty())
}

fn plan_status(status: Option<&str>) -> PlanStatus {
    match status {
        Some("in_progress") => PlanStatus::Running,
        Some("completed") => PlanStatus::Done,
        _ => PlanStatus::Pending,
    }
}

/// A call's title in place of its paths' full form: the parts under `cwd`
/// as paths from there, as the adapter wrote them. A command's stays as it
/// was run.
pub fn within(cwd: &Path, mut call: ToolCall) -> ToolCall {
    if call.kind != ToolKind::Execute {
        call.title = relative(cwd, &call.title);
    }
    call
}

/// `text` with the paths under `cwd` in it written from there.
pub(crate) fn relative(cwd: &Path, text: &str) -> String {
    let base = cwd.to_string_lossy();
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        return text.to_owned();
    }
    text.replace(&format!("{base}/"), "")
}

/// The Agent tool, and the name it had before.
pub(crate) fn is_agent(name: &str) -> bool {
    matches!(name, "Agent" | "Task")
}

fn is_shell(name: &str) -> bool {
    matches!(name, "Bash" | "PowerShell")
}

/// A non-empty string of `value`'s field `key`.
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)?
        .as_str()
        .filter(|text| !text.trim().is_empty())
}

fn number(value: &Value, key: &str) -> Option<i64> {
    value.get(key)?.as_i64()
}

fn title_and_kind(name: &str, input: &Value) -> (String, ToolKind) {
    let path = |keys: &[&str]| keys.iter().find_map(|key| text(input, key));
    match name {
        "Bash" | "PowerShell" => (
            text(input, "command").map_or_else(
                || "Run a command".to_owned(),
                |command| format!("Run {command}"),
            ),
            ToolKind::Execute,
        ),
        "Read" | "NotebookRead" => {
            let Some(file) = path(&["file_path", "notebook_path", "path"]) else {
                return ("Read a file".to_owned(), ToolKind::Read);
            };
            let offset = number(input, "offset").filter(|offset| *offset > 0);
            let range = match (offset, number(input, "limit").filter(|limit| *limit > 0)) {
                (from, Some(limit)) => {
                    let from = from.unwrap_or(1);
                    format!(" ({from} - {})", from.saturating_add(limit - 1))
                }
                (Some(from), None) => format!(" (from line {from})"),
                (None, None) => match text(input, "pages") {
                    Some(pages) => format!(" (pages {pages})"),
                    None => String::new(),
                },
            };
            (format!("Read {file}{range}"), ToolKind::Read)
        }
        "Edit" | "MultiEdit" => (
            path(&["file_path"])
                .map_or_else(|| "Edit a file".to_owned(), |file| format!("Edit {file}")),
            ToolKind::Edit,
        ),
        "Write" => (
            path(&["file_path", "path"])
                .map_or_else(|| "Write a file".to_owned(), |file| format!("Write {file}")),
            ToolKind::Edit,
        ),
        "NotebookEdit" => {
            let verb = match text(input, "edit_mode") {
                Some("insert") => "Insert",
                Some("delete") => "Delete",
                _ => "Edit",
            };
            let cell = text(input, "cell_id").map_or_else(String::new, |cell| format!(" {cell}"));
            let title = match path(&["notebook_path"]) {
                Some(file) => format!("{verb} cell{cell} in {file}"),
                None => format!("{verb} a notebook cell"),
            };
            (title, ToolKind::Edit)
        }
        "Glob" => {
            let pattern = text(input, "pattern").unwrap_or("files");
            let title = match path(&["path"]) {
                Some(folder) => format!("Find {pattern} in {folder}"),
                None => format!("Find {pattern}"),
            };
            (title, ToolKind::Search)
        }
        "Grep" => (grep_title(input), ToolKind::Search),
        "LS" => (
            path(&["path"]).map_or_else(|| "List files".to_owned(), |dir| format!("List {dir}")),
            ToolKind::Search,
        ),
        "ToolSearch" => (
            text(input, "query").map_or_else(
                || "Find tools".to_owned(),
                |query| format!("Find tools: {query}"),
            ),
            ToolKind::Search,
        ),
        "WebFetch" => {
            let from = number(input, "offset")
                .filter(|offset| *offset > 0)
                .map_or_else(String::new, |offset| format!(" (from char {offset})"));
            (
                text(input, "url").map_or_else(
                    || "Fetch a page".to_owned(),
                    |url| format!("Fetch {url}{from}"),
                ),
                ToolKind::Fetch,
            )
        }
        "WebSearch" => (
            text(input, "query").map_or_else(
                || "Search the web".to_owned(),
                |query| format!("Search \"{query}\""),
            ),
            ToolKind::Fetch,
        ),
        "Agent" | "Task" => {
            let fork = text(input, "subagent_type") == Some("fork");
            let effort = text(input, "effort")
                .filter(|_| !fork)
                .map_or_else(String::new, |effort| format!(" ({effort} effort)"));
            let described = text(input, "description").unwrap_or("Agent");
            (format!("{described}{effort}"), ToolKind::Think)
        }
        "TodoWrite" => ("Update to-dos".to_owned(), ToolKind::Think),
        "TaskCreate" => (
            text(input, "subject").map_or_else(
                || "Create a task".to_owned(),
                |subject| format!("Create task: {subject}"),
            ),
            ToolKind::Think,
        ),
        "TaskUpdate" => {
            let named = text(input, "subject")
                .map(str::to_owned)
                .or_else(|| text(input, "taskId").map(|id| format!("#{id}")));
            (
                named.map_or_else(
                    || "Update a task".to_owned(),
                    |named| format!("Update task: {named}"),
                ),
                ToolKind::Think,
            )
        }
        "TaskList" => ("List tasks".to_owned(), ToolKind::Think),
        "TaskGet" => (
            text(input, "taskId")
                .map_or_else(|| "Get a task".to_owned(), |id| format!("Get task #{id}")),
            ToolKind::Think,
        ),
        "AskUserQuestion" => {
            let questions = questions(input);
            let title = match questions.as_slice() {
                [only] => text(only, "question")
                    .unwrap_or("Asking for your input")
                    .to_owned(),
                _ => "Asking for your input".to_owned(),
            };
            (title, ToolKind::Other)
        }
        "ExitPlanMode" => ("Approve plan".to_owned(), ToolKind::Other),
        "EnterPlanMode" => ("Enter plan mode".to_owned(), ToolKind::Other),
        "Skill" => (
            text(input, "skill").map_or_else(
                || "Use a skill".to_owned(),
                |skill| format!("Skill {skill}"),
            ),
            ToolKind::Other,
        ),
        "Monitor" => (
            text(input, "description")
                .map_or_else(|| "Monitor".to_owned(), |what| format!("Monitor {what}")),
            ToolKind::Execute,
        ),
        "Workflow" => (
            text(input, "name").map_or_else(
                || "Run a workflow".to_owned(),
                |workflow| format!("Workflow {workflow}"),
            ),
            ToolKind::Other,
        ),
        "TaskStop" | "KillShell" | "KillBash" => (
            path(&["task_id", "shell_id"]).map_or_else(
                || "Stop a task".to_owned(),
                |task| format!("Stop task {task}"),
            ),
            ToolKind::Execute,
        ),
        "TaskOutput" | "BashOutput" => (
            path(&["task_id", "bash_id", "shell_id"]).map_or_else(
                || "Read a task's output".to_owned(),
                |task| format!("Read the output of {task}"),
            ),
            ToolKind::Read,
        ),
        // Code the API runs itself, in a sandbox of its own.
        "code_execution" => ("Run code".to_owned(), ToolKind::Execute),
        "bash_code_execution" => (
            text(input, "command").map_or_else(
                || "Run a command".to_owned(),
                |command| format!("Run {command}"),
            ),
            ToolKind::Execute,
        ),
        "text_editor_code_execution" => {
            let (verb, kind) = match text(input, "command") {
                Some("view") => ("Read", ToolKind::Read),
                Some("create") => ("Write", ToolKind::Edit),
                _ => ("Edit", ToolKind::Edit),
            };
            (
                path(&["path"])
                    .map_or_else(|| format!("{verb} a file"), |path| format!("{verb} {path}")),
                kind,
            )
        }
        "tool_search_tool_regex" | "tool_search_tool_bm25" => (
            text(input, "query").map_or_else(
                || "Find tools".to_owned(),
                |query| format!("Find tools \"{query}\""),
            ),
            ToolKind::Search,
        ),
        _ => match mcp_name(name) {
            Some((server, tool)) => (format!("{server}: {tool}"), ToolKind::Other),
            None if name.is_empty() => ("Tool".to_owned(), ToolKind::Other),
            None => (name.to_owned(), ToolKind::Other),
        },
    }
}

/// An MCP tool's name, `mcp__<server>__<tool>`, as people read it: the
/// server (without the prefix of a connector of claude.ai's) and the tool,
/// with spaces for underscores.
fn mcp_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    let server = server.strip_prefix("claude_ai_").unwrap_or(server);
    Some((server.replace('_', " "), tool.replace('_', " ")))
}

/// Grep's arguments as the command line they stand for, as the adapter
/// titled it.
fn grep_title(input: &Value) -> String {
    let mut title = String::from("grep");
    let flag = |key: &str| input.get(key) == Some(&Value::Bool(true));
    if flag("-i") {
        title.push_str(" -i");
    }
    if flag("-n") {
        title.push_str(" -n");
    }
    for key in ["-A", "-B", "-C"] {
        if let Some(lines) = number(input, key) {
            title.push_str(&format!(" {key} {lines}"));
        }
    }
    let mode = text(input, "output_mode").or_else(|| {
        let set = |key: &str| {
            matches!(input.get(key), Some(Value::Bool(true))) || text(input, key) == Some("true")
        };
        if set("-l") {
            Some("files_with_matches")
        } else if set("-c") {
            Some("count")
        } else {
            None
        }
    });
    match mode {
        Some("files_with_matches") => title.push_str(" -l"),
        Some("count") => title.push_str(" -c"),
        _ => {}
    }
    if let Some(limit) = number(input, "head_limit") {
        title.push_str(&format!(" | head -{limit}"));
    }
    if let Some(glob) = text(input, "glob") {
        title.push_str(&format!(" --include=\"{glob}\""));
    }
    if let Some(kind) = text(input, "type") {
        title.push_str(&format!(" --type={kind}"));
    }
    if flag("multiline") {
        title.push_str(" -P");
    }
    if let Some(pattern) = text(input, "pattern") {
        title.push_str(&format!(" \"{pattern}\""));
    }
    if let Some(path) = text(input, "path").or(text(input, "file_path")) {
        title.push(' ');
        title.push_str(path);
    }
    title
}

fn questions(input: &Value) -> Vec<&Value> {
    input
        .get("questions")
        .and_then(Value::as_array)
        .map(|questions| questions.iter().collect())
        .unwrap_or_default()
}

/// The places a call touches, from its arguments.
fn locations(name: &str, input: &Value) -> Vec<Location> {
    let at = |key: &str, line: Option<u32>| {
        text(input, key)
            .map(|path| {
                vec![Location {
                    path: PathBuf::from(path),
                    line,
                }]
            })
            .unwrap_or_default()
    };
    match name {
        "Read" | "NotebookRead" => {
            let line = number(input, "offset")
                .filter(|offset| *offset > 0)
                .and_then(|offset| u32::try_from(offset).ok());
            at("file_path", line)
        }
        "Edit" | "MultiEdit" => at("file_path", None),
        "Write" => match text(input, "file_path") {
            Some(_) => at("file_path", None),
            None => at("path", None),
        },
        "NotebookEdit" => at("notebook_path", None),
        "Glob" | "LS" => at("path", None),
        "Grep" => match text(input, "path") {
            Some(_) => at("path", None),
            None => at("file_path", None),
        },
        _ => Vec::new(),
    }
}

/// What a call shows before it has a result: the command's terminal, the
/// change an edit makes, what an agent or a fetch was asked, the plan to
/// approve, the questions asked; for an MCP tool or one nobody knows, its
/// arguments.
fn asked(id: &str, name: &str, input: &Value) -> Vec<ToolContent> {
    match name {
        "Bash" | "PowerShell" => vec![ToolContent::Terminal {
            id: id.to_owned(),
            output: String::new(),
            exit: None,
        }],
        "code_execution" => text(input, "code")
            .map(|code| vec![ToolContent::Text(fenced(code, "python"))])
            .unwrap_or_default(),
        "bash_code_execution"
        | "text_editor_code_execution"
        | "tool_search_tool_regex"
        | "tool_search_tool_bm25" => Vec::new(),
        "Edit" => {
            let Some(path) = text(input, "file_path") else {
                return Vec::new();
            };
            edit_diff(path, input).into_iter().collect()
        }
        "MultiEdit" => {
            let Some(path) = text(input, "file_path") else {
                return Vec::new();
            };
            input
                .get("edits")
                .and_then(Value::as_array)
                .map(|edits| {
                    edits
                        .iter()
                        .filter_map(|edit| edit_diff(path, edit))
                        .collect()
                })
                .unwrap_or_default()
        }
        "Write" => {
            let path = text(input, "file_path").or(text(input, "path"));
            let content = ["content", "file_text", "file_content"]
                .iter()
                .find_map(|key| input.get(*key)?.as_str());
            match (path, content) {
                (Some(path), Some(content)) => vec![ToolContent::Diff {
                    path: PathBuf::from(path),
                    old: None,
                    new: content.to_owned(),
                }],
                _ => Vec::new(),
            }
        }
        "NotebookEdit" => match text(input, "new_source") {
            Some(source) if text(input, "edit_mode") != Some("delete") => {
                vec![ToolContent::Text(fenced(source, ""))]
            }
            _ => Vec::new(),
        },
        "Agent" | "Task" => text(input, "prompt")
            .map(|prompt| vec![ToolContent::Text(prompt.to_owned())])
            .unwrap_or_default(),
        "WebFetch" => text(input, "prompt")
            .map(|prompt| vec![ToolContent::Text(prompt.to_owned())])
            .unwrap_or_default(),
        "ExitPlanMode" => text(input, "plan")
            .map(|plan| vec![ToolContent::Text(plan.to_owned())])
            .unwrap_or_default(),
        "AskUserQuestion" => {
            let asked: Vec<String> = questions(input)
                .into_iter()
                .filter_map(|question| {
                    let mut shown = text(question, "question")?.to_owned();
                    for option in question
                        .get("options")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(label) = text(option, "label") {
                            shown.push_str("\n- ");
                            shown.push_str(label);
                            if let Some(description) = text(option, "description") {
                                shown.push_str(": ");
                                shown.push_str(description);
                            }
                        }
                    }
                    Some(shown)
                })
                .collect();
            if asked.is_empty() {
                Vec::new()
            } else {
                vec![ToolContent::Text(asked.join("\n\n"))]
            }
        }
        "TodoWrite" => {
            let steps = plan(name, input).unwrap_or_default();
            if steps.is_empty() {
                return Vec::new();
            }
            let list = steps
                .iter()
                .map(|step| {
                    let mark = match step.status {
                        PlanStatus::Done => "[x]",
                        PlanStatus::Running => "[~]",
                        PlanStatus::Pending => "[ ]",
                    };
                    format!("- {mark} {}", step.text)
                })
                .collect::<Vec<_>>()
                .join("\n");
            vec![ToolContent::Text(list)]
        }
        "Monitor" => text(input, "command")
            .map(|command| vec![ToolContent::Text(fenced(command, "sh"))])
            .unwrap_or_default(),
        _ if known(name) => Vec::new(),
        // An MCP tool or one nobody knows: what it was given.
        _ => match input {
            Value::Object(fields) if fields.is_empty() => Vec::new(),
            Value::Null => Vec::new(),
            input => vec![ToolContent::Text(fenced(&input.to_string(), "json"))],
        },
    }
}

/// The tools whose calls say what they were given in their own way.
fn known(name: &str) -> bool {
    matches!(
        name,
        "Read"
            | "NotebookRead"
            | "Glob"
            | "Grep"
            | "LS"
            | "ToolSearch"
            | "WebSearch"
            | "TaskCreate"
            | "TaskUpdate"
            | "TaskList"
            | "TaskGet"
            | "EnterPlanMode"
            | "Skill"
            | "Workflow"
            | "TaskStop"
            | "KillShell"
            | "KillBash"
            | "TaskOutput"
            | "BashOutput"
    )
}

fn edit_diff(path: &str, edit: &Value) -> Option<ToolContent> {
    let old = edit
        .get("old_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let new = edit
        .get("new_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if old.is_empty() && new.is_empty() {
        return None;
    }
    Some(ToolContent::Diff {
        path: PathBuf::from(path),
        old: Some(old.to_owned()).filter(|old| !old.is_empty()),
        new: new.to_owned(),
    })
}

/// A command's result: its output in its terminal, with the code it exited
/// with when that is known. One handed to the background has not exited:
/// its result only says that it runs.
fn shell_result(
    patch: &mut ToolPatch,
    id: &str,
    content: &Value,
    structured: Option<&Value>,
    failed: bool,
    said: &str,
) {
    let backgrounded = structured
        .and_then(|structured| structured.get("backgroundTaskId"))
        .is_some_and(|task| !task.is_null());
    if backgrounded {
        patch.background = Some(true);
    }
    let shown_pictures = pictures(content);
    let own = structured.filter(|structured| {
        structured.get("stdout").is_some_and(Value::is_string)
            && structured.get("stderr").is_some_and(Value::is_string)
            && structured.get("isImage") != Some(&Value::Bool(true))
            && !backgrounded
    });
    // Claude Code takes a code other than 0 for a success only with a word on
    // what it means (grep's "no matches"): a plain success exited with 0.
    let mut exit = (!failed).then_some(0);
    let mut output = said.to_owned();
    if let Some(own) = own {
        let stdout = own["stdout"].as_str().unwrap_or_default();
        let stderr = own["stderr"].as_str().unwrap_or_default();
        output = [stdout, stderr]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if own.get("interrupted") == Some(&Value::Bool(true)) {
            output = [output.as_str(), "[Command was aborted before completion]"]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            exit = None;
            patch.status = Some(ToolStatus::Cancelled);
        }
        if own
            .get("returnCodeInterpretation")
            .is_some_and(|word| !word.is_null())
        {
            exit = None;
        }
        if let Some(saved) = text(own, "persistedOutputPath") {
            let size = number(own, "persistedOutputSize")
                .map_or_else(String::new, |size| format!(" ({size} bytes total)"));
            output.push_str(&format!(
                "\n[Output truncated{size}: full output saved to {saved}]"
            ));
        }
    }
    if failed && exit.is_none() {
        exit = said
            .strip_prefix("Exit code ")
            .and_then(|rest| rest.split('\n').next())
            .and_then(|code| code.trim().parse().ok());
    }
    if backgrounded {
        exit = None;
    }
    let mut shown = vec![ToolContent::Terminal {
        id: id.to_owned(),
        output,
        exit,
    }];
    shown.extend(shown_pictures);
    patch.content = Some(shown);
}

/// What a read gave: the lines it read, numbered from where it started; a
/// picture; or what Claude Code made of a notebook or a document.
fn read_result(
    patch: &mut ToolPatch,
    input: &Value,
    content: &Value,
    structured: Option<&Value>,
    said: &str,
) {
    let file = structured
        .filter(|structured| text(structured, "type") == Some("text"))
        .and_then(|structured| structured.get("file"));
    if let Some(file) = file
        && let Some(lines) = file.get("content").and_then(Value::as_str)
        && !lines.is_empty()
    {
        let start = number(file, "startLine")
            .or_else(|| number(input, "offset"))
            .unwrap_or(1)
            .max(1);
        let mut numbered = lines
            .strip_suffix('\n')
            .unwrap_or(lines)
            .split('\n')
            .enumerate()
            .map(|(at, line)| format!("{}\t{line}", start.saturating_add(at as i64)))
            .collect::<Vec<_>>()
            .join("\n");
        if file.get("truncatedByTokenCap") == Some(&Value::Bool(true)) {
            let detail = match (number(file, "numLines"), number(file, "totalLines")) {
                (Some(shown), Some(total)) => format!(": showing {shown} of {total} lines"),
                _ => String::new(),
            };
            numbered.push_str(&format!("\n[File truncated{detail}]"));
        }
        let path = text(file, "filePath")
            .or(text(input, "file_path"))
            .unwrap_or_default();
        patch.content = Some(vec![ToolContent::Text(fenced(
            &numbered,
            language_of(path),
        ))]);
        return;
    }
    let mut shown = Vec::new();
    if !said.trim().is_empty() {
        let path = text(input, "file_path").unwrap_or_default();
        shown.push(ToolContent::Text(fenced(said, language_of(path))));
    }
    shown.extend(pictures(content));
    // A picture Claude Code keeps beside the result only.
    if shown.is_empty()
        && let Some(file) = structured
            .filter(|structured| text(structured, "type") == Some("image"))
            .and_then(|structured| structured.get("file"))
        && let Some(picture) = picture(text(file, "type"), text(file, "base64"))
    {
        shown.push(picture);
    }
    if !shown.is_empty() {
        patch.content = Some(shown);
    }
}

/// What an edit or a write changed, from the patch Claude Code kept: a diff
/// for each of its hunks, at the line it starts on.
fn file_change(patch: &mut ToolPatch, structured: &Value) {
    let Some(path) = text(structured, "filePath") else {
        return;
    };
    let path = PathBuf::from(path);
    let mut shown = Vec::new();
    let mut places = Vec::new();
    for hunk in structured
        .get("structuredPatch")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut old = Vec::new();
        let mut new = Vec::new();
        for line in hunk
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Some(gone) = line.strip_prefix('-') {
                old.push(gone);
            } else if let Some(added) = line.strip_prefix('+') {
                new.push(added);
            } else if line.starts_with('\\') {
                // "\ No newline at end of file".
            } else {
                let kept = line.strip_prefix(' ').unwrap_or(line);
                old.push(kept);
                new.push(kept);
            }
        }
        if old.is_empty() && new.is_empty() {
            continue;
        }
        places.push(Location {
            path: path.clone(),
            line: number(hunk, "newStart").and_then(|line| u32::try_from(line).ok()),
        });
        shown.push(ToolContent::Diff {
            path: path.clone(),
            old: Some(old.join("\n")).filter(|old| !old.is_empty()),
            new: new.join("\n"),
        });
    }
    let written = structured.get("content").and_then(Value::as_str);
    match (text(structured, "type"), written) {
        // A new file: all of it is new.
        (Some("create"), Some(written)) if shown.is_empty() => {
            shown.push(ToolContent::Diff {
                path: path.clone(),
                old: None,
                new: written.to_owned(),
            });
            places.push(Location {
                path: path.clone(),
                line: None,
            });
        }
        // An overwrite Claude Code made no patch of: the file before and
        // after, or that it was too large to compare.
        (Some("update"), Some(written)) if shown.is_empty() => {
            match structured.get("originalFile").and_then(Value::as_str) {
                Some(before) => shown.push(ToolContent::Diff {
                    path: path.clone(),
                    old: Some(before.to_owned()),
                    new: written.to_owned(),
                }),
                None => shown.push(ToolContent::Text(format!(
                    "Updated `{}` (what it held before was too large to compare)",
                    path.display()
                ))),
            }
            places.push(Location {
                path: path.clone(),
                line: None,
            });
        }
        _ => {}
    }
    if !shown.is_empty() {
        patch.content = Some(shown);
        patch.locations = Some(places);
    }
}

/// What a subagent reported, or that it was left to run in the background.
fn agent_result(patch: &mut ToolPatch, content: &Value, structured: Option<&Value>, said: &str) {
    let status = structured.and_then(|structured| text(structured, "status"));
    let launched = structured.is_some_and(|structured| {
        structured.get("isAsync") == Some(&Value::Bool(true))
            || matches!(status, Some("async_launched" | "remote_launched"))
    });
    if launched {
        patch.background = Some(true);
        patch.content = Some(vec![ToolContent::Text(
            "Started in the background.".to_owned(),
        )]);
        return;
    }
    if status == Some("completed") {
        patch.background = Some(false);
    }
    patch.tokens = structured.and_then(|structured| structured.get("totalTokens")?.as_u64());
    // Its report without what Claude Code wrote round it for the model.
    let report: Option<String> = structured
        .filter(|_| status == Some("completed"))
        .and_then(|structured| structured.get("content")?.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| text(block, "text"))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .filter(|report| !report.trim().is_empty());
    let report = report.unwrap_or_else(|| unwrap_handback(&strip_agent_trailer(said)));
    let report = partial_note(&report);
    let mut shown = Vec::new();
    if !report.trim().is_empty() {
        shown.push(ToolContent::Text(report));
    }
    shown.extend(pictures(content));
    if !shown.is_empty() {
        patch.content = Some(shown);
    }
}

/// A subagent's raw report without the lines for the model at its end: a
/// `<usage>` block and an `agentId: ... (...)` line.
fn strip_agent_trailer(text: &str) -> String {
    let mut body = text.trim_end();
    if body.ends_with("</usage>")
        && let Some(open) = body.rfind("<usage>")
    {
        body = body[..open].trim_end();
    }
    let line_start = body.rfind('\n').map_or(0, |at| at + 1);
    let last = &body[line_start..];
    let is_id_line = last.strip_prefix("agentId: ").is_some_and(|rest| {
        rest.split_once(' ').is_some_and(|(id, note)| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-')
                && note.starts_with('(')
                && note.ends_with(')')
        })
    });
    if is_id_line {
        body = body[..line_start.saturating_sub(1)].trim_end();
    }
    body.to_owned()
}

/// The frame Claude Code puts round a subagent's report for the model
/// (`[Subagent hand-back] ...`, the report indented under it), taken away.
fn unwrap_handback(text: &str) -> String {
    const HEADER: &str = "[Subagent hand-back] ";
    let at = if text.starts_with(HEADER) {
        Some(0)
    } else {
        text.find(&format!("\n{HEADER}")).map(|at| at + 1)
    };
    let Some(at) = at else {
        return text.to_owned();
    };
    let Some(end) = text[at..].find('\n').map(|end| at + end) else {
        return text.to_owned();
    };
    if !text[at..end].ends_with("The report follows:") {
        return text.to_owned();
    }
    let dedent = |part: &str| {
        part.split('\n')
            .map(|line| line.strip_prefix("  ").unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let notes = dedent(&text[..at.saturating_sub(1)]);
    let report = dedent(&text[end + 1..]);
    if notes.trim().is_empty() {
        report
    } else {
        format!("{}\n\n{report}", notes.trim_end())
    }
}

/// The note Claude Code puts before the report of a subagent stopped at its
/// limit of turns, said for people.
fn partial_note(text: &str) -> String {
    let trimmed = text.strip_prefix("  ").unwrap_or(text);
    let Some(rest) = trimmed.strip_prefix("NOTE: this agent stopped at its ") else {
        return text.to_owned();
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !rest[digits..].starts_with("-turn limit before finishing.") {
        return text.to_owned();
    }
    const LABEL: &str = "[The agent stopped at its limit of turns: what follows is partial]";
    match text.find("\n\n") {
        Some(end) if !text[end + 2..].trim().is_empty() => {
            format!("{LABEL}\n\n{}", text[end + 2..].trim_start())
        }
        _ => LABEL.to_owned(),
    }
}

/// The name Claude Code's own tool of the same kind goes by, for one the
/// API runs itself (a `server_tool_use` block): the two are shown alike.
fn own_name(name: &str) -> &str {
    match name {
        "web_search" => "WebSearch",
        "web_fetch" => "WebFetch",
        other => other,
    }
}

/// Why a tool the API ran itself failed, when what it gave back says so:
/// its result is then an error of its own kind with a code, not `is_error`.
fn server_error(content: &Value) -> Option<&str> {
    let kind = content.get("type").and_then(Value::as_str)?;
    kind.ends_with("_error")
        .then(|| text(content, "error_code").unwrap_or("failed"))
}

/// The hits of a web search the API ran itself, as a list of links.
fn server_hits(content: &Value) -> Option<String> {
    let lines: Vec<String> = content
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("web_search_result"))
        .filter_map(|hit| {
            let (title, url) = (text(hit, "title")?, text(hit, "url")?);
            Some(format!("- [{}]({url})", title.replace(['[', ']'], "")))
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// What code the API ran in its sandbox gave: its output as a terminal's,
/// with how it ended; a file it showed, as text.
fn code_result(patch: &mut ToolPatch, id: &str, content: &Value, said: &str) {
    let part = |key: &str| text(content, key).unwrap_or_default();
    let (out, err) = (part("stdout"), part("stderr"));
    if content.get("return_code").is_some() || !out.is_empty() || !err.is_empty() {
        let exit = number(content, "return_code").map(|code| code as i32);
        let mut output = out.to_owned();
        if !err.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(err);
        }
        if exit.is_some_and(|code| code != 0) {
            patch.status = Some(ToolStatus::Failed);
        }
        patch.content = Some(vec![ToolContent::Terminal {
            id: id.to_owned(),
            output,
            exit,
        }]);
        return;
    }
    let shown = text(content, "content").unwrap_or(said);
    if !shown.trim().is_empty() {
        patch.content = Some(vec![ToolContent::Text(fenced(shown, ""))]);
    }
}

/// A web search's hits as a list of links, with what the model said
/// between them.
fn search_hits(structured: &Value) -> String {
    let mut lines = Vec::new();
    for entry in structured
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match entry {
            Value::String(said) if !said.trim().is_empty() => lines.push(said.trim().to_owned()),
            entry => {
                for hit in entry
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let (Some(title), Some(url)) = (text(hit, "title"), text(hit, "url")) {
                        lines.push(format!("- [{}]({url})", title.replace(['[', ']'], "")));
                    }
                }
            }
        }
    }
    lines.join("\n")
}

/// The answers to the questions asked, each under its question.
fn answers(structured: &Value) -> String {
    let Some(answers) = structured.get("answers").and_then(Value::as_object) else {
        return String::new();
    };
    answers
        .iter()
        .filter_map(|(question, answer)| {
            let answer = match answer {
                Value::String(answer) => answer.clone(),
                Value::Array(chosen) => chosen
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => return None,
            };
            Some(format!("**{}**\n{answer}", question.trim()))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Whether what a failed call said is that the user refused it or stopped
/// it, as Claude Code words that.
fn refused(said: &str) -> bool {
    let said = said.trim_start();
    said.starts_with("The user doesn't want to proceed with this tool use")
        || said.starts_with("The user doesn't want to take this action")
        || said.starts_with("[Request interrupted by user")
}

/// A result's text: the string, or its blocks of text one after another,
/// without the notes Claude Code adds for the model (`<system-reminder>`).
pub(crate) fn plain_text(content: &Value) -> String {
    let joined = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                Value::String(text) => Some(text.clone()),
                block if block.get("type").and_then(Value::as_str) == Some("text") => {
                    block.get("text").and_then(Value::as_str).map(str::to_owned)
                }
                block => other_block(block),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(_) => other_block(content).unwrap_or_default(),
        _ => String::new(),
    };
    strip_tag(&joined, "system-reminder").trim_end().to_owned()
}

/// A block of a result that is neither text nor a picture, in words.
fn other_block(block: &Value) -> Option<String> {
    let kind = block.get("type").and_then(Value::as_str)?;
    Some(match kind {
        "image" => return None,
        "document" => match text(block, "title") {
            Some(title) => format!("[document \"{title}\"]"),
            None => "[document]".to_owned(),
        },
        "tool_reference" => format!("Tool: {}", text(block, "tool_name").unwrap_or("?")),
        "web_search_result" => format!(
            "{} ({})",
            text(block, "title").unwrap_or_default(),
            text(block, "url").unwrap_or_default()
        ),
        // A page the API fetched itself: what it is, not all it holds.
        "web_fetch_result" => {
            let title = block
                .get("content")
                .and_then(|document| text(document, "title"))
                .unwrap_or_default();
            let url = text(block, "url").unwrap_or_default();
            if title.is_empty() {
                format!("Fetched {url}")
            } else {
                format!("Fetched {title} ({url})")
            }
        }
        kind if kind.ends_with("_error") => {
            format!(
                "Failed: {}",
                text(block, "error_code").unwrap_or("no reason given")
            )
        }
        _ => block.to_string(),
    })
}

/// The pictures among a result's blocks.
fn pictures(content: &Value) -> Vec<ToolContent> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("image"))
        .filter_map(|block| {
            let source = block.get("source")?;
            match text(source, "type") {
                Some("base64") => picture(text(source, "media_type"), text(source, "data")),
                _ => Some(ToolContent::Text(match text(source, "url") {
                    Some(url) => format!("[image: {url}]"),
                    None => "[image]".to_owned(),
                })),
            }
        })
        .collect()
}

fn picture(mime: Option<&str>, data: Option<&str>) -> Option<ToolContent> {
    let mime = mime.unwrap_or("image/png");
    match BASE64.decode(data?.trim()) {
        Ok(bytes) if !bytes.is_empty() => Some(ToolContent::Image {
            mime: mime.to_owned(),
            data: bytes.into(),
        }),
        _ => Some(ToolContent::Text(format!("[image: {mime}]"))),
    }
}

/// What a result of an MCP tool, or of one nobody knows, gave: its text as
/// it is (a JSON answer as code) and its pictures.
fn shown_result(content: &Value) -> Vec<ToolContent> {
    let said = plain_text(content);
    let mut shown = Vec::new();
    if !said.trim().is_empty() {
        let trimmed = said.trim();
        let json = (trimmed.starts_with('{') || trimmed.starts_with('['))
            && serde_json::from_str::<Value>(trimmed).is_ok();
        shown.push(ToolContent::Text(if json {
            fenced(trimmed, "json")
        } else {
            said
        }));
    }
    shown.extend(pictures(content));
    shown
}

/// `text` without each `<tag>...</tag>` in it.
pub(crate) fn strip_tag(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if !text.contains(&open) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let Some(length) = rest[start + open.len()..].find(&close) else {
            break;
        };
        out.push_str(&rest[..start]);
        rest = &rest[start + open.len() + length + close.len()..];
    }
    out.push_str(rest);
    out
}

/// `text` as a code block, in a fence longer than any run of backticks in it.
pub(crate) fn fenced(text: &str, language: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for character in text.chars() {
        run = if character == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!(
        "{fence}{language}\n{}\n{fence}",
        text.strip_suffix('\n').unwrap_or(text)
    )
}

/// The language a code block of the file `path` is in, for its colours.
fn language_of(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some((_, extension)) = name.rsplit_once('.') else {
        return "";
    };
    match extension.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "go" => "go",
        "py" => "python",
        "js" | "mjs" | "cjs" => "js",
        "ts" | "mts" | "cts" => "ts",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "json" | "jsonl" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" => "markdown",
        "sh" | "bash" | "zsh" => "sh",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "swift" => "swift",
        "java" => "java",
        "kt" => "kotlin",
        "html" => "html",
        "css" => "css",
        "sql" => "sql",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diff(path: &str, old: Option<&str>, new: &str) -> ToolContent {
        ToolContent::Diff {
            path: PathBuf::from(path),
            old: old.map(str::to_owned),
            new: new.to_owned(),
        }
    }

    #[test]
    fn a_command_is_a_terminal_titled_by_what_it_runs() {
        let input = json!({"command": "cargo test", "description": "Run the tests"});
        let made = call("toolu_1", "Bash", &input, None);
        assert_eq!(made.title, "Run cargo test");
        assert_eq!(made.kind, ToolKind::Execute);
        assert_eq!(made.status, ToolStatus::Running);
        assert_eq!(
            made.content,
            [ToolContent::Terminal {
                id: "toolu_1".into(),
                output: String::new(),
                exit: None
            }]
        );
        assert_eq!(made.raw_input, Some(input.clone()));
        assert!(!made.work.background);

        // Its own output, not the text the model got, and the code it exited with.
        let structured = json!({
            "stdout": "ok\n", "stderr": "warning: x", "interrupted": false,
            "noOutputExpected": false,
        });
        let done = result(
            "toolu_1",
            "Bash",
            &input,
            &json!("ok\nwarning: x<system-reminder>a note</system-reminder>"),
            Some(&structured),
            false,
        );
        assert_eq!(done.status, Some(ToolStatus::Done));
        assert_eq!(
            done.content,
            Some(vec![ToolContent::Terminal {
                id: "toolu_1".into(),
                output: "ok\n\nwarning: x".into(),
                exit: Some(0)
            }])
        );

        // A failure names its code first; one whose meaning is said has none.
        let failed = result(
            "toolu_1",
            "Bash",
            &input,
            &json!("Exit code 2\nboom"),
            None,
            true,
        );
        assert_eq!(failed.status, Some(ToolStatus::Failed));
        assert_eq!(
            failed.content,
            Some(vec![ToolContent::Terminal {
                id: "toolu_1".into(),
                output: "Exit code 2\nboom".into(),
                exit: Some(2)
            }])
        );
        let explained = json!({"stdout": "", "stderr": "", "interrupted": false,
            "returnCodeInterpretation": "No matches found"});
        let quiet = result(
            "toolu_1",
            "Bash",
            &input,
            &json!(""),
            Some(&explained),
            false,
        );
        assert!(matches!(
            quiet.content.as_deref(),
            Some([ToolContent::Terminal { exit: None, .. }])
        ));

        // Stopped while it ran.
        let stopped = json!({"stdout": "half", "stderr": "", "interrupted": true});
        let patch = result(
            "toolu_1",
            "Bash",
            &input,
            &json!("half"),
            Some(&stopped),
            false,
        );
        assert_eq!(patch.status, Some(ToolStatus::Cancelled));
        assert!(matches!(
            patch.content.as_deref(),
            Some([ToolContent::Terminal { output, exit: None, .. }])
                if output == "half\n[Command was aborted before completion]"
        ));
    }

    #[test]
    fn a_command_handed_to_the_background_is_over_and_runs_on() {
        let input = json!({"command": "cargo build", "run_in_background": true});
        let made = call("toolu_2", "Bash", &input, None);
        assert!(made.work.background, "asked to run in the background");

        let structured = json!({
            "stdout": "", "stderr": "", "interrupted": false,
            "backgroundTaskId": "bash_7",
        });
        let said = json!("Command running in background with ID: bash_7");
        let patch = result("toolu_2", "Bash", &input, &said, Some(&structured), false);
        assert_eq!(patch.background, Some(true));
        assert_eq!(patch.status, Some(ToolStatus::Done));
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Terminal {
                id: "toolu_2".into(),
                output: "Command running in background with ID: bash_7".into(),
                exit: None
            }])
        );
        // One that timed out into the background is the same.
        let timed = json!({"stdout": "", "stderr": "", "interrupted": false,
            "backgroundTaskId": "b8", "timedOutAfterMs": 120000});
        let plain = json!({"command": "sleep 900"});
        assert_eq!(
            result("t", "Bash", &plain, &json!("moved"), Some(&timed), false).background,
            Some(true)
        );
        assert_eq!(
            result("t", "Bash", &plain, &json!("x"), None, false).background,
            None
        );
    }

    #[test]
    fn a_read_names_its_file_and_lines_and_shows_them_numbered() {
        let input = json!({"file_path": "/w/src/main.rs", "offset": 10, "limit": 5});
        let made = call("r1", "Read", &input, None);
        assert_eq!(made.title, "Read /w/src/main.rs (10 - 14)");
        assert_eq!(made.kind, ToolKind::Read);
        assert_eq!(
            made.locations,
            [Location {
                path: "/w/src/main.rs".into(),
                line: Some(10)
            }]
        );
        let from = call(
            "r",
            "Read",
            &json!({"file_path": "/a.md", "offset": 7}),
            None,
        );
        assert_eq!(from.title, "Read /a.md (from line 7)");
        let whole = call("r", "Read", &json!({"file_path": "/a.md"}), None);
        assert_eq!(
            (whole.title.as_str(), whole.locations[0].line),
            ("Read /a.md", None)
        );
        // Numbers no record should hold are no trouble either.
        let huge = json!({"file_path": "/a.md", "offset": i64::MAX, "limit": i64::MAX});
        assert!(
            call("r", "Read", &huge, None)
                .title
                .starts_with("Read /a.md (")
        );
        let odd = json!({"type": "text", "file": {"content": "a\nb", "startLine": i64::MAX}});
        assert!(
            result("r", "Read", &huge, &json!("a"), Some(&odd), false)
                .content
                .is_some()
        );

        let structured = json!({"type": "text", "file": {
            "filePath": "/w/src/main.rs", "content": "fn a() {}\nfn b() {}\n",
            "numLines": 2, "startLine": 10, "totalLines": 40,
        }});
        let raw =
            json!("    10\tfn a() {}\n    11\tfn b() {}\n<system-reminder>x</system-reminder>");
        let patch = result("r1", "Read", &input, &raw, Some(&structured), false);
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text(
                "```rust\n10\tfn a() {}\n11\tfn b() {}\n```".into()
            )])
        );

        // A picture it read is shown as one.
        let picture = json!([{"type": "image", "source": {
            "type": "base64", "media_type": "image/png", "data": "iVBORw=="}}]);
        let shown = result(
            "r2",
            "Read",
            &json!({"file_path": "/a.png"}),
            &picture,
            None,
            false,
        );
        assert_eq!(
            shown.content,
            Some(vec![ToolContent::Image {
                mime: "image/png".into(),
                data: vec![0x89, b'P', b'N', b'G'].into()
            }])
        );
    }

    #[test]
    fn an_edit_is_a_diff_and_its_result_the_hunks_it_made() {
        let input = json!({"file_path": "/w/a.rs", "old_string": "x", "new_string": "y"});
        let made = call("e1", "Edit", &input, None);
        assert_eq!(
            (made.title.as_str(), made.kind),
            ("Edit /w/a.rs", ToolKind::Edit)
        );
        assert_eq!(made.content, [diff("/w/a.rs", Some("x"), "y")]);
        assert_eq!(made.locations[0].path, PathBuf::from("/w/a.rs"));

        let structured = json!({
            "filePath": "/w/a.rs", "oldString": "x", "newString": "y",
            "originalFile": null, "userModified": false, "replaceAll": false,
            "structuredPatch": [{"oldStart": 3, "oldLines": 3, "newStart": 3, "newLines": 3,
                "lines": [" fn a() {", "-    x", "+    y", " }"]}],
        });
        let patch = result(
            "e1",
            "Edit",
            &input,
            &json!("updated"),
            Some(&structured),
            false,
        );
        assert_eq!(
            patch.content,
            Some(vec![diff(
                "/w/a.rs",
                Some("fn a() {\n    x\n}"),
                "fn a() {\n    y\n}"
            )])
        );
        assert_eq!(
            patch.locations,
            Some(vec![Location {
                path: "/w/a.rs".into(),
                line: Some(3)
            }])
        );

        // A failed edit keeps what it meant to change, and says why.
        let failed = result("e1", "Edit", &input, &json!("String not found"), None, true);
        assert_eq!(failed.status, Some(ToolStatus::Failed));
        assert_eq!(
            failed.content,
            Some(vec![
                diff("/w/a.rs", Some("x"), "y"),
                ToolContent::Text("```\nString not found\n```".into())
            ])
        );
        // One the user refused was stopped, not failed.
        let refused = result(
            "e1",
            "Edit",
            &input,
            &json!(
                "The user doesn't want to proceed with this tool use. The tool use was rejected."
            ),
            None,
            true,
        );
        assert_eq!(refused.status, Some(ToolStatus::Cancelled));

        // The older tool of several edits at once: a diff each.
        let multi = json!({"file_path": "/w/b.rs", "edits": [
            {"old_string": "a", "new_string": "b"}, {"old_string": "", "new_string": "c"}]});
        assert_eq!(
            call("m", "MultiEdit", &multi, None).content,
            [diff("/w/b.rs", Some("a"), "b"), diff("/w/b.rs", None, "c")]
        );
    }

    #[test]
    fn a_write_is_a_new_file_or_the_change_it_made_to_one() {
        let input = json!({"file_path": "/w/new.txt", "content": "hello\n"});
        let made = call("w1", "Write", &input, None);
        assert_eq!(made.title, "Write /w/new.txt");
        assert_eq!(made.content, [diff("/w/new.txt", None, "hello\n")]);

        let created = json!({"type": "create", "filePath": "/w/new.txt", "content": "hello\n",
            "structuredPatch": [], "originalFile": null});
        let patch = result(
            "w1",
            "Write",
            &input,
            &json!("File created"),
            Some(&created),
            false,
        );
        assert_eq!(
            patch.content,
            Some(vec![diff("/w/new.txt", None, "hello\n")])
        );

        let updated = json!({"type": "update", "filePath": "/w/new.txt", "content": "hello\n",
            "structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1,
                "lines": ["-hi", "+hello"]}], "originalFile": "hi\n"});
        let patch = result("w1", "Write", &input, &json!("ok"), Some(&updated), false);
        assert_eq!(
            patch.content,
            Some(vec![diff("/w/new.txt", Some("hi"), "hello")])
        );

        // An overwrite without a patch: before and after.
        let whole = json!({"type": "update", "filePath": "/w/new.txt", "content": "b",
            "structuredPatch": [], "originalFile": "a"});
        let patch = result("w1", "Write", &input, &json!("ok"), Some(&whole), false);
        assert_eq!(
            patch.content,
            Some(vec![diff("/w/new.txt", Some("a"), "b")])
        );
    }

    #[test]
    fn a_notebook_edit_is_a_diff_of_its_cell() {
        let input = json!({"notebook_path": "/w/n.ipynb", "cell_id": "c1", "new_source": "x = 2"});
        let made = call("n1", "NotebookEdit", &input, None);
        assert_eq!(made.title, "Edit cell c1 in /w/n.ipynb");
        assert_eq!(made.content, [ToolContent::Text("```\nx = 2\n```".into())]);
        let structured = json!({"new_source": "x = 2", "old_source": "x = 1", "cell_type": "code",
            "language": "python", "edit_mode": "replace", "notebook_path": "/w/n.ipynb",
            "original_file": "{}", "updated_file": "{}"});
        let patch = result(
            "n1",
            "NotebookEdit",
            &input,
            &json!("ok"),
            Some(&structured),
            false,
        );
        assert_eq!(
            patch.content,
            Some(vec![diff("/w/n.ipynb", Some("x = 1"), "x = 2")])
        );
    }

    #[test]
    fn searches_say_what_they_look_for_and_list_what_they_found() {
        let glob = call(
            "g",
            "Glob",
            &json!({"pattern": "**/*.rs", "path": "/w"}),
            None,
        );
        assert_eq!(
            (glob.title.as_str(), glob.kind),
            ("Find **/*.rs in /w", ToolKind::Search)
        );
        let found = json!({"filenames": ["/w/a.rs", "/w/b.rs"], "numFiles": 2,
            "durationMs": 3, "truncated": false});
        let patch = result(
            "g",
            "Glob",
            &json!({}),
            &json!("/w/a.rs\n/w/b.rs"),
            Some(&found),
            false,
        );
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text("```\n/w/a.rs\n/w/b.rs\n```".into())])
        );

        let grep = call(
            "s",
            "Grep",
            &json!({"pattern": "fn main", "path": "src", "-i": true, "output_mode": "content", "glob": "*.rs"}),
            None,
        );
        assert_eq!(grep.title, "grep -i --include=\"*.rs\" \"fn main\" src");
        assert_eq!(grep.kind, ToolKind::Search);
        let listed = call(
            "s",
            "Grep",
            &json!({"pattern": "x", "output_mode": "files_with_matches"}),
            None,
        );
        assert_eq!(listed.title, "grep -l \"x\"");
    }

    #[test]
    fn the_web_is_fetched_and_searched() {
        let fetch = call(
            "f",
            "WebFetch",
            &json!({"url": "https://example.com", "prompt": "What is it?"}),
            None,
        );
        assert_eq!(
            (fetch.title.as_str(), fetch.kind),
            ("Fetch https://example.com", ToolKind::Fetch)
        );
        assert_eq!(fetch.content, [ToolContent::Text("What is it?".into())]);
        let answered = json!({"bytes": 10, "code": 200, "codeText": "OK",
            "result": "An example.", "durationMs": 5, "url": "https://example.com"});
        let patch = result(
            "f",
            "WebFetch",
            &json!({}),
            &json!("An example."),
            Some(&answered),
            false,
        );
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text("An example.".into())])
        );

        // The same two run by the API itself are shown alike.
        let own = call("s1", "web_search", &json!({"query": "rust gpui"}), None);
        assert_eq!(
            (own.title.as_str(), own.kind),
            ("Search \"rust gpui\"", ToolKind::Fetch)
        );
        let found = json!([
            {"type": "web_search_result", "title": "GPUI [docs]", "url": "https://gpui.rs",
                "encrypted_content": "..."},
            {"type": "web_search_result", "title": "Zed", "url": "https://zed.dev"}]);
        let patch = result("s1", "web_search", &json!({}), &found, None, false);
        assert_eq!(patch.status, Some(ToolStatus::Done));
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text(
                "- [GPUI docs](https://gpui.rs)\n- [Zed](https://zed.dev)".into()
            )])
        );
        // Its failure is said in what it gives back, with no `is_error`.
        let refused =
            json!({"type": "web_search_tool_result_error", "error_code": "max_uses_exceeded"});
        let patch = result(
            "s1",
            "web_search",
            &json!({"query": "x"}),
            &refused,
            None,
            false,
        );
        assert_eq!(patch.status, Some(ToolStatus::Failed));
        let page = json!({"type": "web_fetch_result", "url": "https://example.com",
            "content": {"type": "document", "title": "Example", "source": {"type": "text", "data": "long"}}});
        let fetched = call(
            "s2",
            "web_fetch",
            &json!({"url": "https://example.com"}),
            None,
        );
        assert_eq!(fetched.title, "Fetch https://example.com");
        let patch = result("s2", "web_fetch", &json!({}), &page, None, false);
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text(
                "Fetched Example (https://example.com)".into()
            )])
        );
        // Code it runs in its own sandbox: what was run, and what came out.
        let code = call("s3", "code_execution", &json!({"code": "print(1)"}), None);
        assert_eq!(
            (code.title.as_str(), code.kind),
            ("Run code", ToolKind::Execute)
        );
        assert_eq!(
            code.content,
            [ToolContent::Text("```python\nprint(1)\n```".into())]
        );
        let ran = json!({"type": "code_execution_result", "stdout": "1\n", "stderr": "", "return_code": 0});
        let patch = result("s3", "code_execution", &json!({}), &ran, None, false);
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Terminal {
                id: "s3".into(),
                output: "1\n".into(),
                exit: Some(0),
            }])
        );
        let broke = json!({"type": "bash_code_execution_result", "stdout": "", "stderr": "no such file", "return_code": 2});
        let patch = result(
            "s4",
            "bash_code_execution",
            &json!({"command": "cat x"}),
            &broke,
            None,
            false,
        );
        assert_eq!(patch.status, Some(ToolStatus::Failed));

        let search = call("w", "WebSearch", &json!({"query": "rust gpui"}), None);
        assert_eq!(
            (search.title.as_str(), search.kind),
            ("Search \"rust gpui\"", ToolKind::Fetch)
        );
        let hits = json!({"query": "rust gpui", "durationSeconds": 1.0, "results": [
            {"tool_use_id": "srv", "content": [{"title": "GPUI", "url": "https://gpui.rs"}]},
            "Some commentary."]});
        let patch = result(
            "w",
            "WebSearch",
            &json!({}),
            &json!("dump"),
            Some(&hits),
            false,
        );
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text(
                "- [GPUI](https://gpui.rs)\nSome commentary.".into()
            )])
        );
    }

    #[test]
    fn an_agent_is_its_description_its_kind_and_in_the_end_its_report() {
        let input = json!({"description": "Check the tests", "prompt": "Run them all",
            "subagent_type": "Explore"});
        let made = call("a1", "Agent", &input, None);
        assert_eq!(made.title, "Check the tests");
        assert_eq!(
            (made.kind, made.agent.as_deref()),
            (ToolKind::Think, Some("Explore"))
        );
        assert_eq!(made.content, [ToolContent::Text("Run them all".into())]);
        // Its older name, no kind named, and an effort asked for.
        let task = call(
            "a2",
            "Task",
            &json!({"description": "Look", "effort": "high"}),
            Some("a1"),
        );
        assert_eq!(task.title, "Look (high effort)");
        assert_eq!(
            (task.agent.as_deref(), task.parent.as_deref()),
            (Some(""), Some("a1"))
        );

        let report = json!({"status": "completed", "agentId": "x", "prompt": "Run them all",
            "content": [{"type": "text", "text": "All 12 pass."}],
            "totalToolUseCount": 3, "totalDurationMs": 900, "totalTokens": 48211, "usage": {}});
        let raw = json!([{"type": "text", "text": "All 12 pass.\nagentId: x (use SendMessage to go on)\n<usage>total_tokens: 1</usage>"}]);
        let patch = result("a1", "Agent", &input, &raw, Some(&report), false);
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text("All 12 pass.".into())])
        );
        assert_eq!((patch.tokens, patch.background), (Some(48211), Some(false)));

        // Without the structured report, the raw one loses what was for the model.
        let patch = result("a1", "Agent", &input, &raw, None, false);
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text("All 12 pass.".into())])
        );
        let framed = "[Subagent hand-back] The text below is the final report. The report follows:\n  Done.\n  - one";
        assert_eq!(unwrap_handback(framed), "Done.\n- one");

        // Started in the background: its call is over, the agent is not.
        let launched = json!({"status": "async_launched", "isAsync": true, "agentId": "x",
            "description": "Check the tests", "prompt": "Run them all", "outputFile": "/tmp/x"});
        let patch = result(
            "a1",
            "Agent",
            &input,
            &json!("Async agent launched"),
            Some(&launched),
            false,
        );
        assert_eq!(patch.background, Some(true));
        assert_eq!(patch.status, Some(ToolStatus::Done));
        let asked = call(
            "a3",
            "Agent",
            &json!({"description": "d", "run_in_background": true}),
            None,
        );
        assert!(asked.work.background);
    }

    #[test]
    fn the_to_do_tool_is_a_plan_and_the_task_tools_keep_a_list() {
        let input = json!({"todos": [
            {"content": "Read", "status": "completed", "activeForm": "Reading"},
            {"content": "Write", "status": "in_progress", "activeForm": "Writing"},
            {"content": "Test", "status": "pending", "activeForm": "Testing"}]});
        let step = |text: &str, status| PlanStep {
            text: text.into(),
            status,
        };
        assert_eq!(
            plan("TodoWrite", &input),
            Some(vec![
                step("Read", PlanStatus::Done),
                step("Writing", PlanStatus::Running),
                step("Test", PlanStatus::Pending),
            ])
        );
        assert_eq!(plan("Bash", &input), None);
        assert!(is_plan("TodoWrite") && is_plan("TaskUpdate") && !is_plan("Task"));
        assert_eq!(call("t", "TodoWrite", &input, None).title, "Update to-dos");

        let mut tasks = Tasks::default();
        let create =
            json!({"subject": "Fix it", "description": "the bug", "activeForm": "Fixing it"});
        let made = json!({"task": {"id": "1", "subject": "Fix it"}});
        assert_eq!(
            tasks.apply("TaskCreate", &create, &json!("ok"), Some(&made), false),
            Some(vec![step("Fix it", PlanStatus::Pending)])
        );
        // An older answer in words.
        let older = json!({"subject": "Ship"});
        assert_eq!(
            tasks
                .apply(
                    "TaskCreate",
                    &older,
                    &json!("Task #2 created successfully: Ship"),
                    None,
                    false
                )
                .map(|steps| steps.len()),
            Some(2)
        );
        let update = json!({"taskId": "1", "status": "in_progress"});
        let confirmed = json!({"success": true, "taskId": "1", "updatedFields": ["status"]});
        assert_eq!(
            tasks.apply("TaskUpdate", &update, &json!("ok"), Some(&confirmed), false),
            Some(vec![
                step("Fixing it", PlanStatus::Running),
                step("Ship", PlanStatus::Pending)
            ])
        );
        // One that did not take changes nothing.
        let refused = json!({"success": false, "taskId": "1", "updatedFields": []});
        assert_eq!(
            tasks.apply("TaskUpdate", &update, &json!("no"), Some(&refused), false),
            None
        );
        let listed = json!({"tasks": [{"id": "2", "subject": "Ship", "status": "completed", "blockedBy": []}]});
        assert_eq!(
            tasks.apply("TaskList", &json!({}), &json!(""), Some(&listed), false),
            Some(vec![step("Ship", PlanStatus::Done)])
        );
        let deleted = json!({"taskId": "2", "status": "deleted"});
        assert_eq!(
            tasks.apply("TaskUpdate", &deleted, &json!("ok"), None, false),
            Some(vec![])
        );
        assert_eq!(
            tasks.apply(
                "TaskList",
                &json!({}),
                &json!("#5 [in_progress] Land it (bob)"),
                None,
                false
            ),
            Some(vec![step("Land it (bob)", PlanStatus::Running)])
        );
    }

    #[test]
    fn questions_plans_and_skills_say_what_they_are() {
        let ask = json!({"questions": [{"question": "Which one?", "header": "Pick",
            "multiSelect": false, "options": [
                {"label": "A", "description": "the first"}, {"label": "B", "description": "the second"}]}]});
        let made = call("q", "AskUserQuestion", &ask, None);
        assert_eq!(made.title, "Which one?");
        assert_eq!(
            made.content,
            [ToolContent::Text(
                "Which one?\n- A: the first\n- B: the second".into()
            )]
        );
        let answered = json!({"questions": [], "answers": {"Which one?": "B"}});
        let patch = result(
            "q",
            "AskUserQuestion",
            &ask,
            &json!("User answered"),
            Some(&answered),
            false,
        );
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text("**Which one?**\nB".into())])
        );

        let exit = json!({"plan": "1. Do it\n2. Check it"});
        let made = call("p", "ExitPlanMode", &exit, None);
        assert_eq!(made.title, "Approve plan");
        assert_eq!(
            made.content,
            [ToolContent::Text("1. Do it\n2. Check it".into())]
        );
        let patch = result(
            "p",
            "ExitPlanMode",
            &exit,
            &json!("User has approved"),
            None,
            false,
        );
        assert_eq!(patch.title.as_deref(), Some("Plan approved"));
        assert_eq!(patch.content, None, "the plan stays as it was shown");

        let skill = call("k", "Skill", &json!({"skill": "review"}), None);
        assert_eq!(
            (skill.title.as_str(), skill.kind),
            ("Skill review", ToolKind::Other)
        );
        let patch = result(
            "k",
            "Skill",
            &json!({"skill": "review"}),
            &json!("Launching skill: review"),
            None,
            false,
        );
        assert_eq!(patch.content, None);
        assert_eq!(patch.raw_output, Some(json!("Launching skill: review")));
    }

    #[test]
    fn an_mcp_tool_and_an_unknown_one_show_their_input_and_their_result() {
        let input = json!({"query": "rust", "limit": 3});
        let made = call("m", "mcp__claude_ai_Claude_Docs__read_doc", &input, None);
        assert_eq!(made.title, "Claude Docs: read doc");
        assert_eq!(made.kind, ToolKind::Other);
        let shown_input =
            ToolContent::Text("```json\n{\"query\":\"rust\",\"limit\":3}\n```".into());
        assert_eq!(made.content, std::slice::from_ref(&shown_input));
        let patch = result(
            "m",
            "mcp__claude_ai_Claude_Docs__read_doc",
            &input,
            &json!([{"type": "text", "text": "Found **two**."}]),
            None,
            false,
        );
        assert_eq!(
            patch.content,
            Some(vec![
                shown_input,
                ToolContent::Text("Found **two**.".into())
            ])
        );
        // A JSON answer is shown as code.
        let patch = result(
            "m",
            "mcp__s__t",
            &json!({}),
            &json!("{\"ok\":true}"),
            None,
            false,
        );
        assert_eq!(
            patch.content,
            Some(vec![ToolContent::Text(
                "```json\n{\"ok\":true}\n```".into()
            )])
        );

        let unknown = call("u", "Frobnicate", &json!({"level": 3}), None);
        assert_eq!(unknown.title, "Frobnicate");
        assert_eq!(
            unknown.content,
            [ToolContent::Text("```json\n{\"level\":3}\n```".into())]
        );
        let patch = result("u", "Frobnicate", &json!({}), &json!("done"), None, false);
        assert_eq!(patch.content, Some(vec![ToolContent::Text("done".into())]));
        // An input still being written, or none, is no trouble.
        assert_eq!(call("u", "Bash", &json!({}), None).title, "Run a command");
        assert_eq!(call("u", "Read", &Value::Null, None).title, "Read a file");
    }

    #[test]
    fn a_title_names_paths_under_the_folder_from_there() {
        let made = call("r", "Read", &json!({"file_path": "/w/p/src/a.rs"}), None);
        assert_eq!(
            within(Path::new("/w/p"), made.clone()).title,
            "Read src/a.rs"
        );
        assert_eq!(
            within(Path::new("/w/p/"), made.clone()).title,
            "Read src/a.rs"
        );
        assert_eq!(
            within(Path::new("/elsewhere"), made).title,
            "Read /w/p/src/a.rs"
        );
        let command = call("b", "Bash", &json!({"command": "cat /w/p/a.rs"}), None);
        assert_eq!(
            within(Path::new("/w/p"), command).title,
            "Run cat /w/p/a.rs"
        );
    }
}
