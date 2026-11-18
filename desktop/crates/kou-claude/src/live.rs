//! A live session: the `claude` process, the exchange with it, and what the
//! conversation is told of it.
//!
//! The session runs on plain threads, as `kou-acp`'s does: one owns its
//! state ([`session`]), and one each reads Claude Code's output and its
//! standard error, writes its input, waits for its exit, and forwards the
//! interface's commands and requests. There is no async runtime.
//!
//! Claude Code is a child of the app's, or, with [`Options::keep`], the
//! program of a host that keeps it apart from the app ([`kept`]): a session
//! closed ends it, one let go of without a close leaves it working, and the
//! next session of the same keeper takes it up where it stands.

mod ask;
mod kept;
mod link;
mod partial;
mod process;
mod session;
mod stream;
mod wire;

use crate::{Offered, Options};
use anyhow::Context as _;
use kou_thread::Backend;
use link::{Event, Link, Resume};
use serde_json::Value;
use session::{Session, Wiring};
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// Start Claude Code for `options` and return the conversation's ends. A
/// program that cannot be started is an error here; whatever goes wrong
/// later is told in the updates ([`kou_thread::Update::Closed`]).
///
/// With [`Options::keep`], Claude Code is kept by a host of its own: one of
/// the session's that is still there is found and taken up as it stands,
/// else one is started.
pub fn connect(mut options: Options) -> anyhow::Result<Backend> {
    let (sender, events) = mpsc::channel();
    let program = link::program_name(&options);
    let mut next_host = 1;
    if let Some(keeper) = options.keep.clone() {
        let (found, next) = kept::find(&keeper);
        next_host = next;
        if let Some((n, attached)) = found {
            // Greeted, it is taken up where it stands; never greeted (it
            // was left as soon as it was started), it is greeted anew.
            let hello = attached.hello.clone();
            let socket = kept::keep_at(&keeper, n).socket;
            let link = Link::kept(0, attached, &sender, &program, socket)?;
            let found = hello.greeting.is_some().then_some(hello);
            let wiring = (link, found, next_host);
            return start_threads(options, None, wiring, sender, events);
        }
    }
    // A conversation nothing was said in has no record, and neither has one
    // whose record is gone: Claude Code refuses to go on with either, and
    // ends. Such a one is begun anew, and the session says so.
    let kept = options.resume.as_deref().is_none_or(|session| {
        options
            .home
            .clone()
            .or_else(crate::records::transcript::default_home)
            .and_then(|home| crate::records::transcript::record(&home, &options.cwd, session))
            .is_some()
    });
    let lost = if kept { None } else { options.resume.take() };
    let model = options.model.clone();
    let resume = match &options.resume {
        Some(session) => Resume::Session(session.clone()),
        None => Resume::New,
    };
    let args = link::arguments(&options, &resume, model.as_deref(), false);
    let link = match &options.keep {
        None => Link::start(0, &options, &args, &sender)?,
        Some(keeper) => {
            let attached = kept::start(keeper, 1, &options, &args)?;
            next_host = next_host.max(2);
            let socket = kept::keep_at(keeper, 1).socket;
            match Link::kept(0, attached, &sender, &program, socket) {
                Ok(link) => link,
                Err(error) => {
                    // The host is not left behind with nobody to find it.
                    kept::keep_at(keeper, 1)
                        .attach()
                        .ok()
                        .flatten()
                        .map(|attached| attached.client.stop());
                    return Err(error);
                }
            }
        }
    };
    let runner = link.runner();
    let started = start_threads(options, lost, (link, None, next_host), sender, events);
    if started.is_err() {
        // Half a session is none: Claude Code does not stay behind.
        runner.kill();
    }
    started
}

/// The link the session starts with, how things stand in the host it was
/// found in, and the number of the next host (see [`Wiring`]).
type Start = (Link, Option<crate::host::Hello>, u32);

fn start_threads(
    options: Options,
    lost: Option<String>,
    (link, found, next_host): Start,
    sender: mpsc::Sender<Event>,
    events: mpsc::Receiver<Event>,
) -> anyhow::Result<Backend> {
    let (backend, end) = kou_thread::channel();
    let spawn = |name: &str, body: Box<dyn FnOnce() + Send>| {
        thread::Builder::new()
            .name(name.into())
            .spawn(body)
            .with_context(|| format!("could not start the {name} thread"))
    };
    {
        let (sender, commands) = (sender.clone(), end.commands.clone());
        spawn(
            "kou-claude commands",
            Box::new(move || {
                while let Ok(command) = commands.recv_blocking() {
                    if sender.send(Event::Command(command)).is_err() {
                        return;
                    }
                }
                let _ = sender.send(Event::CommandsClosed);
            }),
        )?;
    }
    {
        let (sender, requests) = (sender.clone(), end.requests.clone());
        spawn(
            "kou-claude requests",
            Box::new(move || {
                while let Ok(request) = requests.recv_blocking() {
                    if sender.send(Event::Request(request)).is_err() {
                        return;
                    }
                }
            }),
        )?;
    }
    let session = Session::new(
        options,
        lost,
        Wiring {
            events,
            sender,
            updates: end.updates,
            commands: end.commands,
            requests: end.requests,
            link,
            found,
            next_host,
        },
    );
    spawn("kou-claude session", Box::new(move || session.run()))?;
    Ok(backend)
}

/// The models Claude Code offers under `options` (its account, its folder),
/// asked without opening a conversation: the process is started, greeted
/// (`initialize`), and ended. Nothing is sent to a model and no record is
/// written. Blocks no longer than `patience`; call it off the main thread.
/// (A child of the app's, kept or not: nothing of it is to outlive this.)
pub fn models(options: &Options, patience: Duration) -> anyhow::Result<Offered> {
    let deadline = Instant::now() + patience;
    let (sender, events) = mpsc::channel();
    let args = link::arguments(options, &Resume::New, options.model.as_deref(), true);
    let mut link = Link::start(0, options, &args, &sender)?;
    drop(sender);
    let answer = greet(&mut link, &events, deadline, patience);
    // However it went, the process and whatever it started end here.
    link.close_input();
    link.stop(Duration::from_millis(100));
    let answer = answer.map_err(|why| {
        let program = link::program_name(options);
        let mut reason = format!("{program} {why}");
        let last_words = link.last_words();
        if !last_words.is_empty() {
            reason.push_str(":\n");
            reason.push_str(&last_words.join("\n"));
        }
        anyhow::anyhow!(reason)
    })?;
    let models = wire::models(&answer);
    let current = wire::current_model(&models, options.model.as_deref());
    Ok(Offered {
        models: models.into_iter().map(|model| model.choice).collect(),
        current,
    })
}

/// Send `initialize` on `link` and wait for its answer, at most until
/// `deadline`. An error is what went wrong, said after the program's name.
fn greet(
    link: &mut Link,
    events: &mpsc::Receiver<Event>,
    mut deadline: Instant,
    patience: Duration,
) -> Result<Value, String> {
    const ID: &str = "kou-models";
    link.write(&wire::control_request(ID, wire::initialize()));
    let mut gone = false;
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        let event = match events.recv_timeout(wait) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) if gone => {
                return Err("ended before it answered".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "did not answer within {} seconds",
                    patience.as_secs_f32()
                ));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("ended before it answered".into());
            }
        };
        match event {
            Event::Message(_, message) => {
                let response = message.get("response").unwrap_or(&Value::Null);
                if wire::text_of(&message, "type") != Some("control_response")
                    || wire::text_of(response, "request_id") != Some(ID)
                {
                    continue;
                }
                if wire::text_of(response, "subtype") == Some("error") {
                    let error = wire::text_of(response, "error").unwrap_or("for no reason given");
                    return Err(format!("refused to start: {error}"));
                }
                return Ok(response.get("response").cloned().unwrap_or(Value::Null));
            }
            Event::Exited(_, exit) => link.exit = Some(exit),
            Event::StdoutClosed(_) => link.stdout_closed = true,
            Event::StderrClosed(_) => link.stderr_closed = true,
            _ => {}
        }
        if (link.exit.is_some() || link.stdout_closed) && !gone {
            // What it said last is still to be read: a moment longer.
            gone = true;
            deadline = deadline.min(Instant::now() + Duration::from_millis(500));
        }
        if gone && link.stderr_closed && link.exit.is_some() {
            return Err(match link.exit {
                Some(exit) => format!("exited with {} before it answered", exit.describe()),
                None => "ended before it answered".into(),
            });
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use kou_thread::Choice;
    use std::path::PathBuf;

    /// The scripted Claude Code (`examples/claude_fake.rs`), which `cargo
    /// test -p kou-claude` builds beside the tests.
    fn fake_claude() -> PathBuf {
        let mut path = std::env::current_exe().expect("this test's path");
        path.pop();
        path.pop();
        path.push("examples");
        path.push("claude_fake");
        assert!(path.exists(), "{} is missing", path.display());
        path
    }

    /// Options for the fake in a folder of its own, with a home of its own,
    /// and where it writes its process id.
    fn fake_options(folder: &tempfile::TempDir, env: &[(&str, &str)]) -> (Options, PathBuf) {
        let root = folder.path().canonicalize().unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let pid = root.join("pid");
        let mut options = Options::new(fake_claude(), &root);
        options.env = vec![
            ("CLAUDE_CONFIG_DIR".into(), home.display().to_string()),
            ("CLAUDE_FAKE_PIDFILE".into(), pid.display().to_string()),
        ];
        options.env.extend(
            env.iter()
                .map(|(name, value)| ((*name).into(), (*value).into())),
        );
        (options, pid)
    }

    fn gone(pid_file: &PathBuf) -> bool {
        // A process ended before it wrote its id (a busy machine starts it
        // late) never ran anything.
        let Ok(pid) = std::fs::read_to_string(pid_file) else {
            return true;
        };
        let pid: i32 = pid.parse().unwrap();
        // SAFETY: signal 0 only asks whether the process exists.
        unsafe { libc::kill(pid, 0) != 0 }
    }

    #[test]
    fn the_models_are_asked_of_claude_code_which_is_gone_afterwards() {
        let folder = tempfile::tempdir().unwrap();
        let (options, pid) = fake_options(&folder, &[]);
        let offered = models(&options, Duration::from_secs(30)).unwrap();
        let choice = |id: &str, name: &str, description: &str| Choice {
            id: id.into(),
            name: name.into(),
            description: Some(description.into()),
        };
        assert_eq!(
            offered,
            Offered {
                models: vec![
                    choice(
                        "default",
                        "Default (recommended)",
                        "Fake Opus for most work"
                    ),
                    choice("sonnet", "Sonnet", "Fake Sonnet"),
                    choice("haiku", "Haiku", "Fake Haiku"),
                ],
                current: Some("default".into()),
            }
        );
        assert!(gone(&pid));
        // Nothing was said in a conversation, so none was kept.
        assert!(!folder.path().join("home/projects").exists());

        // The one it would start with, when the options name one.
        let (mut options, _) = fake_options(&folder, &[]);
        options.model = Some("sonnet".into());
        let offered = models(&options, Duration::from_secs(30)).unwrap();
        assert_eq!(offered.current.as_deref(), Some("sonnet"));
    }

    #[test]
    fn claude_code_that_does_not_answer_in_time_is_an_error_and_is_ended() {
        let folder = tempfile::tempdir().unwrap();
        let (options, pid) = fake_options(&folder, &[("CLAUDE_FAKE_SILENT", "1")]);
        let error = models(&options, Duration::from_millis(300)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "claude_fake did not answer within 0.3 seconds"
        );
        assert!(gone(&pid));
    }

    #[test]
    fn claude_code_that_dies_before_it_answers_is_an_error_with_its_last_words() {
        let folder = tempfile::tempdir().unwrap();
        let (options, _) = fake_options(&folder, &[("CLAUDE_FAKE_CRASH", "1")]);
        let error = models(&options, Duration::from_secs(30)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "claude_fake exited with code 3 before it answered:\nfake claude: cannot read its settings\nfake claude: giving up"
        );
        let error = models(
            &Options::new("/no/such/claude", folder.path()),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "could not start /no/such/claude");
    }
}
