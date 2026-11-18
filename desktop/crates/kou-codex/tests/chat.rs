use futures::{FutureExt as _, executor::block_on, select};
use kou_agents::Launch;
use kou_claude::host::Keep;
use kou_thread::{Backend, Block, Command, PermissionRequest, Request, Stop, ToolContent, Update};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const PATIENCE: Duration = Duration::from_secs(15);

fn example() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("scripted_codex{}", std::env::consts::EXE_SUFFIX))
}

fn launch(dir: &Path) -> Launch {
    Launch {
        program: example(),
        env: vec![],
        args: vec!["app-server".into()],
        cwd: dir.to_owned(),
    }
}

fn until(backend: &Backend, found: impl Fn(&Update) -> bool) -> Vec<Update> {
    block_on(async {
        let mut timeout = futures_timer::Delay::new(PATIENCE).fuse();
        let mut seen = Vec::new();
        loop {
            let update = select! {
                _ = timeout => panic!("timed out after {seen:?}"),
                next = backend.updates.recv().fuse() => next.expect("backend closed"),
            };
            let done = found(&update);
            assert!(
                done || !matches!(update, Update::Closed(_)),
                "unexpected {update:?}"
            );
            seen.push(update);
            if done {
                return seen;
            }
        }
    })
}

fn send(backend: &Backend, cmd: Command) {
    backend.commands.try_send(cmd).unwrap();
}
fn prompt(s: &str) -> Vec<Block> {
    vec![Block::Text(s.into())]
}
fn permission(updates: &[Update]) -> &PermissionRequest {
    let Some(Update::Permission(request)) = updates.last() else {
        panic!("{updates:?}");
    };
    request
}

#[test]
fn one_connection_routes_partial_items_steering_permissions_and_stop() {
    let dir = tempfile::tempdir().unwrap();
    let launch = launch(dir.path());
    let models = kou_codex::models(&launch, PATIENCE).unwrap();
    assert_eq!(models.models.len(), 2);
    assert_eq!(models.current.as_deref(), Some("two"));
    assert_eq!(
        kou_codex::sessions(&launch, PATIENCE).unwrap()[0].title,
        "Fixture"
    );
    let chat = kou_codex::connect(launch, None, None, None).unwrap();
    send(&chat, Command::Prompt(prompt("reject")));
    let rejected = until(&chat, |u| matches!(u, Update::TurnEnded(Stop::Failed(_))));
    assert!(rejected.contains(&Update::PromptTaken(None)));
    send(&chat, Command::Prompt(prompt("first")));
    let opened = until(&chat, |u| matches!(u, Update::Permission(_)));
    assert_eq!(
        opened
            .iter()
            .filter(|u| matches!(u, Update::TurnStarted))
            .count(),
        1
    );
    assert!(!opened.iter().any(|u| matches!(u, Update::UserChunk(_))));
    let ask = permission(&opened);
    assert_eq!(
        ask.options
            .iter()
            .map(|o| o.id.as_str())
            .collect::<Vec<_>>(),
        ["accept", "decline"]
    );
    send(
        &chat,
        Command::Permit {
            request: ask.id,
            option: "accept".into(),
        },
    );
    let asked = until(&chat, |u| matches!(u, Update::Permission(_)));
    assert!(asked.iter().any(|u| matches!(u, Update::Tool(t) if matches!(t.content.as_slice(), [ToolContent::Terminal { output, exit:Some(0), .. }] if output=="/work"))));
    send(
        &chat,
        Command::Answer {
            request: permission(&asked).id,
            answers: vec![vec!["kou".into()]],
        },
    );
    let chunks = until(&chat, |u| u == &Update::AssistantChunk("!".into()));
    let text: String = chunks
        .iter()
        .filter_map(|u| {
            if let Update::AssistantChunk(s) = u {
                Some(s.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(text, "hello!");
    chat.requests
        .try_send(Request::Steer(prompt("also this")))
        .unwrap();
    let steered = until(&chat, |u| matches!(u, Update::PromptId(_)));
    assert!(steered.contains(&Update::UserChunk(Block::Text("also this".into()))));
    send(&chat, Command::Cancel);
    until(&chat, |u| u == &Update::TurnEnded(Stop::Cancelled));
    send(&chat, Command::Close);
    until(&chat, |u| u == &Update::Closed(None));
}

#[cfg(unix)]
#[test]
fn reconnect_restores_the_running_turn_and_its_unanswered_request() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let keep = Keep {
        host: example(),
        host_args: vec!["host".into()],
        socket: dir.path().join("pane.sock"),
        idle: Duration::from_secs(10),
        away: Duration::from_secs(60),
        stall: PATIENCE,
    };
    let first = kou_codex::connect(launch(dir.path()), None, None, Some(keep.clone())).unwrap();
    send(&first, Command::Prompt(prompt("first")));
    until(&first, |u| matches!(u, Update::Permission(_)));
    drop(first);
    let next = kou_codex::connect(launch(dir.path()), None, None, Some(keep.clone())).unwrap();
    let restored = until(&next, |u| matches!(u, Update::Permission(_)));
    assert!(restored.contains(&Update::UserChunk(Block::Text("first".into()))));
    assert!(restored.contains(&Update::AssistantChunk("Checking the folder".into())));
    assert_eq!(
        restored
            .iter()
            .filter(|u| matches!(u, Update::TurnStarted))
            .count(),
        1
    );
    send(
        &next,
        Command::Permit {
            request: permission(&restored).id,
            option: "accept".into(),
        },
    );
    until(
        &next,
        |u| matches!(u,Update::Permission(p) if p.tool.id=="ask"),
    );
    send(&next, Command::Close);
    until(&next, |u| u == &Update::Closed(None));
    for _ in 0..100 {
        if !keep.socket.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("closing the pane left a keeper behind");
}

/// A Responses endpoint for the installed CLI. Every completed turn has a
/// distinct answer, which makes dropped or duplicated replay items visible.
fn local_model() -> (std::net::SocketAddr, std::sync::mpsc::Sender<()>) {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let (guard, stop) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut n = 0;
        while matches!(stop.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            stream.set_read_timeout(Some(PATIENCE)).unwrap();
            let mut reader = BufReader::new(&stream);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                    length = n.trim().parse().unwrap();
                }
            }
            reader.read_exact(&mut vec![0; length]).unwrap();
            if !first.starts_with("POST /v1/responses ") {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                continue;
            }
            n += 1;
            let text = format!("Local answer {n}.");
            let id = format!("msg_{n}");
            let response = format!("resp_{n}");
            use serde_json::json;
            let item = json!({"type":"message","role":"assistant","id":id,"content":[{"type":"output_text","text":text}]});
            let events = [
                json!({"type":"response.created","response":{"id":response}}),
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","role":"assistant","id":id,"content":[]}}),
                json!({"type":"response.output_text.delta","item_id":id,"output_index":0,"content_index":0,"delta":text}),
                json!({"type":"response.output_item.done","output_index":0,"item":item}),
                json!({"type":"response.completed","response":{"id":response,"status":"completed","output":[item],"usage":{"input_tokens":5,"output_tokens":3,"total_tokens":8}}}),
            ];
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").unwrap();
            for event in events {
                writeln!(stream, "data: {event}\n").unwrap();
            }
        }
    });
    (addr, guard)
}

#[test]
#[ignore = "requires the installed Codex CLI and a loopback socket"]
fn installed_cli_streams_forks_and_resumes_with_an_isolated_account() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("profile");
    let cwd = root.path().canonicalize().unwrap().join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&cwd).unwrap();
    let (address, _guard) = local_model();
    std::fs::write(
        home.join("config.toml"),
        format!(
            r#"
model_provider = "localtest"
model = "gpt-5.2"
[model_providers.localtest]
name = "Local test"
base_url = "http://{address}/v1"
wire_api = "responses"
supports_websockets = false
"#
        ),
    )
    .unwrap();
    let launch = Launch {
        program: "codex".into(),
        args: vec!["app-server".into()],
        env: vec![
            ("CODEX_HOME".into(), home.display().to_string()),
            ("HOME".into(), root.path().display().to_string()),
            ("NO_PROXY".into(), "127.0.0.1".into()),
        ],
        cwd,
    };
    let offered = kou_codex::models(&launch, PATIENCE).unwrap();
    assert_eq!(offered.current.as_deref(), Some("gpt-5.2"));
    for id in ["read-only", "auto", "full-access"] {
        assert!(
            offered.access.choices.iter().any(|p| p.id == id),
            "{:?}",
            offered.access
        );
    }
    let empty = kou_codex::connect(launch.clone(), None, None, None).unwrap();
    let ready = until(&empty, |u| matches!(u, Update::Ready { .. }));
    let Some(Update::Ready {
        session: empty_id, ..
    }) = ready.last()
    else {
        unreachable!()
    };
    send(&empty, Command::Close);
    until(&empty, |u| u == &Update::Closed(None));
    let chat = kou_codex::connect_with_access(
        launch.clone(),
        Some(empty_id.clone()),
        None,
        None,
        Some("full-access".into()),
    )
    .unwrap();
    let ready = until(&chat, |u| matches!(u, Update::Ready { .. }));
    let Update::Ready {
        session: original, ..
    } = ready.last().unwrap()
    else {
        unreachable!()
    };
    assert_ne!(original, empty_id);
    until(
        &chat,
        |u| matches!(u, Update::Access(a) if a.current.as_deref() == Some("full-access")),
    );
    send(&chat, Command::SetAccess("auto".into()));
    until(
        &chat,
        |u| matches!(u, Update::Access(a) if a.current.as_deref() == Some("auto")),
    );
    send(&chat, Command::SetMode("plan".into()));
    until(&chat, |u| matches!(u, Update::Mode(mode) if mode == "plan"));
    let mut turn = String::new();
    for n in 1..=2 {
        send(&chat, Command::Prompt(prompt(&format!("Local prompt {n}"))));
        let updates = until(&chat, |u| matches!(u, Update::TurnEnded(_)));
        assert_eq!(updates.last(), Some(&Update::TurnEnded(Stop::Finished)));
        let answer: String = updates
            .iter()
            .filter_map(|u| {
                if let Update::AssistantChunk(s) = u {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(answer, format!("Local answer {n}."));
        turn = updates
            .iter()
            .find_map(|u| {
                if let Update::PromptTaken(Some(id)) = u {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .unwrap();
    }
    chat.requests
        .try_send(Request::Rewind {
            prompt: Some(turn),
            blocks: prompt("Changed prompt"),
        })
        .unwrap();
    let fork = until(&chat, |u| {
        matches!(u, Update::TurnEnded(_) | Update::Rewound(Err(_)))
    });
    assert_eq!(
        fork.last(),
        Some(&Update::TurnEnded(Stop::Finished)),
        "{fork:?}"
    );
    let new_id = fork
        .iter()
        .find_map(|u| {
            if let Update::Ready { session, .. } = u {
                Some(session)
            } else {
                None
            }
        })
        .unwrap();
    assert_ne!(new_id, original);
    assert!(
        fork.iter()
            .any(|u| matches!(u, Update::Ready { mode:Some(m), .. } if m == "plan"))
    );
    assert!(
        fork.iter()
            .any(|u| matches!(u, Update::Access(a) if a.current.as_deref() == Some("auto")))
    );
    send(&chat, Command::Close);
    until(&chat, |u| u == &Update::Closed(None));
    let sessions = kou_codex::sessions(&launch, PATIENCE).unwrap();
    for (id, expected) in [(original, "Local prompt 2"), (new_id, "Changed prompt")] {
        assert!(sessions.iter().any(|s| s.id == *id));
        let chat = kou_codex::connect(launch.clone(), Some(id.clone()), None, None).unwrap();
        let replay = until(&chat, |u| {
            u == &Update::UserChunk(Block::Text(expected.into()))
        });
        assert!(replay.contains(&Update::UserChunk(Block::Text("Local prompt 1".into()))));
        assert!(replay.contains(&Update::AssistantChunk("Local answer 1.".into())));
        send(&chat, Command::Close);
        until(&chat, |u| u == &Update::Closed(None));
    }
}
