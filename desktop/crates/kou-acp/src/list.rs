//! Asking an agent something outside any conversation: which sessions it
//! keeps (`session/list`), and which models a session of its would offer.
//! The agent is started, asked, and stopped. Nothing is sent to a model.

use crate::{
    Launch, process,
    selectors::{Selectors, Target},
};
use agent_client_protocol_schema::v1 as acp;
use anyhow::{Context as _, bail};
use kou_thread::{Choice, Provider};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// A session an agent keeps, as a list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastSession {
    /// The id to resume it by ([`crate::Options::resume`]).
    pub id: String,
    /// What the agent calls it; empty when it has no name for it.
    pub title: String,
    /// When it last changed, when the agent says.
    pub updated: Option<SystemTime>,
    /// The folder it works in.
    pub cwd: PathBuf,
}

// More than any list a person reads; an agent that pages for ever is cut off.
const PAGES: u64 = 20;

/// The models a session of an agent's offers, before there is one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offered {
    /// The models, as the agent orders them.
    pub models: Vec<Choice>,
    /// The groups it offers them in, when it does (see
    /// [`kou_thread::Provider`]).
    pub providers: Vec<Provider>,
    /// The one a new session starts with.
    pub current: Option<String>,
}

/// The sessions the agent keeps for the folder `cwd`, latest first. `None`
/// when the agent does not list its sessions. Blocks for as long as the
/// agent takes, and no longer than `patience`; call it off the main thread.
pub fn sessions(
    launch: &Launch,
    cwd: &Path,
    client: (&str, &str),
    patience: Duration,
) -> anyhow::Result<Option<Vec<PastSession>>> {
    let mut agent = Asked::start(launch, cwd, patience)?;
    let hello = agent.hello(client)?;
    let lists = hello
        .pointer("/agentCapabilities/sessionCapabilities/list")
        .is_some_and(|list| !list.is_null());
    if !lists {
        return Ok(None);
    }
    let mut found = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..PAGES {
        let mut params = json!({ "cwd": cwd });
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let listed: acp::ListSessionsResponse =
            serde_json::from_value(agent.call("session/list", params)?)
                .context("the agent's list of sessions could not be read")?;
        found.extend(listed.sessions.into_iter().map(|session| PastSession {
            id: session.session_id.0.to_string(),
            title: kou_thread::plain_title(&session.title.unwrap_or_default()).to_owned(),
            updated: session.updated_at.as_deref().and_then(rfc3339),
            cwd: session.cwd,
        }));
        cursor = listed.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    // Latest first; those the agent gave no time for keep its order, after.
    found.sort_by_key(|session| std::cmp::Reverse(session.updated));
    Ok(Some(found))
}

/// The models a session in the folder `cwd` would offer. An agent says so
/// only of a session, so one is opened and left at once: nothing is said in
/// it, and an agent keeps no session nothing was said in. `None` when the
/// agent offers no choice of model. Blocks like [`sessions`].
pub fn models(
    launch: &Launch,
    cwd: &Path,
    client: (&str, &str),
    patience: Duration,
) -> anyhow::Result<Option<Offered>> {
    let mut agent = Asked::start(launch, cwd, patience)?;
    agent.hello(client)?;
    let opened: acp::NewSessionResponse =
        serde_json::from_value(agent.call("session/new", json!({ "cwd": cwd, "mcpServers": [] }))?)
            .context("the agent's new session could not be read")?;
    let selectors = Selectors::new(opened.modes, opened.config_options);
    Ok(selectors.get(Target::Model).map(|selector| Offered {
        models: selector.choices.clone(),
        current: selector.current.clone(),
        providers: selectors.providers.clone(),
    }))
}

// An agent started to be asked a thing or two, and stopped when dropped.
struct Asked {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    deadline: Instant,
    next: u64,
}

impl Asked {
    fn start(launch: &Launch, cwd: &Path, patience: Duration) -> anyhow::Result<Self> {
        anyhow::ensure!(
            cwd.is_absolute(),
            "the folder must be absolute, got {}",
            cwd.display()
        );
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .envs(launch.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if !launch.cwd.as_os_str().is_empty() {
            command.current_dir(&launch.cwd);
        }
        process::own_group(&mut command);
        let mut child = command
            .spawn()
            .with_context(|| format!("could not start {}", launch.program.display()))?;
        let (stdin, stdout) = (child.stdin.take(), child.stdout.take());
        let (told, lines) = mpsc::channel::<String>();
        let mut asked = Self {
            child,
            stdin,
            lines,
            deadline: Instant::now() + patience,
            next: 1,
        };
        let Some(stdout) = stdout else {
            bail!("the agent process has no pipes");
        };
        // Read apart, so that an agent that says nothing cannot hold this
        // up for longer than it is given.
        thread::Builder::new()
            .name("kou-acp asking".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if told.send(line).is_err() {
                        break;
                    }
                }
            })
            .context("could not read the agent")?;
        // (`asked` is dropped, and the agent stopped, on that error too.)
        asked.next = 1;
        Ok(asked)
    }

    fn hello(&mut self, client: (&str, &str)) -> anyhow::Result<Value> {
        self.call(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {"name": client.0, "version": client.1},
            }),
        )
    }

    // Ask, and wait for the answer.
    fn call(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next;
        self.next += 1;
        let mut line = serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))?;
        line.push(b'\n');
        let stdin = self
            .stdin
            .as_mut()
            .context("the agent process has no pipes")?;
        stdin
            .write_all(&line)
            .and_then(|()| stdin.flush())
            .context("the agent stopped listening")?;
        answer(id, &self.lines, self.deadline)
    }
}

impl Drop for Asked {
    fn drop(&mut self) {
        self.stdin = None;
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

// The answer to request `id`, whatever else the agent says meanwhile.
fn answer(id: u64, lines: &mpsc::Receiver<String>, deadline: Instant) -> anyhow::Result<Value> {
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        let line = match lines.recv_timeout(wait) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => bail!("the agent did not answer in time"),
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("the agent ended without answering"),
        };
        let Ok(mut message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) != Some(id) || message.get("method").is_some()
        {
            continue;
        }
        if let Some(error) = message.get("error") {
            let said = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("an error without a message");
            bail!("the agent answered: {said}");
        }
        return Ok(message
            .get_mut("result")
            .map(Value::take)
            .unwrap_or(Value::Null));
    }
}

/// A time as agents write it: `2026-11-12T12:06:23.336Z`, or with an offset
/// from UTC in place of the `Z`.
fn rfc3339(text: &str) -> Option<SystemTime> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    // Where the time of day ends and the offset begins.
    let end = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(end);
    let mut clock = clock.split(':');
    let hours = clock.next()?.parse::<i64>().ok()?;
    let minutes = clock.next()?.parse::<i64>().ok()?;
    let seconds = clock.next().unwrap_or("0").parse::<f64>().ok()?;
    let offset = match zone {
        "" | "Z" | "z" => 0,
        zone => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (hours, minutes) = zone[1..].split_once(':')?;
            sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60)
        }
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(0.0..61.0).contains(&seconds) {
        return None;
    }
    // Days since 1970-01-01 of a date in the Gregorian calendar.
    let (y, m) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let of_era = y.rem_euclid(400);
    let of_year = (153 * m + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    let whole = days * 86_400 + hours * 3600 + minutes * 60 - offset;
    let since = u64::try_from(whole).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(since) + Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read_as_agents_write_them() {
        let at = |seconds: u64, millis: u64| {
            Some(UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_millis(millis))
        };
        assert_eq!(rfc3339("1970-01-01T00:00:00Z"), at(0, 0));
        assert_eq!(rfc3339("2026-11-12T12:06:23.336Z"), at(1_794_485_183, 336));
        // The same instant, told from two hours east of it.
        assert_eq!(rfc3339("2026-11-12T14:06:23+02:00"), at(1_794_485_183, 0));
        assert_eq!(rfc3339("2000-02-29T23:59:59-00:30"), at(951_870_599, 0));
        assert_eq!(rfc3339("yesterday"), None);
        assert_eq!(rfc3339("2026-13-01T00:00:00Z"), None);
    }
}
