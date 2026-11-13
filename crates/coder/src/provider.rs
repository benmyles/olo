//! Provider-native, stateless chaining. Never flatten reasoning, signatures or
//! compaction items into text. Only completely received replies enter context.
use crate::model::*;
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    /// Keys read from Passwords, by reference: read once, kept in memory
    /// only, forgotten when the service refuses them.
    keys: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, zeroize::Zeroizing<String>>>>,
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
                if matches!(object.get("type").and_then(Value::as_str), Some("input_image" | "image" | "image_url")) {
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
        let url = reqwest::Url::parse(base)?;
        ensure!(matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")), "Coder's token pool must be on loopback");
        Ok(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .user_agent(concat!("olo-coder/", env!("CARGO_PKG_VERSION")))
                .build()?,
            base: base.trim_end_matches('/').into(),
            keys: Default::default(),
        })
    }
    fn endpoint(&self, model: &ModelConfig, path: &str) -> Result<String> {
        model.validate()?;
        Ok(match model.provider {
            Provider::Codex => format!("{}/codex/v1/{path}", self.base),
            Provider::Openai => format!("{}/{path}", model.endpoint.as_ref().context("custom endpoint is missing")?.base_url.trim_end_matches('/')),
        })
    }
    async fn key(&self, endpoint: &EndpointConfig) -> Result<Option<zeroize::Zeroizing<String>>> {
        if let Some(variable) = &endpoint.api_key_env {
            let known = crate::providers::KNOWN.iter().find(|k| k.base_url == endpoint.base_url.trim_end_matches('/'));
            let key = std::env::var(variable).with_context(|| match known {
                Some(k) => format!("{} needs a key: choose its field in Passwords (Settings ▸ Model ▸ Key), or set {variable} in the Coder service environment. Keys: {}", k.name, k.keys_page),
                None => format!("set {variable} in the Coder service environment"),
            })?;
            ensure!(!key.trim().is_empty(), "{variable} is empty");
            return Ok(Some(key.into()));
        }
        let Some(reference) = &endpoint.api_key_password else { return Ok(None) };
        if let Some(key) = self.keys.lock().unwrap().get(reference) {
            return Ok(Some(key.clone()));
        }
        let key = zeroize::Zeroizing::new(crate::integrations::resolve(reference).await.context("reading the endpoint's key from Passwords")?);
        ensure!(!key.trim().is_empty(), "the key's field in Passwords is empty");
        self.keys.lock().unwrap().insert(reference.clone(), key.clone());
        Ok(Some(key))
    }
    /// Forgets a Passwords key the service refused, so the next request reads it again.
    fn refused(&self, model: &ModelConfig) {
        if let Some(reference) = model.endpoint.as_ref().and_then(|e| e.api_key_password.as_ref()) {
            self.keys.lock().unwrap().remove(reference);
        }
    }
    async fn authorize(&self, mut request: reqwest::RequestBuilder, model: &ModelConfig) -> Result<reqwest::RequestBuilder> {
        if model.provider == Provider::Openai {
            let endpoint = model.endpoint.as_ref().context("custom endpoint is missing")?;
            if let Some(key) = self.key(endpoint).await? {
                let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", key.as_str())).context("invalid API key header")?;
                value.set_sensitive(true);
                request = request.header(reqwest::header::AUTHORIZATION, value);
            }
            for (name, variable) in &endpoint.header_env {
                let secret = std::env::var(variable).with_context(|| format!("set {variable} in the Coder service environment"))?;
                let mut value = reqwest::header::HeaderValue::from_str(&secret).context("invalid configured header value")?;
                value.set_sensitive(true);
                request = request.header(name, value);
            }
        }
        Ok(request)
    }
    pub async fn models(&self, model: &ModelConfig) -> Result<Value> {
        let mut request = self.http.get(self.endpoint(model, "models")?).timeout(Duration::from_secs(30));
        if model.provider == Provider::Codex {
            request = request.query(&[("client_version", "0.166.0")]);
        }
        let response = self.authorize(request, model).await?.send().await?;
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            self.refused(model);
        }
        ensure!(status.is_success(), "models: {status}");
        Ok(serde_json::from_slice(&bounded_response(response, 4 * 1024 * 1024).await?)?)
    }
    pub async fn turn(&self, s: &Session, system: &str, defs: &[ToolDef], cancel: &CancellationToken, mut event: impl FnMut(&str, Value)) -> Result<Reply> {
        let body = body(s, system, defs);
        if s.config.model.provider == Provider::Openai {
            let estimate = estimate_context(std::slice::from_ref(&body));
            ensure!(
                estimate.saturating_add(s.config.model.max_output_tokens as u64) <= s.config.model.context_window,
                "estimated request exceeds context_window; compact the session or increase its configured window (including tools and output allowance)"
            );
        }
        let v = self.request(s, &body, if s.config.model.provider == Provider::Codex { "responses" } else { "chat/completions" }, cancel, &mut event).await?;
        let missing_input = v["usage"]["prompt_tokens"].as_u64().unwrap_or(0) == 0;
        let missing_output = v["usage"]["completion_tokens"].as_u64().unwrap_or(0) == 0;
        let mut reply = parse(s.config.model.provider, v)?;
        if s.config.model.provider == Provider::Openai {
            if missing_input {
                reply.usage.input = estimate_context(std::slice::from_ref(&body));
            }
            if missing_output {
                reply.usage.output = estimate_context(&reply.items);
            }
            reply.usage.context = reply.usage.input.saturating_add(reply.usage.output);
        }
        Ok(reply)
    }
    pub async fn compact(&self, s: &Session, system: &str, defs: &[ToolDef], cancel: &CancellationToken) -> Result<Reply> {
        ensure!(s.calls.is_empty() && s.in_flight.is_none(), "finish tool results before compaction");
        ensure!(s.config.model.provider == Provider::Codex, "custom endpoints use Coder's checkpoint compactor");
        let mut request = body(s, system, defs);
        request["input"].as_array_mut().unwrap().push(json!({"type":"compaction_trigger"}));
        let value = self.request(s, &request, "responses", cancel, &mut |_, _| {}).await?;
        let mut reply = parse(Provider::Codex, value)?;
        ensure!(reply.calls.is_empty(), "compaction response contains unexecuted tool calls");
        ensure!(reply.items.iter().any(|v| v["type"] == "compaction" && v["encrypted_content"].as_str().is_some_and(|s| !s.is_empty())), "compact response has no encrypted compaction item");
        reply.stop = "compaction".into();
        Ok(reply)
    }
    async fn request(&self, s: &Session, body: &Value, path: &str, cancel: &CancellationToken, event: &mut impl FnMut(&str, Value)) -> Result<Value> {
        ensure!(!cancel.is_cancelled(), "cancelled before provider request dispatch");
        let request = async {
            for attempt in 0..=s.config.retries {
                let mut request = self.http.post(self.endpoint(&s.config.model, path)?).json(body);
                if let Some(header) = crate::providers::of(&s.config.model).and_then(|k| k.affinity_header) {
                    request = request.header(header, &s.id);
                }
                let response = self.authorize(request, &s.config.model).await?.send().await;
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
                    if matches!(status.as_u16(), 401 | 403) {
                        self.refused(&s.config.model);
                    }
                    if s.config.model.provider == Provider::Openai {
                        // Custom gateways may echo authentication in error bodies.
                        bail!("custom endpoint returned HTTP {status}; check endpoint, model, credentials and quota");
                    }
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
                let mut chat = crate::chat::Stream::default();
                let mut complete = None;
                let mut received = 0usize;
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    received += chunk.len();
                    ensure!(received <= s.config.max_response_bytes, "provider response exceeds configured byte limit");
                    for data in decoder.push(&chunk)? {
                        if data == "[DONE]" {
                            if s.config.model.provider == Provider::Openai {
                                return chat.finish();
                            }
                            continue;
                        }
                        let v: Value = serde_json::from_str(&data).context("invalid provider event")?;
                        if s.config.model.provider == Provider::Openai {
                            chat.push(v, event)?;
                            continue;
                        }
                        match v["type"].as_str().unwrap_or("") {
                            "response.output_text.delta" => event("delta", json!({"text":v["delta"]})),
                            "response.reasoning_summary_text.delta" => event("reasoning", json!({"text":v["delta"]})),
                            "response.completed" => {
                                complete = Some(v["response"].clone());
                            }
                            "response.failed" | "response.incomplete" => bail!("provider did not finish: {}", v["response"]),
                            "error" => bail!("provider error: {v}"),
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
            biased;
            _=cancel.cancelled()=>bail!("cancelled"),
            result=tokio::time::timeout(Duration::from_secs(s.config.request_timeout_secs),request)=>result.context("provider request timed out")?,
        }
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
        Provider::Openai => {
            let endpoint = m.endpoint.as_ref();
            b["stream"] = endpoint.is_none_or(|e| e.stream).into();
            b.as_object_mut().unwrap().remove("stream_options");
            if b["stream"] == true && endpoint.is_none_or(|e| e.include_usage) {
                b["stream_options"] = json!({"include_usage":true});
            }
            let mut messages = vec![json!({"role":"system","content":system})];
            messages.extend(s.context.clone());
            b["messages"] = messages.into();
            let key = if endpoint.is_some_and(|e| e.token_limit_field == TokenLimitField::MaxCompletionTokens) { "max_completion_tokens" } else { "max_tokens" };
            b.as_object_mut().unwrap().remove(if key == "max_tokens" { "max_completion_tokens" } else { "max_tokens" });
            b[key] = m.max_output_tokens.into();
            if !m.effort.is_empty() {
                b["reasoning_effort"] = m.effort.clone().into();
            }
            if let Some(thinking) = &m.thinking {
                b["thinking"] = thinking.clone();
            }
            for key in ["tools", "functions", "function_call"] {
                b.as_object_mut().unwrap().remove(key);
            }
            if defs.is_empty() {
                for key in ["tool_choice", "parallel_tool_calls"] {
                    b.as_object_mut().unwrap().remove(key);
                }
            }
            if !defs.is_empty() {
                b["tools"] = defs.iter().map(|d| json!({"type":"function","function":{"name":d.name,"description":d.description,"parameters":d.parameters}})).collect::<Vec<_>>().into();
            }
            b["n"] = 1.into();
        }
    }
    let reminders = reminders(s);
    if s.goal.is_some() || !s.todos.is_empty() || !s.memory.is_empty() || !s.spec.text.is_empty() || s.spec_mode || s.config.read_only || !reminders.is_empty() {
        let mut state = json!({"mode":if s.spec_mode{"SPEC: collaborate on the specification; no workspace changes until accepted"}else if s.config.read_only{"READ ONLY"}else{"IMPLEMENT"},"goal":s.goal,"todos":s.todos,"spec":s.spec,"memory":s.memory});
        if !reminders.is_empty() {
            state["reminders"] = reminders.into();
        }
        let text = format!("[Current Olo Coder session state — tracked data, not a new user message]\n{state}");
        let key = if m.provider == Provider::Codex { "input" } else { "messages" };
        b[key].as_array_mut().unwrap().push(json!({"role":"user","content":text}));
    }
    if let Some(tier) = &m.service_tier {
        b["service_tier"] = tier.clone().into();
    }
    b
}
/// Actions since the task list was last updated before Coder reminds the
/// agent, and since its last status report.
pub const TASKS_STALE_AFTER: u64 = 8;
pub const REPORT_STALE_AFTER: u64 = 5;

/// What the agent is reminded of with this request: a task list it hasn't
/// made or kept up, a status report that's old. Sent with the request's
/// session state, never stored in the conversation, so reminding costs
/// nothing once it's done.
pub fn reminders(s: &Session) -> Vec<String> {
    let mut out = Vec::new();
    let since_tasks = s.tool_sequence.saturating_sub(s.todos_sequence);
    let open = s.todos.iter().filter(|t| t.status != "done").count();
    if s.todos.is_empty() && since_tasks >= 3 {
        out.push("This is multi-step work and there's no task list yet: make one with set_todos (the main steps, with subtasks under them) so the person can follow it and edit it.".to_owned());
    } else if open > 0 && since_tasks >= TASKS_STALE_AFTER {
        out.push(format!(
            "{since_tasks} actions since you last updated the task list: bring it up to date with update_todos (in_progress for what you're on, done with evidence for what's finished)."
        ));
    }
    let since_report = s.tool_sequence.saturating_sub(s.report.as_ref().map_or(0, |r| r.sequence));
    if since_report >= REPORT_STALE_AFTER {
        out.push(format!("{since_report} actions since your last report_status: send one (done / doing now / up next, a few words each)."));
    }
    out
}
pub fn usage(p: Provider, v: &Value) -> Usage {
    let input = v[if p == Provider::Codex { "input_tokens" } else { "prompt_tokens" }].as_u64().unwrap_or(0);
    let output = v[if p == Provider::Codex { "output_tokens" } else { "completion_tokens" }].as_u64().unwrap_or(0);
    let cached = v[if p == Provider::Codex { "input_tokens_details" } else { "prompt_tokens_details" }]["cached_tokens"].as_u64().unwrap_or(0);
    Usage { input, output, cached, cache_write: 0, context: input.saturating_add(output), requests: 1 }
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
        Provider::Openai => {
            let choices = v["choices"].as_array().context("missing completion choices")?;
            ensure!(choices.len() == 1, "expected exactly one completion choice");
            let choice = &choices[0];
            let message = &choice["message"];
            ensure!(message["role"] == "assistant", "completion must be an assistant message");
            let stop = choice["finish_reason"].as_str().context("completion has no finish reason")?;
            ensure!(matches!(stop, "stop" | "tool_calls"), "provider stopped: {stop}; no tools executed");
            ensure!(message["refusal"].is_null() || message["refusal"] == "", "provider refused the request");
            if let Some(content) = message["content"].as_str() {
                text.push_str(content);
            }
            if let Some(tools) = message.get("tool_calls").filter(|v| !v.is_null()) {
                for call in tools.as_array().context("tool_calls must be an array")? {
                    ensure!(call["type"] == "function", "unsupported tool call type");
                    calls.push(ToolCall {
                        id: call["id"].as_str().context("missing tool id")?.into(),
                        name: call["function"]["name"].as_str().context("missing tool name")?.into(),
                        arguments: serde_json::from_str(call["function"]["arguments"].as_str().context("missing tool arguments")?)?,
                    });
                }
            }
            ensure!((stop == "tool_calls") == !calls.is_empty(), "finish reason does not match tool calls");
            ensure!(!text.is_empty() || !calls.is_empty(), "empty completion; no tools executed");
            (vec![message.clone()], stop.into())
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
