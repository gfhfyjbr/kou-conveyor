//! A real child on a real pseudo-terminal: what it prints is what [`Terminal::text`] reads,
//! what is submitted reaches it, and what it reports arrives as events.

#![cfg(unix)]

use gpui::{AppContext as _, Entity, TestAppContext};
use kou_term::{Exit, Spawn, Terminal, TerminalEvent};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

fn sh(script: &str) -> Spawn {
    Spawn {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), script.into()],
        env: Vec::new(),
        cwd: std::env::temp_dir(),
    }
}

type Events = Rc<RefCell<Vec<TerminalEvent>>>;

fn open(cx: &mut TestAppContext, spawn: Spawn) -> (Entity<Terminal>, Events) {
    // The child and its reader thread are outside the test scheduler's control.
    cx.executor().allow_parking();
    let terminal = cx
        .update(|cx| Terminal::open(spawn, cx))
        .expect("start the child");
    let events: Events = Rc::default();
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&terminal, move |_, event, _| {
            events.borrow_mut().push(event.clone());
        })
        .detach();
    });
    (terminal, events)
}

/// Lets the child, its reader thread and the test executor run until `done` holds.
/// The test clock only moves when told to, so each round also advances it by a frame.
fn wait_for(
    cx: &mut TestAppContext,
    terminal: &Entity<Terminal>,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        if Instant::now() >= deadline {
            let shown = terminal.read_with(cx, |terminal, _| terminal.text(None, true));
            panic!("timed out waiting for {what}; the terminal shows {shown:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
        cx.executor().advance_clock(Duration::from_millis(17));
    }
}

fn text(cx: &mut TestAppContext, terminal: &Entity<Terminal>) -> String {
    terminal.read_with(cx, |terminal, _| terminal.text(None, false))
}

fn exited(events: &Events) -> Option<Option<i32>> {
    events.borrow().iter().find_map(|event| match event {
        TerminalEvent::Exited(status) => Some(*status),
        _ => None,
    })
}

#[gpui::test]
fn prints_and_exits_with_its_status(cx: &mut TestAppContext) {
    let (terminal, events) = open(cx, sh("printf hello; exit 3"));
    assert!(terminal.read_with(cx, |terminal, _| terminal.pid().is_some()));
    wait_for(cx, &terminal, "the exit", |_| exited(&events).is_some());

    assert_eq!(exited(&events), Some(Some(3)));
    assert_eq!(text(cx, &terminal), "hello");
    terminal.read_with(cx, |terminal, _| {
        assert_eq!(terminal.exit_status(), Some(Some(3)));
        assert_eq!(terminal.bytes_received(), 5);
        let last = terminal.last_output().expect("the child wrote");
        assert!(last.elapsed() < Duration::from_secs(15));
    });
    assert!(events.borrow().contains(&TerminalEvent::Wakeup));
}

#[gpui::test]
fn spawn_inside_an_entity_constructor(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let terminal = cx.new(|cx| Terminal::spawn(sh("printf constructed"), cx).expect("start"));
    wait_for(cx, &terminal, "the output", |cx| {
        text(cx, &terminal) == "constructed"
    });
    wait_for(cx, &terminal, "the exit", |cx| {
        terminal.read_with(cx, |terminal, _| terminal.exit_status()) == Some(Some(0))
    });
}

#[gpui::test]
fn the_child_gets_the_terminal_environment(cx: &mut TestAppContext) {
    let mut spawn = sh("printf '%s|%s|%s|' \"$TERM\" \"$COLORTERM\" \"$KOU_TEST\"; pwd");
    spawn.env.push(("KOU_TEST".into(), "from spawn".into()));
    let directory = tempfile::tempdir().expect("a temporary directory");
    spawn.cwd = directory.path().canonicalize().expect("a real path");
    let (terminal, events) = open(cx, spawn.clone());
    wait_for(cx, &terminal, "the exit", |_| exited(&events).is_some());
    assert_eq!(
        text(cx, &terminal).replace('\n', ""),
        format!(
            "xterm-256color|truecolor|from spawn|{}",
            spawn.cwd.display()
        )
    );
}

#[gpui::test]
fn the_child_sees_a_resize(cx: &mut TestAppContext) {
    let (terminal, events) = open(cx, sh("stty size; read line; stty size"));
    wait_for(cx, &terminal, "the first size", |cx| {
        text(cx, &terminal).contains("24 80")
    });

    terminal.read_with(cx, |terminal, _| {
        assert_eq!(terminal.size(), (80, 24));
        terminal.resize(100, 30);
        assert_eq!(terminal.size(), (100, 30));
        terminal.write(b"\r");
    });
    wait_for(cx, &terminal, "the exit", |_| exited(&events).is_some());
    let text = text(cx, &terminal);
    assert!(text.contains("30 100"), "{text:?}");
}

#[gpui::test]
fn submit_reaches_the_program(cx: &mut TestAppContext) {
    let (terminal, events) = open(cx, sh("cat"));
    terminal.read_with(cx, |terminal, _| {
        terminal.submit("hello from another agent")
    });
    // The line discipline echoes what was typed and `cat` prints the line it then read.
    wait_for(cx, &terminal, "cat's copy of the line", |cx| {
        text(cx, &terminal) == "hello from another agent\nhello from another agent"
    });

    terminal.read_with(cx, |terminal, _| terminal.kill());
    wait_for(cx, &terminal, "the exit after kill", |_| {
        exited(&events).is_some()
    });
    // Hung up: signal 1.
    assert_eq!(exited(&events), Some(Some(129)));
}

#[gpui::test]
fn submissions_stay_in_order(cx: &mut TestAppContext) {
    // Echo is off, so the screen shows only what `cat` read and wrote back.
    let (terminal, _events) = open(cx, sh("stty -echo; echo ready; cat"));
    wait_for(cx, &terminal, "the program", |cx| {
        text(cx, &terminal) == "ready"
    });
    terminal.read_with(cx, |terminal, _| {
        terminal.submit("second");
        terminal.submit("third line\nof two");
        terminal.submit("");
    });
    wait_for(cx, &terminal, "every line", |cx| {
        text(cx, &terminal) == "ready\nsecond\nthird line\nof two"
    });
    assert_eq!(
        terminal.read_with(cx, |terminal, _| terminal.text(Some(2), false)),
        "third line\nof two"
    );
}

#[gpui::test]
fn paste_is_bracketed_when_the_program_asks(cx: &mut TestAppContext) {
    // `cat -v` shows the escape character as `^[`, so the markers can be read back.
    let (terminal, _events) = open(cx, sh("stty raw -echo; printf '\\033[?2004h'; cat -v"));
    wait_for(cx, &terminal, "bracketed paste mode", |cx| {
        terminal.read_with(cx, |terminal, _| terminal.bytes_received() > 0)
    });
    terminal.read_with(cx, |terminal, _| terminal.paste("two\nlines"));
    wait_for(cx, &terminal, "the pasted text", |cx| {
        text(cx, &terminal).contains("^[[200~two")
    });
    let shown = text(cx, &terminal).replace('\n', "");
    assert!(shown.contains("^[[200~two"), "{shown:?}");
    assert!(shown.contains("lines^[[201~"), "{shown:?}");

    // Enter after a submitted paste is a separate carriage return.
    terminal.read_with(cx, |terminal, _| terminal.submit("go"));
    wait_for(cx, &terminal, "the submitted text", |cx| {
        text(cx, &terminal).contains("^[[200~go^[[201~^M")
    });
}

#[gpui::test]
fn titles_notifications_progress_and_directory_arrive(cx: &mut TestAppContext) {
    let script = "printf '\\033]0;my title\\007'; \
                  printf '\\033]9;build finished\\007'; \
                  printf '\\033]777;notify;Agent;needs input\\007'; \
                  printf '\\033]99;i=1:d=0;Kitty\\033\\\\\\033]99;i=1:p=body;style\\033\\\\'; \
                  printf '\\033]9;4;1;42\\007'; \
                  printf '\\033]7;file://localhost/tmp/some%%20dir\\007'; \
                  printf '\\007'";
    let (terminal, events) = open(cx, sh(script));
    wait_for(cx, &terminal, "the exit", |_| exited(&events).is_some());

    let events = events.borrow();
    let notification = |title: Option<&str>, body: &str| TerminalEvent::Notification {
        title: title.map(str::to_string),
        body: body.to_string(),
    };
    for expected in [
        TerminalEvent::Title("my title".into()),
        notification(None, "build finished"),
        notification(Some("Agent"), "needs input"),
        notification(Some("Kitty"), "style"),
        TerminalEvent::Progress(Some(42)),
        TerminalEvent::Cwd(PathBuf::from("/tmp/some dir")),
        TerminalEvent::Bell,
        TerminalEvent::Exited(Some(0)),
    ] {
        assert!(
            events.contains(&expected),
            "missing {expected:?} in {events:?}"
        );
    }
    terminal.read_with(cx, |terminal, _| {
        assert_eq!(terminal.title().as_deref(), Some("my title"));
        assert_eq!(terminal.cwd(), Some(PathBuf::from("/tmp/some dir")));
    });
    // None of it is printed.
    assert_eq!(text(cx, &terminal), "");
}

#[gpui::test]
fn queries_are_answered_without_a_view(cx: &mut TestAppContext) {
    // The program asks for the cursor position, the background colour and the Kitty
    // keyboard flags, and prints the replies with the escapes made visible.
    let script = "stty raw -echo; printf 'ab\\033[6n\\033]11;?\\007\\033[?u'; \
                  dd bs=1 count=35 2>/dev/null | cat -v";
    let (terminal, events) = open(cx, sh(script));
    wait_for(cx, &terminal, "the replies", |cx| {
        exited(&events).is_some() || text(cx, &terminal).contains("^[[?0u")
    });
    let shown = text(cx, &terminal);
    assert!(shown.contains("^[[1;3R"), "{shown:?}");
    assert!(shown.contains("^[]11;rgb:0000/0000/0000^G"), "{shown:?}");
    assert!(shown.contains("^[[?0u"), "{shown:?}");
}

#[gpui::test]
fn kill_escalates_when_the_hang_up_is_ignored(cx: &mut TestAppContext) {
    let (terminal, events) = open(
        cx,
        sh("trap '' HUP; printf ready; while :; do sleep 1; done"),
    );
    wait_for(cx, &terminal, "the trap to be set", |cx| {
        text(cx, &terminal) == "ready"
    });
    let asked = Instant::now();
    terminal.read_with(cx, |terminal, _| terminal.kill());
    wait_for(cx, &terminal, "the exit after kill", |_| {
        exited(&events).is_some()
    });
    // Killed: signal 9, after the grace period.
    assert_eq!(exited(&events), Some(Some(137)));
    assert!(asked.elapsed() >= Duration::from_millis(900));
}

#[gpui::test]
fn dropping_the_terminal_ends_the_child(cx: &mut TestAppContext) {
    let (terminal, _events) = open(cx, sh("cat"));
    let pid = terminal
        .read_with(cx, |terminal, _| terminal.pid())
        .expect("a running child") as libc::pid_t;
    cx.update(|_| drop(terminal));
    cx.run_until_parked();
    let deadline = Instant::now() + Duration::from_secs(10);
    // Signal 0 only asks whether the process is still there.
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(Instant::now() < deadline, "the child outlived its terminal");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui::test]
fn a_program_that_cannot_start_is_an_error(cx: &mut TestAppContext) {
    let missing = Spawn {
        program: PathBuf::from("/nonexistent/kou-term-test-program"),
        args: Vec::new(),
        env: Vec::new(),
        cwd: std::env::temp_dir(),
    };
    let error = cx
        .update(|cx| Terminal::open(missing, cx))
        .expect_err("no such program");
    assert!(
        format!("{error:#}").contains("kou-term-test-program"),
        "{error:#}"
    );

    let mut nowhere = sh("true");
    nowhere.cwd = PathBuf::from("/nonexistent/kou-term-test-directory");
    let error = cx
        .update(|cx| Terminal::open(nowhere, cx))
        .expect_err("no such directory");
    assert!(
        format!("{error:#}").contains("working directory"),
        "{error:#}"
    );
}

#[gpui::test]
fn wakeups_are_coalesced(cx: &mut TestAppContext) {
    // Enough output for thousands of reads if every one of them woke the interface.
    let script = "i=0; while [ $i -lt 20000 ]; do echo \"line $i of many\"; i=$((i+1)); done";
    let (terminal, events) = open(cx, sh(script));
    let mut frames = 0;
    wait_for(cx, &terminal, "the exit", |_| {
        frames += 1;
        exited(&events).is_some()
    });
    let wakeups = events
        .borrow()
        .iter()
        .filter(|event| **event == TerminalEvent::Wakeup)
        .count();
    assert!(wakeups >= 1);
    // One per frame of test clock at most: `wait_for` advances a frame per round.
    assert!(
        wakeups <= frames + 1,
        "{wakeups} wakeups in {frames} frames"
    );
    let text = terminal.read_with(cx, |terminal, _| terminal.text(Some(1), true));
    assert_eq!(text, "line 19999 of many");
    // The history is capped at about ten thousand lines; the emulator drops history in
    // whole pages, so the exact count depends on how many lines fit a page.
    let all = terminal.read_with(cx, |terminal, _| terminal.text(None, true));
    let kept = all.lines().count();
    assert!((9_500..=10_500 + 23).contains(&kept), "{kept} lines kept");
}

/// Standard base64, for writing Kitty graphics commands into shell scripts.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (index, byte)| {
            value | u32::from(*byte) << (16 - 8 * index)
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([40, 160, 220, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode a PNG");
    bytes.into_inner()
}

#[gpui::test]
fn a_program_probes_for_pictures_and_shows_one(cx: &mut TestAppContext) {
    // The way icat and others start: a query whose answer decides how to send, then the
    // picture itself in chunks, then text after it.
    let encoded = base64(&png(16, 16));
    // Every chunk but the last is a whole number of base64 quads, as the protocol asks.
    let (first, rest) = encoded.split_at(encoded.len() / 8 * 4);
    let script = format!(
        "stty raw -echo; \\
         printf '\\033_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\\033\\\\'; \\
         dd bs=1 count=12 2>/dev/null | cat -v; printf ' probed\\r\\n'; \\
         printf 'above\\r\\n\\033_Ga=T,f=100,c=4,r=2,q=2,m=1;{first}\\033\\\\'; \\
         printf '\\033_Gm=0;{rest}\\033\\\\'; \\
         printf '\\r\\nbelow'"
    );
    let (terminal, events) = open(cx, sh(&script));
    wait_for(cx, &terminal, "the exit", |_| exited(&events).is_some());
    let shown = text(cx, &terminal);
    // The answer reached the program, and the picture left no text behind: it covers
    // two rows, the cursor stopped on the second, and "below" follows on the next line.
    assert_eq!(shown, "^[_Gi=31;OK^[\\ probed\nabove\n\n\nbelow");
}

#[gpui::test]
fn size_reports_agree_with_the_pty(cx: &mut TestAppContext) {
    // A program sizing a picture asks in three ways and reads the window size too; all
    // must describe the same cells.
    let script = "stty raw -echo min 0 time 0; printf '\\033[14t\\033[16t\\033[18t'; \\
                  sleep 0.5; dd bs=256 count=1 2>/dev/null | cat -v; printf ' '; \\
                  python3 -c 'import fcntl,termios,struct; \\
                  print(*struct.unpack(\"HHHH\", fcntl.ioctl(0, termios.TIOCGWINSZ, bytes(8))))'";
    let (terminal, events) = open(cx, sh(script));
    wait_for(cx, &terminal, "the exit", |_| exited(&events).is_some());
    let shown = text(cx, &terminal).replace('\n', "");
    // 80 by 24 cells of 8 by 18 pixels, the size a terminal has before a view measures it.
    assert!(
        shown.contains("^[[4;432;640t^[[6;18;8t^[[8;24;80t 24 80 640 432"),
        "{shown:?}"
    );
}

#[gpui::test]
fn the_exit_says_how_the_program_ended_and_how_long_it_ran(cx: &mut TestAppContext) {
    let (terminal, events) = open(cx, sh("sleep 0.2; exit 3"));
    wait_for(cx, &terminal, "the exit", |_| exited(&events).is_some());
    let exit: Exit = terminal
        .read_with(cx, |terminal, _| terminal.exit())
        .expect("an exit");
    assert_eq!(
        (exit.code, exit.signal, exit.status()),
        (Some(3), None, Some(3))
    );
    assert!(exit.ran_for >= Duration::from_millis(150), "{exit:?}");
    assert!(exit.ran_for < Duration::from_secs(10), "{exit:?}");

    let (terminal, events) = open(cx, sh("printf ready; sleep 30"));
    wait_for(cx, &terminal, "the program", |cx| {
        text(cx, &terminal) == "ready"
    });
    terminal.read_with(cx, |terminal, _| terminal.kill());
    wait_for(cx, &terminal, "the exit", |_| exited(&events).is_some());
    let exit = terminal
        .read_with(cx, |terminal, _| terminal.exit())
        .expect("an exit");
    // Hung up: signal 1, which a shell reports as 129.
    assert_eq!(
        (exit.code, exit.signal, exit.status()),
        (None, Some(1), Some(129))
    );
    assert!(!exit.succeeded());
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 delivers nothing; it only asks whether the process is there.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[gpui::test]
fn a_program_is_handed_to_another_terminal_and_does_not_notice(cx: &mut TestAppContext) {
    // A program that sets a mode, prints, and then answers what it is sent.
    let (terminal, _events) = open(
        cx,
        sh(r#"printf '\033[?2004h'; echo first; while read line; do echo "got $line"; done"#),
    );
    wait_for(cx, &terminal, "the first line", |cx| {
        text(cx, &terminal).contains("first")
    });
    let pid = terminal
        .read_with(cx, |terminal, _| terminal.pid())
        .expect("the program runs");
    assert!(terminal.read_with(cx, |terminal, _| terminal.takes_bracketed_paste()));

    // Let go: the terminal that started it is dropped, the program stays.
    let released = terminal
        .update(cx, |terminal, _| terminal.release())
        .expect("a running program can be released");
    assert_eq!(released.pid, pid);
    assert!(
        terminal
            .update(cx, |terminal, _| terminal.release())
            .is_none(),
        "once"
    );
    drop(terminal);
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        alive(pid),
        "dropping the terminal that let it go ends nothing"
    );

    // Carried on: the screen as it was, the mode it had set, and it answers.
    let adopted = cx
        .update(|cx| Terminal::adopt(released, cx))
        .expect("adopt the program");
    assert!(
        text(cx, &adopted).contains("first"),
        "{}",
        text(cx, &adopted)
    );
    assert!(adopted.read_with(cx, |terminal, _| terminal.takes_bracketed_paste()));
    assert_eq!(
        adopted.read_with(cx, |terminal, _| terminal.pid()),
        Some(pid)
    );
    adopted.read_with(cx, |terminal, _| terminal.submit("hello"));
    wait_for(cx, &adopted, "the answer", |cx| {
        text(cx, &adopted).contains("got hello")
    });

    // And handed on once more, as a second rebuild would.
    let released = adopted
        .update(cx, |terminal, _| terminal.release())
        .expect("released again");
    drop(adopted);
    cx.run_until_parked();
    let (third, events) = {
        let third = cx
            .update(|cx| Terminal::adopt(released, cx))
            .expect("adopt it again");
        let events: Events = Rc::default();
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&third, move |_, event, _| {
                events.borrow_mut().push(event.clone());
            })
            .detach();
        });
        (third, events)
    };
    let shown = text(cx, &third);
    assert!(
        shown.contains("first") && shown.contains("got hello"),
        "{shown}"
    );
    third.read_with(cx, |terminal, _| terminal.submit("again"));
    wait_for(cx, &third, "the second answer", |cx| {
        text(cx, &third).contains("got again")
    });

    // The terminal that holds it now is the one that ends it, and hears of it.
    third.read_with(cx, |terminal, _| terminal.kill());
    wait_for(cx, &third, "the end of the program", |_| {
        exited(&events).is_some()
    });
    assert!(!alive(pid));
}

#[gpui::test]
fn a_released_program_nobody_takes_is_ended(cx: &mut TestAppContext) {
    let (terminal, _events) = open(cx, sh("echo ready; sleep 30"));
    wait_for(cx, &terminal, "the program", |cx| {
        text(cx, &terminal).contains("ready")
    });
    let released = terminal
        .update(cx, |terminal, _| terminal.release())
        .expect("released");
    let pid = released.pid;
    released.discard();
    // The terminal that started it still waits on it, and reaps it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(pid));
    drop(terminal);
}
