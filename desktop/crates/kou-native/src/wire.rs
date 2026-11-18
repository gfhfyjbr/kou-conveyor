//! What the Go runner writes: the lines of its output and the records of a
//! session file (format 2). Decoding is lenient where the Go side is strict,
//! since a reader that shows a conversation should survive a newer writer.

use serde::Deserialize;
use serde_json::value::RawValue;

/// One line of runner output or of a session file, before its kind is known.
/// The runner's own output is a bare session item; a session file wraps items
/// and operations in `{"type": ..., "data": ...}`; what streams with `-stream`
/// and the run's last words have a `type` too.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Line<'a> {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(borrow, default)]
    pub data: Option<&'a RawValue>,

    // {"type":"error"}
    #[serde(default)]
    pub message: Option<String>,
    // {"type":"exit"}
    #[serde(default)]
    pub code: Option<i32>,

    // {"type":"delta"}
    #[serde(default)]
    pub turn: Option<String>,
    #[serde(default)]
    pub index: Option<i64>,
    #[serde(rename = "kind", default)]
    pub delta_kind: Option<String>,
    #[serde(default)]
    pub reset: bool,
    // {"type":"output"}
    #[serde(default)]
    pub call: Option<String>,
    #[serde(default)]
    pub operation: Option<String>,
    #[serde(default)]
    pub skipped: Option<i64>,
    // both
    #[serde(default)]
    pub text: Option<String>,

    // A bare session item.
    #[serde(rename = "Kind", default)]
    pub item_kind: Option<String>,
    #[serde(rename = "Sequence", default)]
    pub sequence: Option<u64>,
    #[serde(rename = "Data", borrow, default)]
    pub item_data: Option<&'a RawValue>,
}

/// The `data` of an `item` record of a session file.
#[derive(Debug, Deserialize)]
pub(crate) struct ItemRecord<'a> {
    #[serde(rename = "Item", borrow)]
    pub item: Item<'a>,
    /// The operations of a tool call status, kept beside the item: whole, or
    /// by ID alone when the file already has them.
    #[serde(rename = "Operations", borrow, default)]
    pub operations: Option<Vec<&'a RawValue>>,
}

/// A session item: one thing that happened, in order.
#[derive(Debug, Deserialize)]
pub(crate) struct Item<'a> {
    #[serde(rename = "Sequence", default)]
    pub sequence: u64,
    #[serde(rename = "Kind", default)]
    pub kind: String,
    #[serde(rename = "Data", borrow, default)]
    pub data: Option<&'a RawValue>,
}

/// The `data` of an `operation` record: progress of a tool call's operation
/// between the statuses recorded for the call.
#[derive(Debug, Deserialize)]
pub(crate) struct OperationRecord<'a> {
    #[serde(rename = "Operation", borrow)]
    pub operation: Operation<'a>,
}

/// Input to the agent: a user's message, or a control message of the harness.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Input<'a> {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Kind")]
    pub kind: String,
    #[serde(rename = "Payload", borrow)]
    pub payload: Option<&'a RawValue>,
    #[serde(rename = "Delivery")]
    pub delivery: String,
}

/// The payload of a control input.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Control {
    #[serde(rename = "Mode")]
    pub mode: String,
    #[serde(rename = "Reason")]
    pub reason: String,
}

/// The payload of a user's message that brought images or linked files; a
/// message of text alone is a JSON string.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Message {
    #[serde(rename = "Text")]
    pub text: String,
    #[serde(rename = "Images")]
    pub images: Vec<Image>,
    #[serde(rename = "Files")]
    pub files: Vec<LinkedFile>,
}

/// An image of a message: a data URL of its bytes, or an http(s) URL.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Image {
    #[serde(rename = "Label")]
    pub label: String,
    #[serde(rename = "URL")]
    pub url: String,
}

/// What the model saw of a file a message linked.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LinkedFile {
    #[serde(rename = "Label")]
    pub label: String,
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "Content")]
    pub content: String,
}

/// A request to the model began.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Turn {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Type")]
    pub kind: String,
    #[serde(rename = "Model")]
    pub model: String,
}

/// The session was branched from another.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Fork {
    #[serde(rename = "ParentID")]
    pub parent_id: String,
}

/// The model answered a turn.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ModelResponse<'a> {
    #[serde(rename = "TurnID")]
    pub turn_id: String,
    #[serde(rename = "Response", borrow)]
    pub response: Response<'a>,
}

/// A model's answer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Response<'a> {
    #[serde(rename = "Stop")]
    pub stop: String,
    #[serde(rename = "Output", borrow)]
    pub output: Option<Vec<OutputItem<'a>>>,
    #[serde(rename = "Usage", borrow)]
    pub usage: Usage<'a>,
    #[serde(rename = "Failure")]
    pub failure: Option<Failure>,
}

/// One item of a model's answer: its type says what `data` is.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct OutputItem<'a> {
    #[serde(rename = "Type")]
    pub kind: String,
    #[serde(rename = "Data", borrow)]
    pub data: Option<&'a RawValue>,
}

/// Text the model wrote.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct OutputMessage {
    #[serde(rename = "Role")]
    pub role: String,
    #[serde(rename = "Text")]
    pub text: String,
}

/// Reasoning the provider summarized.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Reasoning {
    #[serde(rename = "Summary")]
    pub summary: Option<Vec<String>>,
}

/// A tool call the model made.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ToolCall {
    #[serde(rename = "CallID")]
    pub call_id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Arguments")]
    pub arguments: String,
}

/// Token counts of one answer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Usage<'a> {
    #[serde(rename = "InputTokens")]
    pub input_tokens: i64,
    #[serde(rename = "OutputTokens")]
    pub output_tokens: i64,
    #[serde(rename = "ReasoningTokens")]
    pub reasoning_tokens: i64,
    /// The provider's own usage object, which may carry a cost.
    #[serde(rename = "Raw", borrow)]
    pub raw: Option<&'a RawValue>,
}

/// Why an answer failed.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Failure {
    #[serde(rename = "Code")]
    pub code: String,
    #[serde(rename = "Message")]
    pub message: String,
}

/// Where a tool call stands, with the operations it started.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ToolCallStatus<'a> {
    #[serde(rename = "CallID")]
    pub call_id: String,
    #[serde(rename = "Status")]
    pub status: CallStatus,
    #[serde(rename = "Operations", borrow)]
    pub operations: Option<Vec<&'a RawValue>>,
}

/// The call's own error, when it never became an operation.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CallStatus {
    #[serde(rename = "Error")]
    pub error: String,
}

/// A unit of work a tool call started: a command, a file change, a script.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Operation<'a> {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Type")]
    pub kind: String,
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "State", borrow)]
    pub state: Option<&'a RawValue>,
    #[serde(rename = "Idempotency", borrow)]
    pub idempotency: Option<&'a RawValue>,
}

/// State of a `shell` operation.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ShellState {
    #[serde(rename = "Input")]
    pub input: ShellInput,
    #[serde(rename = "Result")]
    pub result: Option<ShellResult>,
    #[serde(rename = "TerminalError")]
    pub terminal_error: String,
}

/// The command a `shell` operation runs.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ShellInput {
    #[serde(rename = "Command")]
    pub command: String,
}

/// How a command ended.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ShellResult {
    #[serde(rename = "Out")]
    pub out: String,
    #[serde(rename = "Err")]
    pub err: String,
    #[serde(rename = "ExitCode")]
    pub exit_code: i32,
    #[serde(rename = "TimedOut")]
    pub timed_out: bool,
    #[serde(rename = "Duration")]
    pub duration: f64,
}

/// State of a `file` operation: a read, an edit, a write, a patch or a search
/// of the transcript.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FileState {
    #[serde(rename = "Action")]
    pub action: String,
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "Offset")]
    pub offset: i64,
    #[serde(rename = "Content")]
    pub content: String,
    #[serde(rename = "OldString")]
    pub old_string: String,
    #[serde(rename = "NewString")]
    pub new_string: String,
    #[serde(rename = "Patch")]
    pub patch: String,
    #[serde(rename = "Query")]
    pub query: String,
    #[serde(rename = "Result")]
    pub result: Option<FileResult>,
    #[serde(rename = "TerminalError")]
    pub terminal_error: String,
}

/// What a `file` operation did.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FileResult {
    #[serde(rename = "Text")]
    pub text: String,
    #[serde(rename = "Files")]
    pub files: Vec<String>,
}

/// State of a `view_image` operation.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ViewImageState {
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "Result")]
    pub result: Option<ViewImageResult>,
}

/// The picture a `view_image` operation read, without its bytes.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ViewImageResult {
    #[serde(rename = "OriginalWidth")]
    pub width: i64,
    #[serde(rename = "OriginalHeight")]
    pub height: i64,
    #[serde(rename = "OriginalMIMEType")]
    pub mime: String,
    #[serde(rename = "Error")]
    pub error: String,
}

/// State of a `skill_use` operation.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SkillUseState {
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "TerminalError")]
    pub terminal_error: String,
}

/// State of a `code` operation: a script whose calls are tool calls.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CodeState {
    #[serde(rename = "Code")]
    pub code: String,
    #[serde(rename = "Config")]
    pub config: CodeConfig,
    #[serde(rename = "Result")]
    pub result: Option<CodeResult>,
    #[serde(rename = "TerminalError")]
    pub terminal_error: String,
}

/// Where a script runs.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CodeConfig {
    #[serde(rename = "Directory")]
    pub directory: String,
}

/// What a script did so far.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CodeResult {
    #[serde(rename = "Calls")]
    pub calls: Option<Vec<CodeCall>>,
    #[serde(rename = "Logs")]
    pub logs: String,
    #[serde(rename = "Value")]
    pub value: String,
    #[serde(rename = "Error")]
    pub error: String,
}

/// One tool call a script made.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CodeCall {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Arguments")]
    pub arguments: String,
    #[serde(rename = "Output")]
    pub output: String,
    #[serde(rename = "Error")]
    pub error: String,
    #[serde(rename = "ExitCode")]
    pub exit_code: Option<i32>,
    #[serde(rename = "Started")]
    pub started: Option<String>,
    #[serde(rename = "Finished")]
    pub finished: Option<String>,
    #[serde(rename = "Running")]
    pub running: bool,
}

/// State of a `remote_job` operation, which plugins' tools run as.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RemoteJobState {
    #[serde(rename = "TerminalResult")]
    pub terminal_result: String,
    #[serde(rename = "TerminalError")]
    pub terminal_error: String,
}

/// Decodes a raw JSON value, or the type's default when there is none.
pub(crate) fn decode<'a, T: Deserialize<'a> + Default>(raw: Option<&'a RawValue>) -> Option<T> {
    match raw {
        Some(raw) if raw.get() != "null" => serde_json::from_str(raw.get()).ok(),
        _ => Some(T::default()),
    }
}
