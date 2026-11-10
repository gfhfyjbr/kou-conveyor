//! Pure translator fixture. No recorded operation is dispatched.
use kou_domain::{CallStatus, Operation, ToolCall, ToolResult};
use kou_runtime::{
    operation::Spec,
    tool::{Bash, BashConfig, Translator},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, Write},
    sync::Arc,
};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Input {
    action: String,
    name: String,
    bash: BashConfig,
    call: ToolCall,
    status: CallStatus,
    operations: Vec<Operation>,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Output {
    status: Option<CallStatus>,
    specs: Vec<Spec>,
    result: Option<ToolResult>,
    running: String,
    error: String,
}
fn run(input: Input) -> Output {
    let mut output = Output::default();
    if input.name != "Bash" {
        output.error = format!("unknown translator {:?}", input.name);
        return output;
    }
    let bash = Bash::new(input.bash);
    let operations: Vec<Arc<Operation>> = input.operations.into_iter().map(Arc::new).collect();
    match input.action.as_str() {
        "translate" => {
            output.status = Some(bash.translate(&input.call.arguments, &mut |spec| {
                output.specs.push(spec);
                format!("submitted-{}", output.specs.len())
            }))
        }
        "result" => match bash.result(&input.call.call_id, &input.status, &operations) {
            Ok(value) => output.result = Some(value),
            Err(error) => output.error = error.to_string(),
        },
        "running" => output.running = bash.running(&operations),
        _ => output.error = format!("unknown oracle action {:?}", input.action),
    }
    output
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let line = line?;
        assert!(line.len() <= 8 << 20, "oracle line too large");
        kou_domain::validate_json(line.as_bytes())?;
        serde_json::to_writer(&mut output, &run(serde_json::from_str(&line)?))?;
        writeln!(output)?;
    }
    Ok(())
}
