//! Tool definitions and the built-in ones (Go `harness/tool/static.go`).
//!
//! Providers send every definition with each request, at the head of the
//! prompt-cache prefix. Descriptions are verbatim, and each built-in schema is
//! written once with sorted names and no whitespace: the text Go's
//! deterministic encoding of `map[string]any` puts on the wire.
use super::*;
use crate::operation::shell::MAX_OUTPUT_LENGTH;
use kou_domain::Json;
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeMap};
use std::{collections::BTreeMap, sync::LazyLock};

pub const BASH_NAME: &str = "Bash";
pub const VIEW_IMAGE_NAME: &str = "ViewImage";
pub const READ_NAME: &str = "Read";
pub const EDIT_NAME: &str = "Edit";
pub const WRITE_NAME: &str = "Write";
pub const APPLY_PATCH_NAME: &str = "apply_patch";
pub const CODE_NAME: &str = "Code";
pub const TRANSCRIPT_SEARCH_NAME: &str = "TranscriptSearch";

/// The built-in tools in the order the model sees them.
pub const STATIC_NAMES: [&str; 9] = [
    BASH_NAME,
    READ_NAME,
    EDIT_NAME,
    WRITE_NAME,
    APPLY_PATCH_NAME,
    VIEW_IMAGE_NAME,
    TRANSCRIPT_SEARCH_NAME,
    CODE_NAME,
    SKILL_USE_NAME,
];

pub const TOOL_FUNCTION: &str = "function";

/// Seconds a Bash command may run unless the call or the configuration says otherwise.
pub const DEFAULT_SHELL_TIMEOUT: u32 = 10 * 60;
const MAX_SHELL_TIMEOUT: u32 = 4 * 60 * 60;

pub const READ_DESCRIPTION: &str = "Read a text file: its lines, numbered, from `offset` (a line number, 1 by default) up to `limit` lines (2000 by default) and about 40,000 bytes; search a larger file with rg -n and read the parts you need. Long lines are cut at 2000 bytes. Prefer it to cat, sed -n or head in Bash: it is cheaper and its line numbers can be quoted. ViewImage shows images.";
pub const EDIT_DESCRIPTION: &str = "Replace text in a file. `old_string` must match the file exactly, whitespace and indentation included and without the line numbers Read shows, and appear exactly once unless `replace_all` is set: include enough surrounding lines to make it unique. Read the file before editing it. The result shows the lines around the change. Use it instead of sed, python or heredocs for changes to existing files.";
pub const WRITE_DESCRIPTION: &str = "Create a file or replace its whole content, creating missing directories. To change part of an existing file use Edit, which keeps the rest as it is.";
pub const APPLY_PATCH_DESCRIPTION: &str = "Apply a patch to files of the workspace, in this format:\n\n*** Begin Patch\n*** Update File: path/to/file.py\n@@ def function_name():\n context line, a space first\n-removed line\n+added line\n*** Add File: path/to/new.txt\n+each line of the new file, a + first\n*** Delete File: path/to/old.txt\n*** End Patch\n\nPaths are relative to the workspace. In an Update File, give 3 lines of context before and after each change; a line starting with @@ names the function or class the change is in, to find the right place when the context alone is ambiguous (several @@ lines stack for nested scopes); write *** End of File after a change that reaches the end of the file. *** Move to: new/path right after *** Update File: renames the file. A patch that does not apply changes nothing and says which section was not found; read the file again then and copy its lines exactly.";
pub const CODE_DESCRIPTION: &str = "Run JavaScript in an isolated VM whose only capabilities are the functions declared in the system prompt under \"Code tool\": they run the tools (shell commands, files, images, skills) and return their results as values, so one call can do several steps, branch on results and run independent work at once with Promise.all. Write the body of an async function: `await` the calls, `console.log` what should be reported, and `return` a value (a string or JSON) that the result ends with. The result lists every tool call the code made; call outputs are shown only for calls that failed, or when the code logs and returns nothing.";
pub const TRANSCRIPT_SEARCH_DESCRIPTION: &str = "Search the session's full transcript, including what was compacted away or pruned from the context: the user's messages, your messages, tool calls and their outputs. Returns the matching lines with their context. Use it to recover exact code, commands, errors or outputs from earlier in the session.";

/// Go `llm.Tool`, with the field shapes of `kou_provider::Tool`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Tool {
    #[serde(rename = "Type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub description: String,
    /// The members of the schema object. Each value is kept and sent as it was
    /// written, where Go holds a schema as a map and writes it with sorted
    /// names at every depth: a registered tool must give its values that way.
    /// Go writes a missing schema as an empty object, never null.
    #[serde(serialize_with = "object_or_empty")]
    pub parameters: Option<BTreeMap<String, Json>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Definition {
    #[serde(deserialize_with = "null_default")]
    pub tool: Tool,
    pub metadata: Json,
}

fn null_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    de: D,
) -> std::result::Result<T, D::Error> {
    Option::<T>::deserialize(de).map(Option::unwrap_or_default)
}
fn object_or_empty<S: Serializer>(
    parameters: &Option<BTreeMap<String, Json>>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match parameters {
        Some(members) => members.serialize(serializer),
        None => serializer.serialize_map(Some(0))?.end(),
    }
}

/// Fields are declared in sorted name order, which is the order they are written in.
#[derive(Default, Serialize)]
struct Property<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<Scalar>,
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    maximum: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    minimum: Option<i64>,
    #[serde(rename = "type")]
    kind: &'static str,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Scalar {
    Integer(i64),
    Boolean(bool),
}

fn string(description: &str) -> Property<'_> {
    Property {
        description,
        kind: "string",
        ..Property::default()
    }
}
fn integer(description: &str, minimum: i64, maximum: Option<i64>) -> Property<'_> {
    Property {
        description,
        maximum,
        minimum: Some(minimum),
        kind: "integer",
        ..Property::default()
    }
}
fn encode<T: Serialize + ?Sized>(value: &T) -> Json {
    let text = serde_json::to_string(value).expect("built-in tool schema encodes");
    Json::parse(&text).expect("built-in tool schema is JSON")
}
fn function(
    name: &str,
    description: &str,
    properties: &[(&str, Property<'_>)],
    required: &[&str],
) -> Arc<Definition> {
    let properties: BTreeMap<&str, &Property<'_>> = properties
        .iter()
        .map(|(name, property)| (*name, property))
        .collect();
    Arc::new(Definition {
        tool: Tool {
            kind: TOOL_FUNCTION.into(),
            name: name.into(),
            description: description.into(),
            parameters: Some(BTreeMap::from([
                ("properties".to_owned(), encode(&properties)),
                ("required".to_owned(), encode(required)),
                ("type".to_owned(), encode("object")),
            ])),
        },
        metadata: Json::default(),
    })
}

static BUILT_IN: LazyLock<[Arc<Definition>; 9]> = LazyLock::new(|| {
    let output_length = format!(
        "Maximum characters per output text field. Truncated text keeps its head and tail, around a marker stating how much was omitted, and path to the file with the complete stream. Defaults to {DEFAULT_MAX_OUTPUT_LENGTH}."
    );
    let timeout = format!(
        "Seconds the command may run before it is stopped (SIGTERM, then SIGKILL). Defaults to {DEFAULT_SHELL_TIMEOUT}; 0 lets it run; at most {MAX_SHELL_TIMEOUT}."
    );
    [
        function(
            BASH_NAME,
            "Execute a shell command in background. Independent commands may be issued as parallel tool calls in one turn. Command child processes are killed when the shell exits.",
            &[
                ("command", string("The shell command to execute.")),
                (
                    "max_output_length",
                    Property {
                        default: Some(Scalar::Integer(DEFAULT_MAX_OUTPUT_LENGTH as i64)),
                        ..integer(&output_length, 1, Some(MAX_OUTPUT_LENGTH))
                    },
                ),
                (
                    "timeout",
                    Property {
                        kind: "number",
                        ..integer(&timeout, 0, Some(MAX_SHELL_TIMEOUT.into()))
                    },
                ),
            ],
            &["command"],
        ),
        function(
            READ_NAME,
            READ_DESCRIPTION,
            &[
                (
                    "path",
                    string("The file to read, absolute or relative to the workspace."),
                ),
                (
                    "offset",
                    integer(
                        "The line to start at, counting from 1. Defaults to the first line.",
                        1,
                        None,
                    ),
                ),
                (
                    "limit",
                    integer("How many lines to show. Defaults to 2000.", 1, None),
                ),
            ],
            &["path"],
        ),
        function(
            EDIT_NAME,
            EDIT_DESCRIPTION,
            &[
                (
                    "path",
                    string("The file to edit, absolute or relative to the workspace."),
                ),
                (
                    "old_string",
                    string("The text to replace, exactly as it is in the file."),
                ),
                ("new_string", string("The text that takes its place.")),
                (
                    "replace_all",
                    Property {
                        default: Some(Scalar::Boolean(false)),
                        kind: "boolean",
                        ..string(
                            "Replace every occurrence of old_string instead of requiring exactly one.",
                        )
                    },
                ),
            ],
            &["path", "old_string", "new_string"],
        ),
        function(
            WRITE_NAME,
            WRITE_DESCRIPTION,
            &[
                (
                    "path",
                    string("The file to write, absolute or relative to the workspace."),
                ),
                ("content", string("The whole content of the file.")),
            ],
            &["path", "content"],
        ),
        function(
            APPLY_PATCH_NAME,
            APPLY_PATCH_DESCRIPTION,
            &[(
                "input",
                string("The patch, from *** Begin Patch to *** End Patch."),
            )],
            &["input"],
        ),
        function(
            VIEW_IMAGE_NAME,
            "View a local JPEG, PNG, BMP, TIFF, or WebP image.",
            &[(
                "path",
                string("Image file path, absolute or relative to the workspace."),
            )],
            &["path"],
        ),
        function(
            TRANSCRIPT_SEARCH_NAME,
            TRANSCRIPT_SEARCH_DESCRIPTION,
            &[
                (
                    "query",
                    string("A regular expression (Go/RE2 syntax, case-insensitive) to look for."),
                ),
                (
                    "limit",
                    integer(
                        "How many matches to show at most. Defaults to 20.",
                        1,
                        Some(200),
                    ),
                ),
            ],
            &["query"],
        ),
        function(
            CODE_NAME,
            CODE_DESCRIPTION,
            &[("code", string("The JavaScript to run."))],
            &["code"],
        ),
        function(
            SKILL_USE_NAME,
            "Load the instructions for a registered skill.",
            &[("name", string("The exact name of the skill to load."))],
            &["name"],
        ),
    ]
});

/// Every built-in definition, at the index of its name in [`STATIC_NAMES`].
pub(super) fn built_in() -> &'static [Arc<Definition>; 9] {
    &BUILT_IN
}

#[cfg(test)]
mod tests;
