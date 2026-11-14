//! The command line's guidance, checked against the command line itself:
//! every command written in its texts runs through a real shell and the real
//! client (this crate's binary holds it, as `kou-desktop` does) against a fake
//! app, and what the texts say the commands print and return is what they do.

#![cfg(unix)]

use kou_ipc::{
    Activity, Harness, Incoming, Layout, Pane, PaneKind, Request, Response, Server, Session, Space,
    Tab,
};
use kou_mcp::Flavour;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// The program `$KOU_BIN` names in a pane.
const BIN: &str = env!("CARGO_BIN_EXE_kou-mcp");

/// The caller's own pane.
const ME: &str = "p1";

struct FakeApp {
    dir: tempfile::TempDir,
    socket: PathBuf,
    seen: Arc<Mutex<Vec<Request>>>,
    server: Option<Server>,
    thread: Option<JoinHandle<()>>,
}

impl FakeApp {
    fn start(answer: fn(&Request) -> Response) -> FakeApp {
        // A socket's path must stay under about a hundred bytes.
        let dir = tempfile::Builder::new()
            .prefix("kou-mcp-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = dir.path().join("kou.sock");
        let (server, incoming) = Server::bind(&socket).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let thread = thread::spawn({
            let seen = seen.clone();
            move || {
                while let Ok(Incoming { request, reply }) = incoming.recv_blocking() {
                    reply.send(answer(&request));
                    seen.lock().unwrap().push(request);
                }
            }
        });
        FakeApp {
            dir,
            socket,
            seen,
            server: Some(server),
            thread: Some(thread),
        }
    }

    fn seen(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }

    /// Run `script` with `sh`, as an agent's shell tool would, in a pane's
    /// environment and in `cwd`.
    fn run(&self, script: &str, cwd: &Path) -> Output {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("set -e\n{script}"))
            .current_dir(cwd)
            .env("KOU_BIN", BIN)
            .env("KOU_SOCKET", &self.socket)
            .env("KOU_PANE", ME)
            .env("KOU_SESSION", "s1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    }
}

impl Drop for FakeApp {
    fn drop(&mut self) {
        drop(self.server.take());
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

fn pane(id: &str, activity: Activity) -> Pane {
    Pane {
        id: id.into(),
        session: "s1".into(),
        harness: "codex".into(),
        title: "Fix flaky login test".into(),
        cwd: "/work".into(),
        kind: PaneKind::Terminal,
        activity,
        parent: (id != ME).then(|| ME.into()),
        since_ms: 0,
    }
}

/// An app where every worker is done by the time anyone waits for it.
fn answer(request: &Request) -> Response {
    match request {
        Request::Ping => Response::Pong {
            version: kou_ipc::VERSION,
        },
        Request::Harnesses => Response::Harnesses {
            harnesses: vec![Harness {
                id: "codex".into(),
                name: "Codex".into(),
                version: Some("0.160.0".into()),
                ready: true,
                message: None,
                native: true,
            }],
        },
        Request::PaneSpawn(spawn) => Response::Pane(Pane {
            harness: spawn.harness.clone(),
            title: spawn.title.clone().unwrap_or_default(),
            parent: spawn.from.clone(),
            ..pane("p7", Activity::Working)
        }),
        Request::PaneWait { panes, .. } => Response::Waited {
            panes: panes.iter().map(|id| pane(id, Activity::Idle)).collect(),
            timed_out: false,
        },
        Request::PaneRead { .. } => Response::Text {
            text: "> done: 2 files changed\n".into(),
        },
        Request::PaneInfo { pane: id } => Response::Pane(pane(id, Activity::Working)),
        Request::PaneList { .. } => Response::Panes {
            panes: vec![pane(ME, Activity::Working), pane("p7", Activity::Idle)],
        },
        Request::PaneSend { .. } | Request::PaneClose { .. } | Request::PaneFocus { .. } => {
            Response::Done
        }
        Request::TabSet { name, icon, .. } => Response::Tab(Tab {
            id: "t1".into(),
            session: "s1".into(),
            name: name.clone().unwrap_or_default(),
            named: true,
            icon: icon.clone(),
            layout: Layout::Panes,
            panes: vec![ME.into()],
            active: ME.into(),
            front: true,
        }),
        Request::TabIcons => Response::Text {
            text: "auto\neye\nbug".into(),
        },
        Request::TabList { .. } => Response::Tabs { tabs: Vec::new() },
        Request::SessionList { .. } => Response::Sessions {
            sessions: Vec::new(),
        },
        Request::SessionRename { name, .. } => Response::Session(Session {
            id: "s1".into(),
            name: name.clone(),
            ..Session::default()
        }),
        Request::SpaceList => Response::Spaces { spaces: Vec::new() },
        Request::SpaceRename { name, .. } => Response::Space(Space {
            id: "sp1".into(),
            name: name.clone(),
            ..Space::default()
        }),
        Request::Hook { .. } => Response::Done,
    }
}

/// The command lines of a text: every `sh` block as one script, and every
/// inline span that is a whole command (a bare `pane spawn` names one).
fn commands(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut block: Option<Vec<&str>> = None;
    for line in text.lines() {
        match (&mut block, line.trim_start()) {
            (None, "```sh") => block = Some(Vec::new()),
            (Some(lines), "```") => {
                found.push(lines.join("\n"));
                block = None;
            }
            (Some(lines), _) => lines.push(line),
            (None, _) => {
                for span in line.split('`').skip(1).step_by(2) {
                    let words = span.split_whitespace().count();
                    let noun = ["pane ", "tab ", "session ", "space ", "harness "]
                        .iter()
                        .any(|noun| span.starts_with(noun));
                    if span.starts_with("\"$KOU_BIN\" ") {
                        found.push(span.to_owned());
                    } else if noun && words >= 3 {
                        found.push(format!("\"$KOU_BIN\" {span}"));
                    }
                }
            }
        }
    }
    assert!(block.is_none(), "an unclosed block");
    found
}

/// The body of each quoted heredoc in `script`.
fn heredocs(script: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    let mut lines = script.lines();
    while let Some(line) = lines.next() {
        let Some((_, rest)) = line.split_once("<<'") else {
            continue;
        };
        let delimiter = rest.trim_end_matches('\'');
        let body: Vec<&str> = lines
            .by_ref()
            .take_while(|line| *line != delimiter)
            .collect();
        bodies.push(body.join("\n") + "\n");
    }
    bodies
}

#[test]
fn every_command_in_the_command_lines_texts_runs_and_does_what_they_say() {
    let flavour = Flavour::Commands;
    let mut scripts = commands(flavour.instructions());
    scripts.extend(commands(flavour.guide()));
    assert!(scripts.len() >= 15, "{} commands found", scripts.len());

    let app = FakeApp::start(answer);
    let cwd = app.dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    for script in &scripts {
        // The briefs the texts say were written with a file tool.
        for name in script
            .split_whitespace()
            .skip_while(|word| *word != "--prompt-file")
            .skip(1)
            .step_by(2)
        {
            std::fs::write(
                cwd.join(name),
                format!("The brief in {name}, with \"quotes\" and $HOME.\n"),
            )
            .unwrap();
        }
        let before = app.seen().len();
        let output = app.run(script, &cwd);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stderr.is_empty(),
            "{script}\n{:?}: {stderr}",
            output.status
        );
        let asked = app.seen()[before..].to_vec();
        assert!(
            !asked.is_empty() || script.contains("--help"),
            "{script} asked nothing"
        );

        let prompts: Vec<String> = asked
            .iter()
            .filter_map(|request| match request {
                Request::PaneSpawn(spawn) => {
                    // A new pane is started as the caller's child.
                    assert_eq!(spawn.from.as_deref(), Some(ME), "{script}");
                    spawn.prompt.clone()
                }
                Request::PaneSend { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        // A brief in a quoted heredoc arrives exactly as written.
        for body in heredocs(script) {
            assert!(
                prompts.contains(&body),
                "{script}\nnot sent as written: {body:?}\nsent: {prompts:?}"
            );
        }
        for request in &asked {
            match request {
                Request::PaneWait {
                    panes, timeout_ms, ..
                } => {
                    // Under a shell tool's usual two minutes, and never for yourself.
                    assert!(timeout_ms.is_some_and(|ms| ms < 120_000), "{script}");
                    assert!(!panes.iter().any(|pane| pane == ME), "{script}");
                }
                Request::PaneSend { pane, .. } | Request::PaneClose { pane } => {
                    assert_ne!(pane, ME, "{script}");
                }
                Request::PaneRead {
                    scrollback: true,
                    lines,
                    ..
                } => {
                    assert!(lines.is_some(), "{script}: --scrollback without --lines");
                }
                Request::PaneSpawn(spawn) if spawn.harness != "shell" => {
                    // A worker's brief goes through stdin or a file, not a quoted argument.
                    assert!(
                        spawn
                            .prompt
                            .as_deref()
                            .is_some_and(|prompt| prompt.ends_with('\n')),
                        "{script}"
                    );
                }
                _ => {}
            }
        }
    }
    // Every kind of request the texts teach was sent.
    let seen = app.seen();
    let sent = |kind: fn(&Request) -> bool| seen.iter().any(kind);
    assert!(sent(
        |request| matches!(request, Request::PaneSpawn(spawn) if spawn.placement == kou_ipc::Placement::Split)
    ));
    assert!(sent(
        |request| matches!(request, Request::PaneWait { panes, .. } if panes.len() == 2)
    ));
    assert!(sent(|request| matches!(
        request,
        Request::PaneRead {
            scrollback: true,
            ..
        }
    )));
    assert!(sent(|request| matches!(request, Request::PaneSend { .. })));
    assert!(sent(|request| matches!(request, Request::PaneClose { .. })));
    assert!(sent(
        |request| matches!(request, Request::PaneInfo { pane } if pane == ME)
    ));
    assert!(sent(
        |request| matches!(request, Request::TabSet { tab: None, pane: Some(pane), name: Some(_), .. } if pane == ME)
    ));
    assert!(sent(|request| matches!(
        request,
        Request::TabSet { icon: Some(_), .. }
    )));
    assert!(sent(|request| matches!(request, Request::Harnesses)));
}

#[test]
fn spawn_prints_the_new_panes_id_first_and_json_carries_the_parent() {
    let app = FakeApp::start(answer);
    let cwd = app.dir.path();
    let output = app.run(
        "\"$KOU_BIN\" pane spawn codex --title \"Fix flaky login test\" - <<'BRIEF'\nFix it.\nBRIEF",
        cwd,
    );
    let line = String::from_utf8(output.stdout).unwrap();
    let fields: Vec<&str> = line.trim_end().split('\t').collect();
    assert_eq!(
        fields,
        [
            "p7",
            "working",
            "codex",
            "terminal",
            "0s",
            "/work",
            "Fix flaky login test"
        ]
    );

    // The plain line has no parent; --json has it, null for the caller itself.
    let mine = String::from_utf8(app.run("\"$KOU_BIN\" pane info --json", cwd).stdout).unwrap();
    assert!(
        mine.contains("\"id\":\"p1\"") && mine.contains("\"parent\":null"),
        "{mine}"
    );
    let listed = String::from_utf8(app.run("\"$KOU_BIN\" pane list --json", cwd).stdout).unwrap();
    assert!(
        listed.contains("\"id\":\"p7\"") && listed.contains("\"parent\":\"p1\""),
        "{listed}"
    );
}

#[test]
fn a_wait_says_blocked_and_succeeds_or_exits_4_when_its_time_runs_out() {
    fn blocked(request: &Request) -> Response {
        match request {
            Request::PaneWait { panes, .. } => Response::Waited {
                panes: panes.iter().map(|id| pane(id, Activity::Blocked)).collect(),
                timed_out: false,
            },
            other => answer(other),
        }
    }
    fn busy(request: &Request) -> Response {
        match request {
            Request::PaneWait { panes, .. } => Response::Waited {
                panes: panes.iter().map(|id| pane(id, Activity::Working)).collect(),
                timed_out: true,
            },
            other => answer(other),
        }
    }
    let app = FakeApp::start(blocked);
    let output = app.run("\"$KOU_BIN\" pane wait p7 --timeout 110", app.dir.path());
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("p7\tblocked\t")
    );

    let app = FakeApp::start(busy);
    let output = app.run("\"$KOU_BIN\" pane wait p7 --timeout 110", app.dir.path());
    assert_eq!(output.status.code(), Some(4));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("p7\tworking\t")
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("timed out")
    );
    assert!(matches!(
        app.seen()[..],
        [Request::PaneWait {
            timeout_ms: Some(110_000),
            ..
        }]
    ));
    // Without --timeout the app is asked to wait for as long as it takes.
    drop(app.run("\"$KOU_BIN\" pane wait p7 || true", app.dir.path()));
    assert!(matches!(
        app.seen()[1],
        Request::PaneWait {
            timeout_ms: None,
            ..
        }
    ));
}

#[test]
fn the_exit_statuses_are_the_ones_the_text_gives() {
    fn refuse(_: &Request) -> Response {
        Response::Error {
            message: "no pane p8".into(),
        }
    }
    let app = FakeApp::start(refuse);
    let cwd = app.dir.path();
    let output = app.run("\"$KOU_BIN\" pane read p8", cwd);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("no pane p8")
    );
    assert_eq!(
        app.run("\"$KOU_BIN\" pane read p8 --no-such-option", cwd)
            .status
            .code(),
        Some(2)
    );
    // Under `set -e` a refusal stops the script.
    assert_eq!(
        app.run("\"$KOU_BIN\" pane read p8\necho reached", cwd)
            .stdout,
        b""
    );

    let gone = cwd.join("gone.sock");
    let output = Command::new(BIN)
        .args(["pane", "list"])
        .env("KOU_SOCKET", &gone)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
}
