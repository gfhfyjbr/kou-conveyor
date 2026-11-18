//! The server as a real process, started the way a harness starts it: with
//! the pane's environment, talking on its stdin and stdout.

#![cfg(unix)]

use kou_ipc::{Activity, Incoming, Pane, Request, Response, Server};
use serde_json::{Value, json};
use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SERVER: &str = env!("CARGO_BIN_EXE_kou-mcp");

#[test]
fn the_process_serves_its_pane_and_exits_when_its_input_ends() {
    let dir = tempfile::Builder::new()
        .prefix("kou-mcp-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("kou.sock");
    let (server, incoming) = Server::bind(&socket).unwrap();
    let app = thread::spawn(move || {
        let mut held = Vec::new();
        let mut seen = Vec::new();
        while let Ok(Incoming { request, reply }) = incoming.recv_blocking() {
            seen.push(request.clone());
            match request {
                Request::PaneInfo { pane } => reply.send(Response::Pane(Pane {
                    id: pane,
                    session: "s3".into(),
                    harness: "claude".into(),
                    activity: Activity::Working,
                    ..Pane::default()
                })),
                // The wait never ends: the process must still exit with its input.
                _ => held.push(reply),
            }
        }
        seen
    });

    let mut child = Command::new(SERVER)
        .env("KOU_SOCKET", &socket)
        .env("KOU_PANE", "p7")
        .env("KOU_SESSION", "s3")
        .env_remove("KOU_BIN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut send = |message: Value| {
        let mut line = serde_json::to_vec(&message).unwrap();
        line.push(b'\n');
        input.write_all(&line).unwrap();
        input.flush().unwrap();
    };
    let mut receive = || {
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };

    send(
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" },
        }}),
    );
    let answer = receive();
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    send(
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "describe_pane", "arguments": {},
        }}),
    );
    let answer = receive();
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("p7 (this is you): claude in a terminal, working for 0s."),
        "{text}"
    );
    assert_eq!(
        answer["result"]["structuredContent"]["pane"]["session"],
        "s3"
    );

    send(
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "wait_for_panes", "arguments": { "panes": ["p8"], "timeout_seconds": 600 },
        }}),
    );
    thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    drop(input);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );

    drop(server);
    let seen = app.join().unwrap();
    assert_eq!(seen[0], Request::PaneInfo { pane: "p7".into() });
    assert!(matches!(&seen[1], Request::PaneWait { panes, .. } if panes == &["p8"]));
}
