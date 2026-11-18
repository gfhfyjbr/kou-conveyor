//! One private JSON-RPC connection to `codex app-server` over stdio.

use anyhow::{Context as _, Result, bail, ensure};
use async_channel::{Receiver, Sender};
use futures::{FutureExt as _, select_biased};
use kou_agents::Launch;
use kou_claude::host::{self, Hello, Keep, Received};
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    thread,
    time::Instant,
};

enum Process {
    Child(Child),
    Keeper(host::Client),
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Self::Child(child) = self {
            #[cfg(unix)]
            // SAFETY: this child leads its own group and has not been reaped.
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(crate) struct Rpc {
    process: Process,
    input: Option<Sender<String>>,
    pub incoming: Receiver<Result<Value>>,
    /// Notifications and server requests can arrive before an RPC response.
    pub buffered: VecDeque<Value>,
}

impl Rpc {
    pub fn start(launch: &Launch, keep: Option<&Keep>) -> Result<(Self, Option<Hello>)> {
        let (send, incoming) = async_channel::unbounded();
        let buffered = VecDeque::new();
        if let Some(keep) = keep {
            let attached = match keep.attach()? {
                Some(found) => found,
                None => keep.start(&launch.program, &launch.args, &launch.cwd, &launch.env)?,
            };
            thread::Builder::new()
                .name("codex keeper output".into())
                .spawn(move || {
                    for event in attached.received {
                        let message = match event {
                            Received::Line(line) => parse(&line),
                            Received::Exit(exit) => Err(anyhow::anyhow!("Codex {exit}")),
                            Received::Replaced => Err(anyhow::anyhow!(
                                "another pane attached to this Codex session"
                            )),
                            Received::Stderr(_) | Received::Noise(_) => continue,
                            Received::Closed => break,
                        };
                        if send.send_blocking(message).is_err() {
                            break;
                        }
                    }
                })?;
            return Ok((
                Self {
                    process: Process::Keeper(attached.client),
                    input: None,
                    incoming,
                    buffered,
                },
                Some(attached.hello),
            ));
        }
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .envs(launch.env.iter().cloned())
            .current_dir(&launch.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .spawn()
            .with_context(|| format!("could not start {} app-server", launch.program.display()))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let process = Process::Child(child);
        let (input, outgoing) = async_channel::unbounded();
        let errors = send.clone();
        thread::Builder::new()
            .name("codex input".into())
            .spawn(move || {
                if let Err(error) = write_lines(stdin, outgoing) {
                    let _ = errors.send_blocking(Err(error.into()));
                }
            })?;
        thread::Builder::new()
            .name("codex output".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let result = line
                        .map_err(anyhow::Error::from)
                        .and_then(|line| parse(&line));
                    if send.send_blocking(result).is_err() {
                        return;
                    }
                }
                let _ =
                    send.send_blocking(Err(anyhow::anyhow!("Codex app-server closed its output")));
            })?;
        Ok((
            Self {
                process,
                input: Some(input),
                incoming,
                buffered,
            },
            None,
        ))
    }

    /// Explicitly closing a pane stops its keeper. Dropping it detaches.
    pub fn stop(&self) {
        if let Process::Keeper(client) = &self.process {
            let _ = client.stop();
        }
    }

    pub fn write(&self, message: Value) -> Result<()> {
        let line = message.to_string();
        match &self.process {
            Process::Child(_) => self
                .input
                .as_ref()
                .expect("child writer")
                .try_send(line)
                .map_err(|_| anyhow::anyhow!("Codex's input is closed")),
            Process::Keeper(client) => Ok(client.write(&line)?),
        }
    }

    pub async fn initialize(&mut self, deadline: Instant) -> Result<()> {
        let params = json!({
            "clientInfo": {"name": "kou-desktop", "title": "kou", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true, "requiredHumanApproval": true}
        });
        self.call("initialize", params, deadline).await?;
        self.write(json!({"method":"initialized"}))
    }

    pub fn error(&self, id: &Value, code: i32, message: &str) -> Result<()> {
        self.write(json!({"id": id, "error": {"code": code, "message": message}}))
    }

    pub async fn call(&mut self, method: &str, params: Value, deadline: Instant) -> Result<Value> {
        // Responses still in flight from an earlier attachment cannot match.
        let id = uuid::Uuid::new_v4().to_string();
        self.write(json!({"id":id, "method":method, "params":params}))?;
        let mut timeout =
            futures_timer::Delay::new(deadline.saturating_duration_since(Instant::now())).fuse();
        loop {
            let message = select_biased! {
                _ = timeout => bail!("Codex did not answer {method} in time"),
                next = self.incoming.recv().fuse() => next.context("lost the connection to Codex")??,
            };
            if message.get("method").is_some() {
                self.buffered.push_back(message);
            } else if message["id"] == id {
                if let Some(error) = message.get("error") {
                    bail!(
                        "{method}: {}",
                        error["message"]
                            .as_str()
                            .unwrap_or("Codex refused the request")
                    );
                }
                return message
                    .get("result")
                    .cloned()
                    .with_context(|| format!("{method}: missing result"));
            }
        }
    }

    pub async fn pages(
        &mut self,
        method: &str,
        mut params: Value,
        deadline: Instant,
    ) -> Result<Vec<Value>> {
        let mut seen = HashSet::new();
        let mut all = Vec::new();
        loop {
            let mut page = self.call(method, params.clone(), deadline).await?;
            let data = page["data"]
                .as_array_mut()
                .with_context(|| format!("{method}: missing data"))?;
            all.append(data);
            let Some(cursor) = page["nextCursor"].as_str() else {
                return Ok(all);
            };
            ensure!(
                seen.insert(cursor.to_owned()),
                "{method}: repeated page cursor"
            );
            params["cursor"] = json!(cursor);
        }
    }
}

fn parse(line: &str) -> Result<Value> {
    serde_json::from_str(line).context("Codex wrote an invalid JSON-RPC message")
}

fn write_lines(mut stdin: ChildStdin, outgoing: Receiver<String>) -> std::io::Result<()> {
    while let Ok(line) = outgoing.recv_blocking() {
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
    }
    Ok(())
}
