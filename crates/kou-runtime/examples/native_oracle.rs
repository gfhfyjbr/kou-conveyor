use kou_runtime::{Cancellation, CodeConfig, CommandTool, NativeConfig, NativeHost, run_code};
use serde::Deserialize;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{self, Read},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

#[derive(Deserialize)]
struct Request {
    #[serde(rename = "Code")]
    code: String,
    #[serde(rename = "Directory")]
    directory: PathBuf,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    io::stdin().take(1 << 20).read_to_end(&mut bytes)?;
    let request: Request = serde_json::from_slice(&bytes)?;
    if !request.directory.is_absolute() {
        return Err("fixture directory must be absolute".into());
    }
    let tools = vec![
        CommandTool { name:"echo".into(), command:vec!["/bin/cat".into()], environment:BTreeMap::new(), directory:PathBuf::new() },
        CommandTool { name:"probe".into(), command:vec!["/bin/sh".into(),"-c".into(),r#"printf '%s:%s:%s:' "$KOU_CONVEYOR_TOOL_NAME" "$KOU_CONVEYOR_TOOL_CALL_ID" "$EXTRA"; cat"#.into()], environment:BTreeMap::from([("EXTRA".into(),"own".into())]), directory:PathBuf::new() },
        CommandTool { name:"fail".into(), command:vec!["/bin/sh".into(),"-c".into(),"printf reason; exit 5".into()], environment:BTreeMap::new(), directory:PathBuf::new() },
    ];
    let native = NativeConfig {
        shell: "/bin/sh".into(),
        directory: request.directory.clone(),
        environment: Some(BTreeMap::new()),
        command_timeout: Some(Duration::from_secs(1)),
        call_id: "native".into(),
        tools,
        skills: BTreeMap::from([("example".into(), request.directory.join("SKILL.md"))]),
        termination_grace: Duration::from_millis(100),
        ..Default::default()
    };
    let host = NativeHost::new(native)?;
    let vm = CodeConfig {
        tools: host.tool_names(),
        timeout: Duration::from_secs(3),
        max_calls: 30,
        ..Default::default()
    };
    let result = run_code(
        Rc::new(RefCell::new(host)),
        &vm,
        &request.code,
        Cancellation::default(),
    );
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
