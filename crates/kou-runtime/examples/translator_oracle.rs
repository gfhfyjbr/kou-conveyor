//! Pure translator fixture. No recorded operation is dispatched.
use kou_domain::{CallStatus, Operation, ToolCall, ToolResult};
use kou_runtime::{
    operation::Spec,
    tool::{
        Bash, BashConfig, Command, CommandConfig, File, FileConfig, Skill, SkillCatalog, SkillUse,
        Translator,
    },
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
    kind: String,
    bash: BashConfig,
    file: FileConfig,
    command: CommandConfig,
    skills: Vec<Skill>,
    call: ToolCall,
    status: CallStatus,
    operations: Vec<Operation>,
    steps: Vec<Step>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Step {
    #[serde(rename = "CallID")]
    call_id: String,
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
    sequence: Vec<Output>,
    waits: bool,
}
fn run(input: Input) -> Output {
    let mut output = Output::default();
    let translator: Box<dyn Translator> = if input.kind == "skill_use" {
        let catalog = SkillCatalog::default();
        for skill in input.skills {
            if let Err(error) = catalog.register(skill) {
                output.error = error.to_string();
                return output;
            }
        }
        Box::new(SkillUse::new(catalog))
    } else if input.kind == "command" {
        match Command::new(input.command) {
            Ok(command) => Box::new(command),
            Err(error) => {
                output.error = error.to_string();
                return output;
            }
        }
    } else if input.name == "Bash" {
        Box::new(Bash::new(input.bash))
    } else {
        Box::new(File::new(input.kind, input.name, input.file))
    };
    output.waits = translator.waits();
    let operations: Vec<Arc<Operation>> = input.operations.into_iter().map(Arc::new).collect();
    match input.action.as_str() {
        "new" => {}
        "translate" => {
            output.status = Some(translator.translate_call(&input.call, &mut |spec| {
                output.specs.push(spec);
                format!("submitted-{}", output.specs.len())
            }))
        }
        "result" => match translator.result(&input.call.call_id, &input.status, &operations) {
            Ok(value) => output.result = Some(value),
            Err(error) => output.error = error.to_string(),
        },
        "sequence" => {
            for step in input.steps {
                let operations: Vec<_> = step.operations.into_iter().map(Arc::new).collect();
                let mut part = Output {
                    waits: output.waits,
                    ..Output::default()
                };
                match translator.result(&step.call_id, &step.status, &operations) {
                    Ok(value) => part.result = Some(value),
                    Err(error) => part.error = error.to_string(),
                }
                output.sequence.push(part);
            }
        }
        "running" => output.running = translator.running(&operations),
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
