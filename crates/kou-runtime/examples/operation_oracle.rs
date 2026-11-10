//! Pure file/value transition fixture. Compute dispatches are never executed.
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
    result_kind: String,
    result: Json,
}
impl EventSpec {
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
    value: Json,
    event: Option<EventSpec>,
}
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Dispatch {
    #[serde(rename = "Type")]
    kind: &'static str,
    source: String,
    #[serde(rename = "CorrelationID")]
    correlation_id: &'static str,
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
                        correlation_id: dispatch.correlation_id,
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
