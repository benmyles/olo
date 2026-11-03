//! How Olo's AI features (Preferences ▸ AI) reach their models:
//! OpenAI's (`AI_MODELS`, Astra 6 first) through the Token Pool's Codex
//! accounts, its gateway on this computer. Claude (through the pool's
//! Claude accounts, or straight to Anthropic with a saved API key) is
//! kept for when Claude models are offered again; none is now. One blocking call at a time (the Messages API, or the
//! Responses API for OpenAI's); call it off the UI thread (a feature's
//! own process or a worker). A call the server turns away for now (429
//! rate limited, 503/529 overloaded) is tried again after a wait.
//!
//! `OLO_AI_BASE_URL` sends every call somewhere else instead (a fake
//! server, in tests): never a real model there.

use std::{io::Write, path::PathBuf, time::Duration};

use olo_protocol::{AiFeature, AiSettings};
use serde_json::{Value, json};

/// The API version every call asks for.
const API_VERSION: &str = "2026-10-21";
/// The pool's port unless `OLO_POOL_PORT` says otherwise (as the
/// pool's own `gateway::PORT`; not linked, to keep the pool's server out
/// of every AI feature).
const POOL_PORT: u16 = 17891;
/// A model looking at a screen and thinking can take a while.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Tries at a call the server turns away for now.
const TRIES: u32 = 4;
/// The longest wait for another try; asked to wait longer (a quota that
/// resets in an hour), the call gives up with the server's answer.
const LONGEST_WAIT: Duration = Duration::from_secs(60);
/// How the features introduce themselves (the Token Pool lists requests
/// by it).
const USER_AGENT: &str = concat!("olo-ai/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("No Anthropic API key is saved (Preferences ▸ AI), and the Token Pool is off.")]
    NoKey,
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
    olo_xdg::config_dir().join("anthropic-key")
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
    if let Ok(base) = std::env::var("OLO_AI_BASE_URL") {
        return Ok((base.trim_end_matches('/').to_owned(), None));
    }
    if settings.use_pool {
        let port = std::env::var("OLO_POOL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(POOL_PORT);
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

/// Ask `feature`'s model (one still offered: [`AiFeature::model_id`]);
/// its JSON answer.
pub fn ask(settings: &AiSettings, feature: &AiFeature, ask: &Ask) -> Result<Value, Error> {
    let feature = &AiFeature { model: feature.model_id().into(), ..feature.clone() };
    if is_openai(&feature.model) {
        return ask_openai(feature, ask);
    }
    let (base, key) = endpoint(settings)?;
    let url = format!("{base}/v1/messages");
    let client = http(&base)?;
    let body = body(feature, ask);
    let reply = send(&base, || {
        let mut req = client.post(&url).header("anthropic-version", API_VERSION).json(&body);
        if fallbacks(&feature.model) {
            req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        if let Some(key) = &key {
            req = req.header("x-api-key", key);
        }
        req
    })?;
    let status = reply.status();
    let reply: Value = reply.json().map_err(|e| Error::Api(format!("{status}: {e}")))?;
    if !status.is_success() {
        let message = reply["error"]["message"].as_str().map(str::to_owned).unwrap_or_else(|| status.to_string());
        return Err(Error::Api(message));
    }
    answer(&reply)
}

fn http(base: &str) -> Result<reqwest::blocking::Client, Error> {
    reqwest::blocking::Client::builder().timeout(TIMEOUT).user_agent(USER_AGENT).build().map_err(|e| Error::Unreachable(base.to_owned(), e.to_string()))
}

/// Send the request `make` builds, again after a wait while the server
/// turns it away for now; the last answer.
fn send(base: &str, make: impl Fn() -> reqwest::blocking::RequestBuilder) -> Result<reqwest::blocking::Response, Error> {
    let mut tries = 0;
    loop {
        tries += 1;
        let reply = make().send().map_err(|e| Error::Unreachable(base.to_owned(), e.to_string()))?;
        if !matches!(reply.status().as_u16(), 429 | 503 | 529) || tries >= TRIES {
            return Ok(reply);
        }
        match wait(tries, reply.headers()) {
            Some(d) => std::thread::sleep(d),
            None => return Ok(reply),
        }
    }
}

/// How long to wait after try `tries` was turned away: what the server
/// asks (`retry-after`, in seconds), else 2, 4, 8 s, each with up to a
/// quarter more so callers turned away together don't all come back
/// together. None if the server asks for longer than `LONGEST_WAIT`.
fn wait(tries: u32, headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let asked = headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f64>().ok()).filter(|s| s.is_finite() && *s >= 0.0);
    if let Some(s) = asked {
        let d = Duration::from_secs_f64(s);
        return (d <= LONGEST_WAIT).then_some(d);
    }
    let base = Duration::from_secs(1 << tries.min(5));
    let jitter = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos()) as f64 / 1e9 / 4.0;
    Some(base.mul_f64(1.0 + jitter))
}

/// The Responses API body for `ask` (as Codex sends it: streamed, not
/// stored, the system prompt as instructions).
pub fn openai_body(feature: &AiFeature, ask: &Ask) -> Value {
    let content: Vec<Value> = ask
        .content
        .iter()
        .map(|b| match b["type"].as_str() {
            Some("image") => {
                json!({"type": "input_image", "image_url": format!("data:{};base64,{}", b["source"]["media_type"].as_str().unwrap_or("image/png"), b["source"]["data"].as_str().unwrap_or(""))})
            }
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

fn ask_openai(feature: &AiFeature, ask: &Ask) -> Result<Value, Error> {
    let base = match std::env::var("OLO_AI_BASE_URL") {
        Ok(base) => base.trim_end_matches('/').to_owned(),
        Err(_) => {
            let port = std::env::var("OLO_POOL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(POOL_PORT);
            format!("http://127.0.0.1:{port}/codex")
        }
    };
    let url = format!("{base}/v1/responses");
    let client = http(&base)?;
    let body = openai_body(feature, ask);
    let reply = send(&base, || client.post(&url).json(&body))?;
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
        let opus = AiFeature { model: "claude-opus-5-5".into(), effort: "medium".into(), enabled: true };
        let ask = Ask { system: "s".into(), content: vec![text_block("hi")], schema: json!({"type": "object"}), max_tokens: 100 };
        let b = body(&opus, &ask);
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["output_config"]["effort"], "medium");
        assert_eq!(b["output_config"]["format"]["type"], "json_schema");
        assert_eq!(b["fallbacks"], "default");
        assert!(b.get("thinking").is_none());
        let haiku = AiFeature { model: "claude-haiku-5-5".into(), ..opus };
        assert!(body(&haiku, &ask).get("fallbacks").is_none());
    }

    #[test]
    fn astra_by_default_and_for_models_no_longer_offered() {
        let f = AiSettings::default().smart_tiling;
        assert_eq!((f.model_id(), f.model_name(), f.effort.as_str()), ("gpt-6-astra", "Astra 6", "medium"));
        let old = AiFeature { model: "claude-opus-5-5".into(), ..f.clone() };
        assert_eq!(old.model_id(), "gpt-6-astra");
        let sol = AiFeature { model: "gpt-6.1-sol".into(), ..f };
        assert_eq!((sol.model_id(), sol.model_name()), ("gpt-6.1-sol", "Sol 6.1"));
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
    fn waits_before_trying_again() {
        let mut h = reqwest::header::HeaderMap::new();
        let d = wait(1, &h).unwrap();
        assert!(d >= Duration::from_secs(2) && d <= Duration::from_millis(2500), "{d:?}");
        assert!(wait(3, &h).unwrap() >= Duration::from_secs(8));
        h.insert("retry-after", "7".parse().unwrap());
        assert_eq!(wait(1, &h), Some(Duration::from_secs(7)));
        h.insert("retry-after", "3600".parse().unwrap());
        assert_eq!(wait(1, &h), None);
    }

    /// A server on loopback that turns the first `busy` requests away
    /// (429, "retry-after: 0") and then answers 200; how many it saw.
    fn busy_server(busy: usize) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{BufRead, BufReader, Read};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let _ = reader.read_exact(&mut vec![0; length]);
                let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let reply = if n < busy {
                    "HTTP/1.1 429 Too Many Requests\r\nretry-after: 0\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}"
                } else {
                    "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}"
                };
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (base, seen)
    }

    #[test]
    fn tries_again_when_turned_away() {
        let (base, seen) = busy_server(2);
        let client = http(&base).unwrap();
        let reply = send(&base, || client.post(&base).json(&json!({}))).unwrap();
        assert_eq!(reply.status(), 200);
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 3);
        // Turned away every time: the last answer, after TRIES tries.
        let (base, seen) = busy_server(100);
        let reply = send(&base, || client.post(&base).json(&json!({}))).unwrap();
        assert_eq!(reply.status(), 429);
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), TRIES as usize);
    }

    #[test]
    fn reads_answers() {
        let ok = json!({"stop_reason": "end_turn", "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": "{\"a\": 1}"}]});
        assert_eq!(answer(&ok).unwrap(), json!({"a": 1}));
        assert!(matches!(answer(&json!({"stop_reason": "refusal", "content": []})), Err(Error::Refused)));
        assert!(matches!(answer(&json!({"stop_reason": "max_tokens", "content": []})), Err(Error::Truncated)));
    }
}
