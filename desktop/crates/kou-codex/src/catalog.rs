use crate::rpc::Rpc;
use anyhow::{Context as _, Result};
use kou_agents::Launch;
use kou_thread::Choice;
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct Offered {
    pub models: Vec<Choice>,
    pub current: Option<String>,
    pub access: kou_thread::Access,
}

#[derive(Debug)]
pub struct PastSession {
    pub id: String,
    pub title: String,
    pub updated: Option<SystemTime>,
}

#[derive(Default)]
pub(crate) struct Catalog {
    entries: Vec<Value>,
}

impl Catalog {
    pub async fn read(rpc: &mut Rpc, until: Instant) -> Result<Self> {
        Ok(Self {
            entries: rpc.pages("model/list", json!({}), until).await?,
        })
    }

    pub fn offered(&self, configured: Option<String>) -> Offered {
        let models = self
            .entries
            .iter()
            .filter_map(|v| choice(v, "model", "displayName"))
            .collect();
        let default = self
            .entries
            .iter()
            .find(|v| v["isDefault"] == true)
            .and_then(|v| v["model"].as_str())
            .map(str::to_owned);
        Offered {
            models,
            current: configured.or(default),
            access: Default::default(),
        }
    }

    pub fn efforts(&self, model: &str) -> (Vec<Choice>, Option<String>) {
        let Some(entry) = self.entries.iter().find(|v| v["model"] == model) else {
            return Default::default();
        };
        let levels = entry["supportedReasoningEfforts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| choice(v, "reasoningEffort", "reasoningEffort"))
            .collect();
        (
            levels,
            entry["defaultReasoningEffort"].as_str().map(str::to_owned),
        )
    }
}

pub(crate) fn choice(value: &Value, key: &str, name: &str) -> Option<Choice> {
    let id = value[key].as_str()?;
    Some(Choice {
        id: id.into(),
        name: value[name].as_str().unwrap_or(id).into(),
        description: value["description"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    })
}

/// Query the catalog and the effective model for this account and folder
/// without opening a conversation.
pub fn models(launch: &Launch, patience: Duration) -> Result<Offered> {
    futures::executor::block_on(async {
        let until = Instant::now() + patience;
        let (mut rpc, _) = Rpc::start(launch, None)?;
        rpc.initialize(until).await?;
        let catalog = Catalog::read(&mut rpc, until).await?;
        let config = rpc
            .call("config/read", json!({"cwd":launch.cwd}), until)
            .await?;
        let mut offered = catalog.offered(config["config"]["model"].as_str().map(str::to_owned));
        let profiles = crate::access::Profiles::read(&mut rpc, &launch.cwd, until).await;
        offered.access = profiles.offered(&Value::Null);
        Ok(offered)
    })
}

/// The account's conversations in the pane's working directory, latest first.
pub fn sessions(launch: &Launch, patience: Duration) -> Result<Vec<PastSession>> {
    futures::executor::block_on(async {
        let until = Instant::now() + patience;
        let (mut rpc, _) = Rpc::start(launch, None)?;
        rpc.initialize(until).await?;
        let folder = &launch.cwd;
        let canonical = folder.canonicalize().unwrap_or_else(|_| folder.clone());
        let filter = if canonical == *folder {
            json!(folder)
        } else {
            json!([folder, canonical])
        };
        let all = rpc
            .pages(
                "thread/list",
                json!({"cwd": filter, "sortKey": "updated_at", "modelProviders": []}),
                until,
            )
            .await?;
        all.iter()
            .filter(|v| {
                v["cwd"]
                    .as_str()
                    .map(std::path::Path::new)
                    .is_some_and(|p| p == folder || p == canonical)
            })
            .map(|v| {
                Ok(PastSession {
                    id: v["id"]
                        .as_str()
                        .context("thread/list: missing thread id")?
                        .into(),
                    title: v["name"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .or_else(|| v["preview"].as_str())
                        .unwrap_or_default()
                        .lines()
                        .next()
                        .unwrap_or("")
                        .into(),
                    updated: v["updatedAt"]
                        .as_u64()
                        .and_then(|s| UNIX_EPOCH.checked_add(Duration::from_secs(s))),
                })
            })
            .collect()
    })
}
