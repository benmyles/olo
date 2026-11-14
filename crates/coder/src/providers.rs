//! Known model services: an OpenAI-compatible endpoint Coder already knows how
//! to set up (its address, where its key usually is, its models and their
//! limits), so choosing one is a single step. A known service is still an
//! ordinary `openai` endpoint in the configuration, recognised by its address,
//! so a hand-written endpoint to the same place is the same service.
use crate::model::{EndpointConfig, ModelConfig, Provider, TokenLimitField};

pub struct Known {
    /// What `/provider` and `spawn_agent` call it.
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    /// The environment variable its key is usually in.
    pub key_env: &'static str,
    /// Where a person gets a key.
    pub keys_page: &'static str,
    /// A header that keeps a session on one replica, so its prompt prefix
    /// stays cached; the session's id is sent in it.
    pub affinity_header: Option<&'static str>,
    pub models: &'static [KnownModel],
}
pub struct KnownModel {
    pub id: &'static str,
    pub name: &'static str,
    pub context_window: u64,
    pub max_output_tokens: u32,
    /// Its reasoning efforts; the first is the one chosen for it.
    pub efforts: &'static [&'static str],
}

pub const FIREWORKS: Known = Known {
    id: "fireworks",
    name: "Fireworks AI",
    base_url: "https://api.fireworks.ai/inference/v1",
    key_env: "FIREWORKS_API_KEY",
    keys_page: "https://app.fireworks.ai/settings/users/api-keys",
    affinity_header: Some("x-session-affinity"),
    models: &[
        // Same model as deepseek-v4p1-flash-1017 on Fireworks' Ultrafast
        // serving path (selected by this id): 1M context, 384K output.
        KnownModel {
            id: "accounts/fireworks/routers/deepseek-v4p1-flash-1017-ultrafast",
            name: "DeepSeek V4.1 Flash · Ultrafast",
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            efforts: &["high", "low", "max"],
        },
        KnownModel { id: "accounts/fireworks/models/deepseek-v4p1-flash-1017", name: "DeepSeek V4.1 Flash", context_window: 1_000_000, max_output_tokens: 128_000, efforts: &["high", "low", "max"] },
    ],
};

pub const KNOWN: &[Known] = &[FIREWORKS];

pub fn by_id(id: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.id == id)
}

/// The title of the Passwords item Coder keeps a service's key in: one
/// per service (by its name, or its address's host), so any session, or a
/// subagent, on that service finds the key pasted for it once.
pub fn key_title(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let name = KNOWN.iter().find(|k| k.base_url == base).map(|k| k.name.to_owned());
    let name = name.or_else(|| reqwest::Url::parse(base).ok().and_then(|u| u.host_str().map(str::to_owned))).unwrap_or_else(|| base.to_owned());
    format!("Olo Coder: {name} API key")
}

/// The known service a model runs on, by its endpoint's address.
pub fn of(model: &ModelConfig) -> Option<&'static Known> {
    let url = model.endpoint.as_ref()?.base_url.trim_end_matches('/');
    (model.provider == Provider::Openai).then(|| KNOWN.iter().find(|k| k.base_url == url)).flatten()
}

impl Known {
    pub fn model(&self, id: &str) -> Option<&'static KnownModel> {
        self.models.iter().find(|m| m.id == id)
    }
    /// A model on this service, its first one when `model` is None.
    pub fn config(&self, model: Option<&str>) -> ModelConfig {
        let mut config = ModelConfig::for_provider(Provider::Openai);
        // No key yet: one pasted in Settings is kept in Passwords; until
        // then the service's own variable (`key_env`) is used if it has it.
        config.endpoint = Some(EndpointConfig { base_url: self.base_url.into(), token_limit_field: TokenLimitField::MaxTokens, ..EndpointConfig::default() });
        let known = model.and_then(|m| self.model(m)).unwrap_or(&self.models[0]);
        config.model = model.unwrap_or(known.id).into();
        self.fit(&mut config);
        config
    }
    /// The limits and effort of `config.model`, when it's one of ours.
    pub fn fit(&self, config: &mut ModelConfig) {
        if let Some(m) = self.model(&config.model) {
            config.context_window = m.context_window;
            config.max_output_tokens = m.max_output_tokens;
            if !m.efforts.contains(&config.effort.as_str()) {
                config.effort = m.efforts.first().copied().unwrap_or("").into();
            }
        }
    }
}

/// A model a service lists (`Command::Models`), as a person chooses it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Listed {
    pub id: String,
    /// How it reads: the service's name for it, ours, or its id tidied.
    pub name: String,
    /// Its reasoning efforts, the one it starts at first (none when the
    /// service doesn't say).
    pub efforts: Vec<String>,
    pub context_window: Option<u64>,
}

/// The models a service's list (`Command::Models`'s answer) offers, in its
/// order (the token pool's priority), without those it hides.
pub fn listed(model: &ModelConfig, value: &serde_json::Value) -> Vec<Listed> {
    let rows = value["data"].as_array().or_else(|| value["models"].as_array()).or_else(|| value.as_array());
    let known = of(model);
    let mut out: Vec<(i64, Listed)> = rows
        .into_iter()
        .flatten()
        .filter(|r| r["visibility"].as_str() != Some("hide"))
        .filter_map(|r| {
            let id = r["slug"].as_str().or(r["id"].as_str()).or(r.as_str())?.to_owned();
            let ours = known.and_then(|k| k.model(&id));
            let name = r["display_name"].as_str().map(str::to_owned).or_else(|| ours.map(|m| m.name.to_owned())).unwrap_or_else(|| tidy(&id));
            let mut efforts: Vec<String> = r["supported_reasoning_levels"].as_array().into_iter().flatten().filter_map(|l| l["effort"].as_str().map(str::to_owned)).collect();
            if efforts.is_empty() {
                efforts = ours.map(|m| m.efforts.iter().map(|e| (*e).to_owned()).collect()).unwrap_or_default();
            } else if let Some(first) = r["default_reasoning_level"].as_str().and_then(|d| efforts.iter().position(|e| e == d)) {
                let d = efforts.remove(first);
                efforts.insert(0, d);
            }
            let context_window = r["context_window"].as_u64().or_else(|| ours.map(|m| m.context_window));
            Some((r["priority"].as_i64().unwrap_or(i64::MAX), Listed { id, name, efforts, context_window }))
        })
        .collect();
    // Ours first, in our order; then the service's, by its priority, else by name.
    let rank = |l: &Listed| known.and_then(|k| k.models.iter().position(|m| m.id == l.id)).unwrap_or(usize::MAX);
    out.sort_by(|(pa, a), (pb, b)| rank(a).cmp(&rank(b)).then(pa.cmp(pb)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out.dedup_by(|a, b| a.1.id == b.1.id);
    out.into_iter().map(|(_, l)| l).collect()
}

/// A model id as a name: its last part (`accounts/x/models/llama-v3p1-8b`
/// is "llama-v3p1-8b").
fn tidy(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_owned()
}

/// How a reasoning effort reads ("" is the model's own).
pub fn effort_name(effort: &str) -> String {
    match effort {
        "" => "Model’s default".into(),
        "xhigh" => "Extra high".into(),
        "none" => "None".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_read_as_names_in_the_services_order() {
        let pool = serde_json::json!({"models":[
            {"slug":"b","display_name":"GPT B","visibility":"list","priority":2,"default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"}],"context_window":272000},
            {"slug":"codex-auto-review","display_name":"Codex Auto Review","visibility":"hide","priority":0},
            {"slug":"a","display_name":"GPT A","visibility":"list","priority":1}
        ]});
        let l = listed(&ModelConfig::default(), &pool);
        assert_eq!(l.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["GPT A", "GPT B"]);
        assert_eq!(l[1].efforts, ["medium", "low"]);
        assert_eq!(l[1].context_window, Some(272000));
        let fireworks = serde_json::json!({"data":[{"id":"accounts/fireworks/models/zeta-8b"},{"id":"accounts/fireworks/models/deepseek-v4p1-flash-1017"}]});
        let l = listed(&FIREWORKS.config(None), &fireworks);
        assert_eq!(l[0].name, "DeepSeek V4.1 Flash");
        assert_eq!(l[1].name, "zeta-8b");
        assert_eq!(effort_name("xhigh"), "Extra high");
    }
    #[test]
    fn known_services_validate_and_are_recognised() {
        for k in KNOWN {
            for m in k.models {
                let c = k.config(Some(m.id));
                c.validate().unwrap();
                assert_eq!(of(&c).map(|k| k.id), Some(k.id));
                assert_eq!((c.context_window, c.max_output_tokens), (m.context_window, m.max_output_tokens));
                assert!(m.efforts.contains(&c.effort.as_str()));
            }
        }
        let c = FIREWORKS.config(None);
        assert_eq!(c.model, "accounts/fireworks/routers/deepseek-v4p1-flash-1017-ultrafast");
        assert!(c.endpoint.as_ref().unwrap().api_key_env.is_none(), "no key until one is given");
        assert!(of(&ModelConfig::default()).is_none());
        assert_eq!(key_title("https://api.fireworks.ai/inference/v1/"), "Olo Coder: Fireworks AI API key");
        assert_eq!(key_title("https://llm.example.com/v1"), "Olo Coder: llm.example.com API key");
    }
}
