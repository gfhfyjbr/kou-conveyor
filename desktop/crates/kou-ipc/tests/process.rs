//! The client as a real process, started the way a harness starts a hook:
//! through `sh -c '"$KOU_BIN" hook <harness> <event>'`.

#![cfg(unix)]

use kou_ipc::{Incoming, Request, Response, Server, Session, Space, Tab, VERSION};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CLIENT: &str = env!("CARGO_BIN_EXE_kou-ipc-cli");

/// The hook line kou-agents writes into a harness's settings.
const HOOK: &str = r#""$KOU_BIN" hook claude PreToolUse"#;

/// What reaches the app within `wait`. A tool call's hook gives up on the
/// app when its short budget runs out, which a machine busy with the rest
/// of the tests can make happen: `None` then, not a test that waits for ever.
fn arrives(incoming: &async_channel::Receiver<Incoming>, wait: Duration) -> Option<Incoming> {
    let until = Instant::now() + wait;
    loop {
        if let Ok(incoming) = incoming.try_recv() {
            return Some(incoming);
        }
        if Instant::now() >= until {
            return None;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn shell(line: &str, socket: &Path) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", line])
        .env("KOU_BIN", CLIENT)
        .env("KOU_SOCKET", socket)
        .env("KOU_PANE", "p7")
        .env("KOU_SESSION", "s1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run_with_stdin(mut command: Command, input: &[u8]) -> (Output, Duration) {
    let started = Instant::now();
    let mut child = command.stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    (output, started.elapsed())
}

#[test]
fn a_hook_with_no_app_exits_zero_quickly_and_silently() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("absent.sock");
    let (output, took) = run_with_stdin(shell(HOOK, &socket), br#"{"tool_name":"Bash"}"#);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Two process starts and no waiting; a loaded machine gets some slack.
    assert!(took < Duration::from_millis(1000), "took {took:?}");
    println!("hook with no app: {took:?}");
}

#[test]
fn a_hook_whose_stdin_never_closes_still_returns() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("kou.sock");
    let (_server, incoming) = Server::bind(&socket).unwrap();
    // The hook always returns in time. Its event arrives unless the machine
    // was too busy for the hook's budget, so that part gets a few tries.
    for attempt in 1.. {
        let started = Instant::now();
        let mut child = shell(HOOK, &socket).stdin(Stdio::piped()).spawn().unwrap();
        // Hold the pipe open and write nothing, as a harness that passes its
        // payload some other way would.
        let held = child.stdin.take().unwrap();
        let status = child.wait().unwrap();
        let took = started.elapsed();
        drop(held);
        assert_eq!(status.code(), Some(0));
        assert!(took < Duration::from_millis(1500), "took {took:?}");
        // The event still arrives; only the payload it never got is missing.
        let Some(arrived) = arrives(&incoming, Duration::from_millis(500)) else {
            assert!(attempt < 8, "the event never arrived in {attempt} tries");
            continue;
        };
        assert_eq!(
            arrived.request,
            Request::Hook {
                pane: "p7".into(),
                harness: "claude".into(),
                event: "PreToolUse".into(),
                payload: serde_json::Value::Null,
            }
        );
        println!("hook with stdin held open: {took:?}");
        break;
    }
}

#[test]
fn a_hook_does_not_wait_for_a_busy_app_and_its_event_still_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("kou.sock");
    let (_server, incoming) = Server::bind(&socket).unwrap();
    // The app takes nothing in for a while, as one busy drawing would, and
    // answers after the hook has gone.
    let busy = Duration::from_millis(1200);
    let app = thread::spawn(move || {
        thread::sleep(busy);
        let Incoming { request, reply } = incoming.recv_blocking().unwrap();
        reply.send(Response::Done);
        request
    });
    let stop = r#""$KOU_BIN" hook claude Stop"#;
    let (output, took) = run_with_stdin(shell(stop, &socket), br#"{"hook_event_name":"Stop"}"#);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert!(took < busy, "the hook waited for the app: {took:?}");
    assert_eq!(
        app.join().unwrap(),
        Request::Hook {
            pane: "p7".into(),
            harness: "claude".into(),
            event: "Stop".into(),
            payload: serde_json::json!({ "hook_event_name": "Stop" }),
        }
    );
}

#[test]
fn a_hook_reaches_the_app_with_its_payload() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("kou.sock");
    let (_server, incoming) = Server::bind(&socket).unwrap();
    // The ping comes last, and is the one request that waits for its answer
    // however busy the machine is.
    let app = thread::spawn(move || {
        let mut seen = Vec::new();
        loop {
            let Incoming { request, reply } = incoming.recv_blocking().unwrap();
            let last = request == Request::Ping;
            reply.send(match request {
                Request::Ping => Response::Pong { version: VERSION },
                _ => Response::Done,
            });
            seen.push(request);
            if last {
                return seen;
            }
        }
    });

    // A turn's end, whose hook waits for a busy app (a tool call's would
    // give up on a machine busy with the other tests).
    let payload = br#"{"hook_event_name":"Stop","last_assistant_message":"npm test passes"}"#;
    let ended = r#""$KOU_BIN" hook claude Stop"#;
    let (output, _) = run_with_stdin(shell(ended, &socket), payload);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty() && output.stderr.is_empty());

    // Codex's notify program gets its payload as an argument and no stdin to speak of.
    let notify = format!(
        r#""$KOU_BIN" hook codex notify '{}'"#,
        r#"{"type":"agent-turn-complete","turn-id":"12"}"#
    );
    let output = shell(&notify, &socket)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty() && output.stderr.is_empty());

    let output = shell(r#""$KOU_BIN" ping"#, &socket)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "pong (protocol 1)\n"
    );

    let seen = app.join().unwrap();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_eq!(
        seen[0],
        Request::Hook {
            pane: "p7".into(),
            harness: "claude".into(),
            event: "Stop".into(),
            payload: serde_json::from_slice(payload).unwrap(),
        }
    );
    assert_eq!(
        seen[1],
        Request::Hook {
            pane: "p7".into(),
            harness: "codex".into(),
            event: "notify".into(),
            payload: serde_json::json!({ "type": "agent-turn-complete", "turn-id": "12" }),
        }
    );
    assert_eq!(seen[2], Request::Ping);
}

#[test]
fn a_pane_names_its_tab_session_and_space() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("kou.sock");
    let (_server, incoming) = Server::bind(&socket).unwrap();
    let app = thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..4 {
            let Incoming { request, reply } = incoming.recv_blocking().unwrap();
            reply.send(match &request {
                Request::TabSet { name, .. } => Response::Tab(Tab {
                    id: "t1".into(),
                    session: "s1".into(),
                    name: name.clone().unwrap_or_default(),
                    named: true,
                    panes: vec!["p7".into()],
                    active: "p7".into(),
                    front: true,
                    ..Tab::default()
                }),
                Request::SessionList { .. } => Response::Sessions {
                    sessions: vec![Session {
                        id: "s1".into(),
                        space: "w1".into(),
                        name: "Payments".into(),
                        cwd: "/work".into(),
                        tabs: 1,
                        selected: true,
                        ..Session::default()
                    }],
                },
                Request::SpaceRename { name, .. } => Response::Space(Space {
                    id: "w1".into(),
                    name: name.clone(),
                    path: "/work".into(),
                    sessions: 1,
                }),
                _ => Response::Done,
            });
            seen.push(request);
        }
        seen
    });
    let run = |line: &str| {
        let output = shell(line, &socket).stdin(Stdio::null()).output().unwrap();
        (
            output.status.code().unwrap(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    };

    // Quoted or not, it is one name.
    assert_eq!(
        run(r#""$KOU_BIN" tab rename 'api server'"#),
        (0, "t1\ts1\tfront\tpanes\t-\tp7\tapi server\n".into())
    );
    assert_eq!(
        run(r#""$KOU_BIN" tab rename api server"#),
        (0, "t1\ts1\tfront\tpanes\t-\tp7\tapi server\n".into())
    );
    assert_eq!(
        run(r#""$KOU_BIN" session list"#),
        (0, "s1\tw1\tselected\t1\t/work\tPayments\n".into())
    );
    assert_eq!(
        run(r#""$KOU_BIN" space rename -- -Client- work"#),
        (0, "w1\t1\t/work\t-Client- work\n".into())
    );

    let seen = app.join().unwrap();
    let rename = Request::TabSet {
        tab: None,
        pane: Some("p7".into()),
        name: Some("api server".into()),
        icon: None,
    };
    assert_eq!(seen[0], rename);
    assert_eq!(seen[1], rename);
    assert_eq!(seen[2], Request::SessionList { space: None });
    assert_eq!(
        seen[3],
        Request::SpaceRename {
            space: None,
            pane: Some("p7".into()),
            name: "-Client- work".into(),
        }
    );
}

#[test]
fn naming_outside_a_pane_says_what_to_pass() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("absent.sock");
    for (line, message) in [
        (
            r#""$KOU_BIN" tab rename api"#,
            "kou-desktop: tab rename needs a tab: no --tab was given and $KOU_PANE is not set\n",
        ),
        (
            r#""$KOU_BIN" space rename Work"#,
            "kou-desktop: space rename needs a space: no --space was given and $KOU_PANE is not set\n",
        ),
    ] {
        let output = shell(line, &socket)
            .env_remove("KOU_PANE")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        // 2, bad usage: the app (absent here, which would be 3) is never called.
        assert_eq!(output.status.code(), Some(2), "{line}");
        assert!(output.stdout.is_empty(), "{line}");
        assert_eq!(String::from_utf8_lossy(&output.stderr), message);
    }
    let output = shell(r#""$KOU_BIN" session rename"#, &socket)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with(
        "kou-desktop: session rename needs a name\n\nkou-desktop: a workspace for coding agents"
    ));
}

#[test]
fn exit_statuses_tell_the_caller_what_happened() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("absent.sock");
    let status = |line: &str| {
        let output = shell(line, &socket).stdin(Stdio::null()).output().unwrap();
        (output.status.code().unwrap(), output)
    };
    let (code, output) = status(r#""$KOU_BIN" ping"#);
    assert_eq!(code, 3, "the app is not running");
    assert!(String::from_utf8_lossy(&output.stderr).contains("not running"));
    let (code, output) = status(r#""$KOU_BIN" pane wait"#);
    assert_eq!(code, 2, "bad usage");
    assert!(output.stdout.is_empty());
    let (code, output) = status(r#""$KOU_BIN" --help"#);
    assert_eq!(code, 0);
    assert!(String::from_utf8_lossy(&output.stdout).contains("kou-desktop pane spawn <harness>"));
    // Outside a pane a hook has no one to tell, and says nothing.
    let output = shell(HOOK, &socket)
        .env_remove("KOU_PANE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
}
