use kou_domain::{ControlMessage, Input, Json, Reasoning, Response, ToolResultOutput};
use kou_engine::context::{Builder, Compacted, Error, MaskOptions, Memory, PruneOptions, Skill};
use kou_provider::{Model, Tool};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead},
    sync::{Arc, Mutex},
};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Pruning {
    budget: i64,
    keep: usize,
    bytes: usize,
    first: Option<Vec<String>>,
    thinking_turns: usize,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Masking {
    results: usize,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Spec {
    skills: Vec<Skill>,
    model: Model,
    system: String,
    tools: Vec<Tool>,
    pruning: Pruning,
    masking: Masking,
    notes_path: String,
    notes: String,
    summaries: String,
    transcript: String,
    notes_error: bool,
    steps: Vec<Step>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Step {
    action: String,
    input: Input,
    control: ControlMessage,
    response: Response,
    reasoning: Reasoning,
    #[serde(rename = "CallID")]
    call_id: String,
    output: Vec<ToolResultOutput>,
    running: bool,
    focus: String,
    budget: i64,
    kept: Option<Compacted>,
    live: Vec<String>,
    encoded: Json,
    notes: String,
    notes_error: bool,
    model: Model,
    system: String,
    tools: Vec<Tool>,
    skills: Vec<Skill>,
    pruning: Pruning,
    masking: Masking,
}
fn prune(value: &Pruning) -> PruneOptions {
    PruneOptions {
        budget: value.budget,
        keep: value.keep,
        bytes: value.bytes,
        thinking_turns: value.thinking_turns,
        first: value.first.clone().map(|names| {
            Arc::new(move |name: &str| names.iter().any(|value| value == name))
                as Arc<dyn Fn(&str) -> bool + Send + Sync>
        }),
    }
}
fn new_builder(spec: &Spec, notes: Arc<Mutex<(String, bool)>>) -> Builder {
    let mut current = Builder::new(&spec.skills);
    current.set_model(spec.model.clone());
    current.set_system_prompt(&spec.system);
    current.set_tools(spec.tools.clone());
    current.set_pruning(prune(&spec.pruning));
    current.set_masking(MaskOptions {
        results: spec.masking.results,
    });
    current.set_transcript(spec.transcript.clone());
    current.set_memory(Memory {
        notes: spec.notes_path.clone(),
        summaries: spec.summaries.clone(),
        read_notes: Some(Arc::new(move || {
            let state = notes.lock().unwrap();
            if state.1 {
                Err(io::Error::other("synthetic notes failure"))
            } else {
                Ok(state.0.clone())
            }
        })),
    });
    current
}
fn run(spec: &Spec) -> Vec<Value> {
    let notes = Arc::new(Mutex::new((spec.notes.clone(), spec.notes_error)));
    let mut current = new_builder(spec, notes.clone());
    let mut saved = Json::default();
    let mut output = Vec::new();
    for step in &spec.steps {
        let result: Result<(), Error> = (|| {
            match step.action.as_str() {
                "input" => current.add_external_input(&step.input)?,
                "control" => current.add_control(&step.control)?,
                "response" => current.add_response(step.response.clone()),
                "reasoning" => current.add_reasoning(step.reasoning.clone()),
                "result" => {
                    current.add_tool_result(&step.call_id, step.output.clone(), step.running)
                }
                "refresh" => current.refresh_tool_result(&step.call_id, step.output.clone()),
                "commit" => current.commit(),
                "resumed" => current.resumed(),
                "model" => current.set_model(step.model.clone()),
                "system" => current.set_system_prompt(&step.system),
                "tools" => current.set_tools(step.tools.clone()),
                "skills" => current.set_skills(&step.skills),
                "pruning" => current.set_pruning(prune(&step.pruning)),
                "masking" => current.set_masking(MaskOptions {
                    results: step.masking.results,
                }),
                "build" => output.push(json!({"Build": current.build()})),
                "compaction-request" => output
                    .push(json!({"Build": current.build_compaction(&step.focus, step.budget)})),
                "compact" => {
                    let made = current.compact(&step.response, step.kept.as_ref());
                    output.push(json!({"Compacted": made.clone().unwrap_or_default(), "OK": made.is_some()}));
                }
                "checkpoint" => {
                    saved = current.checkpoint(&step.live)?;
                    output.push(json!({"Checkpoint": saved}));
                }
                "restore" | "restart" => {
                    if step.action == "restart" {
                        current = new_builder(spec, notes.clone());
                    }
                    current.restore_checkpoint(if step.encoded.is_missing() {
                        &saved
                    } else {
                        &step.encoded
                    })?;
                }
                "notes" => *notes.lock().unwrap() = (step.notes.clone(), step.notes_error),
                "notes-update" => {
                    let text = current.notes_update();
                    output.push(json!({"NotesUpdate": text.clone().unwrap_or_default(), "Changed": text.is_some()}));
                }
                other => return Err(Error(format!("unknown context action {other:?}"))),
            }
            Ok(())
        })();
        if let Err(error) = result {
            output.push(json!({"Error": error.to_string()}));
        }
    }
    output
}
fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.expect("oracle input");
        let value = match serde_json::from_str::<Spec>(&line) {
            Ok(spec) => json!(run(&spec)),
            Err(error) => json!({"Error": error.to_string()}),
        };
        println!("{}", value);
    }
}
