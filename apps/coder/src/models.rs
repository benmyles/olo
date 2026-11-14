//! Models as a person chooses them: where one runs (a service), which, how
//! hard it thinks, and its key, kept in Passwords. Shared by the window and
//! the terminal.
use olo_coder_core::{ModelConfig, Provider, providers};

pub use providers::Listed;

/// The reasoning efforts a model may take, when its service doesn't say
/// ("" is the model's own).
pub const EFFORTS: [&str; 8] = ["", "none", "low", "medium", "high", "xhigh", "max", "ultra"];

/// The efforts `id` takes on its service, as listed (the generic set when
/// the list doesn't say), always with the model's own default first.
pub fn efforts(listed: &[Listed], id: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    match listed.iter().find(|m| m.id == id).filter(|m| !m.efforts.is_empty()) {
        Some(m) => {
            let mut e = m.efforts.clone();
            // In order of effort, not with the default first.
            e.sort_by_key(|x| EFFORTS.iter().position(|y| y == x).unwrap_or(usize::MAX));
            out.extend(e);
        }
        None => out.extend(EFFORTS[1..].iter().map(|e| (*e).to_owned())),
    }
    out
}

/// How an effort reads.
pub fn effort_name(effort: &str) -> String {
    providers::effort_name(effort)
}
/// The effort a name says (`effort_name`'s other way).
pub fn effort_of(name: &str) -> String {
    EFFORTS.iter().find(|e| effort_name(e) == name).map(|e| (*e).to_owned()).unwrap_or_else(|| name.to_lowercase())
}
/// The generic efforts, by name.
pub fn effort_names() -> Vec<String> {
    EFFORTS.iter().map(|e| effort_name(e)).collect()
}

/// The models a subagent or another job may use, by name: those the
/// session's service lists (`extra` first, then saved models).
pub fn choices(view: &crate::client::View, extra: Option<(&str, &ModelConfig)>) -> Vec<(String, ModelConfig)> {
    let mut out = Vec::new();
    if let Some((label, model)) = extra {
        out.push((label.into(), model.clone()));
    }
    if let Some(s) = &view.session {
        for l in &view.models {
            let mut model = s.config.model.clone();
            choose(&mut model, &view.models, &l.id);
            out.push((l.name.clone(), model));
        }
    }
    out.extend(view.profiles.iter().map(|p| (format!("{} (saved)", p.name), p.model.clone())));
    out
}

/// The services a model runs on, in a menu's order: the token pool, each
/// known service, then any other OpenAI-compatible one.
pub fn services() -> Vec<String> {
    let mut out = vec!["Token Pool (ChatGPT)".to_owned()];
    out.extend(providers::KNOWN.iter().map(|k| k.name.to_owned()));
    out.push("Other OpenAI-compatible service".into());
    out
}
/// Which of `services` `model` runs on.
pub fn service(model: &ModelConfig) -> usize {
    match providers::of(model) {
        Some(k) => 1 + providers::KNOWN.iter().position(|o| o.id == k.id).unwrap_or(0),
        None if model.provider == Provider::Codex => 0,
        None => providers::KNOWN.len() + 1,
    }
}
/// The service a model runs on, by name ("Fireworks AI", or the address's host).
pub fn service_name(model: &ModelConfig) -> String {
    match (providers::of(model), model.provider) {
        (Some(k), _) => k.name.to_owned(),
        (None, Provider::Codex) => "the Token Pool".into(),
        (None, _) => model.endpoint.as_ref().and_then(|e| reqwest_host(&e.base_url)).unwrap_or_else(|| "the custom service".into()),
    }
}
fn reqwest_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?']).next()?.to_owned();
    (!host.is_empty()).then_some(host)
}
/// A fresh model on service `index` of `services`, keeping `keep`'s
/// address for a custom service.
pub fn service_model(index: usize, keep: &ModelConfig) -> ModelConfig {
    match index {
        0 => ModelConfig::for_provider(Provider::Codex),
        i if i <= providers::KNOWN.len() => providers::KNOWN[i - 1].config(None),
        _ => {
            let mut m = ModelConfig::for_provider(Provider::Openai);
            if providers::of(keep).is_none() && keep.provider == Provider::Openai {
                m.endpoint = keep.endpoint.clone();
                m.model = keep.model.clone();
            }
            m
        }
    }
}
/// The models a service is known to have, before it's asked (ours, for a
/// known service; none for the pool or another service).
pub fn known(model: &ModelConfig) -> Vec<Listed> {
    providers::of(model)
        .map(|k| k.models.iter().map(|m| Listed { id: m.id.into(), name: m.name.into(), efforts: m.efforts.iter().map(|e| (*e).into()).collect(), context_window: Some(m.context_window) }).collect())
        .unwrap_or_default()
}
/// How model `id` reads: its listed name, ours, else its id (a pool
/// model's tidied: "gpt-6.1-terra" is "GPT-6.1 Terra").
pub fn model_name(model: &ModelConfig, listed: &[Listed], id: &str) -> String {
    listed
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.name.clone())
        .or_else(|| providers::of(model).and_then(|k| k.model(id)).map(|m| m.name.to_owned()))
        .unwrap_or_else(|| if model.provider == Provider::Codex { pool_name(id) } else { id.rsplit('/').next().unwrap_or(id).to_owned() })
}
fn pool_name(id: &str) -> String {
    let mut parts = id.split('-').map(|p| match p {
        "gpt" => "GPT".to_owned(),
        p if p.starts_with(|c: char| c.is_ascii_alphabetic()) => p[..1].to_uppercase() + &p[1..],
        p => p.to_owned(),
    });
    let first = parts.next().unwrap_or_default();
    match parts.next() {
        Some(second) if first == "GPT" => std::iter::once(format!("GPT-{second}")).chain(parts).collect::<Vec<_>>().join(" "),
        Some(second) => std::iter::once(first).chain(std::iter::once(second)).chain(parts).collect::<Vec<_>>().join(" "),
        None => first,
    }
}
/// Chooses `id` on `model`'s service, with its known limits and an effort it takes.
pub fn choose(model: &mut ModelConfig, listed: &[Listed], id: &str) {
    model.model = id.to_owned();
    if let Some(k) = providers::of(model) {
        k.fit(model);
    } else if let Some(m) = listed.iter().find(|m| m.id == id) {
        if !model.effort.is_empty() && !m.efforts.is_empty() && !m.efforts.contains(&model.effort) {
            model.effort = m.efforts[0].clone();
        }
    }
}

/// What's typed into a key field.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyInput {
    Empty,
    /// op://vault/item/field: a field in Passwords.
    Reference(String),
    /// $NAME, or a NAME_LIKE_THIS: the Coder service's environment variable.
    Variable(String),
    /// Anything else: the key itself, to be kept in Passwords.
    Secret(String),
}
pub fn key_input(text: &str) -> KeyInput {
    let t = text.trim();
    let variable = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') && s.bytes().next().is_some_and(|b| !b.is_ascii_digit());
    if t.is_empty() {
        KeyInput::Empty
    } else if t.starts_with("op://") {
        KeyInput::Reference(t.into())
    } else if let Some(v) = t.strip_prefix('$').filter(|v| variable(v)) {
        KeyInput::Variable(v.into())
    } else if variable(t) && t.contains('_') {
        KeyInput::Variable(t.into())
    } else {
        KeyInput::Secret(t.into())
    }
}

/// Where a model's key comes from, as Settings says it.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyState {
    /// The service needs none (the token pool).
    NotNeeded,
    /// Kept in Passwords, as the item titled so.
    Kept(String),
    /// None given for a known service: the item titled so, if a key was
    /// pasted for it before.
    Unset(String),
    /// A field in Passwords.
    Reference(String),
    /// The Coder service's environment variable.
    Variable(String),
    /// None given: a known service will look for one kept for it.
    Missing,
}
pub fn key_state(model: &ModelConfig) -> KeyState {
    let Some(e) = model.endpoint.as_ref().filter(|_| model.provider == Provider::Openai) else { return KeyState::NotNeeded };
    if let Some(r) = &e.api_key_password {
        KeyState::Reference(r.clone())
    } else if let Some(v) = &e.api_key_env {
        KeyState::Variable(v.clone())
    } else if let Some(t) = &e.api_key_secret {
        KeyState::Kept(t.clone())
    } else if providers::of(model).is_some() {
        // Found by its title, if it was pasted for this service before.
        KeyState::Unset(providers::key_title(&e.base_url))
    } else {
        KeyState::Missing
    }
}
/// Points the endpoint at a key (not a pasted one: `keep_key` keeps that,
/// then this points at it as `KeyInput::Secret`'s title).
pub fn set_key(model: &mut ModelConfig, input: &KeyInput) {
    let Some(e) = &mut model.endpoint else { return };
    e.api_key_password = None;
    e.api_key_env = None;
    e.api_key_secret = None;
    match input {
        KeyInput::Empty => {}
        KeyInput::Reference(r) => e.api_key_password = Some(r.clone()),
        KeyInput::Variable(v) => e.api_key_env = Some(v.clone()),
        KeyInput::Secret(_) => e.api_key_secret = Some(providers::key_title(&e.base_url)),
    }
}
/// The key as its field shows it: a reference or a variable (a kept key
/// is never shown; its field is empty, and says where it's kept).
pub fn key_text(model: &ModelConfig) -> String {
    match key_state(model) {
        KeyState::Reference(r) => r,
        KeyState::Variable(v) => format!("${v}"),
        _ => String::new(),
    }
}

/// Keeps `secret`, the key pasted for `model`'s service, in Passwords (in
/// its own item, which a new key replaces), starting Passwords if it isn't
/// running; it asks to be unlocked if it's locked. Blocks: call it from a
/// worker.
pub fn keep_key(model: &ModelConfig, secret: &str) -> Result<(), String> {
    let e = model.endpoint.as_ref().ok_or("This service takes no key.")?;
    let request = olo_keychain::Request::KeepSecret {
        title: providers::key_title(&e.base_url),
        secret: secret.trim().to_owned(),
        note: format!("The API key Olo Coder uses for {} ({}). Paste a new one in Coder’s Settings ▸ Model to replace it.", service_name(model), e.base_url),
        replace: true,
    };
    let mut started = false;
    loop {
        let answer = olo_keychain::call(&request);
        match &answer {
            Ok(olo_keychain::Response::Saved { .. }) => return Ok(()),
            Ok(olo_keychain::Response::Error { message }) => return Err(format!("Passwords couldn’t keep it: {message}")),
            Ok(olo_keychain::Response::Locked) => return Err("Unlock Passwords to keep the key there.".into()),
            Ok(_) => return Err("Passwords didn’t keep the key.".into()),
            Err(olo_keychain::CallError::NotRunning) if !started => {
                started = true;
                let _ = olo_xdg::spawn_detached("olo-passwords --keychain");
                for _ in 0..50 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if olo_keychain::socket_path().exists() {
                        break;
                    }
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// "1M", "272K", "128K": a token count as Settings says it.
pub fn tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000 && n % 100_000 == 0 => format!("{}{}M", n / 1_000_000, if n % 1_000_000 == 0 { String::new() } else { format!(".{}", n % 1_000_000 / 100_000) }),
        n if n >= 1_000 => format!("{}K", (n + 500) / 1_000),
        n => n.to_string(),
    }
}

/// The agent's other jobs a model does: their names and settings.
pub const ROLES: [(&str, &str); 3] = [("Supervisor", "supervisor.model"), ("Approval reviewer", "reviewer"), ("Summarising", "compaction.model")];

/// The model the session's settings (`config`) give the agent.
pub fn agent_model(config: &serde_json::Value) -> Option<ModelConfig> {
    serde_json::from_value(config["model"].clone()).ok()
}

/// What role `r`'s menu offers and which it shows: "Same as the agent",
/// the agent's service's models (`listed`), and the model it has if
/// that's on another service.
pub fn role_menu(config: &serde_json::Value, agent: &ModelConfig, listed: &[Listed], r: usize) -> (Vec<String>, usize) {
    let mut labels = vec!["Same as the agent".to_owned()];
    labels.extend(listed.iter().map(|l| l.name.clone()));
    let current: Option<ModelConfig> = config.pointer(&crate::options::pointer(ROLES[r].1)).and_then(|v| serde_json::from_value(v.clone()).ok());
    let Some(current) = current else { return (labels, 0) };
    let same_service = current.provider == agent.provider && current.endpoint.as_ref().map(|e| e.base_url.trim_end_matches('/')) == agent.endpoint.as_ref().map(|e| e.base_url.trim_end_matches('/'));
    if same_service {
        if r == 1 && current.model == agent.model {
            return (labels, 0);
        }
        if let Some(i) = listed.iter().position(|l| l.id == current.model) {
            return (labels, i + 1);
        }
    }
    labels.push(format!("{} on {}", model_name(&current, &[], &current.model), service_name(&current)));
    let at = labels.len() - 1;
    (labels, at)
}

/// What choosing item `k` of role `r`'s menu sets it to (None: nothing,
/// it's the one it has).
pub fn role_value(agent: &ModelConfig, listed: &[Listed], r: usize, k: usize) -> Option<serde_json::Value> {
    // The reviewer decides quickly: the least effort the model takes.
    let quick = |mut m: ModelConfig| {
        if r == 1 && efforts(listed, &m.model).iter().any(|e| e == "low") {
            m.effort = "low".into();
        }
        m
    };
    match k {
        0 if r == 1 => Some(serde_json::json!(quick(agent.clone()))),
        0 => Some(serde_json::Value::Null),
        k if k <= listed.len() => {
            let mut m = agent.clone();
            choose(&mut m, listed, &listed[k - 1].id);
            Some(serde_json::json!(quick(m)))
        }
        _ => None,
    }
}

/// The agent's model going from `old` to `new` (another service): the
/// reviewer's, if it was the agent's own, which goes with it (the other
/// jobs with none of their own follow by themselves).
pub fn following(config: &serde_json::Value, old: &ModelConfig, new: &ModelConfig) -> Option<serde_json::Value> {
    let (_, at) = role_menu(config, old, &[], 1);
    (at == 0).then(|| role_value(new, &known(new), 1, 0)).flatten()
}

/// What the service or the session said, as the person would put it in
/// Settings (`get_key`: beside the key is Get a Key, which says where
/// one is made).
pub fn friendly(e: &str, get_key: bool) -> String {
    let e = e.trim_start_matches("Error: ");
    let e = match e.find(" Paste another in Settings ▸ Model.").or_else(|| e.find(" Get one at ")).or_else(|| e.find(": paste it in Settings ▸ Model")) {
        Some(at) if get_key && e.contains("key") => format!("{} Paste another, or Get a Key.", &e[..at]),
        _ => e.to_owned(),
    };
    let e = e.as_str();
    if e.contains("stop the running turn") || e.contains("the agent is working") {
        return "The agent is working: its model changes once it stops.".into();
    }
    if e.contains("invalid digit") || e.contains("expected") && e.contains("integer") {
        return "That needs a whole number.".into();
    }
    let mut c = e.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn services_round_trip_and_keys_follow_what_was_typed() {
        let m0 = ModelConfig::default();
        for i in 0..services().len() {
            assert_eq!(service(&service_model(i, &m0)), i);
        }
        let mut m = service_model(1, &m0);
        assert_eq!(key_state(&m), KeyState::Unset("Olo Coder: Fireworks AI API key".into()));
        assert_eq!(key_text(&m), "");
        set_key(&mut m, &key_input(" op://Private/Fireworks/credential "));
        assert_eq!(m.endpoint.as_ref().unwrap().api_key_password.as_deref(), Some("op://Private/Fireworks/credential"));
        m.validate().unwrap();
        assert_eq!(key_text(&m), "op://Private/Fireworks/credential");
        set_key(&mut m, &key_input("$FIREWORKS_API_KEY"));
        assert_eq!(key_state(&m), KeyState::Variable("FIREWORKS_API_KEY".into()));
        assert_eq!(key_input("FIREWORKS_API_KEY"), KeyInput::Variable("FIREWORKS_API_KEY".into()));
        assert_eq!(key_input("fw_3ZkQ9xYw"), KeyInput::Secret("fw_3ZkQ9xYw".into()));
        assert_eq!(key_input("sk-ABCDEF123"), KeyInput::Secret("sk-ABCDEF123".into()));
        set_key(&mut m, &key_input("fw_3ZkQ9xYw"));
        assert_eq!(key_state(&m), KeyState::Kept("Olo Coder: Fireworks AI API key".into()));
        m.validate().unwrap();
        assert!(model_name(&m, &[], &m.model.clone()).contains("Ultrafast"));
        // A custom service keeps its address when chosen again.
        let mut custom = service_model(services().len() - 1, &m0);
        custom.endpoint.as_mut().unwrap().base_url = "https://llm.example.com/v1".into();
        assert_eq!(service_model(services().len() - 1, &custom).endpoint.unwrap().base_url, "https://llm.example.com/v1");
        assert_eq!(service_name(&custom), "llm.example.com");
    }
    #[test]
    fn numbers_and_efforts_read_well() {
        assert_eq!(model_name(&ModelConfig::default(), &[], "gpt-6.1-terra"), "GPT-6.1 Terra");
        assert_eq!(model_name(&ModelConfig::default(), &[], "codex-auto-review"), "Codex Auto Review");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_500_000), "1.5M");
        assert_eq!(tokens(272_000), "272K");
        assert_eq!(tokens(128_000), "128K");
        let listed = [Listed { id: "a".into(), name: "A".into(), efforts: vec!["medium".into(), "low".into(), "xhigh".into()], context_window: None }];
        assert_eq!(efforts(&listed, "a"), ["", "low", "medium", "xhigh"]);
        assert_eq!(effort_name("xhigh"), "Extra high");
        assert_eq!(efforts(&[], "b").len(), EFFORTS.len());
    }
}
