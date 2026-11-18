use super::{
    access::Profiles,
    catalog::{Catalog, choice},
    deadline,
    items::{Items, input},
    requests::Requests,
    rpc::Rpc,
};
use anyhow::{Context as _, Result, ensure};
use futures::{FutureExt as _, select};
use kou_agents::Launch;
use kou_claude::host::Keep;
use kou_thread::{
    BackendEnd, Block, Capabilities, Choice, Command, Level, Offers, PlanStatus, PlanStep, Request,
    Stop, Update, Usage,
};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path};

pub(crate) async fn run(
    launch: &Launch,
    resume: Option<String>,
    model: Option<String>,
    access: Option<String>,
    keep: Option<&Keep>,
    end: &BackendEnd,
) -> Result<()> {
    let (mut rpc, hello) = Rpc::start(launch, keep)?;
    // Closing a pane must also interrupt a stalled handshake or RPC. Other
    // commands stay ordered while the worker waits for the server.
    let (forward, commands) = async_channel::unbounded();
    let session_end = BackendEnd {
        commands,
        requests: end.requests.clone(),
        updates: end.updates.clone(),
    };
    let closing = async {
        while let Ok(command) = end.commands.recv().await {
            if command == Command::Close {
                return true;
            }
            if forward.try_send(command).is_err() {
                break;
            }
        }
        false
    };
    let stop = futures::select_biased! {
        explicit = closing.fuse() => explicit,
        result = connected(&mut rpc, hello, &launch.cwd, resume, model, access, &session_end).fuse() => return result,
    };
    if stop {
        rpc.stop();
    }
    Ok(())
}

async fn connected(
    rpc: &mut Rpc,
    hello: Option<kou_claude::host::Hello>,
    cwd: &Path,
    resume: Option<String>,
    model: Option<String>,
    initial_access: Option<String>,
    end: &BackendEnd,
) -> Result<()> {
    let kept = hello.as_ref().and_then(|h| h.codex.as_ref());
    let resume = kept.and_then(|k| k.thread.clone()).or(resume);
    let prepared = async {
        if !kept.is_some_and(|k| k.initialized) {
            rpc.initialize(deadline()).await?;
        }
        let catalog = Catalog::read(rpc, deadline()).await;
        let modes = rpc
            .call("collaborationMode/list", json!({}), deadline())
            .await
            .ok()
            .and_then(|v| v["data"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|m| choice(m, "mode", "name"))
            .collect();
        let mut params = json!({"cwd": cwd});
        let profiles = Profiles::read(rpc, cwd, deadline()).await;
        if let Some(id) = &initial_access {
            profiles.apply(id, &mut params)?;
        }
        if let Some(model) = model {
            params["model"] = json!(model);
        }
        let running = kept.is_some_and(|k| k.turn.is_some());
        let mut opened = open_thread(rpc, params, resume.as_deref(), running).await?;
        let access = profiles.offered(&opened);
        if let Some(expected) = &initial_access {
            ensure!(
                access.current.as_ref() == Some(expected),
                "Codex did not apply the requested permissions ({expected})"
            );
        }
        let id = opened["thread"]["id"]
            .as_str()
            .context("Codex did not return its thread id")?
            .to_owned();
        let history = if resume.as_deref() == Some(&id) {
            history(rpc, &mut opened["thread"]).await?
        } else {
            Vec::new()
        };
        Ok::<_, anyhow::Error>((catalog, modes, profiles, access, opened, id, history))
    }
    .await;
    let (catalog, modes, profiles, access, opened, id, history) = match prepared {
        Ok(opened) => opened,
        Err(error) => {
            // A live keeper found on reconnect still owns its running turn.
            if hello.as_ref().is_none_or(|h| h.fresh) {
                rpc.stop();
            }
            return Err(error);
        }
    };
    let mut session = Session {
        rpc,
        catalog: catalog.unwrap_or_default(),
        modes,
        mode: opened["collaborationMode"]["mode"]
            .as_str()
            .map(str::to_owned),
        profiles,
        access,
        model: opened["model"].as_str().unwrap_or_default().into(),
        effort: opened["reasoningEffort"].as_str().map(str::to_owned),
        id,
        cwd,
        end,
        active: None,
        turns: Vec::new(),
        finished: HashSet::new(),
        items: Items::default(),
        requests: Requests::default(),
    };
    session.ready();
    if let Some(previous) = resume.as_deref().filter(|id| *id != session.id) {
        session.send(Update::Notice(
            Level::Info,
            format!("Codex has no saved history for {previous}. Started a new chat."),
        ));
    }
    if let Some(title) = opened["thread"]["name"].as_str() {
        session.send(Update::Title(title.into()));
    }
    let cached = kept.and_then(|k| k.turn.as_deref().zip(k.events.as_ref()));
    for turn in &history {
        if !(turn["status"] == "inProgress" && cached.is_some_and(|(id, _)| turn["id"] == id)) {
            session.replay(turn);
        }
    }
    if let Some(turn) = kept.and_then(|k| k.turn.as_deref()) {
        session.started(turn, false);
    }
    if let Some((id, events)) = cached
        && session.active.as_deref() == Some(id)
    {
        for event in events {
            session.message(event.clone())?;
        }
    }
    // Requests still outstanding when the previous view went away are not
    // part of thread history. The keeper saved their original RPC ids.
    for request in hello.into_iter().flat_map(|h| h.pending) {
        session.message(request)?;
    }
    session.listen().await
}

async fn open_thread(
    rpc: &mut Rpc,
    params: Value,
    resume: Option<&str>,
    running: bool,
) -> Result<Value> {
    if let Some(id) = resume {
        let mut request = params.clone();
        request["threadId"] = json!(id);
        match rpc.call("thread/resume", request, deadline()).await {
            Ok(opened) => return Ok(opened),
            // Codex assigns an ID at thread/start but writes its rollout
            // only after the first prompt. Other resume errors are actionable.
            Err(error)
                if !running
                    && error.to_string()
                        == format!("thread/resume: no rollout found for thread id {id}") => {}
            Err(error) => return Err(error),
        }
    }
    rpc.call("thread/start", params, deadline()).await
}

async fn history(rpc: &mut Rpc, thread: &mut Value) -> Result<Vec<Value>> {
    if thread["historyMode"] == "paginated" {
        return rpc
            .pages(
                "thread/turns/list",
                json!({"threadId":thread["id"],
            "sortDirection":"asc", "itemsView":"full"}),
                deadline(),
            )
            .await;
    }
    Ok(thread["turns"]
        .as_array_mut()
        .map(std::mem::take)
        .unwrap_or_default())
}

struct Session<'a> {
    rpc: &'a mut Rpc,
    id: String,
    cwd: &'a Path,
    end: &'a BackendEnd,
    catalog: Catalog,
    modes: Vec<Choice>,
    mode: Option<String>,
    profiles: Profiles,
    access: kou_thread::Access,
    model: String,
    effort: Option<String>,
    active: Option<String>,
    turns: Vec<String>,
    finished: HashSet<String>,
    items: Items,
    requests: Requests,
}

impl Session<'_> {
    fn send(&self, update: Update) {
        let _ = self.end.updates.try_send(update);
    }

    fn ready(&self) {
        self.send(Update::Ready {
            session: self.id.clone(),
            capabilities: Capabilities {
                images: true,
                embedded_files: true,
                resume: true,
            },
            models: self.catalog.offered(None).models,
            model: Some(self.model.clone()),
            modes: self.modes.clone(),
            mode: self.mode.clone().or_else(|| {
                self.modes
                    .iter()
                    .find(|m| m.id == "default")
                    .map(|m| m.id.clone())
            }),
        });
        self.send(Update::Offers(Offers {
            steer: true,
            rewind: true,
            ..Default::default()
        }));
        self.efforts();
        self.send(Update::Access(self.access.clone()));
    }

    fn efforts(&self) {
        let (levels, default) = self.catalog.efforts(&self.model);
        self.send(Update::Efforts {
            levels,
            current: self.effort.clone().or(default),
        });
    }

    fn started(&mut self, turn: &str, own_prompt: bool) {
        if self.active.as_deref() == Some(turn) || self.finished.contains(turn) {
            return;
        }
        self.active = Some(turn.into());
        self.turns.push(turn.into());
        self.items = Items::default();
        self.items.suppress_first_user = own_prompt;
        self.send(Update::TurnStarted);
    }

    fn ended(&mut self, turn: &Value) {
        let Some(id) = turn["id"].as_str() else {
            return;
        };
        if self.active.as_deref() != Some(id) {
            return;
        }
        for item in turn["items"].as_array().into_iter().flatten() {
            for update in self.items.item(id, item, true, self.cwd) {
                self.send(update);
            }
        }
        self.active = None;
        self.finished.insert(id.into());
        let stop = match turn["status"].as_str() {
            Some("completed") => Stop::Finished,
            Some("interrupted") => Stop::Cancelled,
            _ => Stop::Failed(
                turn["error"]["message"]
                    .as_str()
                    .unwrap_or("Codex could not finish the turn")
                    .into(),
            ),
        };
        self.send(Update::TurnEnded(stop));
        self.requests.turn_ended(&self.end.updates);
    }

    fn replay(&mut self, turn: &Value) {
        let Some(id) = turn["id"].as_str() else {
            return;
        };
        self.started(id, false);
        let active = turn["status"] == "inProgress";
        for item in turn["items"].as_array().into_iter().flatten() {
            for update in self.items.restore(id, item, active, self.cwd) {
                self.send(update);
            }
        }
        if !active {
            self.ended(turn);
        }
    }

    async fn listen(&mut self) -> Result<()> {
        let mut requests_open = true;
        loop {
            if let Some(message) = self.rpc.buffered.pop_front() {
                self.message(message)?;
                continue;
            }
            let requests = if requests_open {
                self.end.requests.recv().left_future()
            } else {
                futures::future::pending().right_future()
            };
            select! {
                message = self.rpc.incoming.recv().fuse() => self.message(message.context("lost the connection to Codex")??)?,
                cmd = self.end.commands.recv().fuse() => match cmd {
                    Ok(Command::Close) => { self.rpc.stop(); return Ok(()); }
                    Err(_) => return Ok(()),
                    Ok(command) => self.command(command).await?,
                },
                request = requests.fuse() => match request {
                    Ok(Request::Steer(blocks)) => {
                        if let Err(error) = self.steer(&blocks).await { self.send(Update::Notice(Level::Warning, error.to_string())); }
                    }
                    Ok(Request::Rewind { prompt, blocks }) => {
                        if let Err(error) = self.rewind(prompt.as_deref(), &blocks).await { self.send(Update::Rewound(Err(error.to_string()))); }
                    }
                    Err(_) => requests_open = false,
                }
            }
        }
    }

    async fn command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Prompt(blocks) => match self.start_turn(None, &blocks).await {
                Ok(turn) => {
                    self.send(Update::PromptTaken(Some(turn.clone())));
                    self.started(&turn, true);
                }
                Err(error) => {
                    self.send(Update::PromptTaken(None));
                    if self.active.is_none() {
                        self.send(Update::TurnEnded(Stop::Failed(error.to_string())));
                    } else {
                        self.send(Update::Notice(Level::Warning, error.to_string()));
                    }
                }
            },
            Command::Cancel => {
                if let Some(turn) = &self.active
                    && let Err(error) = self
                        .rpc
                        .call(
                            "turn/interrupt",
                            json!({"threadId":self.id,"turnId":turn}),
                            deadline(),
                        )
                        .await
                {
                    self.send(Update::Notice(Level::Warning, error.to_string()));
                }
            }
            Command::Permit { request, option } => {
                self.requests
                    .answer(self.rpc, request, Some(&option), &[], &self.end.updates)?
            }
            Command::Answer { request, answers } => {
                self.requests
                    .answer(self.rpc, request, None, &answers, &self.end.updates)?
            }
            Command::SetModel(model) => {
                if self
                    .catalog
                    .offered(None)
                    .models
                    .iter()
                    .any(|m| m.id == model)
                {
                    self.model = model.clone();
                    self.effort = self.catalog.efforts(&model).1;
                    self.send(Update::Model(model));
                    self.efforts();
                }
            }
            Command::SetMode(mode) if self.modes.iter().any(|m| m.id == mode) => {
                self.mode = Some(mode.clone());
                self.send(Update::Mode(mode));
            }
            Command::SetAccess(id) => {
                let mut params = json!({"threadId": self.id});
                let result = match self.profiles.apply(&id, &mut params) {
                    Ok(()) => self
                        .rpc
                        .call("thread/settings/update", params, deadline())
                        .await
                        .map(|_| ()),
                    Err(error) => Err(error),
                };
                // The response is empty. thread/settings/updated carries the effective policy.
                if let Err(error) = result {
                    self.send(Update::Notice(Level::Warning, error.to_string()));
                }
            }
            Command::SetEffort(effort)
                if self
                    .catalog
                    .efforts(&self.model)
                    .0
                    .iter()
                    .any(|e| e.id == effort) =>
            {
                self.effort = Some(effort);
                self.efforts();
            }
            _ => {}
        }
        Ok(())
    }

    async fn start_turn(&mut self, target: Option<&str>, blocks: &[Block]) -> Result<String> {
        ensure!(self.active.is_none(), "Codex is already working");
        ensure!(!blocks.is_empty(), "The prompt is empty");
        let mut params = json!({
            "threadId": target.unwrap_or(&self.id),
            "input": input(blocks),
            "model": self.model,
            "effort": self.effort
        });
        if let Some(mode) = &self.mode {
            params["collaborationMode"] = json!({"mode":mode,"settings":{"model":self.model,
                "reasoning_effort":self.effort,"developer_instructions":null}});
        }
        let response = self.rpc.call("turn/start", params, deadline()).await?;
        Ok(response["turn"]["id"]
            .as_str()
            .context("turn/start: missing turn id")?
            .into())
    }

    async fn steer(&mut self, blocks: &[Block]) -> Result<()> {
        let turn = self
            .active
            .as_ref()
            .context("Codex has already finished the turn")?;
        self.rpc
            .call(
                "turn/steer",
                json!({"threadId":self.id,"expectedTurnId":turn,"input":input(blocks)}),
                deadline(),
            )
            .await?;
        // The userMessage item places the accepted input where Codex read it.
        Ok(())
    }

    async fn rewind(&mut self, prompt: Option<&str>, blocks: &[Block]) -> Result<()> {
        ensure!(self.active.is_none(), "Stop Codex before editing a prompt");
        let mut target = self.id.clone();
        let mut keep = self.turns.len();
        if let Some(turn) = prompt {
            keep = self.turns.iter().position(|id| id == turn).context(
                "Codex can edit the first prompt of a turn. This message was sent during the turn.",
            )?;
            let mut params = json!({"cwd":self.cwd,"model":self.model});
            if let Some(access) = &self.access.current {
                self.profiles.apply(access, &mut params)?;
            }
            let method = if keep == 0 {
                "thread/start"
            } else {
                params["threadId"] = json!(self.id);
                params["lastTurnId"] = json!(self.turns[keep - 1]);
                "thread/fork"
            };
            let mut opened = self.rpc.call(method, params, deadline()).await?;
            if let Some(expected) = &self.access.current {
                ensure!(
                    self.profiles.offered(&opened).current.as_ref() == Some(expected),
                    "Codex did not retain the selected permissions for the edited prompt"
                );
            }
            target = opened["thread"]["id"]
                .as_str()
                .context("Codex did not return the new thread id")?
                .into();
            ensure!(
                target != self.id,
                "Codex did not create a separate thread for this edit"
            );
            let prefix = if keep > 0 {
                history(self.rpc, &mut opened["thread"]).await?
            } else {
                Vec::new()
            };
            ensure!(
                prefix
                    .iter()
                    .map(|t| t["id"].as_str())
                    .eq(self.turns[..keep].iter().map(|id| Some(id.as_str()))),
                "This Codex version could not fork at the selected turn"
            );
        }
        let turn = self.start_turn(Some(&target), blocks).await?;
        if target != self.id {
            self.id = target;
            self.turns.truncate(keep);
            self.finished = self.turns.iter().cloned().collect();
            self.ready();
        }
        self.send(Update::Rewound(Ok(turn.clone())));
        self.started(&turn, true);
        Ok(())
    }

    fn message(&mut self, message: Value) -> Result<()> {
        let Some(method) = message["method"].as_str() else {
            return Ok(());
        };
        let p = &message["params"];
        if p["threadId"].as_str().is_some_and(|id| id != self.id) {
            if let Some(id) = message.get("id") {
                self.rpc
                    .error(id, -32602, "Request belongs to a different thread")?;
            }
            return Ok(());
        }
        if message.get("id").is_some() {
            if p["isBlocking"] != false
                && p["turnId"]
                    .as_str()
                    .is_some_and(|id| self.finished.contains(id))
            {
                return Ok(());
            }
            return self.requests.receive(self.rpc, &message, &self.end.updates);
        }
        let turn = p["turnId"].as_str();
        if let Some(turn) = turn
            && self.active.as_deref() != Some(turn)
            && method != "thread/tokenUsage/updated"
        {
            return Ok(());
        }
        match method {
            "thread/settings/updated" => {
                let access = self.profiles.offered(&p["threadSettings"]);
                if self.access != access {
                    self.access = access;
                    self.send(Update::Access(self.access.clone()));
                }
            }
            "turn/started" => {
                if let Some(id) = p["turn"]["id"].as_str() {
                    self.started(id, false);
                }
            }
            "turn/completed" => self.ended(&p["turn"]),
            "item/started" | "item/completed" => {
                if let Some(turn) = turn {
                    for update in
                        self.items
                            .item(turn, &p["item"], method == "item/completed", self.cwd)
                    {
                        self.send(update);
                    }
                }
            }
            "serverRequest/resolved" => {
                if let Some(update) = self.requests.resolved(&p["requestId"]) {
                    self.send(update);
                }
            }
            "thread/name/updated" => {
                if let Some(title) = p["threadName"].as_str() {
                    self.send(Update::Title(title.into()));
                }
            }
            "turn/plan/updated" => {
                self.send(Update::Plan(
                    p["plan"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| {
                            Some(PlanStep {
                                text: v["step"].as_str()?.into(),
                                status: match v["status"].as_str() {
                                    Some("completed") => PlanStatus::Done,
                                    Some("inProgress") => PlanStatus::Running,
                                    _ => PlanStatus::Pending,
                                },
                            })
                        })
                        .collect(),
                ));
            }
            "thread/tokenUsage/updated" => {
                let u = &p["tokenUsage"];
                self.send(Update::Usage(Usage {
                    context_used: u["last"]["totalTokens"].as_u64(),
                    context_limit: u["modelContextWindow"].as_u64(),
                    input_tokens: u["total"]["inputTokens"].as_u64(),
                    output_tokens: u["total"]["outputTokens"].as_u64(),
                    cost_usd: None,
                }));
            }
            "error" if p["willRetry"] == true => {
                if let Some(reason) = p["error"]["message"].as_str() {
                    self.send(Update::Notice(Level::Warning, reason.into()));
                }
            }
            _ => {
                if let Some(update) = self.items.delta(method, p) {
                    self.send(update);
                }
            }
        }
        Ok(())
    }
}
