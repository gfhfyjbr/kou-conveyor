use crate::rpc::Rpc;
use anyhow::{Context as _, Result};
use kou_thread::{Access, Choice};
use serde_json::{Value, json};
use std::{path::Path, time::Instant};

struct Preset {
    choice: Choice,
    profile: String,
    approval: Option<&'static str>,
    reviewer: Option<&'static str>,
}

#[derive(Default)]
pub(crate) struct Profiles(Vec<Preset>);

impl Profiles {
    pub async fn read(rpc: &mut Rpc, cwd: &Path, until: Instant) -> Self {
        let Ok(profiles) = rpc
            .pages("permissionProfile/list", json!({"cwd": cwd}), until)
            .await
        else {
            return Self::default();
        };
        let requirements = rpc
            .call("configRequirements/read", json!({}), until)
            .await
            .unwrap_or_default();
        let features = rpc
            .pages("experimentalFeature/list", json!({}), until)
            .await
            .unwrap_or_default();
        let guardian = features
            .iter()
            .any(|f| f["name"] == "guardian_approval" && f["enabled"] == true);
        Self::from_server(&profiles, &requirements["requirements"], guardian)
    }

    fn from_server(profiles: &[Value], limits: &Value, guardian: bool) -> Self {
        let mut presets = Vec::new();
        for entry in profiles.iter().filter(|p| p["allowed"] == true) {
            let Some(profile) = entry["id"].as_str() else {
                continue;
            };
            let mut add = |id: &str,
                           name: &str,
                           description: &str,
                           approval: Option<&'static str>,
                           reviewer: Option<&'static str>| {
                if !allowed(&limits["allowedApprovalPolicies"], approval)
                    || !allowed(&limits["allowedApprovalsReviewers"], reviewer)
                {
                    return;
                }
                presets.push(Preset {
                    choice: Choice {
                        id: id.into(),
                        name: name.into(),
                        description: Some(description.into()),
                    },
                    profile: profile.into(),
                    approval,
                    reviewer,
                });
            };
            match profile {
                ":read-only" => add(
                    "read-only",
                    "Read Only",
                    "Ask before editing files or accessing the network.",
                    Some("on-request"),
                    Some("user"),
                ),
                ":workspace" => {
                    add(
                        "auto",
                        "Auto",
                        "Edit workspace files and run commands. Ask before accessing the network or editing elsewhere.",
                        Some("on-request"),
                        Some("user"),
                    );
                    if guardian {
                        add(
                            "auto-review",
                            "Approve for me",
                            "Codex reviews requests for additional access.",
                            Some("on-request"),
                            Some("auto_review"),
                        );
                    }
                }
                ":danger-full-access" => add(
                    "full-access",
                    "Full Access",
                    "Run commands with network access and edit files outside the workspace without asking.",
                    Some("never"),
                    Some("user"),
                ),
                name => add(
                    &format!("profile/{name}"),
                    name,
                    entry["description"]
                        .as_str()
                        .unwrap_or("Named Codex permission profile."),
                    None,
                    None,
                ),
            }
        }
        Self(presets)
    }

    pub fn offered(&self, effective: &Value) -> Access {
        let current = self
            .0
            .iter()
            .find(|preset| {
                effective["activePermissionProfile"]["id"] == preset.profile
                    && preset
                        .approval
                        .is_none_or(|p| effective["approvalPolicy"] == p)
                    && preset
                        .reviewer
                        .is_none_or(|r| reviewer_matches(&effective["approvalsReviewer"], r))
            })
            .map(|p| p.choice.id.clone());
        let confirm = self
            .0
            .iter()
            .filter(|p| p.profile == ":danger-full-access")
            .map(|p| p.choice.id.clone())
            .collect();
        Access {
            choices: self.0.iter().map(|p| p.choice.clone()).collect(),
            current,
            confirm,
        }
    }

    pub fn apply(&self, id: &str, params: &mut Value) -> Result<()> {
        let preset = self.0.iter().find(|p| p.choice.id == id).with_context(|| {
            format!("Codex does not offer the permission preset {id} in this folder")
        })?;
        params["permissions"] = json!(preset.profile);
        if let Some(policy) = preset.approval {
            params["approvalPolicy"] = json!(policy);
        }
        if let Some(reviewer) = preset.reviewer {
            params["approvalsReviewer"] = json!(reviewer);
        }
        Ok(())
    }
}

fn reviewer_matches(value: &Value, expected: &str) -> bool {
    value == expected || (expected == "auto_review" && value == "guardian_subagent")
}

fn allowed(list: &Value, wanted: Option<&str>) -> bool {
    wanted.is_none_or(|wanted| {
        list.as_array()
            .is_none_or(|values| values.iter().any(|value| reviewer_matches(value, wanted)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_follow_the_servers_profile_and_approval_requirements() {
        let profiles = [
            json!({"id": ":workspace", "allowed": true}),
            json!({"id": ":danger-full-access", "allowed": true}),
            json!({"id": "restricted", "allowed": false}),
            json!({"id": "team", "allowed": true, "description": "Project policy"}),
        ];
        let limits = json!({"allowedApprovalPolicies": ["on-request"], "allowedApprovalsReviewers": ["user"]});
        let presets = Profiles::from_server(&profiles, &limits, true);
        let offered = presets.offered(&json!({"activePermissionProfile":{"id":"team"}}));
        assert_eq!(offered.current.as_deref(), Some("profile/team"));
        assert_eq!(
            offered
                .choices
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["auto", "profile/team"]
        );
        let mut params = json!({});
        assert!(presets.apply("full-access", &mut params).is_err());
        presets.apply("profile/team", &mut params).unwrap();
        assert_eq!(params, json!({"permissions":"team"}));

        let profiles = Profiles::from_server(&profiles, &Value::Null, true);
        let offered = profiles.offered(&json!({"activePermissionProfile":{"id":":workspace"},
            "approvalPolicy":"on-request","approvalsReviewer":"guardian_subagent"}));
        assert_eq!(offered.current.as_deref(), Some("auto-review"));
        assert_eq!(offered.confirm, ["full-access"]);
        profiles.apply("auto-review", &mut params).unwrap();
        assert_eq!(
            params,
            json!({"permissions":":workspace","approvalPolicy":"on-request","approvalsReviewer":"auto_review"})
        );
    }
}
