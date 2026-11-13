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
        config.endpoint = Some(EndpointConfig { base_url: self.base_url.into(), api_key_env: Some(self.key_env.into()), token_limit_field: TokenLimitField::MaxTokens, ..EndpointConfig::default() });
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

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(c.endpoint.as_ref().unwrap().api_key_env.as_deref(), Some("FIREWORKS_API_KEY"));
        assert!(of(&ModelConfig::default()).is_none());
    }
}
