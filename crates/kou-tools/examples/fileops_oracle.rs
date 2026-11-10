use kou_tools::fileops::{self, EditOptions, ReadOptions};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, Read},
    path::PathBuf,
};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Request {
    action: String,
    name: PathBuf,
    content: String,
    old: String,
    replacement: String,
    read_options: ReadOptions,
    edit_options: EditOptions,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    io::stdin().take(8 << 20).read_to_end(&mut bytes)?;
    let request: Request = serde_json::from_slice(&bytes)?;
    let result = match request.action.as_str() {
        "read" => fileops::read(&request.name, request.read_options)
            .map(|v| serde_json::to_value(v).unwrap()),
        "write" => fileops::write(&request.name, &request.content)
            .map(|v| serde_json::to_value(v).unwrap()),
        "edit" => fileops::edit(
            &request.name,
            &request.old,
            &request.replacement,
            request.edit_options,
        )
        .map(|v| serde_json::to_value(v).unwrap()),
        "patch" => kou_tools::patch::apply_patch(&request.content, &request.name)
            .map(|value| json!({"Summary":value.summary(),"Files":value.files()})),
        _ => return Err("unsupported fixture action".into()),
    };
    let response = match result {
        Ok(value) => json!({"Result":value,"Error":""}),
        Err(error) => json!({"Result":Value::Null,"Error":error.to_string()}),
    };
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}
