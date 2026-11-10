use kou_domain::{CallStatus, Item, Json, Operation, ToolCall, ToolResultOutput};
use kou_engine::{
    context::{Builder, Error, MaskOptions, PruneOptions, Result},
    coordinator::{History, ReplayTools},
};
use kou_provider::{Model, Tool};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead},
    sync::Arc,
};
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Pruning {
    budget: i64,
    keep: usize,
    bytes: usize,
    thinking_turns: usize,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Masking {
    results: usize,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Checkpoint {
    state: Json,
    items: Vec<Item>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Spec {
    items: Vec<Item>,
    checkpoint: Option<Checkpoint>,
    available: Option<Vec<String>>,
    model: Model,
    system: String,
    tools: Vec<Tool>,
    pruning: Pruning,
    masking: Masking,
    export: bool,
    state_probe: bool,
}
struct Tools(Vec<String>);
fn operations_text(operations: &[Arc<Operation>]) -> String {
    operations
        .iter()
        .map(|operation| format!("{}:{}", operation.id, operation.status))
        .collect::<Vec<_>>()
        .join(",")
}
impl ReplayTools for Tools {
    fn available(&self, name: &str) -> bool {
        self.0.iter().any(|known| known == name)
    }
    fn result(
        &self,
        call: &ToolCall,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<Vec<ToolResultOutput>> {
        if call.name == "Reject" {
            return Err(Error("synthetic translate failure".into()));
        }
        let text = if status.error.is_empty() {
            format!(
                "translated {}: {}",
                call.call_id,
                operations_text(operations)
            )
        } else {
            format!("Error: {}", status.error)
        };
        Ok(vec![ToolResultOutput {
            kind: "text".into(),
            value: text,
        }])
    }
    fn running(&self, call: &ToolCall, operations: &[Arc<Operation>]) -> String {
        if call.name == "Describe" {
            format!("progress {}", operations_text(operations))
        } else {
            String::new()
        }
    }
}
fn run(spec: Spec) -> Result<Value> {
    let mut builder = Builder::default();
    builder.set_model(spec.model);
    builder.set_system_prompt(&spec.system);
    builder.set_tools(spec.tools);
    builder.set_pruning(PruneOptions {
        budget: spec.pruning.budget,
        keep: spec.pruning.keep,
        bytes: spec.pruning.bytes,
        thinking_turns: spec.pruning.thinking_turns,
        ..PruneOptions::default()
    });
    builder.set_masking(MaskOptions {
        results: spec.masking.results,
    });
    let names = spec.available.unwrap_or_else(|| {
        [
            "Read",
            "Write",
            "Edit",
            "apply_patch",
            "Bash",
            "Describe",
            "Reject",
        ]
        .iter()
        .map(|name| (*name).into())
        .collect()
    });
    let mut current = History::new(builder, Arc::new(Tools(names)));
    let mut from_checkpoint = false;
    if let Some(checkpoint) = spec.checkpoint
        && current.restore_checkpoint(&checkpoint.state)
    {
        from_checkpoint = true;
        for item in checkpoint.items {
            current.apply_recorded(item)?;
        }
    }
    if !from_checkpoint {
        for item in spec.items {
            let sequence = item.sequence;
            current
                .apply_recorded(item)
                .map_err(|error| Error(format!("load session item {sequence}: {error}")))?;
        }
    }
    current.context.resumed();
    if spec.export {
        Ok(json!({"State": current.checkpoint()?}))
    } else {
        let mut output = json!({"Build": current.context.build()});
        if spec.state_probe {
            output["Probe"] = serde_json::to_value(current.inspect())
                .map_err(|error| Error(error.to_string()))?;
        }
        Ok(output)
    }
}
fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.expect("oracle input");
        let output = match serde_json::from_str::<Spec>(&line) {
            Ok(spec) => match run(spec) {
                Ok(value) => value,
                Err(error) => json!({"Error":error.to_string()}),
            },
            Err(error) => json!({"Error":error.to_string()}),
        };
        println!("{output}");
    }
}
