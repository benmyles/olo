//! Service-wide named model configurations. Sessions copy a profile on selection,
//! so editing the catalog never changes an already-running agent.
use crate::{Engine, model::*, provider, tools};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    pub id: String,
    pub name: String,
    pub model: ModelConfig,
}
impl ModelProfile {
    pub fn validate(&self) -> Result<()> {
        ensure!(uuid::Uuid::parse_str(&self.id).is_ok(), "invalid profile ID");
        ensure!(!self.name.trim().is_empty() && self.name.len() <= 128, "profile name must be 1..128 bytes");
        self.model.validate()
    }
}
impl Engine {
    pub(crate) async fn profiles(&self, action: &str, profile: Option<ModelProfile>, id: Option<String>, session: Option<String>, role: &str) -> Result<Value> {
        match action {
            "" | "list" => Ok(json!({"profiles":self.model_profiles.lock().unwrap().clone()})),
            "save" | "remove" => {
                let mut saved = self.model_profiles.lock().unwrap();
                let mut profiles = saved.clone();
                if action == "save" {
                    let mut profile = profile.context("profile required")?;
                    profile.name = profile.name.trim().to_owned();
                    profile.validate()?;
                    ensure!(!profiles.iter().any(|p| p.id != profile.id && p.name.eq_ignore_ascii_case(profile.name.trim())), "a profile already has this name");
                    if let Some(old) = profiles.iter_mut().find(|p| p.id == profile.id) {
                        *old = profile;
                    } else {
                        ensure!(profiles.len() < 128, "at most 128 model profiles");
                        profiles.push(profile);
                    }
                } else {
                    let id = id.context("profile ID required")?;
                    ensure!(profiles.iter().any(|p| p.id == id), "profile not found");
                    profiles.retain(|p| p.id != id);
                }
                tools::atomic_write(&self.directory.join("model-profiles.json"), &serde_json::to_vec_pretty(&profiles)?)?;
                *saved = profiles.clone();
                drop(saved);
                self.transient("", "model_profiles_changed", json!({"profiles":profiles}));
                Ok(json!({"profiles":profiles}))
            }
            "discover" => {
                let draft = profile.context("profile required")?;
                draft.model.validate()?;
                let client = match &self.pool {
                    Some(base) => provider::Client::at(base),
                    None => provider::Client::new(),
                }?;
                let found = client.models(&draft.model).await?;
                // In the service's order, by name, without those it hides.
                let listed = crate::providers::listed(&draft.model, &found);
                let models = listed.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
                ensure!(!models.is_empty(), "This service listed no models: check its address and key.");
                Ok(json!({"draft":draft,"models":models,"listed":listed}))
            }
            "apply" => {
                let profile = self.model_profiles.lock().unwrap().iter().find(|p| Some(&p.id) == id.as_ref()).cloned().context("profile not found")?;
                let session = session.context("session required")?;
                let _idle = self.idle_guard(&session)?;
                self.change(&session, "configured", |s| {
                    let mut config = s.config.clone();
                    match role {
                        "primary" => config.model = profile.model.clone(),
                        "supervisor" => config.supervisor.model = Some(profile.model.clone()),
                        "reviewer" => config.reviewer = profile.model.clone(),
                        "compaction" => config.compaction.model = Some(profile.model.clone()),
                        _ => bail!("unknown model role"),
                    }
                    config.validate()?;
                    s.config = config;
                    Ok(json!({"profile":profile.id,"role":role}))
                })
            }
            _ => bail!("unknown model profile action"),
        }
    }
}
