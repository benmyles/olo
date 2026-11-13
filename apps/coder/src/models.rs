//! Named models are configured once and selected by role throughout Coder.
use olo_coder_core::ModelConfig;
pub const EFFORTS: [&str; 8] = ["Provider default", "none", "low", "medium", "high", "xhigh", "max", "ultra"];
pub fn effort(value: &str) -> usize {
    EFFORTS.iter().position(|v| *v == value).unwrap_or(0)
}
pub fn effort_value(index: usize) -> String {
    if index == 0 { String::new() } else { EFFORTS[index].into() }
}
pub fn choices(view: &crate::client::View, extra: Option<(&str, &ModelConfig)>) -> Vec<(String, ModelConfig)> {
    let mut out = Vec::new();
    if let Some((label, model)) = extra {
        out.push((label.into(), model.clone()));
    }
    if let Some(s) = &view.session {
        out.push(("Current primary".into(), s.config.model.clone()));
        for id in &view.models {
            let mut model = s.config.model.clone();
            model.model = id.clone();
            out.push((format!("Model · {id}"), model));
        }
    }
    out.extend(view.profiles.iter().map(|p| (format!("Profile · {}", p.name), p.model.clone())));
    out
}

/// Where a model runs, as a menu: the token pool, each known service, then a
/// custom endpoint. `pool` and `custom` are how this interface says those two.
pub fn services(pool: &str, custom: &str) -> Vec<String> {
    let mut out = vec![pool.to_owned()];
    out.extend(olo_coder_core::providers::KNOWN.iter().map(|k| k.name.to_owned()));
    out.push(custom.into());
    out
}
/// Which of `services` `model` runs on.
pub fn service(model: &ModelConfig) -> usize {
    use olo_coder_core::{Provider, providers};
    match providers::of(model) {
        Some(k) => 1 + providers::KNOWN.iter().position(|o| o.id == k.id).unwrap_or(0),
        None if model.provider == Provider::Codex => 0,
        None => providers::KNOWN.len() + 1,
    }
}
/// A fresh model on service `index` of `services`.
pub fn service_model(index: usize) -> ModelConfig {
    use olo_coder_core::{Provider, providers};
    match index {
        0 => ModelConfig::for_provider(Provider::Codex),
        i if i <= providers::KNOWN.len() => providers::KNOWN[i - 1].config(None),
        _ => ModelConfig::for_provider(Provider::Openai),
    }
}
/// The model ids a service is known to have (none for the pool or a custom endpoint).
pub fn known_ids(model: &ModelConfig) -> Vec<String> {
    olo_coder_core::providers::of(model).map(|k| k.models.iter().map(|m| m.id.to_owned()).collect()).unwrap_or_default()
}
/// The endpoint's key as it's typed: a Passwords reference or a variable name.
pub fn key_text(model: &ModelConfig) -> String {
    model.endpoint.as_ref().and_then(|e| e.api_key_password.clone().or_else(|| e.api_key_env.clone())).unwrap_or_default()
}
/// Sets the endpoint's key from what was typed: op://… is a Passwords field,
/// anything else the name of an environment variable.
pub fn set_key(model: &mut ModelConfig, text: &str) {
    if let Some(e) = &mut model.endpoint {
        let text = text.trim();
        e.api_key_password = text.starts_with("op://").then(|| text.to_owned());
        e.api_key_env = (!text.is_empty() && !text.starts_with("op://")).then(|| text.to_owned());
    }
}
/// Chooses `id` on `model`'s service, with its known limits and effort.
pub fn choose(model: &mut ModelConfig, id: &str) {
    model.model = id.to_owned();
    if let Some(k) = olo_coder_core::providers::of(model) {
        k.fit(model);
    }
}
/// How a model id reads: a known model's name, else the id.
pub fn model_name(model: &ModelConfig, id: &str) -> String {
    olo_coder_core::providers::of(model).and_then(|k| k.model(id)).map(|m| m.name.to_owned()).unwrap_or_else(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn services_round_trip_and_keys_follow_what_was_typed() {
        for i in 0..services("pool", "custom").len() {
            assert_eq!(service(&service_model(i)), i);
        }
        let mut m = service_model(1);
        assert_eq!(key_text(&m), "FIREWORKS_API_KEY");
        set_key(&mut m, " op://Private/Fireworks/credential ");
        assert_eq!(m.endpoint.as_ref().unwrap().api_key_password.as_deref(), Some("op://Private/Fireworks/credential"));
        assert!(m.endpoint.as_ref().unwrap().api_key_env.is_none());
        m.validate().unwrap();
        assert_eq!(key_text(&m), "op://Private/Fireworks/credential");
        assert!(model_name(&m, &m.model.clone()).contains("Ultrafast"));
    }
}
