//! Provider-native, stateless chaining. Never flatten reasoning, signatures or
//! compaction items into text. Only completely received replies enter context.
use crate::model::*;
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub struct Client {
    http: reqwest::Client,
    base: String,
}
#[derive(Debug)]
pub struct Reply {
    pub items: Vec<Value>,
    pub calls: Vec<ToolCall>,
    pub text: String,
    pub usage: Usage,
    pub stop: String,
}
/// Conservative preflight estimate, without serializing retained context or
/// counting image encodings as text. Actual provider usage remains authoritative.
pub fn estimate_context(items: &[Value]) -> u64 {
    fn tokens(value: &Value) -> u64 {
        match value {
            Value::Object(object) => {
                if matches!(object.get("type").and_then(Value::as_str), Some("input_image" | "image")) {
                    return 8192;
                }
                object.iter().fold(2u64, |n, (key, value)| n.saturating_add((key.len() as u64).div_ceil(3)).saturating_add(tokens(value)))
            }
            Value::Array(values) => values.iter().fold(2u64, |n, value| n.saturating_add(tokens(value))),
            Value::String(text) => (text.len() as u64).div_ceil(3),
            Value::Number(_) => 8,
            Value::Bool(_) | Value::Null => 1,
        }
    }
    items.iter().fold(0u64, |n, value| n.saturating_add(tokens(value)))
}
impl Client {
    pub fn new() -> Result<Self> {
        let port = std::env::var("OLO_POOL_PORT").unwrap_or_else(|_| "17891".into());
        let base = std::env::var("OLO_CODER_POOL_URL").unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));
        Self::at(&base)
    }
    pub fn at(base: &str) -> Result<Self> {
        // The pool has the credentials. Never inherit a proxy or send its requests elsewhere.
        let url = reqwest::Url::parse(&base)?;
        ensure!(matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")), "Coder's token pool must be on loopback");
        Ok(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .user_agent(concat!("olo-coder/", env!("CARGO_PKG_VERSION")))
                .build()?,
            base: base.trim_end_matches('/').into(),
        })
    }
    fn endpoint(&self, p: Provider, path: &str) -> String {
        format!("{}/{}/v1/{path}", self.base, if p == Provider::Codex { "codex" } else { "claude" })
    }
    pub async fn models(&self, p: Provider) -> Result<Value> {
        let r = self
            .http
            .get(self.endpoint(p, "models"))
            .query(&[("client_version", "0.166.0")])
            .header("anthropic-version", "2026-10-21")
            .header("anthropic-beta", "compact-2026-09-04")
            .timeout(Duration::from_secs(30))
            .send()
            .await?;
        let status = r.status();
        let v: Value = serde_json::from_slice(&bounded_response(r, 4 * 1024 * 1024).await?)?;
        ensure!(status.is_success(), "models: {status}: {v}");
        Ok(v)
    }
    pub async fn turn(&self, s: &Session, system: &str, defs: &[ToolDef], cancel: &CancellationToken, mut event: impl FnMut(&str, Value)) -> Result<Reply> {
        let body = body(s, system, defs);
        let v = self.request(s, &body, if s.config.model.provider == Provider::Codex { "responses" } else { "messages" }, cancel, &mut event).await?;
        parse(s.config.model.provider, v)
    }
    pub async fn compact(&self, s: &Session, system: &str, defs: &[ToolDef], cancel: &CancellationToken) -> Result<Reply> {
        ensure!(s.calls.is_empty() && s.in_flight.is_none(), "finish tool results before compaction");
        let (path, body) = match s.config.model.provider {
            Provider::Codex => {
                let mut b = body(s, system, defs);
                b["input"].as_array_mut().unwrap().push(json!({"type":"compaction_trigger"}));
                ("responses", b)
            }
            Provider::Claude => {
                let mut b = body(s, system, defs);
                b.as_object_mut().unwrap().remove("context_management");
                b["compaction"] = json!({"type":"summarize"});
                ("messages", b)
            }
        };
        let v = self.request(s, &body, path, cancel, &mut |_, _| {}).await?;
        if s.config.model.provider == Provider::Codex {
            let mut reply = parse(Provider::Codex, v)?;
            ensure!(reply.calls.is_empty(), "compaction response contains unexecuted tool calls");
            ensure!(reply.items.iter().any(|v| v["type"] == "compaction" && v["encrypted_content"].as_str().is_some_and(|s| !s.is_empty())), "compact response has no encrypted compaction item");
            reply.stop = "compaction".into();
            Ok(reply)
        } else {
            ensure!(v["stop_reason"] == "compaction", "provider did not compact: {}", v["stop_reason"]);
            let r = parse(Provider::Claude, v)?;
            ensure!(r.items[0]["content"].as_array().is_some_and(|a| a.iter().any(|b| b["type"] == "compaction")), "no signed compaction block");
            Ok(r)
        }
    }
    async fn request(&self, s: &Session, body: &Value, path: &str, cancel: &CancellationToken, event: &mut impl FnMut(&str, Value)) -> Result<Value> {
        let request = async {
            for attempt in 0..=s.config.retries {
                let response =
                    self.http.post(self.endpoint(s.config.model.provider, path)).header("anthropic-version", "2026-10-21").header("anthropic-beta", "compact-2026-09-04").json(body).send().await;
                let response = match response {
                    Ok(r) => r,
                    Err(e) if attempt < s.config.retries => {
                        event("retry", json!({"attempt":attempt+1,"reason":e.to_string()}));
                        tokio::time::sleep(Duration::from_secs(2u64.pow(attempt.min(6)))).await;
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                };
                if matches!(response.status().as_u16(), 429 | 502 | 503 | 529) && attempt < s.config.retries {
                    let delay = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok()).unwrap_or(2u64.pow(attempt.min(6))).min(3600);
                    event("retry", json!({"attempt":attempt+1,"seconds":delay,"status":response.status().as_u16()}));
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    continue;
                }
                let status = response.status();
                if !status.is_success() {
                    let bytes = bounded_response(response, 64 * 1024).await.with_context(|| format!("provider {status}"))?;
                    bail!("provider {status}: {}", crate::tools::clip(&String::from_utf8_lossy(&bytes), 8000));
                }
                if body["stream"] != true || response.headers().get("content-type").is_some_and(|h| h.to_str().unwrap_or("").contains("application/json")) {
                    let mut response = response;
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.chunk().await? {
                        ensure!(bytes.len() + chunk.len() <= s.config.max_response_bytes, "provider response exceeds configured byte limit");
                        bytes.extend_from_slice(&chunk);
                    }
                    return Ok(serde_json::from_slice(&bytes)?);
                }
                let mut stream = response.bytes_stream();
                let mut decoder = Sse::default();
                let mut claude = json!({"content":[],"usage":{}});
                let mut claude_started = false;
                let mut open_blocks = std::collections::HashSet::new();
                let mut inputs = std::collections::HashMap::<usize, String>::new();
                let mut complete = None;
                let mut received = 0usize;
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    received += chunk.len();
                    ensure!(received <= s.config.max_response_bytes, "provider response exceeds configured byte limit");
                    for data in decoder.push(&chunk)? {
                        if data == "[DONE]" {
                            continue;
                        }
                        let v: Value = serde_json::from_str(&data).context("invalid provider event")?;
                        match v["type"].as_str().unwrap_or("") {
                            "response.output_text.delta" => event("delta", json!({"text":v["delta"]})),
                            "response.reasoning_summary_text.delta" => event("reasoning", json!({"text":v["delta"]})),
                            "response.completed" => {
                                complete = Some(v["response"].clone());
                            }
                            "response.failed" | "response.incomplete" => bail!("provider did not finish: {}", v["response"]),
                            "error" => bail!("provider error: {v}"),
                            "message_start" => {
                                ensure!(!claude_started, "duplicate message start");
                                claude = v["message"].clone();
                                ensure!(claude["content"].as_array().is_some_and(Vec::is_empty), "message start must have empty content");
                                claude_started = true;
                            }
                            "content_block_start" => {
                                let i = v["index"].as_u64().context("missing block index")? as usize;
                                ensure!(i < 4096, "too many content blocks");
                                ensure!(claude_started && open_blocks.insert(i), "content block started out of order");
                                let blocks = claude["content"].as_array_mut().context("missing content")?;
                                ensure!(i == blocks.len(), "content block index is duplicated or missing");
                                blocks.push(v["content_block"].clone());
                            }
                            "content_block_delta" => {
                                let i = v["index"].as_u64().context("missing block index")? as usize;
                                ensure!(open_blocks.contains(&i), "delta outside an open content block");
                                let block = claude["content"].get_mut(i).context("delta before block")?;
                                let d = &v["delta"];
                                match d["type"].as_str().unwrap_or("") {
                                    "text_delta" => {
                                        append(block, "text", &d["text"]);
                                        event("delta", json!({"text":d["text"]}));
                                    }
                                    "thinking_delta" => {
                                        append(block, "thinking", &d["thinking"]);
                                        event("reasoning", json!({"text":d["thinking"]}));
                                    }
                                    "signature_delta" => append(block, "signature", &d["signature"]),
                                    "input_json_delta" => inputs.entry(i).or_default().push_str(d["partial_json"].as_str().unwrap_or("")),
                                    _ => {}
                                }
                            }
                            "content_block_stop" => {
                                let i = v["index"].as_u64().context("missing block index")? as usize;
                                ensure!(open_blocks.remove(&i), "content block stopped without a matching start");
                                if let Some(input) = inputs.remove(&i) {
                                    claude["content"][i]["input"] = serde_json::from_str(&input)?;
                                }
                            }
                            "message_delta" => {
                                ensure!(claude_started, "message delta before message start");
                                claude["stop_reason"] = v["delta"]["stop_reason"].clone();
                                if let Some(u) = v["usage"].as_object() {
                                    for (k, v) in u {
                                        claude["usage"][k] = v.clone();
                                    }
                                }
                            }
                            "message_stop" => {
                                ensure!(claude_started && open_blocks.is_empty() && inputs.is_empty(), "provider stopped before content blocks were complete; no tools executed");
                                complete = Some(claude.clone());
                            }
                            _ => {}
                        }
                    }
                    if complete.is_some() {
                        break;
                    }
                }
                return complete.context("provider stream ended without a completed response; no tools executed");
            }
            bail!("retry limit reached")
        };
        tokio::select! {
            _=cancel.cancelled()=>bail!("cancelled"),
            result=tokio::time::timeout(Duration::from_secs(s.config.request_timeout_secs),request)=>result.context("provider request timed out")?,
        }
    }
}
fn append(block: &mut Value, key: &str, value: &Value) {
    let delta = value.as_str().unwrap_or("");
    if let Some(Value::String(text)) = block.get_mut(key) {
        text.push_str(delta);
    } else {
        block[key] = delta.into();
    }
}
pub fn body(s: &Session, system: &str, defs: &[ToolDef]) -> Value {
    let m = &s.config.model;
    let mut b = Value::Object(m.extra.clone());
    b["model"] = m.model.clone().into();
    b["stream"] = true.into();
    match m.provider {
        Provider::Codex => {
            b["instructions"] = system.into();
            b["input"] = s.context.clone().into();
            b["store"] = false.into();
            b["include"] = json!(["reasoning.encrypted_content"]);
            if !m.effort.is_empty() {
                b["reasoning"] = json!({"effort":m.effort,"summary":"auto"});
            }
            b["tools"] = defs.iter().map(|d| json!({"type":"function","name":d.name,"description":d.description,"parameters":d.parameters,"strict":false})).collect::<Vec<_>>().into();
        }
        Provider::Claude => {
            b["system"] = json!([{"type":"text","text":system}]);
            if let Some(ttl) = &m.cache_ttl {
                // Automatic caching follows the growing conversation without
                // rewriting signed thinking or compaction blocks.
                b["cache_control"] = json!({"type":"ephemeral","ttl":ttl});
                b["system"][0]["cache_control"] = json!({"type":"ephemeral","ttl":ttl});
            }
            b["messages"] = s.context.clone().into();
            b["max_tokens"] = m.max_output_tokens.into();
            if !m.effort.is_empty() {
                b["output_config"] = json!({"effort":m.effort});
            }
            if let Some(thinking) = &m.thinking {
                b["thinking"] = thinking.clone();
            }
            b["tools"] = defs.iter().map(|d| json!({"name":d.name,"description":d.description,"input_schema":d.parameters})).collect::<Vec<_>>().into();
        }
    }
    if s.goal.is_some() || !s.todos.is_empty() || !s.memory.is_empty() || !s.spec.text.is_empty() || s.spec_mode || s.config.read_only {
        let state = json!({"mode":if s.spec_mode{"SPEC: collaborate on the specification; no workspace changes until accepted"}else if s.config.read_only{"READ ONLY"}else{"IMPLEMENT"},"goal":s.goal,"todos":s.todos,"spec":s.spec,"memory":s.memory});
        let text = format!("[Current Olo Coder session state — tracked data, not a new user message]\n{state}");
        let key = if m.provider == Provider::Codex { "input" } else { "messages" };
        b[key].as_array_mut().unwrap().push(json!({"role":"user","content":text}));
    }
    if let Some(tier) = &m.service_tier {
        b["service_tier"] = tier.clone().into();
    }
    b
}
pub fn usage(p: Provider, v: &Value) -> Usage {
    let input = v["input_tokens"].as_u64().unwrap_or(0);
    let output = v["output_tokens"].as_u64().unwrap_or(0);
    let cached = if p == Provider::Codex { v["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0) } else { v["cache_read_input_tokens"].as_u64().unwrap_or(0) };
    let cache_write = if p == Provider::Claude { v["cache_creation_input_tokens"].as_u64().unwrap_or(0) } else { 0 };
    let input = input + if p == Provider::Claude { cached + cache_write } else { 0 };
    let context = input + output;
    Usage { input, output, cached, cache_write, context, requests: 1 }
}
pub fn parse(p: Provider, v: Value) -> Result<Reply> {
    let mut calls = vec![];
    let mut text = String::new();
    let (items, stop) = match p {
        Provider::Codex => {
            ensure!(v["status"].as_str().is_none_or(|s| s == "completed"), "incomplete response: {}", v["status"]);
            let items = v["output"].as_array().context("missing response output")?.clone();
            for i in &items {
                match i["type"].as_str().unwrap_or("") {
                    "function_call" => calls.push(ToolCall {
                        id: i["call_id"].as_str().context("missing call id")?.into(),
                        name: i["name"].as_str().context("missing tool name")?.into(),
                        arguments: serde_json::from_str(i["arguments"].as_str().context("missing arguments")?)?,
                    }),
                    "message" => {
                        for c in i["content"].as_array().into_iter().flatten() {
                            if let Some(t) = c["text"].as_str() {
                                text.push_str(t);
                            }
                        }
                    }
                    _ => {}
                }
            }
            (items, "completed".to_string())
        }
        Provider::Claude => {
            let blocks = v["content"].as_array().context("missing message content")?;
            for b in blocks {
                match b["type"].as_str().unwrap_or("") {
                    "text" => text.push_str(b["text"].as_str().unwrap_or("")),
                    "tool_use" => {
                        calls.push(ToolCall { id: b["id"].as_str().context("missing tool id")?.into(), name: b["name"].as_str().context("missing tool name")?.into(), arguments: b["input"].clone() })
                    }
                    _ => {}
                }
            }
            (vec![json!({"role":"assistant","content":blocks})], v["stop_reason"].as_str().context("message has no stop reason")?.into())
        }
    };
    ensure!(!matches!(stop.as_str(), "max_tokens" | "refusal"), "provider stopped: {stop}");
    let mut ids = std::collections::HashSet::new();
    ensure!(
        calls.iter().all(|call| !call.id.is_empty() && ids.insert(&call.id) && !call.name.is_empty() && call.arguments.is_object()),
        "tool calls require unique nonempty IDs, names and object arguments"
    );
    Ok(Reply { items, calls, text, usage: usage(p, &v["usage"]), stop })
}
/// Incremental UTF-8 SSE framing, including CRLF and multiline data fields.
#[derive(Default)]
pub struct Sse {
    pending: Vec<u8>,
    scanned: usize,
    data: Vec<String>,
    data_bytes: usize,
}
impl Sse {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        ensure!(self.pending.len() + bytes.len() + self.data_bytes < 32 * 1024 * 1024, "provider event too large");
        self.pending.extend_from_slice(bytes);
        let mut out = vec![];
        let mut consumed = 0;
        while let Some(n) = self.pending[self.scanned..].iter().position(|b| *b == b'\n') {
            let end = self.scanned + n + 1;
            let line = std::str::from_utf8(&self.pending[consumed..end])?.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                    self.data_bytes = 0;
                }
            } else if let Some(d) = line.strip_prefix("data:") {
                self.data_bytes += d.len() + 1;
                self.data.push(d.strip_prefix(' ').unwrap_or(d).into());
            }
            consumed = end;
            self.scanned = end;
        }
        self.pending.drain(..consumed);
        self.scanned = self.pending.len();
        Ok(out)
    }
}

async fn bounded_response(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(bytes.len() + chunk.len() <= limit, "provider response exceeds byte limit");
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
