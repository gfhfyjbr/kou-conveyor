//! The socket end to end: a real listener and real clients in a temporary directory.

#![cfg(unix)]

use kou_ipc::{Incoming, NotRunning, Pane, Request, Response, Server, Until, VERSION, call};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::thread;
use std::time::{Duration, Instant};

const SOON: Option<Duration> = Some(Duration::from_secs(5));

#[test]
fn a_request_is_answered_over_a_private_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("kou.sock");
    let (server, incoming) = Server::bind(&path).unwrap();
    assert_eq!(server.path(), path);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    let parent = std::fs::metadata(path.parent().unwrap()).unwrap();
    assert_eq!(parent.permissions().mode() & 0o777, 0o700);

    let app = thread::spawn(move || {
        let Incoming { request, reply } = incoming.recv_blocking().unwrap();
        assert_eq!(request, Request::Ping);
        reply.send(Response::Pong { version: VERSION });
    });
    assert_eq!(
        call(&path, &Request::Ping, SOON).unwrap(),
        Response::Pong { version: VERSION }
    );
    app.join().unwrap();

    // A second instance must not take the socket from a live one.
    let error = Server::bind(&path).err().unwrap().to_string();
    assert!(error.contains("already listens"), "{error}");

    drop(server);
    assert!(!path.exists(), "dropping the server removes the socket");
    let error = call(&path, &Request::Ping, SOON).unwrap_err();
    assert!(error.downcast_ref::<NotRunning>().is_some(), "{error:#}");
}

#[test]
fn a_wait_is_held_open_while_other_requests_are_served() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kou.sock");
    let (_server, incoming) = Server::bind(&path).unwrap();

    let waiter = thread::spawn({
        let path = path.clone();
        move || {
            let wait = Request::PaneWait {
                panes: vec!["p2".into()],
                until: Until::Idle,
                any: false,
                timeout_ms: None,
            };
            // No client timeout: the app decides how long this takes.
            (call(&path, &wait, None).unwrap(), Instant::now())
        }
    });
    let Incoming { request, reply } = incoming.recv_blocking().unwrap();
    assert!(matches!(request, Request::PaneWait { .. }));

    // With the wait parked, twenty other clients come and go.
    for round in 0..20 {
        let client = thread::spawn({
            let path = path.clone();
            move || {
                call(
                    &path,
                    &Request::PaneInfo {
                        pane: format!("p{round}"),
                    },
                    SOON,
                )
            }
        });
        let Incoming { request, reply } = incoming.recv_blocking().unwrap();
        let Request::PaneInfo { pane } = request else {
            panic!("unexpected {request:?}");
        };
        reply.send(Response::Pane(Pane {
            id: pane,
            ..Pane::default()
        }));
        let Response::Pane(pane) = client.join().unwrap().unwrap() else {
            panic!("expected a pane");
        };
        assert_eq!(pane.id, format!("p{round}"));
    }
    assert!(!waiter.is_finished(), "the wait must still be open");

    let released = Instant::now();
    reply.send(Response::Waited {
        panes: vec![Pane {
            id: "p2".into(),
            ..Pane::default()
        }],
        timed_out: false,
    });
    let (answer, answered) = waiter.join().unwrap();
    assert!(matches!(
        answer,
        Response::Waited {
            timed_out: false,
            ..
        }
    ));
    assert!(answered >= released);
}

#[test]
fn a_dropped_reply_answers_with_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kou.sock");
    let (_server, incoming) = Server::bind(&path).unwrap();
    let app = thread::spawn(move || drop(incoming.recv_blocking().unwrap()));
    let answer = call(&path, &Request::PaneClose { pane: "p1".into() }, SOON).unwrap();
    let Response::Error { message } = answer else {
        panic!("expected an error, got {answer:?}");
    };
    assert!(message.contains("without answering"), "{message}");
    app.join().unwrap();
}

#[test]
fn requests_left_unread_are_refused_when_the_app_stops_listening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kou.sock");
    let (_server, incoming) = Server::bind(&path).unwrap();
    // The app is shutting down: nobody will read the channel again.
    drop(incoming);
    let answer = call(&path, &Request::Ping, SOON).unwrap();
    assert!(matches!(answer, Response::Error { .. }), "{answer:?}");
}

#[test]
fn a_stale_socket_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kou.sock");
    // What a crashed instance leaves behind: the file, and no listener.
    drop(UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    let error = call(&path, &Request::Ping, SOON).unwrap_err();
    assert!(error.downcast_ref::<NotRunning>().is_some(), "{error:#}");

    let (_server, incoming) = Server::bind(&path).unwrap();
    let app = thread::spawn(move || {
        incoming.recv_blocking().unwrap().reply.send(Response::Done);
    });
    assert_eq!(call(&path, &Request::Ping, SOON).unwrap(), Response::Done);
    app.join().unwrap();
}

#[test]
fn a_file_that_is_not_a_socket_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "keep me").unwrap();
    let error = Server::bind(&path).err().unwrap().to_string();
    assert!(error.contains("not a socket"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
}

#[test]
fn garbage_on_the_socket_gets_an_error_line() {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::net::UnixStream;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kou.sock");
    let (_server, incoming) = Server::bind(&path).unwrap();
    let mut stream = UnixStream::connect(&path).unwrap();
    stream.write_all(b"{\"op\":\"levitate\"}\n").unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    let answer: Response = serde_json::from_str(&line).unwrap();
    let Response::Error { message } = answer else {
        panic!("expected an error");
    };
    assert!(message.contains("malformed request"), "{message}");
    assert!(incoming.try_recv().is_err(), "nothing reaches the app");
}

#[test]
fn the_client_gives_up_when_its_timeout_passes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kou.sock");
    let (_server, incoming) = Server::bind(&path).unwrap();
    let started = Instant::now();
    let error = call(&path, &Request::Ping, Some(Duration::from_millis(50))).unwrap_err();
    assert!(error.to_string().contains("in time"), "{error:#}");
    assert!(started.elapsed() < Duration::from_secs(2));
    // The request did arrive; answering a client that left is harmless.
    incoming.recv_blocking().unwrap().reply.send(Response::Done);
}
