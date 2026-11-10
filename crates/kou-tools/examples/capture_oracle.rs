use kou_tools::process::{CancelToken, CapturePaths, ProcessRequest, run_captured_process};
use serde::Deserialize;
use serde_json::json;
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    time::{Duration, Instant},
};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Spec {
    code: String,
    input: Option<String>,
    environment: Option<BTreeMap<String, String>>,
    cancel_before: bool,
    cancel_on_ready: bool,
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 15) as usize] as char);
    }
    output
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    io::stdin().take(8 << 20).read_to_end(&mut bytes)?;
    let spec: Spec = serde_json::from_slice(&bytes)?;
    let directory = tempfile::tempdir()?;
    let root = fs::canonicalize(directory.path())?;
    let paths = CapturePaths {
        stdout: root.join("out"),
        stderr: root.join("err"),
    };
    for path in [&paths.stdout, &paths.stderr] {
        use std::io::Write;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?
            .write_all(b"old capture")?;
    }
    fs::write(root.join("owned-marker"), "fixture")?;
    let mut request = ProcessRequest::new("/bin/sh");
    request.arguments = vec![
        "-c".into(),
        spec.code.into(),
        "capture-oracle".into(),
        root.clone().into_os_string(),
    ];
    request.directory = root;
    request.input = spec.input.map(String::into_bytes);
    request.environment = spec.environment.map(|items| {
        items
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect()
    });
    request.timeout = Some(Duration::from_secs(10));
    request.output_limit = 0;
    let cancel = CancelToken::default();
    if spec.cancel_before {
        cancel.cancel();
    }
    let pid = Cell::new(0);
    let result = run_captured_process(&request, &paths, &cancel, |id| {
        pid.set(id);
        if spec.cancel_on_ready {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if fs::read(&paths.stdout)? == b"ready" {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::other("fixture readiness deadline"));
                }
                std::thread::yield_now();
            }
            cancel.cancel();
        }
        Ok(())
    });
    let (exited, canceled, exit_code, signal, error) = match result {
        Ok(result) if result.canceled => (false, true, None, None, String::new()),
        Ok(result) => (
            true,
            false,
            Some(result.exit_code),
            Some(result.signal),
            String::new(),
        ),
        Err(error) if spec.cancel_before && error.kind() == io::ErrorKind::Interrupted => {
            (false, true, None, None, String::new())
        }
        Err(error) => (false, false, None, None, error.to_string()),
    };
    let gone = pid.get() == 0
        || rustix::process::test_kill_process_group(
            rustix::process::Pid::from_raw(pid.get() as i32).unwrap(),
        ) == Err(rustix::io::Errno::SRCH);
    println!(
        "{}",
        json!({
            "Started": pid.get() != 0, "Exited": exited, "Canceled": canceled,
            "ExitCode": exit_code, "Signal": signal, "Error": error,
            "StdoutHex": hex(&fs::read(&paths.stdout)?), "StderrHex": hex(&fs::read(&paths.stderr)?),
            "OutMode": fs::metadata(&paths.stdout)?.permissions().mode() & 0o777,
            "ErrMode": fs::metadata(&paths.stderr)?.permissions().mode() & 0o777,
            "GroupGone": gone,
        })
    );
    Ok(())
}
