//! How Tabula's AI features (Preferences ▸ AI) reach their models:
//! Claude through the Token Pool's gateway on this computer (its
//! accounts, spent in its order) or straight to Anthropic with the API
//! key saved here; OpenAI's (Astra 6, `gpt-*`) through the pool's Codex
//! accounts only. One blocking call at a time (the Messages API, or the
//! Responses API for OpenAI's); call it off the UI thread (a feature's
//! own process or a worker).
//!
//! `TABULA_AI_BASE_URL` sends every call somewhere else instead (a fake
//! server, in tests): never a real model there.

use std::{io::Write, path::PathBuf, time::Duration};

use serde_json::{Value, json};
use tabula_protocol::{AiFeature, AiSettings};

/// The API version every call asks for.
const API_VERSION: &str = "2026-10-21";
/// The pool's port unless `TABULA_POOL_PORT` says otherwise (as the
/// pool's own `gateway::PORT`; not linked, to keep the pool's server out
/// of every AI feature).
const POOL_PORT: u16 = 17891;
/// A model looking at a screen and thinking can take a while.
const TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("No Anthropic API key is saved (Preferences ▸ AI), and the Token Pool is off.")]
    NoKey,
    #[error("{0} is reached through the Token Pool, which is off (Preferences ▸ AI).")]
    NeedsPool(String),
    #[error("Couldn't reach {0}: {1}")]
    Unreachable(String, String),
    #[error("{0}")]
    Api(String),
    #[error("The model declined.")]
    Refused,
    #[error("The answer was cut off.")]
    Truncated,
    #[error("The answer wasn't what was asked for: {0}")]
    BadAnswer(String),
}

/// Where the saved API key is (only its owner can read it).
pub fn key_path() -> PathBuf {
    tabula_xdg::config_dir().join("anthropic-key")
}

/// The saved API key, or `ANTHROPIC_API_KEY`.
pub fn saved_key() -> Option<String> {
    let saved = std::fs::read_to_string(key_path()).ok().map(|k| k.trim().to_owned()).filter(|k| !k.is_empty());
    saved.or_else(|| std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()))
}

/// Save the API key (empty: forget it).
pub fn save_key(key: &str) -> std::io::Result<()> {
    let path = key_path();
    let key = key.trim();
    if key.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    f.write_all(key.as_bytes())
}

/// One question: what to send, and the shape of JSON wanted back.
pub struct Ask {
    pub system: String,
    /// The user turn's content blocks (text, images).
    pub content: Vec<Value>,
    /// A JSON schema the answer must follow.
    pub schema: Value,
    pub max_tokens: u32,
}

/// An image content block from PNG bytes.
pub fn png_block(png: &[u8]) -> Value {
    use base64::Engine;
    json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png)}})
}

/// A text content block.
pub fn text_block(text: impl Into<String>) -> Value {
    json!({"type": "text", "text": text.into()})
}

/// Where calls go, and the key to send (none through the pool: it puts
/// its account's own in).
fn endpoint(settings: &AiSettings) -> Result<(String, Option<String>), Error> {
    if let Ok(base) = std::env::var("TABULA_AI_BASE_URL") {
        return Ok((base.trim_end_matches('/').to_owned(), None));
    }
    if settings.use_pool {
        let port = std::env::var("TABULA_POOL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(POOL_PORT);
        return Ok((format!("http://127.0.0.1:{port}/claude"), None));
    }
    Ok(("https://api.anthropic.com".into(), Some(saved_key().ok_or(Error::NoKey)?)))
}

/// The request body for `ask` with `feature`'s model and effort.
pub fn body(feature: &AiFeature, ask: &Ask) -> Value {
    let mut body = json!({
        "model": feature.model,
        "max_tokens": ask.max_tokens,
        "system": ask.system,
        "messages": [{"role": "user", "content": ask.content}],
        "output_config": {"effort": feature.effort, "format": {"type": "json_schema", "schema": ask.schema}},
    });
    if fallbacks(&feature.model) {
        body["fallbacks"] = json!("default");
    }
    body
}

/// Whether the model can hand a declined request to another (Haiku can't).
fn fallbacks(model: &str) -> bool {
    !model.starts_with("claude-haiku")
}

/// OpenAI's model, asked through the pool's Codex route.
pub fn is_openai(model: &str) -> bool {
    model.starts_with("gpt-")
}

/// Ask `feature`'s model; its JSON answer.
pub fn ask(settings: &AiSettings, feature: &AiFeature, ask: &Ask) -> Result<Value, Error> {
    if is_openai(&feature.model) {
        return ask_openai(settings, feature, ask);
    }
    let (base, key) = endpoint(settings)?;
    let url = format!("{base}/v1/messages");
    let client = reqwest::blocking::Client::builder().timeout(TIMEOUT).build().map_err(|e| Error::Unreachable(base.clone(), e.to_string()))?;
    let mut req = client.post(&url).header("anthropic-version", API_VERSION).json(&body(feature, ask));
    if fallbacks(&feature.model) {
        req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    if let Some(key) = key {
        req = req.header("x-api-key", key);
    }
    let reply = req.send().map_err(|e| Error::Unreachable(base.clone(), e.to_string()))?;
    let status = reply.status();
    let reply: Value = reply.json().map_err(|e| Error::Api(format!("{status}: {e}")))?;
    if !status.is_success() {
        let message = reply["error"]["message"].as_str().map(str::to_owned).unwrap_or_else(|| status.to_string());
        return Err(Error::Api(message));
    }
    answer(&reply)
}

/// The Responses API body for `ask` (as Codex sends it: streamed, not
/// stored, the system prompt as instructions).
pub fn openai_body(feature: &AiFeature, ask: &Ask) -> Value {
    let content: Vec<Value> = ask
        .content
        .iter()
        .map(|b| match b["type"].as_str() {
            Some("image") => json!({"type": "input_image", "image_url": format!("data:{};base64,{}", b["source"]["media_type"].as_str().unwrap_or("image/png"), b["source"]["data"].as_str().unwrap_or(""))}),
            _ => json!({"type": "input_text", "text": b["text"]}),
        })
        .collect();
    // Its efforts stop at xhigh.
    let effort = if feature.effort == "max" { "xhigh" } else { feature.effort.as_str() };
    json!({
        "model": feature.model,
        "instructions": ask.system,
        "input": [{"type": "message", "role": "user", "content": content}],
        "reasoning": {"effort": effort},
        "text": {"format": {"type": "json_schema", "name": "answer", "strict": true, "schema": ask.schema}},
        "stream": true,
        "store": false,
    })
}

fn ask_openai(settings: &AiSettings, feature: &AiFeature, ask: &Ask) -> Result<Value, Error> {
    let base = match std::env::var("TABULA_AI_BASE_URL") {
        Ok(base) => base.trim_end_matches('/').to_owned(),
        Err(_) if settings.use_pool => {
            let port = std::env::var("TABULA_POOL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(POOL_PORT);
            format!("http://127.0.0.1:{port}/codex")
        }
        Err(_) => return Err(Error::NeedsPool(feature.model.clone())),
    };
    let url = format!("{base}/v1/responses");
    let client = reqwest::blocking::Client::builder().timeout(TIMEOUT).build().map_err(|e| Error::Unreachable(base.clone(), e.to_string()))?;
    let reply = client.post(&url).json(&openai_body(feature, ask)).send().map_err(|e| Error::Unreachable(base.clone(), e.to_string()))?;
    let status = reply.status();
    let text = reply.text().map_err(|e| Error::Api(format!("{status}: {e}")))?;
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().or(v["detail"].as_str()).map(str::to_owned));
        return Err(Error::Api(message.unwrap_or_else(|| status.to_string())));
    }
    openai_answer(&text)
}

/// The JSON in a Responses API reply: a stream of events (the text's
/// deltas, then the whole response), or the response itself.
pub fn openai_answer(reply: &str) -> Result<Value, Error> {
    let mut deltas = String::new();
    let mut whole: Option<Value> = serde_json::from_str::<Value>(reply).ok();
    for line in reply.lines() {
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(event) = serde_json::from_str::<Value>(data.trim()) else { continue };
        match event["type"].as_str() {
            Some("response.output_text.delta") => deltas += event["delta"].as_str().unwrap_or(""),
            Some("response.completed" | "response.incomplete" | "response.failed") => whole = Some(event["response"].clone()),
            Some("error") => return Err(Error::Api(event["message"].as_str().unwrap_or("error").to_owned())),
            _ => {}
        }
    }
    if let Some(r) = &whole {
        if r["status"] == "failed" {
            return Err(Error::Api(r["error"]["message"].as_str().unwrap_or("the response failed").to_owned()));
        }
        if r["status"] == "incomplete" {
            return Err(match r["incomplete_details"]["reason"].as_str() {
                Some("content_filter") => Error::Refused,
                _ => Error::Truncated,
            });
        }
        let text: String = r["output"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| o["type"] == "message")
            .flat_map(|o| o["content"].as_array().into_iter().flatten())
            .filter(|c| c["type"] == "output_text")
            .filter_map(|c| c["text"].as_str())
            .collect();
        if !text.is_empty() {
            deltas = text;
        }
    }
    serde_json::from_str(deltas.trim()).map_err(|e| Error::BadAnswer(e.to_string()))
}

/// The JSON in a Messages API reply.
pub fn answer(reply: &Value) -> Result<Value, Error> {
    match reply["stop_reason"].as_str() {
        Some("refusal") => return Err(Error::Refused),
        Some("max_tokens") => return Err(Error::Truncated),
        _ => {}
    }
    let text: String = reply["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
    serde_json::from_str(text.trim()).map_err(|e| Error::BadAnswer(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_with_effort_schema_and_fallbacks() {
        let s = AiSettings::default();
        let ask = Ask { system: "s".into(), content: vec![text_block("hi")], schema: json!({"type": "object"}), max_tokens: 100 };
        let b = body(&s.smart_tiling, &ask);
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["output_config"]["effort"], "medium");
        assert_eq!(b["output_config"]["format"]["type"], "json_schema");
        assert_eq!(b["fallbacks"], "default");
        assert!(b.get("thinking").is_none());
        let haiku = AiFeature { model: "claude-haiku-5-5".into(), ..s.smart_tiling };
        assert!(body(&haiku, &ask).get("fallbacks").is_none());
    }

    #[test]
    fn asks_openai_in_its_shape() {
        let f = AiFeature { model: "gpt-6-astra".into(), effort: "max".into(), enabled: true };
        let ask = Ask { system: "s".into(), content: vec![png_block(b"x"), text_block("hi")], schema: json!({"type": "object"}), max_tokens: 100 };
        let b = openai_body(&f, &ask);
        assert_eq!(b["instructions"], "s");
        assert_eq!(b["reasoning"]["effort"], "xhigh");
        assert_eq!(b["input"][0]["content"][0]["type"], "input_image");
        assert!(b["input"][0]["content"][0]["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        assert_eq!(b["input"][0]["content"][1], json!({"type": "input_text", "text": "hi"}));
        assert_eq!(b["text"]["format"]["type"], "json_schema");
    }

    #[test]
    fn reads_openai_answers() {
        let stream = "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"a\\\": \"}\n\n\
                      data: {\"type\":\"response.output_text.delta\",\"delta\":\"1}\"}\n\n";
        assert_eq!(openai_answer(stream).unwrap(), json!({"a": 1}));
        let done = json!({"type": "response.completed", "response": {"status": "completed", "output": [{"type": "reasoning"}, {"type": "message", "content": [{"type": "output_text", "text": "{\"b\": 2}"}]}]}});
        assert_eq!(openai_answer(&format!("data: {done}\n")).unwrap(), json!({"b": 2}));
        let cut = json!({"type": "response.incomplete", "response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}});
        assert!(matches!(openai_answer(&format!("data: {cut}\n")), Err(Error::Truncated)));
    }

    #[test]
    fn reads_answers() {
        let ok = json!({"stop_reason": "end_turn", "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": "{\"a\": 1}"}]});
        assert_eq!(answer(&ok).unwrap(), json!({"a": 1}));
        assert!(matches!(answer(&json!({"stop_reason": "refusal", "content": []})), Err(Error::Refused)));
        assert!(matches!(answer(&json!({"stop_reason": "max_tokens", "content": []})), Err(Error::Truncated)));
    }
}
