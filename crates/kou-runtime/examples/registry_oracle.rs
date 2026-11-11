//! Tool registry fixture. Translators are inert labels; submitted specs are
//! recorded and never dispatched.
use kou_domain::{CallStatus, Operation, ToolCall, ToolResult, ToolResultOutput};
use kou_runtime::{
    operation::{self, Spec},
    tool::{
        self, DEFAULT_SHELL_TIMEOUT, Definition, PROFILES, Profile, RegistrationId, Registry,
        STATIC_NAMES, Skill, StaticTranslators, Tool, Translator,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::{
    io::{self, BufRead, Write},
    sync::Arc,
};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Input {
    action: String,
    name: String,
    names: Vec<String>,
    models: Vec<String>,
    message: String,
    limit: i64,
    encoded: String,
    from: u32,
    to: u32,
    enabled: Vec<String>,
    configured: Vec<Configured>,
    steps: Vec<Step>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Configured {
    name: String,
    waits: bool,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Step {
    op: String,
    name: String,
    arguments: String,
    tool: Tool,
    translator: String,
    waits: bool,
    skill: Skill,
    index: i64,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Output {
    error: String,
    text: String,
    description: String,
    names: Vec<String>,
    resolved: Vec<String>,
    definitions: String,
    found: bool,
    status: Option<CallStatus>,
    specs: Vec<Spec>,
    result: Option<ToolResult>,
    result_error: String,
    waits: bool,
    skills: Vec<Arc<Skill>>,
    limit: i64,
    steps: Vec<Output>,
}

/// Says which translator a name resolved to without doing anything.
struct Labelled {
    label: String,
    waits: bool,
}
impl Translator for Labelled {
    fn translate(&self, arguments: &str, submit: &mut dyn FnMut(Spec) -> String) -> CallStatus {
        let call = ToolCall {
            arguments: arguments.into(),
            ..ToolCall::default()
        };
        self.translate_call(&call, submit)
    }
    fn translate_call(&self, call: &ToolCall, _: &mut dyn FnMut(Spec) -> String) -> CallStatus {
        CallStatus {
            error: format!("translated by {} as {}", self.label, call.name),
            ..CallStatus::default()
        }
    }
    fn waits(&self) -> bool {
        self.waits
    }
    fn result(
        &self,
        call_id: &str,
        _: &CallStatus,
        _: &[Arc<Operation>],
    ) -> operation::Result<ToolResult> {
        Ok(ToolResult {
            call_id: call_id.into(),
            output: vec![ToolResultOutput {
                kind: "text".into(),
                value: format!("result of {}", self.label),
            }],
        })
    }
}
fn labelled(label: String, waits: bool) -> Option<Arc<dyn Translator>> {
    Some(Arc::new(Labelled { label, waits }))
}
fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn registry(input: Input) -> Output {
    let mut translators = StaticTranslators::default();
    for slot in input.configured {
        let translator = labelled(format!("static {}", slot.name), slot.waits);
        match slot.name.as_str() {
            tool::BASH_NAME => translators.bash = translator,
            tool::VIEW_IMAGE_NAME => translators.view_image = translator,
            tool::READ_NAME => translators.read = translator,
            tool::EDIT_NAME => translators.edit = translator,
            tool::WRITE_NAME => translators.write = translator,
            tool::APPLY_PATCH_NAME => translators.apply_patch = translator,
            tool::CODE_NAME => translators.code = translator,
            tool::TRANSCRIPT_SEARCH_NAME => translators.transcript_search = translator,
            _ => {
                return Output {
                    error: format!("unknown static slot {:?}", slot.name),
                    ..Output::default()
                };
            }
        }
    }
    let registry = Registry::new(translators, &input.enabled);
    let mut ids = vec![RegistrationId::nil(); input.steps.len()];
    let mut output = Output::default();
    for (index, step) in input.steps.into_iter().enumerate() {
        let mut part = Output::default();
        match step.op.as_str() {
            "definitions" => {
                part.definitions = serde_json::to_string(&registry.static_definitions())
                    .expect("definitions encode");
                part.names = registry.names();
            }
            "resolve" => {
                if let Some(translator) = registry.resolve(&step.name) {
                    let call = ToolCall {
                        call_id: "call".into(),
                        name: step.name,
                        arguments: step.arguments,
                    };
                    let status = translator.translate_call(&call, &mut |spec| {
                        part.specs.push(spec);
                        format!("submitted-{}", part.specs.len())
                    });
                    match translator.result("call", &status, &[]) {
                        Ok(result) => part.result = Some(result),
                        Err(error) => part.result_error = error.to_string(),
                    }
                    part.found = true;
                    part.waits = translator.waits();
                    part.status = Some(status);
                }
            }
            "unavailable" => part.text = registry.unavailable(&step.name),
            "register" => {
                let translator = if step.translator.is_empty() {
                    None
                } else {
                    labelled(step.translator, step.waits)
                };
                let definition = Definition {
                    tool: step.tool,
                    ..Definition::default()
                };
                if let Err(error) = registry.register_tool(definition, translator) {
                    part.error = error.to_string();
                }
            }
            "register_skill" => match registry.register_skill(step.skill) {
                Ok(id) => {
                    ids[index] = id;
                    part.found = !id.is_nil();
                }
                Err(error) => part.error = error.to_string(),
            },
            "unregister_skill" => registry.unregister_skill(match step.index {
                -1 => RegistrationId::nil(),
                index if index >= 0 => ids[index as usize],
                _ => RegistrationId::new_v4(),
            }),
            "skills" => part.skills = registry.skills(),
            _ => part.error = format!("unknown registry step {:?}", step.op),
        }
        output.steps.push(part);
    }
    output
}

fn describe(profile: Profile, models: &[String]) -> Output {
    Output {
        text: profile.name().into(),
        description: profile.description().into(),
        names: strings(profile.tools()),
        // Go's Valid: a parsed profile always is.
        found: true,
        resolved: models
            .iter()
            .map(|model| profile.resolve(model).name().to_owned())
            .collect(),
        ..Output::default()
    }
}
fn parsed(name: &str) -> String {
    match Profile::parse(name) {
        Ok(profile) => profile.name().into(),
        Err(error) => error.to_string(),
    }
}

fn run(input: Input) -> Output {
    let mut output = Output::default();
    match input.action.as_str() {
        "registry" => return registry(input),
        "aliases" => output.names = strings(&tool::aliases(&input.name, &input.names)),
        "profile" => match Profile::parse(&input.name) {
            Ok(profile) => return describe(profile, &input.models),
            Err(error) => output.error = error.to_string(),
        },
        "models" => {
            output.resolved = input
                .models
                .iter()
                .map(|model| Profile::for_model(model).name().to_owned())
                .collect();
        }
        "catalog" => {
            output.text = tool::profile_names();
            output.names = strings(&STATIC_NAMES);
            output.resolved = strings(&tool::all_profile_tools());
            output.limit = DEFAULT_SHELL_TIMEOUT.into();
            output.steps = PROFILES
                .iter()
                .map(|profile| describe(*profile, &input.models))
                .collect();
        }
        "error_status" => {
            let limit = usize::try_from(input.limit).unwrap_or(0);
            output.status = Some(tool::error_status(input.message, limit));
        }
        "max_output_length" => {
            let encoded: Option<&RawValue> = (!input.encoded.is_empty())
                .then(|| serde_json::from_str(&input.encoded).expect("fixture value is JSON"));
            match tool::parse_max_output_length(encoded) {
                Ok(limit) => output.limit = limit as i64,
                // Bounded as the Bash translator, its one caller, reports it.
                Err(error) => output.error = tool::error_status(error.to_string(), 0).error,
            }
        }
        "sweep" => {
            // FNV-1a over every scalar's outcome, as Go's hash/fnv New64a.
            let mut digest = 0xcbf2_9ce4_8422_2325_u64;
            for scalar in (input.from..input.to).filter_map(char::from_u32) {
                for byte in parsed(scalar.encode_utf8(&mut [0; 4])).bytes().chain([0]) {
                    digest = (digest ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
                }
            }
            output.text = format!("{digest:016x}");
        }
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
