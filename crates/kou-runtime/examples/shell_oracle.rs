use base64::{Engine, engine::general_purpose::STANDARD};
use kou_domain::{Json, Operation, Timestamp};
use kou_runtime::operation::{self, shell::*};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Payload {
    kind: u8,
    #[serde(rename = "PID")]
    pid: i64,
    exit_code: i64,
    signal: i64,
    offset: i64,
    size: i64,
    data: String,
    error: String,
}
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
    fn event(self) -> operation::Result<ShellEvent> {
        let result = if matches!(
            self.result_kind.as_str(),
            "created" | "started" | "exited" | "read_output" | "read_completed" | "failure"
        ) {
            let value: Payload = if self.result.as_str() == Some("null") {
                Payload::default()
            } else {
                self.result
                    .decode()
                    .map_err(|error| operation::Error(error.to_string()))?
            };
            match self.result_kind.as_str() {
                "created" => ShellEventResult::Created { kind: value.kind },
                "started" => ShellEventResult::Started { pid: value.pid },
                "exited" => ShellEventResult::Exited {
                    exit_code: value.exit_code,
                    signal: value.signal,
                },
                "read_output" => ShellEventResult::ReadOutput {
                    offset: value.offset,
                    data: STANDARD
                        .decode(value.data)
                        .map_err(|error| operation::Error(error.to_string()))?,
                },
                "read_completed" => ShellEventResult::ReadCompleted { size: value.size },
                "failure" => ShellEventResult::Failure(value.error),
                _ => unreachable!(),
            }
        } else if self.result.is_missing() {
            ShellEventResult::Empty
        } else {
            ShellEventResult::Raw(self.result)
        };
        Ok(ShellEvent {
            kind: self.kind,
            source: self.source,
            correlation_id: self.correlation_id,
            result,
        })
    }
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Spec {
    action: String,
    operation: Operation,
    input: ShellInput,
    base_directory: String,
    max_output_length: i64,
    event: Option<EventSpec>,
    events: Option<Vec<Option<EventSpec>>>,
    at: Timestamp,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Output {
    spec: Option<operation::Spec>,
    state: Option<ShellState>,
    steps: Vec<ShellStep>,
    error: String,
}
fn run(spec: Spec) -> Output {
    let mut output = Output::default();
    let result = (|| -> operation::Result<()> {
        match spec.action.as_str() {
            "shell_spec" => {
                output.spec = Some(shell_spec(
                    spec.input,
                    spec.base_directory,
                    spec.max_output_length,
                )?)
            }
            "shell_decode" => output.state = Some(decode_shell(&spec.operation)?),
            "shell_advance" => {
                let mut shell = Shell::new(spec.operation)?;
                for event in spec.events.unwrap_or_else(|| vec![spec.event]) {
                    if event
                        .as_ref()
                        .is_some_and(|event| event.kind == "timer.fired")
                        && shell.state().process_group_id > 1
                    {
                        return Err(operation::Error(
                            "fixture rejected: fired timer with an unowned process group".into(),
                        ));
                    }
                    output
                        .steps
                        .push(shell.handle_at(event.map(EventSpec::event).transpose()?, spec.at)?);
                }
            }
            _ => {
                return Err(operation::Error(format!(
                    "unknown action {:?}",
                    spec.action
                )));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        output.error = error.to_string();
    }
    output
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let line = line?;
        assert!(line.len() <= 16 << 20, "oracle line too large");
        kou_domain::validate_json(line.as_bytes())?;
        serde_json::to_writer(&mut output, &run(serde_json::from_str(&line)?))?;
        writeln!(output)?;
    }
    Ok(())
}
