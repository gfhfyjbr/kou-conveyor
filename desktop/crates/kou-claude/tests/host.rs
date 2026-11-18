//! The host that keeps a program apart from the app (`kou_claude::host`),
//! against the scripted Claude Code (`examples/claude_fake.rs`) and a
//! stand-in of the host's own (`examples/host_echo.rs`). The host is
//! `examples/claude_host.rs`, a daemon: each test ends the hosts it started
//! and checks that they and their programs are gone, and kills what a
//! failing one left, by the process ids it saw (never by name). Run with
//! `cargo test -p kou-claude --test host`, which builds the examples.

#![cfg(unix)]

use kou_claude::host::{Attached, Exit, Keep, Reason, Received};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

/// Longer than anything here takes, short enough to fail a hung test.
const PATIENCE: Duration = Duration::from_secs(30);
/// How long the host waits for a client that stalls (its `STALL`), and a
/// little more.
const STALL: Duration = Duration::from_secs(3);

/// A program built next to this test.
fn example(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("this test's path");
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    assert!(
        path.exists(),
        "{} is missing: run `cargo test -p kou-claude`, which builds it",
        path.display()
    );
    path
}

/// A test's own folder (with a short path, for its socket), the processes
/// it saw started, and the means to end them.
struct Place {
    keep: Keep,
    /// The program's folder.
    work: PathBuf,
    /// The scripted Claude Code's home: never the user's.
    home: PathBuf,
    root: PathBuf,
    /// Process ids, each with when it started, so that an id given to
    /// another process since is never touched.
    held: Vec<(u32, String)>,
    _folder: tempfile::TempDir,
}

impl Place {
    fn new() -> Self {
        let folder = tempfile::Builder::new()
            .prefix("kh")
            .tempdir_in("/tmp")
            .expect("a temporary folder");
        // Resolved, so that the fake's own idea of its folder matches.
        let root = folder.path().canonicalize().expect("a real path");
        let (work, home) = (root.join("work"), root.join("home"));
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let keep = Keep {
            host: example("claude_host"),
            host_args: Vec::new(),
            socket: folder.path().join("s"),
            idle: Duration::from_secs(600),
            away: Duration::from_secs(86_400),
            stall: STALL,
        };
        Self {
            keep,
            work,
            home,
            root,
            held: Vec::new(),
            _folder: folder,
        }
    }

    fn start_fake(&mut self) -> Attached {
        let env = [(
            "CLAUDE_CONFIG_DIR".to_owned(),
            self.home.display().to_string(),
        )];
        let attached = self
            .keep
            .start(&example("claude_fake"), &[], &self.work, &env)
            .expect("the host starts");
        self.hold(&attached);
        attached
    }

    fn start_echo(&mut self) -> Attached {
        let attached = self.start_echo_or_not().expect("the host starts");
        self.hold(&attached);
        attached
    }

    fn start_echo_or_not(&self) -> anyhow::Result<Attached> {
        self.keep.start(&example("host_echo"), &[], &self.work, &[])
    }

    fn attach(&self) -> Attached {
        self.keep
            .attach()
            .expect("the host is reached")
            .expect("a host is there")
    }

    /// Hold the program of `attached` and its host (the program's parent).
    fn hold(&mut self, attached: &Attached) -> (u32, u32) {
        let program = attached.hello.pid;
        let host = parent_of(program).expect("the program has a parent");
        self.hold_pid(program);
        self.hold_pid(host);
        (program, host)
    }

    fn hold_pid(&mut self, pid: u32) {
        let started = started_at(pid).expect("the process runs");
        self.held.push((pid, started));
    }

    /// The host of `attached` (with all its program started), as held.
    fn host_of(&self, attached: &Attached) -> u32 {
        let program = attached.hello.pid;
        let at = self
            .held
            .iter()
            .position(|(pid, _)| *pid == program)
            .expect("a program held");
        self.held[at + 1].0
    }

    /// Wait until every process held is gone.
    fn wait_all_gone(&self) {
        let deadline = Instant::now() + PATIENCE;
        while let Some((pid, _)) = self.held.iter().find(|held| still(held)) {
            assert!(Instant::now() < deadline, "process {pid} is still running");
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// `#stop` a host, and see the program end, the connection close, the
    /// socket go, and every process held go.
    fn stop(&self, attached: Attached) -> Exit {
        attached.client.stop().expect("the host takes #stop");
        let exit = loop {
            match next(&attached) {
                Received::Exit(exit) => break exit,
                Received::Closed => panic!("the connection closed before the exit was told"),
                _ => {}
            }
        };
        assert_eq!(next(&attached), Received::Closed);
        assert!(!self.keep.socket.exists(), "the socket is still there");
        self.wait_all_gone();
        exit
    }

    fn last_path(&self) -> PathBuf {
        PathBuf::from(format!("{}.last", self.keep.socket.display()))
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        // What a failing test left behind.
        for held in &self.held {
            if still(held) {
                // SAFETY: a plain call, for a process this test started.
                unsafe { libc::kill(held.0 as libc::pid_t, libc::SIGKILL) };
            }
        }
    }
}

fn ps(pid: u32, field: &str) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", field, "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn started_at(pid: u32) -> Option<String> {
    ps(pid, "lstart=")
}

fn parent_of(pid: u32) -> Option<u32> {
    ps(pid, "ppid=")?.parse().ok()
}

/// The process held is still the one that runs under its id.
fn still((pid, started): &(u32, String)) -> bool {
    started_at(*pid).as_ref() == Some(started)
}

fn next(attached: &Attached) -> Received {
    attached
        .received
        .recv_timeout(PATIENCE)
        .expect("something from the host")
}

/// The next line of the program's that `wanted` picks, skipping others.
fn until(attached: &Attached, wanted: impl Fn(&Value) -> bool) -> Value {
    loop {
        match next(attached) {
            Received::Line(line) => {
                let message: Value = serde_json::from_str(&line).expect("a JSON line");
                if wanted(&message) {
                    return message;
                }
            }
            Received::Noise(_) | Received::Stderr(_) => {}
            other => panic!("the program is gone: {other:?}"),
        }
    }
}

fn of_type(kind: &str) -> impl Fn(&Value) -> bool + '_ {
    move |message| message["type"] == kind
}

fn state(word: &str) -> impl Fn(&Value) -> bool + '_ {
    move |message| {
        message["type"] == "system"
            && message["subtype"] == "session_state_changed"
            && message["state"] == word
    }
}

fn initialize(id: &str) -> String {
    json!({"type": "control_request", "request_id": id, "request": {"subtype": "initialize"}})
        .to_string()
}

fn prompt(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": text}}).to_string()
}

fn echo(text: &str) -> String {
    json!({"do": "echo", "text": text}).to_string()
}

fn exit_code(code: i32) -> Exit {
    Exit {
        code: Some(code),
        signal: None,
    }
}

fn signal(signal: i32) -> Exit {
    Exit {
        code: None,
        signal: Some(signal),
    }
}

#[test]
fn a_line_written_reaches_the_program_and_its_answer_comes_back() {
    let mut place = Place::new();
    let attached = place.start_fake();
    let hello = &attached.hello;
    assert_eq!(hello.version, 1);
    assert!(hello.fresh);
    assert!(hello.started > 0);
    assert_eq!(hello.greeting, None);
    assert_eq!(hello.state, None);
    assert!(hello.pending.is_empty());
    assert!(!hello.answered);
    attached.client.write(&initialize("init-1")).unwrap();
    let greeting = until(&attached, of_type("control_response"));
    assert_eq!(greeting["response"]["request_id"], "init-1");
    attached.client.write(&prompt("hello there")).unwrap();
    let answer = until(&attached, of_type("assistant"));
    assert_eq!(
        answer["message"]["content"][0]["text"],
        "You said: hello there"
    );
    until(&attached, state("idle"));
    // The program ended by a stop dies of the signal.
    assert_eq!(place.stop(attached), signal(libc::SIGTERM));
}

#[test]
fn the_program_runs_on_when_the_client_leaves_and_the_next_is_told_how_things_stand() {
    let mut place = Place::new();
    let first = place.start_fake();
    let program = first.hello.pid;
    first.client.write(&initialize("init-1")).unwrap();
    let greeting = until(&first, of_type("control_response"));
    first.client.write(&prompt("permission")).unwrap();
    let request = until(&first, of_type("control_request"));
    assert_eq!(request["request"]["subtype"], "can_use_tool");
    drop(first);

    let second = place.attach();
    let hello = &second.hello;
    assert!(!hello.fresh);
    assert_eq!(hello.pid, program);
    assert_eq!(hello.greeting.as_ref(), Some(&greeting));
    assert_eq!(hello.init.as_ref().unwrap()["subtype"], "init");
    assert_eq!(hello.state.as_ref().unwrap()["state"], "running");
    assert_eq!(hello.pending, std::slice::from_ref(&request));
    assert!(!hello.answered);
    // It hears what the program writes from then on.
    let allow = json!({"type": "control_response", "response": {
        "subtype": "success", "request_id": request["request_id"],
        "response": {"behavior": "allow", "updatedInput": request["request"]["input"]},
    }});
    second.client.write(&allow.to_string()).unwrap();
    let said = until(&second, of_type("assistant"));
    assert!(
        said["message"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(r#""behavior":"allow""#)
    );
    until(&second, state("idle"));
    drop(second);

    // The request answered is no longer pending.
    let third = place.attach();
    assert!(third.hello.pending.is_empty());
    assert_eq!(third.hello.state.as_ref().unwrap()["state"], "idle");
    assert!(!third.hello.answered);
    assert_eq!(third.hello.greeting.as_ref(), Some(&greeting));
    place.stop(third);
}

#[test]
fn a_client_that_comes_back_after_the_answer_is_told_it_was_given() {
    let mut place = Place::new();
    let first = place.start_fake();
    // The answer is given while an agent runs on in the background.
    first.client.write(&prompt("background")).unwrap();
    until(&first, of_type("result"));
    drop(first);

    let second = place.attach();
    assert!(second.hello.answered);
    assert_eq!(second.hello.state.as_ref().unwrap()["state"], "running");
    second.client.write(&prompt("more")).unwrap();
    until(&second, state("idle"));
    drop(second);

    let third = place.attach();
    assert!(!third.hello.answered);
    place.stop(third);
}

#[test]
fn a_request_taken_back_is_no_longer_pending() {
    let mut place = Place::new();
    let first = place.start_fake();
    first.client.write(&prompt("withdraw")).unwrap();
    until(&first, |message| {
        message["type"] == "control_cancel_request" && message["request_id"] == "elicit-w"
    });
    drop(first);

    let second = place.attach();
    assert!(second.hello.pending.is_empty());
    assert_eq!(second.hello.state.as_ref().unwrap()["state"], "running");
    second.client.write(&prompt("go on")).unwrap();
    until(&second, state("idle"));
    place.stop(second);
}

#[test]
fn a_second_client_takes_the_first_one_s_place() {
    let mut place = Place::new();
    let first = place.start_echo();
    let second = place.attach();
    assert!(!second.hello.fresh);
    // The first is told, last.
    assert_eq!(next(&first), Received::Replaced);
    assert_eq!(next(&first), Received::Closed);
    second.client.write(&echo("one")).unwrap();
    assert_eq!(next(&second), Received::Line(echo("one")));
    // Nothing more comes to the first.
    assert!(
        first
            .received
            .recv_timeout(Duration::from_millis(200))
            .is_err()
    );
    place.stop(second);
}

#[test]
fn a_client_whose_place_was_taken_is_no_longer_heard() {
    let mut place = Place::new();
    let first = place.start_echo();
    let program = first.hello.pid;
    // The first writes until the host has turned to the second: what it
    // writes from then on (a stop above all) is not taken.
    let second = place.attach();
    let _ = first.client.write(&echo("late"));
    let _ = first.client.stop();
    second.client.write(&echo("mine")).unwrap();
    assert_eq!(next(&second), Received::Line(echo("mine")));
    let again = place.attach();
    assert_eq!(again.hello.pid, program);
    assert_eq!(next(&second), Received::Replaced);
    place.stop(again);
}

#[test]
fn what_befalls_a_host_is_written_in_the_log_beside_its_socket() {
    let mut place = Place::new();
    let first = place.start_echo();
    let second = place.attach();
    assert_eq!(next(&first), Received::Replaced);
    place.stop(second);
    let log = place
        .keep
        .socket
        .parent()
        .unwrap()
        .join(kou_claude::host::LOG);
    let text = std::fs::read_to_string(&log).expect("the log is written");
    let said: Vec<&str> = text
        .lines()
        .map(|line| line.split_once(": ").expect("a line says something").1)
        .collect();
    assert!(said[0].starts_with("started "), "{text}");
    assert!(said[0].contains("host_echo (pid "), "{text}");
    assert_eq!(
        &said[1..],
        [
            "client 1 came",
            "client 2 took the place of client 1",
            "client 2 asked to stop",
            "host_echo was killed by signal 15",
            "ended (it was asked to stop): host_echo was killed by signal 15",
        ],
        "{text}"
    );
    assert!(text.lines().all(|line| line.contains(" s host ")), "{text}");
}

#[test]
fn what_the_program_says_beside_its_messages_is_told_as_noise_and_standard_error() {
    let mut place = Place::new();
    let attached = place.start_echo();
    let say = |what: &str, text: &str| json!({"do": what, "text": text}).to_string();
    attached.client.write(&say("noise", "plain words")).unwrap();
    assert_eq!(next(&attached), Received::Noise("plain words".into()));
    attached
        .client
        .write(&say("stderr", "a complaint"))
        .unwrap();
    assert_eq!(next(&attached), Received::Stderr("a complaint".into()));
    // Only a JSON object on a line of its own is for the program.
    assert!(attached.client.write("#stop").is_err());
    assert!(attached.client.write("{\"a\":\n1}").is_err());
    attached.client.write(&echo("still here")).unwrap();
    assert_eq!(next(&attached), Received::Line(echo("still here")));
    place.stop(attached);
}

#[test]
fn a_stop_ends_the_program_and_all_it_started_and_the_socket_is_gone() {
    let mut place = Place::new();
    let attached = place.start_fake();
    attached.client.write(&prompt("pid")).unwrap();
    let answer = until(&attached, of_type("assistant"));
    let pids: Vec<u32> = answer["message"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .split(' ')
        .map(|pid| pid.parse().unwrap())
        .collect();
    assert_eq!(pids[0], attached.hello.pid);
    // A command it left running, in its group.
    place.hold_pid(pids[1]);
    assert_eq!(place.stop(attached), signal(libc::SIGTERM));
    let last = place.keep.last().expect("word of the end");
    assert_eq!(last.reason, Reason::Stopped);
    assert_eq!(last.exit, signal(libc::SIGTERM));
}

#[test]
fn the_program_ending_by_itself_is_told_to_the_client_and_ends_the_host() {
    let mut place = Place::new();
    let attached = place.start_echo();
    attached
        .client
        .write(&json!({"do": "exit", "code": 4}).to_string())
        .unwrap();
    assert_eq!(next(&attached), Received::Stderr("leaving".into()));
    assert_eq!(next(&attached), Received::Exit(exit_code(4)));
    assert_eq!(next(&attached), Received::Closed);
    assert!(!place.keep.socket.exists());
    place.wait_all_gone();
}

#[test]
fn with_nobody_there_the_last_words_of_a_program_that_ended_are_kept_beside_the_socket() {
    let mut place = Place::new();
    let attached = place.start_echo();
    for n in 1..=25 {
        let complaint = json!({"do": "stderr", "text": format!("complaint {n}")});
        attached.client.write(&complaint.to_string()).unwrap();
    }
    let leave = json!({"do": "exit", "code": 5, "after_ms": 300});
    attached.client.write(&leave.to_string()).unwrap();
    drop(attached);
    place.wait_all_gone();
    assert!(!place.keep.socket.exists());
    let last = place.keep.last().expect("word of the end");
    assert_eq!(last.reason, Reason::Exited);
    assert_eq!(last.exit, exit_code(5));
    let mut words: Vec<String> = (7..=25).map(|n| format!("complaint {n}")).collect();
    words.push("leaving".into());
    assert_eq!(last.stderr, words);
    assert!(last.ended > 0);

    // A host started there anew takes the word away.
    let again = place.start_echo();
    assert_eq!(place.keep.last(), None);
    place.stop(again);
}

#[test]
fn an_idle_host_with_nobody_there_gives_up_after_its_time() {
    let mut place = Place::new();
    place.keep.idle = Duration::from_secs(1);
    let attached = place.start_fake();
    attached.client.write(&prompt("hello")).unwrap();
    until(&attached, state("idle"));
    let left = Instant::now();
    drop(attached);
    place.wait_all_gone();
    assert!(
        left.elapsed() >= Duration::from_millis(900),
        "it gave up at once"
    );
    assert!(!place.keep.socket.exists());
    let last = place.keep.last().expect("word of the end");
    assert_eq!(last.reason, Reason::Idle);
}

#[test]
fn a_host_whose_program_works_or_waits_for_an_answer_does_not_give_up_when_idle() {
    let mut working = Place::new();
    working.keep.idle = Duration::from_secs(1);
    let attached = working.start_fake();
    attached.client.write(&prompt("hang")).unwrap();
    until(&attached, of_type("assistant"));
    drop(attached);

    let mut asking = Place::new();
    asking.keep.idle = Duration::from_secs(1);
    let attached = asking.start_fake();
    attached.client.write(&prompt("permission")).unwrap();
    until(&attached, of_type("control_request"));
    drop(attached);

    thread::sleep(Duration::from_millis(2500));
    let attached = working.attach();
    assert_eq!(attached.hello.state.as_ref().unwrap()["state"], "running");
    working.stop(attached);
    let attached = asking.attach();
    assert_eq!(attached.hello.pending.len(), 1);
    asking.stop(attached);
}

#[test]
fn a_host_with_nobody_there_gives_up_after_its_away_time_whatever_its_program_does() {
    let mut place = Place::new();
    place.keep.away = Duration::from_secs(1);
    let attached = place.start_fake();
    attached.client.write(&prompt("hang")).unwrap();
    until(&attached, of_type("assistant"));
    drop(attached);
    place.wait_all_gone();
    let last = place.keep.last().expect("word of the end");
    assert_eq!(last.reason, Reason::Away);
    assert_eq!(last.exit, signal(libc::SIGTERM));
}

#[test]
fn a_host_ignores_a_hangup_and_sent_sigterm_ends_its_program_and_takes_its_socket_away() {
    let mut place = Place::new();
    let attached = place.start_echo();
    let host = place.host_of(&attached);
    // SAFETY: a plain call, for the host this test started.
    unsafe { libc::kill(host as libc::pid_t, libc::SIGHUP) };
    attached.client.write(&echo("still")).unwrap();
    assert_eq!(next(&attached), Received::Line(echo("still")));
    // SAFETY: as above.
    unsafe { libc::kill(host as libc::pid_t, libc::SIGTERM) };
    assert_eq!(next(&attached), Received::Exit(signal(libc::SIGTERM)));
    assert_eq!(next(&attached), Received::Closed);
    assert!(!place.keep.socket.exists());
    place.wait_all_gone();
    assert_eq!(
        place.keep.last().expect("word of the end").reason,
        Reason::Terminated
    );
}

#[test]
fn a_second_host_for_the_same_socket_refuses_to_start() {
    let mut place = Place::new();
    let first = place.start_echo();
    let program = first.hello.pid;
    let error = place
        .start_echo_or_not()
        .err()
        .expect("the second host refuses");
    let socket = place.keep.socket.display();
    assert_eq!(
        format!("{error:#}"),
        format!("the keeper at {socket} did not start: a keeper already listens at {socket}")
    );
    // The host there goes on, though the second one's look at it took the
    // place of its client.
    assert_eq!(next(&first), Received::Replaced);
    assert_eq!(next(&first), Received::Closed);
    let again = place.attach();
    assert_eq!(again.hello.pid, program);
    again.client.write(&echo("still")).unwrap();
    assert_eq!(next(&again), Received::Line(echo("still")));
    place.stop(again);
}

#[test]
fn a_stale_socket_file_is_replaced() {
    let mut place = Place::new();
    drop(UnixListener::bind(&place.keep.socket).unwrap());
    assert!(place.keep.socket.exists());
    std::fs::write(
        place.last_path(),
        r#"{"reason":"idle","exit":{"signal":15}}"#,
    )
    .unwrap();
    assert_eq!(
        place.keep.last().map(|last| last.reason),
        Some(Reason::Idle)
    );
    // Nothing listens there.
    assert!(place.keep.attach().unwrap().is_none());
    let attached = place.start_echo();
    assert_eq!(place.keep.last(), None);
    attached.client.write(&echo("fresh")).unwrap();
    assert_eq!(next(&attached), Received::Line(echo("fresh")));
    place.stop(attached);
}

#[test]
fn a_host_that_cannot_start_its_program_says_so_to_whoever_started_it() {
    let place = Place::new();
    let socket = place.keep.socket.display();
    let error = place
        .keep
        .start(Path::new("/no/such/program"), &[], &place.work, &[])
        .err()
        .expect("no program");
    assert_eq!(
        format!("{error:#}"),
        format!(
            "the keeper at {socket} did not start: could not start /no/such/program: No such file or directory (os error 2)"
        )
    );
    assert!(!place.keep.socket.exists());
    let error = place
        .keep
        .start(
            &example("host_echo"),
            &[],
            Path::new("/no/such/folder"),
            &[],
        )
        .err()
        .expect("no folder");
    assert_eq!(
        format!("{error:#}"),
        format!("the keeper at {socket} did not start: the folder /no/such/folder does not exist")
    );
    assert!(!place.keep.socket.exists());
}

#[test]
fn a_program_that_ends_at_once_is_said_to_with_its_last_words() {
    let place = Place::new();
    let env = [
        ("CLAUDE_FAKE_CRASH".to_owned(), "1".to_owned()),
        (
            "CLAUDE_CONFIG_DIR".to_owned(),
            place.home.display().to_string(),
        ),
    ];
    let words = [
        "fake claude: cannot read its settings",
        "fake claude: giving up",
    ];
    let fake = example("claude_fake");
    match place.keep.start(&fake, &[], &place.work, &env) {
        // Gone before it could be attached to.
        Err(error) => assert_eq!(
            format!("{error:#}"),
            format!(
                "{} exited with code 3 at once:\n{}",
                fake.display(),
                words.join("\n")
            )
        ),
        // Attached in time to hear it end.
        Ok(attached) => {
            let exit = loop {
                match next(&attached) {
                    Received::Exit(exit) => break exit,
                    Received::Stderr(_) => {}
                    other => panic!("not the end: {other:?}"),
                }
            };
            assert_eq!(exit, exit_code(3));
            assert_eq!(next(&attached), Received::Closed);
        }
    }
    let last = place.keep.last().expect("word of the end");
    assert_eq!(last.reason, Reason::Exited);
    assert_eq!(last.exit, exit_code(3));
    assert_eq!(last.stderr, words);
    assert!(!place.keep.socket.exists());
}

#[test]
fn a_client_that_stalls_is_dropped_and_the_program_is_not_held_up() {
    let mut place = Place::new();
    drop(place.start_echo());
    let mut stalled = UnixStream::connect(&place.keep.socket).unwrap();
    stalled.set_read_timeout(Some(PATIENCE)).unwrap();
    let mut reader = BufReader::new(stalled.try_clone().unwrap());
    let mut hello = String::new();
    reader.read_line(&mut hello).unwrap();
    assert!(hello.starts_with("#hello "), "{hello}");
    // Far more than the socket holds; and from now on nothing is read.
    let lines = 4000;
    let mark = place.root.join("mark");
    let flood = json!({"do": "flood", "lines": lines, "mark": mark});
    writeln!(stalled, "{flood}").unwrap();
    let asked = Instant::now();
    while !mark.exists() {
        assert!(asked.elapsed() < PATIENCE, "the program never finished");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        asked.elapsed() < STALL,
        "the program was held up for {:?}",
        asked.elapsed()
    );
    // The host gives up on the client: the connection ends before all of it
    // was written.
    thread::sleep(STALL + Duration::from_secs(2));
    let mut rest = Vec::new();
    reader.read_to_end(&mut rest).expect("the connection ends");
    let got = rest.iter().filter(|byte| **byte == b'\n').count();
    assert!(got < lines, "all {got} lines came");
    // Whoever comes next is served.
    let attached = place.attach();
    attached.client.write(&echo("next")).unwrap();
    assert_eq!(next(&attached), Received::Line(echo("next")));
    place.stop(attached);
}
