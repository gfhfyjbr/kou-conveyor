use kou_domain::Json;
use kou_runtime::{Cancellation, CodeConfig, CodeHost, Completion, HostCall, run_code};
use serde::Deserialize;
use std::{
    cell::RefCell,
    collections::VecDeque,
    io::{self, Read},
    rc::Rc,
    time::Duration,
};

#[derive(Default)]
struct Host(VecDeque<Completion>);
impl CodeHost for Host {
    fn start(&mut self, call: HostCall) -> Result<(), String> {
        let (output, result, exit_code) = if call.name == "fail" {
            (
                "host failed\nExit code: 1".to_owned(),
                Err("fail failed:\nhost failed\nExit code: 1".to_owned()),
                1,
            )
        } else if call.plugin {
            let output = format!("echo:{}\n", call.arguments.as_str().unwrap_or("{}"));
            let value = Json::parse(&serde_json::to_string(&output).unwrap()).unwrap();
            (output, Ok(value), 0)
        } else {
            return Err("this oracle only has fake plugin tools".into());
        };
        self.0.push_front(Completion {
            ticket: call.ticket,
            result,
            output,
            exit_code: Some(exit_code),
        });
        Ok(())
    }
    fn wait(&mut self, timeout: Duration) -> Option<Completion> {
        if let Some(value) = self.0.pop_front() {
            Some(value)
        } else {
            std::thread::park_timeout(timeout);
            None
        }
    }
    fn cancel(&mut self) {
        self.0.clear()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[derive(Deserialize)]
    struct Input {
        code: String,
    }
    let mut bytes = String::new();
    io::stdin().take(1024 * 1024).read_to_string(&mut bytes)?;
    let input: Input = serde_json::from_str(&bytes)?;
    let result = run_code(
        Rc::new(RefCell::new(Host::default())),
        &CodeConfig {
            tools: ["echo", "net.fetch", "JSON", "bash", "fail"]
                .map(String::from)
                .into(),
            timeout: Duration::from_secs(1),
            max_calls: 3,
            ..CodeConfig::default()
        },
        &input.code,
        Cancellation::default(),
    );
    serde_json::to_writer(io::stdout().lock(), &result)?;
    Ok(())
}
