use super::*;
use kou_tools::fileops::{self, EditOptions, ReadOptions};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct FileState {
    #[serde(deserialize_with = "null_default")]
    pub action: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub path: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub root: String,
    #[serde(skip_serializing_if = "zero", deserialize_with = "null_default")]
    pub offset: i64,
    #[serde(skip_serializing_if = "zero", deserialize_with = "null_default")]
    pub limit: i64,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub content: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub old_string: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub new_string: String,
    #[serde(skip_serializing_if = "no", deserialize_with = "null_default")]
    pub replace_all: bool,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub patch: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub query: String,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "object_result"
    )]
    pub result: Option<FileResult>,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub terminal_error: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct FileResult {
    #[serde(deserialize_with = "null_default")]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "zero", deserialize_with = "null_default")]
    pub lines: i64,
    #[serde(skip_serializing_if = "zero", deserialize_with = "null_default")]
    pub from: i64,
    #[serde(skip_serializing_if = "zero", deserialize_with = "null_default")]
    pub to: i64,
    #[serde(skip_serializing_if = "no", deserialize_with = "null_default")]
    pub truncated: bool,
}

fn result_shape_error(shape: &str) -> String {
    format!("json: cannot unmarshal JSON {shape} into Go operation.FileResult within \"/Result\"")
}
fn object_result<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> std::result::Result<Option<FileResult>, D::Error> {
    // Borrow the raw span: an owned Json would copy the complete result body.
    let raw = <&serde_json::value::RawValue>::deserialize(de)?;
    let shape = match raw.get().as_bytes()[0] {
        b'n' => return Ok(None),
        b'{' => {
            return serde_json::from_str(raw.get())
                .map(Some)
                .map_err(serde::de::Error::custom);
        }
        b'[' => "array",
        b'\"' => "string",
        b't' | b'f' => "boolean",
        _ => "number",
    };
    Err(serde::de::Error::custom(result_shape_error(shape)))
}
fn decode_error(error: Error) -> String {
    // serde_json attaches its own position to custom errors. Only our fixed
    // Result-shape diagnostic has Go's pointer already and omits that position.
    if let Some((detail, _)) = error.0.split_once(" at line ")
        && ["array", "string", "boolean", "number"]
            .iter()
            .any(|shape| detail == result_shape_error(shape))
    {
        return detail.to_owned();
    }
    error.0
}

impl FileState {
    pub fn validate(&self) -> Result<()> {
        let error = match self.action.as_str() {
            "read" | "write" | "edit" if self.path.is_empty() => Some("file path must be set"),
            "patch" if self.patch.trim().is_empty() => Some("patch must be set"),
            "transcript_search" if self.path.is_empty() => Some("transcript path must be set"),
            "transcript_search" if self.query.trim().is_empty() => Some("query must be set"),
            "read" | "write" | "edit" | "patch" | "transcript_search" => None,
            _ => return Err(Error(format!("unsupported file action {:?}", self.action))),
        };
        if let Some(error) = error {
            return Err(Error(error.into()));
        }
        if self.offset < 0 || self.limit < 0 {
            return Err(Error("offset and limit must not be negative".into()));
        }
        Ok(())
    }
}

pub fn file_spec(state: FileState) -> Result<Spec> {
    state.validate()?;
    Ok(Spec {
        max_output_length: 0,
        kind: "file".into(),
        version: 1,
        state: encode(&state)?,
        idempotency: Json::default(),
    })
}

pub fn decode_file(current: &Operation) -> Result<FileState> {
    if current.kind != "file" || current.version != 1 {
        return Err(Error(format!(
            "decode file operation {:?}: type {:?} version {}: unsupported operation",
            current.id, current.kind, current.version
        )));
    }
    let state: FileState = decode_state(&current.state, "FileState").map_err(|error| {
        Error(format!(
            "decode file operation {:?} state: {}",
            current.id,
            decode_error(error)
        ))
    })?;
    state.validate().map_err(|error| {
        Error(format!(
            "validate file operation {:?} state: {error}",
            current.id
        ))
    })?;
    Ok(state)
}

pub fn advance_file(current: Operation, event: Option<Event>) -> Result<Step> {
    let state = decode_file(&current)?;
    match current.status.as_str() {
        "ready" => {
            if event.is_some() {
                return Err(Error(format!(
                    "advance ready file operation {:?}: unexpected primitive event",
                    current.id
                )));
            }
            if state.result.is_some() || !state.terminal_error.is_empty() {
                return Err(Error(format!(
                    "advance ready file operation {:?}: state is not initial",
                    current.id
                )));
            }
            dispatch(current, state)
        }
        "awaiting" => {
            if let Some(event) = event {
                return file_event(current, state, event);
            }
            match state.action.as_str() {
                "edit" => finish(current, state, "failed", None, "the edit was interrupted by a restart before its result was recorded; read the file to see whether it was made before making it again".into()),
                "patch" => finish(current, state, "failed", None, "the patch was interrupted by a restart before its result was recorded; read the files to see whether it was applied before applying it again".into()),
                _ => dispatch(current, state),
            }
        }
        "canceling" => {
            if event.is_some() {
                return Err(Error(format!(
                    "advance canceling file operation {:?}: unexpected primitive event",
                    current.id
                )));
            }
            finish(
                current,
                state,
                "canceled",
                None,
                "file operation canceled".into(),
            )
        }
        _ => Err(Error(format!(
            "advance file operation {:?}: terminal status {:?}",
            current.id, current.status
        ))),
    }
}

fn dispatch(mut current: Operation, state: FileState) -> Result<Step> {
    current.status = "awaiting".into();
    current.state = encode(&state)?;
    let source = current.id.clone();
    Ok(Step {
        operation: current,
        dispatches: vec![ComputeDispatch {
            source,
            correlation_id: "file",
            file: Box::new(state),
        }],
    })
}
fn finish(
    mut current: Operation,
    mut state: FileState,
    status: &str,
    result: Option<FileResult>,
    error: String,
) -> Result<Step> {
    state.result = result;
    state.terminal_error = error;
    current.status = status.into();
    current.state = encode(&state)?;
    Ok(Step {
        operation: current,
        dispatches: Vec::new(),
    })
}
fn file_event(current: Operation, state: FileState, event: Event) -> Result<Step> {
    if event.source != current.id {
        let error = format!(
            "file primitive event source is {:?}, want {:?}",
            event.source, current.id
        );
        return finish(current, state, "failed", None, error);
    }
    match event.kind.as_str() {
        "primitive.canceled" => finish(
            current,
            state,
            "canceled",
            None,
            "file operation canceled".into(),
        ),
        "primitive.failed" => {
            let message = match event.result {
                EventResult::Failure(error) => error,
                _ => "file operation failed with an invalid result".into(),
            };
            finish(current, state, "failed", None, message)
        }
        "compute.completed" => match event.result {
            EventResult::File(result) => {
                finish(current, state, "completed", Some(result), String::new())
            }
            EventResult::Computed(_) => finish(
                current,
                state,
                "failed",
                None,
                "file operation returned an invalid result".into(),
            ),
            _ => finish(
                current,
                state,
                "failed",
                None,
                "file operation returned an invalid compute result".into(),
            ),
        },
        _ => finish(
            current,
            state,
            "failed",
            None,
            format!("file operation returned unexpected event {:?}", event.kind),
        ),
    }
}

/// Execute an explicitly dispatched file task, never as part of replay/advance.
/// The manager must discard its result in favor of cancellation after a stop.
pub fn run_file(state: &FileState, canceled: impl Fn() -> bool) -> Result<FileResult> {
    state.validate()?;
    if canceled() {
        return Err(Error("context canceled".into()));
    }
    match state.action.as_str() {
        "read" => {
            let result = fileops::read_interruptible(
                &state.path,
                ReadOptions {
                    offset: state.offset,
                    limit: state.limit,
                    ..ReadOptions::default()
                },
                canceled,
            )
            .map_err(|error| Error(error.to_string()))?;
            Ok(FileResult {
                text: result.text,
                lines: result.lines as i64,
                from: result.from as i64,
                to: result.to as i64,
                truncated: result.truncated,
                ..FileResult::default()
            })
        }
        "write" => {
            let result = fileops::write(&state.path, &state.content)
                .map_err(|error| Error(error.to_string()))?;
            let lines = plural(result.lines);
            let text = if result.created {
                format!("Wrote {}: {lines}, {} bytes.", state.path, result.bytes)
            } else {
                format!(
                    "Replaced {} (it had {}): now {lines}, {} bytes.",
                    state.path,
                    plural(result.previous_lines),
                    result.bytes
                )
            };
            Ok(FileResult {
                text,
                files: Some(vec![state.path.clone()]),
                lines: result.lines as i64,
                ..FileResult::default()
            })
        }
        "edit" => {
            let result = fileops::edit(
                &state.path,
                &state.old_string,
                &state.new_string,
                EditOptions {
                    replace_all: state.replace_all,
                },
            )
            .map_err(|error| Error(error.to_string()))?;
            let replaced = if result.replacements == 1 {
                format!("replaced 1 occurrence at line {}", result.line)
            } else {
                format!(
                    "replaced {} occurrences, the first at line {}",
                    result.replacements, result.line
                )
            };
            let mut text = format!("Edited {}: {replaced}.", state.path);
            if !result.snippet.is_empty() {
                text.push_str(" The file now reads around the first change:\n");
                text.push_str(&result.snippet);
            }
            Ok(FileResult {
                text,
                files: Some(vec![state.path.clone()]),
                lines: result.lines as i64,
                ..FileResult::default()
            })
        }
        "patch" => {
            let result = kou_tools::patch::apply_patch(&state.patch, &state.root)
                .map_err(|error| Error(error.to_string()))?;
            Ok(FileResult {
                text: result.summary(),
                files: Some(result.files()),
                ..FileResult::default()
            })
        }
        "transcript_search" => Err(Error(
            "transcript search is not connected to this runtime".into(),
        )),
        _ => unreachable!("validated file action"),
    }
}
fn plural(lines: usize) -> String {
    format!("{lines} {}", if lines == 1 { "line" } else { "lines" })
}
