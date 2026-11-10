//! Pure file/value/skill transition fixture. Dispatches are never executed.
use base64::Engine;
use kou_domain::{Json, Operation};
use kou_runtime::operation::{self, Event, EventResult, FileResult, FileState};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct EventSpec {
    #[serde(rename = "Type")]
    kind: String,
    source: String,
    #[serde(rename = "CorrelationID")]
    correlation_id: String,
    result_kind: String,
    result: Json,
}
impl EventSpec {
    fn skill(self) -> operation::Result<operation::SkillEvent> {
        use operation::SkillEventResult as Result;
        let result = match self.result_kind.as_str() {
            "read_output" => {
                #[derive(Default, Deserialize)]
                #[serde(default, rename_all = "PascalCase")]
                struct Output {
                    offset: i64,
                    data: Option<String>,
                }
                let output: Output = self
                    .result
                    .decode()
                    .map_err(|error| operation::Error(error.to_string()))?;
                let data = base64::engine::GeneralPurpose::new(
                    &base64::alphabet::STANDARD,
                    base64::engine::GeneralPurposeConfig::new()
                        .with_decode_allow_trailing_bits(true),
                )
                .decode(output.data.unwrap_or_default())
                .map_err(|error| operation::Error(error.to_string()))?;
                Result::Output {
                    offset: output.offset,
                    data,
                }
            }
            "read_completed" => {
                #[derive(Default, Deserialize)]
                #[serde(default, rename_all = "PascalCase")]
                struct Completed {
                    size: i64,
                }
                let completed: Completed = self
                    .result
                    .decode()
                    .map_err(|error| operation::Error(error.to_string()))?;
                Result::Completed {
                    size: completed.size,
                }
            }
            "failure" => {
                #[derive(Default, Deserialize)]
                #[serde(default, rename_all = "PascalCase")]
                struct Failure {
                    error: String,
                }
                let failure: Failure = self
                    .result
                    .decode()
                    .map_err(|error| operation::Error(error.to_string()))?;
                Result::Failure(failure.error)
            }
            "compute" | "file" => Result::Raw(self.result),
            "" | "raw" if self.result.is_missing() => Result::Empty,
            "" | "raw" => Result::Raw(self.result),
            _ => {
                return Err(operation::Error(format!(
                    "unknown oracle result kind {:?}",
                    self.result_kind
                )));
            }
        };
        Ok(operation::SkillEvent {
            kind: self.kind,
            source: self.source,
            correlation_id: self.correlation_id,
            result,
        })
    }
    fn event(self) -> operation::Result<Event> {
        let result = match self.result_kind.as_str() {
            "file" => EventResult::File(if self.result.as_str() == Some("null") {
                FileResult::default()
            } else {
                self.result
                    .decode()
                    .map_err(|error| operation::Error(error.to_string()))?
            }),
            "compute" => EventResult::Computed(self.result),
            "failure" => {
                #[derive(Default, Deserialize)]
                #[serde(default, rename_all = "PascalCase")]
                struct Failure {
                    error: String,
                }
                let value: Failure = self
                    .result
                    .decode()
                    .map_err(|error| operation::Error(error.to_string()))?;
                EventResult::Failure(value.error)
            }
            "" | "raw" if self.result.is_missing() => EventResult::Empty,
            "" | "raw" => EventResult::Raw(self.result),
            _ => {
                return Err(operation::Error(format!(
                    "unknown oracle result kind {:?}",
                    self.result_kind
                )));
            }
        };
        Ok(Event {
            kind: self.kind,
            source: self.source,
            result,
        })
    }
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Spec {
    action: String,
    operation: Operation,
    file: FileState,
    path: String,
    value: Json,
    event: Option<EventSpec>,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Dispatch {
    #[serde(rename = "Type")]
    kind: &'static str,
    source: String,
    #[serde(rename = "CorrelationID")]
    correlation_id: String,
    path: String,
    offset: i64,
    count: i64,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Output {
    value: Option<Json>,
    operation: Option<Operation>,
    dispatches: Vec<Dispatch>,
    error: String,
}
fn raw(value: &impl Serialize) -> operation::Result<Json> {
    let text = serde_json::to_string(value).map_err(|error| operation::Error(error.to_string()))?;
    Json::parse(&text).map_err(|error| operation::Error(error.to_string()))
}
fn run(spec: Spec) -> Output {
    let mut output = Output::default();
    let result = (|| -> operation::Result<()> {
        match spec.action.as_str() {
            "value_spec" => output.value = Some(raw(&operation::value_spec(spec.value)?)?),
            "value_decode" => output.value = Some(operation::decode_value(&spec.operation)?),
            "file_spec" => output.value = Some(raw(&operation::file_spec(spec.file)?)?),
            "file_decode" => output.value = Some(raw(&operation::decode_file(&spec.operation)?)?),
            "skill_spec" => output.value = Some(raw(&operation::skill_spec(spec.path)?)?),
            "skill_decode" => output.value = Some(raw(&operation::decode_skill(&spec.operation)?)?),
            "skill_advance" => {
                let step = operation::advance_skill(
                    spec.operation,
                    spec.event.map(EventSpec::skill).transpose()?,
                )?;
                output.operation = step.operation;
                output.dispatches = step
                    .dispatches
                    .into_iter()
                    .map(|request| Dispatch {
                        kind: "io.read",
                        source: request.source,
                        correlation_id: request.correlation_id,
                        path: request.path,
                        offset: request.offset,
                        count: request.count,
                    })
                    .collect();
            }
            "value_advance" | "file_advance" => {
                let event = spec.event.map(EventSpec::event).transpose()?;
                let step = if spec.action == "value_advance" {
                    operation::advance_value(spec.operation, event)?
                } else {
                    operation::advance_file(spec.operation, event)?
                };
                output.operation = Some(step.operation);
                output.dispatches = step
                    .dispatches
                    .into_iter()
                    .map(|dispatch| Dispatch {
                        kind: "compute",
                        source: dispatch.source,
                        correlation_id: dispatch.correlation_id.into(),
                        ..Dispatch::default()
                    })
                    .collect();
            }
            _ => {
                return Err(operation::Error(format!(
                    "unknown oracle action {:?}",
                    spec.action
                )));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        output.value = None;
        output.error = error.to_string();
    }
    output
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let line = line?;
        assert!(line.len() <= 8 << 20, "oracle line too large");
        kou_domain::validate_json(line.as_bytes())?;
        let value = run(serde_json::from_str(&line)?);
        serde_json::to_writer(&mut output, &value)?;
        writeln!(output)?;
    }
    Ok(())
}
