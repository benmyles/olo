//! MCP stdio client, JSON-RPC 2.0. Servers are explicitly configured by the
//! person, their tools use the same approval path as local process execution.
use crate::model::{McpServer, ToolDef};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;
struct Stdio {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
    next: u64,
}
impl Stdio {
    async fn start(server: &McpServer, cwd: &std::path::Path) -> Result<Self> {
        let environment =
            server.env.iter().map(|(key, source)| Ok((key.clone(), std::env::var(source).with_context(|| format!("missing MCP environment variable {source}"))?))).collect::<Result<Vec<_>>>()?;
        let mut child = tokio::process::Command::new(&server.program)
            .args(&server.args)
            .envs(environment)
            .current_dir(cwd)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("OLO_SOCKET")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().context("MCP stdin")?;
        let output = BufReader::new(child.stdout.take().context("MCP stdout")?);
        let mut c = Self { child, input, output, next: 0 };
        let hello = c.call("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"olo-coder","version":env!("CARGO_PKG_VERSION")}})).await?;
        ensure!(matches!(hello["protocolVersion"].as_str(), Some("2025-11-25" | "2025-06-18" | "2025-03-26" | "2024-11-05")), "MCP server selected an unsupported protocol version");
        c.write(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await?;
        Ok(c)
    }
    async fn write(&mut self, v: Value) -> Result<()> {
        self.input.write_all(format!("{v}\n").as_bytes()).await?;
        self.input.flush().await?;
        Ok(())
    }
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await?;
        loop {
            let mut line = Vec::new();
            loop {
                let bytes = self.output.fill_buf().await?;
                ensure!(!bytes.is_empty(), "MCP server exited");
                let n = bytes.iter().position(|b| *b == b'\n').map_or(bytes.len(), |n| n + 1);
                ensure!(line.len() + n <= 8 * 1024 * 1024, "MCP response too large");
                let end = bytes[n - 1] == b'\n';
                line.extend_from_slice(&bytes[..n]);
                self.output.consume(n);
                if end {
                    break;
                }
            }
            let v: Value = serde_json::from_slice(&line)?;
            if v["id"] == id && v.get("method").is_none() {
                return result(v);
            }
            // No sampling or elicitation capability advertised. Explicitly reject server requests.
            if v.get("method").is_some() && v.get("id").is_some() {
                self.write(json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32601,"message":"client capability not enabled"}})).await?;
            }
        }
    }
}

impl Drop for Stdio {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
struct Http {
    client: reqwest::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
    session: Option<String>,
    version: String,
    next: u64,
    cleanup: Vec<tokio::task::JoinHandle<()>>,
}
impl Http {
    fn request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        let mut r = self.client.request(method, &self.url).headers(self.headers.clone()).header("Accept", "application/json, text/event-stream");
        if let Some(id) = &self.session {
            r = r.header("MCP-Session-Id", id);
        }
        if !self.version.is_empty() {
            r = r.header("MCP-Protocol-Version", &self.version);
        }
        r
    }
    async fn write(&self, value: Value) -> Result<()> {
        let r = self.request(reqwest::Method::POST).json(&value).send().await?;
        ensure!(r.status().is_success(), "MCP notification HTTP {}", r.status());
        Ok(())
    }
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        let mut r = self.request(reqwest::Method::POST).json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).send().await?;
        ensure!(r.status().is_success(), "MCP HTTP {}; disconnected operations are never automatically replayed", r.status());
        if method == "initialize" {
            self.session = r.headers().get("MCP-Session-Id").map(|v| v.to_str().map(str::to_owned)).transpose()?;
        }
        let streaming = r.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|s| s.starts_with("text/event-stream"));
        let mut bytes = Vec::new();
        let mut count = 0;
        let mut sse = crate::provider::Sse::default();
        while let Some(chunk) = r.chunk().await? {
            count += chunk.len();
            ensure!(count <= 8 * 1024 * 1024, "MCP response exceeds 8 MB");
            if streaming {
                for frame in sse.push(&chunk)? {
                    // Streamable HTTP may prime a stream with an event ID and
                    // empty data before sending its JSON-RPC response.
                    if frame.is_empty() {
                        continue;
                    }
                    let v: Value = serde_json::from_str(&frame)?;
                    if v.get("method").is_none() && v["id"] == id {
                        return result(v);
                    }
                    if v.get("method").is_some() && v.get("id").is_some() {
                        self.write(json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32601,"message":"client capability not enabled"}})).await?;
                    }
                }
            } else {
                bytes.extend_from_slice(&chunk);
            }
        }
        ensure!(!streaming, "MCP stream ended before its response; outcome unknown");
        let v: Value = serde_json::from_slice(&bytes)?;
        ensure!(v["id"] == id && v.get("method").is_none(), "MCP response id mismatch");
        result(v)
    }
    fn cancel(&mut self, id: u64) {
        self.cleanup.retain(|task| !task.is_finished());
        let request = self.request(reqwest::Method::POST).json(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id,"reason":"client stopped waiting"}}));
        // Remote cleanup is best effort and bounded; it must never delay the
        // local turn's cancellation or its durable outcome checkpoint.
        self.cleanup.push(tokio::spawn(async move {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), request.send()).await;
        }));
    }
}
#[derive(Debug)]
struct RemoteError(Value);
impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MCP: {}", self.0)
    }
}
impl std::error::Error for RemoteError {}
fn result(v: Value) -> Result<Value> {
    ensure!(v["jsonrpc"] == "2.0", "MCP response requires JSON-RPC 2.0");
    ensure!(v.get("result").is_some() != v.get("error").is_some(), "MCP response requires exactly one result or error");
    if let Some(e) = v.get("error") {
        ensure!(e["code"].as_i64().is_some() && e["message"].as_str().is_some(), "invalid MCP error response");
        return Err(RemoteError(e.clone()).into());
    }
    v.get("result").cloned().context("MCP response has no result")
}
impl Drop for Http {
    fn drop(&mut self) {
        if self.session.is_some() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let request = self.request(reqwest::Method::DELETE);
                let cleanup = std::mem::take(&mut self.cleanup);
                runtime.spawn(async move {
                    // Cancellation notifications belong to this session and
                    // must finish (or time out) before it is deleted.
                    for task in cleanup {
                        let _ = task.await;
                    }
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), request.send()).await;
                });
            }
        }
    }
}
enum Transport {
    Stdio(Stdio),
    Http(Http),
}
pub struct Connection {
    transport: Transport,
}
impl Connection {
    pub async fn start(server: &McpServer, cwd: &std::path::Path) -> Result<Self> {
        let token = match &server.auth {
            Some(crate::oauth::Auth::BearerEnv { variable }) => Some(std::env::var(variable)?),
            None => None,
            _ => bail!("use the service connection manager for OAuth or Passwords authentication"),
        };
        Self::start_authenticated(server, cwd, token.as_deref()).await
    }
    pub async fn start_authenticated(server: &McpServer, cwd: &std::path::Path, bearer: Option<&str>) -> Result<Self> {
        if let Some(url) = &server.url {
            ensure!(server.program.is_empty(), "MCP cannot have both program and url");
            let parsed = crate::oauth::secure_url(url)?;
            ensure!(matches!(parsed.scheme(), "http" | "https") && parsed.username().is_empty() && parsed.password().is_none(), "MCP requires an HTTP(S) URL without embedded credentials");
            let mut headers = reqwest::header::HeaderMap::new();
            for (header, env) in &server.header_env {
                ensure!(!matches!(header.to_ascii_lowercase().as_str(), "host" | "content-length" | "mcp-session-id" | "mcp-protocol-version"), "reserved MCP header");
                let value = std::env::var(env).with_context(|| format!("missing MCP header environment variable {env}"))?;
                let mut value = reqwest::header::HeaderValue::from_str(&value)?;
                value.set_sensitive(true);
                headers.insert(reqwest::header::HeaderName::from_bytes(header.as_bytes())?, value);
            }
            if let Some(token) = bearer {
                ensure!(!headers.contains_key("authorization"), "duplicate MCP authorization");
                let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))?;
                value.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
            let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).connect_timeout(std::time::Duration::from_secs(10)).user_agent("olo-coder").build()?;
            let mut h = Http { client, url: url.clone(), headers, session: None, version: String::new(), next: 0, cleanup: Vec::new() };
            let init = h.call("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"olo-coder","version":env!("CARGO_PKG_VERSION")}})).await?;
            h.version = init["protocolVersion"].as_str().context("MCP omitted protocol version")?.into();
            ensure!(matches!(h.version.as_str(), "2025-11-25" | "2025-06-18" | "2025-03-26"), "unsupported MCP protocol version");
            h.write(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await?;
            Ok(Self { transport: Transport::Http(h) })
        } else {
            Ok(Self { transport: Transport::Stdio(Stdio::start(server, cwd).await?) })
        }
    }
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        match &mut self.transport {
            Transport::Stdio(s) => s.call(method, params).await,
            Transport::Http(h) => h.call(method, params).await,
        }
    }
    pub async fn tools(&mut self, prefix: &str) -> Result<Vec<ToolDef>> {
        let mut cursor = Value::Null;
        let mut out = vec![];
        for _ in 0..100 {
            let v = self.call("tools/list", if cursor.is_null() { json!({}) } else { json!({"cursor":cursor}) }).await?;
            for t in v["tools"].as_array().context("missing MCP tools")? {
                out.push(ToolDef {
                    name: format!("mcp__{prefix}__{}", t["name"].as_str().context("missing tool name")?),
                    description: t["description"].as_str().unwrap_or("").into(),
                    parameters: t["inputSchema"].clone(),
                });
            }
            cursor = v["nextCursor"].clone();
            if cursor.is_null() {
                return Ok(out);
            }
        }
        bail!("too many MCP tool pages")
    }

    pub async fn read(&mut self, method: &str, params: Value, cancel: &CancellationToken, timeout: u64) -> Result<Value> {
        ensure!(
            matches!(method, "resources/list" | "resources/templates/list" | "resources/read" | "prompts/list" | "prompts/get" | "ping"),
            "only MCP resources/prompts read methods are allowed here"
        );
        tokio::select! {_ = cancel.cancelled()=>bail!("cancelled"),v=tokio::time::timeout(std::time::Duration::from_secs(timeout),self.call(method,if params.is_null(){json!({})}else{params}))=>v.context("MCP read timed out")?}
    }
    pub fn bearer(&mut self, token: &str) -> Result<()> {
        ensure!(!token.is_empty(), "empty MCP access token");
        let Transport::Http(http) = &mut self.transport else {
            bail!("bearer authentication requires HTTP");
        };
        let mut header = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))?;
        header.set_sensitive(true);
        http.headers.insert(reqwest::header::AUTHORIZATION, header);
        Ok(())
    }
    pub async fn invoke(&mut self, name: &str, args: Value, cancel: &CancellationToken, timeout: u64) -> Result<Value> {
        ensure!(!cancel.is_cancelled(), "MCP cancelled before request dispatch");
        let request_id = match &self.transport {
            Transport::Stdio(s) => s.next + 1,
            Transport::Http(h) => h.next + 1,
        };
        let result = tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(anyhow::anyhow!("MCP cancelled; inspect external state before retrying")),
            r=tokio::time::timeout(std::time::Duration::from_secs(timeout),self.call("tools/call",json!({"name":name,"arguments":args})))=>r.context("MCP timed out; inspect external state before retrying").and_then(|v|v),
        };
        let issued = match &self.transport {
            Transport::Stdio(s) => s.next == request_id,
            Transport::Http(h) => h.next == request_id,
        };
        if issued && result.as_ref().is_err_and(|error| !error.is::<RemoteError>()) {
            match &mut self.transport {
                Transport::Stdio(s) => {
                    if let Some(pid) = s.child.id() {
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    }
                }
                Transport::Http(h) => h.cancel(request_id),
            }
        }
        result
    }
}
