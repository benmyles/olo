//! Comprehensive 3-harness (`claude`, `codex`, `agy`) local-mock-API end-to-end & fuzz
//! test suite for `olo agent`.
//!
//! Zero real network calls: every CLI runs in an isolated scratch `HOME` under `/var/tmp`
//! pointed at an in-process programmable `MockApiServer` speaking Anthropic Messages SSE
//! (`claude`), OpenAI Responses SSE (`codex`), and Google Gemini Developer SSE (`agy`).
//!
//! Every command travels through the EXACT production path:
//!   `bridge_olo.sh` (`olo`) -> `/usr/bin/rex do bridge_olo.lua` (Lua 5.1)
//!   -> scratch `/usr/bin/rex` server + real `client.keymap` JSON schemas
//!   -> `ControlEvent::ClientPerformRequested` -> `RexHubInner::handle_client_perform`
//!   -> real `olo-rex-link agent trust|inspect` -> real `claude` / `codex` / `agy` PTY!

use super::*;
use std::{
    collections::VecDeque,
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

// ---------------------------------------------------------------------------
// Deterministic PRNG for reproducible fuzzing (no extra crate needed)
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn range(&mut self, n: usize) -> usize {
        (self.next_u64() as usize) % n.max(1)
    }

    fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        &slice[self.range(slice.len())]
    }
}

// ---------------------------------------------------------------------------
// Programmable Local Mock LLM API Server (Anthropic + OpenAI Responses + Gemini)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum MockReply {
    /// Stream plain text and end the turn.
    Text(String),
    /// Emit a shell/command tool call that triggers the CLI's real approval dialog.
    RunCommand { command: String, reason: String },
    /// Return an HTTP error status (e.g. 429 or 529 overloaded) with JSON error body.
    HttpError { status: u16, body: String },
    /// Start streaming SSE then drop the TCP connection mid-response.
    MidStreamDrop { partial_text: String },
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct RecordedReq {
    path: String,
    body: String,
}

#[derive(Default)]
struct MockState {
    /// Queued scripted replies per wire family (`"claude"`, `"codex"`, `"agy"`).
    queues: HashMap<&'static str, VecDeque<MockReply>>,
    /// Sticky reply returned for EVERY request of a family until cleared (used for deterministic retry-exhausting API errors).
    sticky: HashMap<&'static str, MockReply>,
    /// All captured HTTP POST requests for assertions (e.g. verifying tool execution vs denial).
    requests: Vec<RecordedReq>,
    req_count: usize,
}

struct MockApiServer {
    port: u16,
    state: Arc<std::sync::Mutex<MockState>>,
    abort: tokio::task::AbortHandle,
}

impl Drop for MockApiServer {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl MockApiServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock API");
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(std::sync::Mutex::new(MockState::default()));
        let st = state.clone();
        let abort = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let st = st.clone();
                tokio::spawn(async move {
                    let _ = handle_mock_http(stream, st).await;
                });
            }
        })
        .abort_handle();
        Self { port, state, abort }
    }

    fn push(&self, cli: &'static str, reply: MockReply) {
        self.state.lock().unwrap().queues.entry(cli).or_default().push_back(reply);
    }

    fn set_sticky(&self, cli: &'static str, reply: Option<MockReply>) {
        let mut g = self.state.lock().unwrap();
        match reply {
            Some(r) => {
                g.sticky.insert(cli, r);
            }
            None => {
                g.sticky.remove(cli);
            }
        }
    }

    fn clear_queues(&self) {
        let mut g = self.state.lock().unwrap();
        g.queues.clear();
        g.sticky.clear();
        g.requests.clear();
    }

    fn requests(&self) -> Vec<RecordedReq> {
        self.state.lock().unwrap().requests.clone()
    }
}

async fn read_http_request(stream: &mut TcpStream) -> Option<(String, String, String)> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 256 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let req_line = lines.next()?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut content_len: Option<usize> = None;
    let mut chunked = false;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let kl = k.trim().to_ascii_lowercase();
            let vl = v.trim().to_ascii_lowercase();
            if kl == "content-length" {
                content_len = vl.parse().ok();
            } else if kl == "transfer-encoding" && vl.contains("chunked") {
                chunked = true;
            }
        }
    }
    let mut body_raw = buf[header_end + 4..].to_vec();
    if let Some(cl) = content_len {
        while body_raw.len() < cl {
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                break;
            }
            body_raw.extend_from_slice(&tmp[..n]);
        }
        body_raw.truncate(cl);
    } else if chunked {
        loop {
            if body_raw.windows(5).any(|w| w == b"0\r\n\r\n") {
                break;
            }
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                break;
            }
            body_raw.extend_from_slice(&tmp[..n]);
            if body_raw.len() > 4 * 1024 * 1024 {
                break;
            }
        }
        // Decode chunked transfer encoding cleanly.
        let mut decoded = Vec::new();
        let mut cur = &body_raw[..];
        while let Some(pos) = cur.windows(2).position(|w| w == b"\r\n") {
            let size_hex = String::from_utf8_lossy(&cur[..pos]);
            let size = usize::from_str_radix(size_hex.trim().split(';').next().unwrap_or("0"), 16).unwrap_or(0);
            cur = &cur[pos + 2..];
            if size == 0 || cur.len() < size {
                break;
            }
            decoded.extend_from_slice(&cur[..size]);
            cur = &cur[size..];
            if cur.starts_with(b"\r\n") {
                cur = &cur[2..];
            }
        }
        body_raw = decoded;
    }
    Some((method, path, String::from_utf8_lossy(&body_raw).into_owned()))
}

async fn write_json(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        529 => "Overloaded",
        _ => "Error",
    };
    let resp = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

async fn write_sse(stream: &mut TcpStream, body: &str) {
    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

async fn handle_mock_http(mut stream: TcpStream, state: Arc<std::sync::Mutex<MockState>>) -> std::io::Result<()> {
    let Some((method, path, body)) = read_http_request(&mut stream).await else {
        return Ok(());
    };
    if method == "GET" {
        write_json(&mut stream, 200, "{}").await;
        return Ok(());
    }

    // Ignore Claude's auxiliary count_tokens / telemetry endpoints and Agy's lightweight title-gen calls.
    if path.contains("count_tokens") {
        write_json(&mut stream, 200, r#"{"input_tokens":42}"#).await;
        return Ok(());
    }
    if path.contains("flash-lite") && path.contains("streamGenerateContent") {
        let chunk = json!({
            "candidates": [{"content": {"role": "model", "parts": [{"text": "Mock Session"}]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 2, "totalTokenCount": 7}
        });
        write_sse(&mut stream, &format!("data: {chunk}\r\n\r\n")).await;
        return Ok(());
    }

    let family: Option<&'static str> = if path.contains("/messages") {
        Some("claude")
    } else if path.contains("/responses") {
        Some("codex")
    } else if path.contains("streamGenerateContent") {
        Some("agy")
    } else {
        None
    };

    let Some(family) = family else {
        write_json(&mut stream, 200, "{}").await;
        return Ok(());
    };

    // Claude Code sometimes sends a small haiku title/topic request without Bash tools; answer quietly without consuming the main queue!
    if family == "claude" && !body.contains("\"Bash\"") && body.len() < 4000 {
        let mut sse = String::new();
        for (ev, data) in [
            (
                "message_start",
                json!({"type":"message_start","message":{"id":"msg_aux","type":"message","role":"assistant","content":[],"model":"claude-haiku-4-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":5,"output_tokens":1}}}),
            ),
            ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"aux"}})),
            ("content_block_stop", json!({"type":"content_block_stop","index":0})),
            ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1}})),
            ("message_stop", json!({"type":"message_stop"})),
        ] {
            sse.push_str(&format!("event: {ev}\ndata: {data}\n\n"));
        }
        write_sse(&mut stream, &sse).await;
        return Ok(());
    }
    // Codex also sends auxiliary `/v1/responses` calls without `exec_command` (e.g. session title / summary);
    // answer quietly so they never steal queued test tool calls or replies!
    if family == "codex" && !body.contains("exec_command") {
        let item = json!({"id":"msg_aux","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Mock Codex"}]});
        let mut sse = String::new();
        for (ev, data) in [
            ("response.created", json!({"type":"response.created","response":{"id":"resp_aux","status":"in_progress"}})),
            (
                "response.output_item.added",
                json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_aux","type":"message","role":"assistant","status":"in_progress","content":[]}}),
            ),
            ("response.content_part.added", json!({"type":"response.content_part.added","item_id":"msg_aux","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}})),
            ("response.output_text.delta", json!({"type":"response.output_text.delta","item_id":"msg_aux","output_index":0,"content_index":0,"delta":"Mock Codex"})),
            ("response.output_text.done", json!({"type":"response.output_text.done","item_id":"msg_aux","output_index":0,"content_index":0,"text":"Mock Codex"})),
            ("response.output_item.done", json!({"type":"response.output_item.done","output_index":0,"item":item})),
            (
                "response.completed",
                json!({"type":"response.completed","response":{"id":"resp_aux","status":"completed","output":[item],"usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}}}),
            ),
        ] {
            sse.push_str(&format!("event: {ev}\ndata: {data}\n\n"));
        }
        write_sse(&mut stream, &sse).await;
        return Ok(());
    }

    let (req_id, reply) = {
        let mut g = state.lock().unwrap();
        g.req_count += 1;
        let id = g.req_count;
        g.requests.push(RecordedReq { path: path.clone(), body: body.clone() });
        let rep = g
            .sticky
            .get(family)
            .cloned()
            .or_else(|| g.queues.get_mut(family).and_then(VecDeque::pop_front))
            .unwrap_or_else(|| MockReply::Text(format!("DEFAULT_{}_{id}", family.to_ascii_uppercase())));
        (id, rep)
    };

    match reply {
        MockReply::HttpError { status, body } => {
            write_json(&mut stream, status, &body).await;
        }
        MockReply::MidStreamDrop { partial_text } => {
            // Advertise chunked SSE, send one partial chunk, then hard-close the socket to simulate a mid-response disconnect!
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = stream.write_all(head.as_bytes()).await;
            let partial = match family {
                "claude" => format!(
                    "event: message_start\ndata: {}\n\nevent: content_block_start\ndata: {}\n\nevent: content_block_delta\ndata: {}\n\n",
                    json!({"type":"message_start","message":{"id":format!("msg_{req_id}"),"type":"message","role":"assistant","content":[],"model":"claude-opus-5-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":1}}}),
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":partial_text}}),
                ),
                "codex" => format!(
                    "event: response.created\ndata: {}\n\nevent: response.output_text.delta\ndata: {}\n\n",
                    json!({"type":"response.created","response":{"id":format!("resp_{req_id}"),"status":"in_progress"}}),
                    json!({"type":"response.output_text.delta","item_id":"m1","output_index":0,"content_index":0,"delta":partial_text}),
                ),
                _ => format!("data: {}\r\n\r\n", json!({"candidates":[{"content":{"role":"model","parts":[{"text":partial_text}]}}]})),
            };
            let chunk_frame = format!("{:x}\r\n{partial}\r\n", partial.len());
            let _ = stream.write_all(chunk_frame.as_bytes()).await;
            let _ = stream.flush().await;
            drop(stream);
        }
        MockReply::Text(text) => match family {
            "claude" => {
                let mut sse = String::new();
                for (ev, data) in [
                    (
                        "message_start",
                        json!({"type":"message_start","message":{"id":format!("msg_{req_id}"),"type":"message","role":"assistant","content":[],"model":"claude-opus-5-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":12,"output_tokens":1}}}),
                    ),
                    ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
                    ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}})),
                    ("content_block_stop", json!({"type":"content_block_stop","index":0})),
                    ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":10}})),
                    ("message_stop", json!({"type":"message_stop"})),
                ] {
                    sse.push_str(&format!("event: {ev}\ndata: {data}\n\n"));
                }
                write_sse(&mut stream, &sse).await;
            }
            "codex" => {
                let mid = format!("msg_{req_id}");
                let rid = format!("resp_{req_id}");
                let item = json!({"id":mid,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":text}]});
                let mut sse = String::new();
                for (ev, data) in [
                    ("response.created", json!({"type":"response.created","response":{"id":rid,"status":"in_progress"}})),
                    (
                        "response.output_item.added",
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":mid,"type":"message","role":"assistant","status":"in_progress","content":[]}}),
                    ),
                    ("response.content_part.added", json!({"type":"response.content_part.added","item_id":mid,"output_index":0,"content_index":0,"part":{"type":"output_text","text":""}})),
                    ("response.output_text.delta", json!({"type":"response.output_text.delta","item_id":mid,"output_index":0,"content_index":0,"delta":text})),
                    ("response.output_text.done", json!({"type":"response.output_text.done","item_id":mid,"output_index":0,"content_index":0,"text":text})),
                    ("response.output_item.done", json!({"type":"response.output_item.done","output_index":0,"item":item})),
                    (
                        "response.completed",
                        json!({"type":"response.completed","response":{"id":rid,"status":"completed","output":[item],"usage":{"input_tokens":12,"output_tokens":10,"total_tokens":22}}}),
                    ),
                ] {
                    sse.push_str(&format!("event: {ev}\ndata: {data}\n\n"));
                }
                write_sse(&mut stream, &sse).await;
            }
            _ => {
                let chunk = json!({
                    "candidates": [{"content": {"role": "model", "parts": [{"text": text}]}, "finishReason": "STOP"}],
                    "usageMetadata": {"promptTokenCount": 12, "candidatesTokenCount": 10, "totalTokenCount": 22}
                });
                write_sse(&mut stream, &format!("data: {chunk}\r\n\r\n")).await;
            }
        },
        MockReply::RunCommand { command, reason } => match family {
            "claude" => {
                let input_json = json!({"command": command, "description": reason}).to_string();
                let mut sse = String::new();
                for (ev, data) in [
                    (
                        "message_start",
                        json!({"type":"message_start","message":{"id":format!("msg_{req_id}"),"type":"message","role":"assistant","content":[],"model":"claude-opus-5-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":12,"output_tokens":1}}}),
                    ),
                    ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":format!("toolu_{req_id}"),"name":"Bash","input":{}}})),
                    ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":input_json}})),
                    ("content_block_stop", json!({"type":"content_block_stop","index":0})),
                    ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":12}})),
                    ("message_stop", json!({"type":"message_stop"})),
                ] {
                    sse.push_str(&format!("event: {ev}\ndata: {data}\n\n"));
                }
                write_sse(&mut stream, &sse).await;
            }
            "codex" => {
                let rid = format!("resp_{req_id}");
                let args_json = json!({
                    "cmd": command,
                    "sandbox_permissions": "require_escalated",
                    "justification": reason,
                })
                .to_string();
                let call = json!({
                    "id": format!("fc_{req_id}"),
                    "type": "function_call",
                    "status": "completed",
                    "call_id": format!("call_{req_id}"),
                    "name": "exec_command",
                    "arguments": args_json,
                });
                let mut sse = String::new();
                for (ev, data) in [
                    ("response.created", json!({"type":"response.created","response":{"id":rid,"status":"in_progress"}})),
                    ("response.output_item.added", json!({"type":"response.output_item.added","output_index":0,"item":call})),
                    ("response.output_item.done", json!({"type":"response.output_item.done","output_index":0,"item":call})),
                    (
                        "response.completed",
                        json!({"type":"response.completed","response":{"id":rid,"status":"completed","output":[call],"usage":{"input_tokens":12,"output_tokens":10,"total_tokens":22}}}),
                    ),
                ] {
                    sse.push_str(&format!("event: {ev}\ndata: {data}\n\n"));
                }
                write_sse(&mut stream, &sse).await;
            }
            _ => {
                let chunk = json!({
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{
                                "functionCall": {
                                    "name": "run_command",
                                    "args": {
                                        "CommandLine": command,
                                        "WaitMsBeforeAsync": 5000,
                                        "toolSummary": reason,
                                    }
                                }
                            }]
                        },
                        "finishReason": "STOP"
                    }],
                    "usageMetadata": {"promptTokenCount": 12, "candidatesTokenCount": 10, "totalTokenCount": 22}
                });
                write_sse(&mut stream, &format!("data: {chunk}\r\n\r\n")).await;
            }
        },
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Live Isolated Rex + `olo` + `olo.lua` + `RexHubInner` Rig
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct LiveRexHarness {
    dir: PathBuf,
    home: PathBuf,
    work: PathBuf,
    olo_bin: PathBuf,
    rex_wrap: PathBuf,
    sock: PathBuf,
    sid: String,
    parent_bid: String,
    mock: MockApiServer,
    inner: Arc<RexHubInner>,
    rex_proc: Child,
    ev_abort: tokio::task::AbortHandle,
    claude_key: String,
}

impl Drop for LiveRexHarness {
    fn drop(&mut self) {
        self.ev_abort.abort();
        let _ = self.rex_proc.kill();
        let _ = self.rex_proc.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl LiveRexHarness {
    async fn start(tag: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(1);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-fuzz-{}-{}-{n}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        let work = dir.join("work");
        let bin = dir.join("bin");
        std::fs::create_dir_all(home.join(".gemini/antigravity-cli/cache")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&bin).unwrap();

        let mock = MockApiServer::start().await;
        let claude_key = "sk-ant-api03-olofuzztest0000000001".to_owned();

        // Seed isolated scratch HOME for all 3 harnesses (ZERO real credentials/network!).
        // Notice: Claude `projects` starts empty so `--trust` is genuinely tested on `work`!
        std::fs::write(
            home.join(".claude.json"),
            serde_json::to_string_pretty(&json!({
                "hasCompletedOnboarding": true,
                "theme": "dark",
                "customApiKeyResponses": { "approved": [&claude_key[claude_key.len() - 20..]], "rejected": [] },
                "projects": {}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            format!(
                "model_provider = \"mock\"\n[model_providers.mock]\nname = \"Mock\"\nbase_url = \"http://127.0.0.1:{}/v1\"\nwire_api = \"responses\"\nenv_key = \"OPENAI_API_KEY\"\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                mock.port,
                work.display()
            ),
        )
        .unwrap();
        std::fs::write(
            home.join(".gemini/antigravity-cli/settings.json"),
            serde_json::to_string_pretty(&json!({
                "modelProvider": "gemini",
                "trustedWorkspaces": [work.display().to_string()],
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            home.join(".gemini/antigravity-cli/cache/onboarding.json"),
            serde_json::to_string_pretty(&json!({
                "consumerOnboardingComplete": true,
                "enterpriseOnboardingComplete": true,
                "onboardingComplete": true,
            }))
            .unwrap(),
        )
        .unwrap();

        // Install REAL `olo` + `olo.lua` scripts into `<dir>/bin`, plus a wrapper forcing
        // `/usr/bin/rex` to talk ONLY to our isolated scratch Unix socket (`-S unix://...`).
        let sock = dir.join("rex.sock");
        let olo_bin = bin.join("olo");
        std::fs::write(&olo_bin, crate::bridge::SKILL_OLO).unwrap();
        std::fs::set_permissions(&olo_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(bin.join("olo.lua"), crate::bridge::SKILL_LUA).unwrap();
        let rex_wrap = bin.join("rex-isolated");
        std::fs::write(&rex_wrap, format!("#!/bin/sh\nexec /usr/bin/rex -S \"unix://{}\" \"$@\"\n", sock.display())).unwrap();
        std::fs::set_permissions(&rex_wrap, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Install minimal stub `olo-op` and `olo-screen` binaries under `<dir>/fake-bins` and point
        // `OLO_REX_BRIDGE_BIN` there so `olo op` / `olo screen` bridge tests never hit the host's
        // live desktop shims!
        let fake_bins = dir.join("fake-bins");
        std::fs::create_dir_all(&fake_bins).unwrap();
        for stub in ["olo-op", "olo-screen"] {
            let p = fake_bins.join(stub);
            std::fs::write(&p, format!("#!/bin/sh\necho \"stub {stub}: $*\"\nexit 0\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // SAFETY: test-only env var used by `bridge::program` to select stand-in binaries.
        unsafe {
            std::env::set_var("OLO_REX_BRIDGE_BIN", &fake_bins);
        }

        // Spawn REAL isolated `/usr/bin/rex server run`.
        let rex_proc = Command::new("/usr/bin/rex")
            .args(["-S", &format!("unix://{}", sock.display()), "server", "run", "--listen", &format!("unix://{}", sock.display()), "--restore=false", "--state-dir"])
            .arg(dir.join("rex-state"))
            .current_dir(&work)
            .env("HOME", &home)
            .env("REX_SERVER_SOCKET", &sock)
            .env_remove("SSH_AUTH_SOCK")
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK")
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .env("XDG_RUNTIME_DIR", &dir)
            .env("XDG_STATE_HOME", &dir)
            .env("XDG_CONFIG_HOME", &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn scratch rex server");

        let deadline = Instant::now() + Duration::from_secs(6);
        while !sock.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(sock.exists(), "scratch rex socket did not appear");

        let ep = RexEndpoint::Unix(sock.clone());
        let (stream, host_hdr) = ep.connect_stream().await.unwrap();
        let (ctrl, mut ev_rx) = ControlClient::connect(stream, &host_hdr, olo_rex_client::ClientIdentity::olo_rex(Some(format!("fuzz-{tag}"))), None).await.unwrap();
        // Register REAL `bridge_catalog()` JSON schemas on the Rex server so every `olo.lua` RPC
        // goes through Rex's ACTUAL schema validation!
        let km_actions = crate::keymap::register_actions_payload();
        ctrl.client_keymap(&km_actions, &[], &[]).await.expect("register bridge keymap actions");

        let created = ctrl.session_create(Some("lead"), None).await.unwrap();
        let sid = created.payload.get("session_id").and_then(Value::as_str).unwrap().to_owned();
        let att = ctrl.session_attach(&sid).await.unwrap();
        let parent_bid = att.payload.view.windows[0].all_blocks().next().unwrap().block_id.clone();

        let mut hosts = HashMap::new();
        hosts.insert(
            "local".to_owned(),
            HostRuntime {
                key: "local".into(),
                label: "This computer".into(),
                endpoint_raw: "local".into(),
                endpoint: Some(ep.clone()),
                is_local: true,
                conn_state: HostConnState::Connected,
                connection_lost: None,
                client_id: ctrl.client_id().map(str::to_owned),
                reconnect_token: None,
                retry: None,
                server_status: None,
                server_version: None,
                server_newer: None,
                helper: HostHelper::Ready,
                helper_current: Some(true),
                helper_sync_err: None,
                bridge_there: None,
                skill_there: None,
                skill_current: None,
                skill_sync_err: None,
                commands_there: None,
                commands_current: None,
                commands_sync_err: None,
                agent_keys: None,
                account: None,
                carried: None,
                agent_sessions: None,
                repos_there: None,
                tailscale: None,
                tailscale_seq: 0,
                synced_hosts: Vec::new(),
                session_summaries: vec![SessionSummary { session_id: sid.clone(), label: "lead".into() }],
                sessions: HashMap::new(),
                pending_focus: HashMap::new(),
                keymap_bindings: Vec::new(),
                keymap_modes: Vec::new(),
                keymap_diagnostics: Vec::new(),
                control: Some(ctrl.clone()),
                input_tx: None,
            },
        );
        let state = Arc::new(Mutex::new(HubState {
            hosts,
            host_order: vec!["local".into()],
            blocks: HashMap::new(),
            forwards: Vec::new(),
            kept_forwards: Vec::new(),
            kept_forward_errors: HashMap::new(),
            kept_forwards_starting: HashSet::new(),
            settings: crate::settings::Settings::default(),
            active_theme: TerminalTheme::default(),
            pools: None,
            repos: Vec::new(),
            carry_choices: Vec::new(),
            kept: Vec::new(),
            sessions_window: olo_rex_client::sessions::Window::default(),
            git_cache: GitCache::new(),
            local_bin_dir: Some(bin),
            subagents: crate::subagents::SubagentRegistry::default(),
            quick_look_cache: HashMap::new(),
        }));
        let inner = Arc::new(RexHubInner {
            state,
            ssh_pool: Arc::new(SshSessionPool::default()),
            askpass: None,
            notify: Arc::new(|_| {}),
            spares: Mutex::new(HashMap::new()),
            agent_socks: Mutex::new(HashMap::new()),
            bridge_calls: bridge::Calls::default(),
            run_jobs: crate::bridge_run::RunJobs::new(),
            run_approvals: crate::bridge_run::RunApprovals::new(|| {}, || {}),
            keeping: Mutex::new(HashMap::new()),
            subagent_events: Arc::new(tokio::sync::Notify::new()),
            subagent_log: Mutex::new(crate::subagents::SubagentEventLog::default()),
            subagent_text_watchers: std::sync::atomic::AtomicUsize::new(0),
        });
        inner.ingest_session("local", &ctrl, &ep, att.payload.view, &[]).await;

        // Drive REAL `ControlEvent` dispatch loop just like production `RexHub`!
        let (inner_ev, ctrl_ev, ep_ev) = (inner.clone(), ctrl.clone(), ep.clone());
        let ev_abort = tokio::spawn(async move {
            while let Some(env) = ev_rx.recv().await {
                inner_ev.handle_control_event("local", &ctrl_ev, &ep_ev, env.seq, env.event).await;
            }
        })
        .abort_handle();

        Self { dir, home, work, olo_bin, rex_wrap, sock, sid, parent_bid, mock, inner, rex_proc, ev_abort, claude_key }
    }

    /// Runs the REAL `olo` shell script -> `/usr/bin/rex do olo.lua` -> Rex server -> `RexHubInner`!
    async fn run_olo(&self, args: &[&str]) -> (i32, String, String) {
        self.run_olo_from(args, Some(&self.sid), Some(&self.parent_bid)).await
    }

    async fn run_olo_from(&self, args: &[&str], sid: Option<&str>, bid: Option<&str>) -> (i32, String, String) {
        let mut cmd = tokio::process::Command::new(&self.olo_bin);
        cmd.args(args)
            .current_dir(&self.work)
            .env("HOME", &self.home)
            .env("PWD", &self.work)
            .env("TMPDIR", &self.dir)
            .env("XDG_RUNTIME_DIR", &self.dir)
            .env("XDG_STATE_HOME", &self.dir)
            .env("XDG_CONFIG_HOME", &self.dir)
            .env("OLO_BRIDGE_REX", &self.rex_wrap)
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(s) = sid {
            cmd.env("REX_SESSION", s);
        }
        if let Some(b) = bid {
            cmd.env("REX_BLOCK", b);
        }
        let out = tokio::time::timeout(Duration::from_secs(45), cmd.output()).await.expect("olo command timed out").expect("spawn olo");
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    }

    /// Runs `olo` expecting success AND strictly single-line valid JSON on stdout!
    async fn run_olo_json(&self, args: &[&str]) -> Value {
        let (code, stdout, stderr) = self.run_olo(args).await;
        if code != 0 {
            // Print live children screens for instant diagnostics on failure!
            let (_, ls_out, _) = self.run_olo(&["agent", "list", "--all", "--fast", "--json"]).await;
            let entries: Vec<(String, String)> = {
                let st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                st.subagents.entries.iter().map(|e| (e.name.clone().unwrap_or_else(|| e.id.clone()), e.block_id.clone())).collect()
            };
            let mut diag = format!("list={ls_out}\n");
            for (nm, bid) in entries {
                let bk = BlockKey { host_key: "local".into(), session_id: self.sid.clone(), block_id: bid };
                if let Some(br) = self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).blocks.get(&bk).cloned() {
                    let scr = br.lock().unwrap_or_else(|e| e.into_inner()).terminal.format(olo_rex_vt::FormatKind::Plain, false, true).unwrap_or_default();
                    diag.push_str(&format!("--- SCREEN [{nm}] ---\n{scr}\n"));
                }
            }
            panic!("olo {args:?} failed (code={code})\nstdout: {stdout}\nstderr: {stderr}\nDIAG:\n{diag}");
        }
        let non_empty: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(non_empty.len(), 1, "olo {args:?} MUST print strictly 1 JSON object, got {} lines:\n{stdout}", non_empty.len());
        serde_json::from_str::<Value>(non_empty[0]).unwrap_or_else(|e| panic!("invalid JSON from olo {args:?}: {e}\n{stdout}"))
    }

    /// Builds the common mock-API `--env` args for launching each real CLI in our scratch HOME.
    fn cli_env_args(&self, cli: &str) -> Vec<String> {
        let mut out = vec!["--env".into(), format!("HOME={}", self.home.display())];
        match cli {
            "claude" => {
                out.extend(["--env".into(), format!("ANTHROPIC_BASE_URL=http://127.0.0.1:{}", self.mock.port), "--env".into(), format!("ANTHROPIC_API_KEY={}", self.claude_key)]);
            }
            "codex" => {
                out.extend(["--env".into(), "OPENAI_API_KEY=sk-test-codex".into(), "--env".into(), format!("CODEX_HOME={}/.codex", self.home.display())]);
            }
            "agy" => {
                out.extend([
                    "--env".into(),
                    format!("GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:{}", self.mock.port),
                    "--env".into(),
                    "GEMINI_API_KEY=test-gemini-key".into(),
                    "--env".into(),
                    format!("CLOUD_CODE_URL=http://127.0.0.1:{}", self.mock.port),
                    "--env".into(),
                    format!("AGY_LLM_GATEWAY_URL=http://127.0.0.1:{}", self.mock.port),
                ]);
            }
            _ => unreachable!(),
        }
        out
    }
}

fn have_program(name: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {name} >/dev/null 2>&1")]).status().is_ok_and(|s| s.success())
}

// ---------------------------------------------------------------------------
// Test 1: Lua 5.1 & Bridge Protocol Torture (real `olo` + `olo.lua` + Rex schema)
// ---------------------------------------------------------------------------

#[test]
fn lua51_bridge_empty_args_prompt_file_and_single_json_invariants() {
    if !std::path::Path::new("/usr/bin/rex").exists() {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("lua51").await;

        // 1. Every subcommand with ZERO args must NEVER fail with Rex 400 `"args" must be array, not object`!
        for zero_cmd in [
            &["agent", "approve"][..],
            &["agent", "start"][..],
            &["agent", "status"][..],
            &["agent", "read"][..],
            &["agent", "watch"][..],
            &["agent", "send"][..],
            &["agent", "stop"][..],
            &["agent", "focus"][..],
            &["agent", "list"][..],
            &["agent", "prune"][..],
            &["ls"][..],
            &["get"][..],
            &["forward"][..],
            &["op"][..],
            &["screen"][..],
            &["run"][..],
        ] {
            let (code, stdout, stderr) = rig.run_olo(zero_cmd).await;
            eprintln!("ZERO_CMD {zero_cmd:?} -> code={code} stdout={stdout:?} stderr={stderr:?}");
            assert!(
                !stderr.contains("must be array, not object") && !stderr.contains("bad_request") && !stderr.contains("invalid options"),
                "olo {zero_cmd:?} hit Lua/schema bug (code={code}): stdout={stdout:?} stderr={stderr:?}"
            );
            if matches!(zero_cmd, ["agent", "approve" | "start" | "status" | "read" | "watch" | "send" | "stop" | "focus"]) {
                assert_eq!(code, 2, "olo {zero_cmd:?} should exit 2 with usage, got code={code} stderr={stderr:?}");
                assert!(stderr.contains("usage:") || stderr.contains("missing agent CLI"), "olo {zero_cmd:?} missing usage line: {stderr:?}");
            }
        }

        // 2. `--prompt-file` and `send --file` (both space and `=` forms, multiline UTF-8 & empty file)
        // must work cleanly in Rex's Lua 5.1 (never `bad argument #2 to read (invalid options:a)`).
        let pf = rig.dir.join("multiline-prompt.md");
        std::fs::write(&pf, "Line 1 — Unicode “quotes” & $HOME\nLine 2 `backticks` 'single'\n").unwrap();
        let ef = rig.dir.join("empty-prompt.md");
        std::fs::write(&ef, "").unwrap();

        // Missing --model fails with Rust validation (proving Lua 5.1 read `--prompt-file` cleanly!).
        let (code, _, stderr) = rig.run_olo(&["agent", "start", "claude", "--prompt-file", pf.to_str().unwrap()]).await;
        assert_eq!(code, 2, "expected --model error, got code={code} stderr={stderr:?}");
        assert!(stderr.contains("--model"), "Lua 5.1 failed before reaching Rust: {stderr:?}");

        let (code, _, stderr) = rig.run_olo(&["agent", "start", "claude", &format!("--prompt-file={}", ef.display())]).await;
        assert_eq!(code, 2);
        assert!(stderr.contains("--model"), "empty --prompt-file failed in Lua: {stderr:?}");

        // `send --file` to nonexistent target reaches Rust target lookup (proving Lua 5.1 read `--file` cleanly!).
        let (code, _, stderr) = rig.run_olo(&["agent", "send", "no-such-child", "--file", pf.to_str().unwrap(), "--enter"]).await;
        assert_eq!(code, 1);
        assert!(stderr.contains("not found"), "send --file failed in Lua: {stderr:?}");

        // Missing file path fails with clear Lua error (not a Lua stack trace!).
        let (code, _, stderr) = rig.run_olo(&["agent", "start", "claude", "--prompt-file", "/var/tmp/definitely-no-such-file-olo"]).await;
        assert_eq!(code, 1);
        assert!(stderr.contains("couldn't read prompt file") && !stderr.contains("bad argument"), "unexpected stderr: {stderr:?}");

        // 3. `list --json` and `prune --json` return strictly 1 JSON object.
        let listed = rig.run_olo_json(&["agent", "list", "--json"]).await;
        assert_eq!(listed["ok"], json!(true));
        let pruned = rig.run_olo_json(&["agent", "prune", "--json"]).await;
        assert_eq!(pruned["ok"], json!(true));
    });
}

// ---------------------------------------------------------------------------
// Test 2: Adversarial ChoiceKind & Approval Parser Fuzzing (500+ cases)
// ---------------------------------------------------------------------------

#[test]
fn fuzz_approval_choice_classification_never_mislabels_persistent_as_once() {
    let mut rng = Rng::new(0x010_5AFE_A990_0001);

    // Real-world traps reported in production + adversarial variants across all 3 harnesses.
    let persistent_or_session_traps = [
        "Yes, and don\u{2019}t ask again for: env",
        "Yes, and don't ask again for: cargo test",
        "Yes, and don\u{2018}t ask again for commands in /home/user/repo",
        "Yes, and dont ask again for `git status`",
        "Yes, and do not ask again for this command",
        "Yes, and always allow in this conversation for commands that start with 'env'",
        "Yes, and always allow for commands that start with 'env' (Persist to settings.json)",
        "Yes, allow reading from /workspace/project during this session",
        "Yes, allow all edits during this session (shift+tab)",
        "Yes, switch to auto mode",
        "Yes, and remember this choice for future turns",
        "Yes, and trust all future runs of this tool",
        "Yes, and auto-accept similar commands",
        "Yes, always run without asking",
        "Yes, allow for this session",
        "Yes, and something totally new and unknown that grants extra scope",
    ];
    for trap in persistent_or_session_traps {
        let k = olo_agent_screen::classify_choice_kind(trap);
        assert_ne!(k, olo_agent_screen::ChoiceKind::Once, "SECURITY VIOLATION: persistent/unknown choice `{trap}` was classified as `Once`!");
    }

    // Valid single-use choices across Claude Code, Codex, and Antigravity.
    let valid_once = ["Yes", "Yes (y)", "Yes, proceed (y)", "Yes, run command", "Yes, run this command", "Yes, allow once", "Allow once"];
    for ok in valid_once {
        assert_eq!(olo_agent_screen::classify_choice_kind(ok), olo_agent_screen::ChoiceKind::Once, "valid single-use choice `{ok}` should be `Once`");
    }

    // Wrapped multi-line Claude/Codex/Agy approval dialogs across 250 randomized layouts.
    let prefixes = ["❯ ", "› ", "> "];
    let indents = ["  ", "   ", "    ", "\t"];
    let apostrophes = ["'", "\u{2019}", "\u{2018}", "`"];
    let cmds = ["env", "cargo test -p api && echo 'ok'", "rm -rf build && make", "git status --short"];
    for i in 0..250 {
        let is_agy = i % 3 == 2;
        let agent = match i % 3 {
            0 => olo_agent_screen::Agent::Claude,
            1 => olo_agent_screen::Agent::Codex,
            _ => olo_agent_screen::Agent::Antigravity,
        };
        let mark = if is_agy { "> " } else { *rng.pick(&prefixes[..2]) };
        let apos = *rng.pick(&apostrophes);
        let indent = *rng.pick(&indents);
        let cmd = *rng.pick(&cmds);
        let selected_idx = rng.range(3);
        let c1 = if is_agy {
            "Yes, run command"
        } else if i % 3 == 1 {
            "Yes, proceed (y)"
        } else {
            "Yes"
        };
        // Deliberately wrap choice 2 onto a continuation line with 2..4 spaces!
        let c2_a = "Yes, and";
        let c2_b = format!("don{apos}t ask again for: {cmd}");
        let c3 = if is_agy { "No, cancel" } else { "No, and tell agent what to do differently (esc)" };

        let m = |idx: usize| if idx == selected_idx { mark } else { "  " };
        let footer = if is_agy {
            "  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command"
        } else if i % 3 == 1 {
            "  Press enter to confirm or esc to cancel"
        } else {
            " Esc to cancel · Tab to amend"
        };
        let question = if is_agy {
            "Run this command?"
        } else if i % 3 == 1 {
            "Would you like to run the following command?"
        } else {
            "Do you want to proceed?"
        };
        let body = if is_agy {
            format!("Command\n────────────────────\nRequesting permission for:\n   {cmd}\n{question}\n")
        } else if i % 3 == 1 {
            format!("{question}\n\n  $ {cmd}\n\n")
        } else {
            format!("Bash command\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n  {cmd}\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n{question}\n")
        };
        let screen = format!("{body}{}1. {c1}\n{}2. {c2_a}\n{indent}{c2_b}\n{}3. {c3}\n{footer}\n", m(0), m(1), m(2));
        let ap = olo_agent_screen::parse_approval(agent, &screen).unwrap_or_else(|| panic!("case {i} failed to parse:\n{screen}"));
        assert_eq!(ap.selected_index, Some(selected_idx), "case {i} wrong selected_index");
        assert_eq!(ap.choices.len(), 3, "case {i} wrong choice count: {:?}", ap.choices);
        assert_eq!(ap.choices[0].kind, olo_agent_screen::ChoiceKind::Once, "case {i} choice 0");
        assert_ne!(ap.choices[1].kind, olo_agent_screen::ChoiceKind::Once, "case {i} choice 1 MUST NOT be Once: {:?}", ap.choices[1]);
        assert_eq!(ap.choices[1].kind, olo_agent_screen::ChoiceKind::DontAskAgain, "case {i} choice 1 should be DontAskAgain: {:?}", ap.choices[1]);
        assert_eq!(ap.choices[2].kind, olo_agent_screen::ChoiceKind::No, "case {i} choice 2");
        assert!(ap.command.as_deref().is_some_and(|c| c.contains(cmd)), "case {i} command missing: {:?}", ap.command);
    }

    // Folder-trust AND `--add-dir` ("Adding a new directory:") prompts must NEVER be misclassified as tool approvals.
    let add_dir_screen = "────────────────────\n Adding a new directory:\n /var/tmp/extra-dir\n This session hasn't worked here before. Is this a directory you created or one you trust?\n ❯ No, exit\n   Yes, add it\n Enter to confirm · Esc to cancel\n";
    let trust = olo_agent_screen::parse_folder_trust(add_dir_screen).expect("must detect --add-dir trust prompt");
    assert_eq!(trust.folder.as_deref(), Some("/var/tmp/extra-dir"));
    assert_eq!(trust.selected_index, 0);
    assert_eq!(trust.trust_index, 1);
    assert!(!trust.selected_trust);
    assert!(olo_agent_screen::parse_approval(olo_agent_screen::Agent::Claude, add_dir_screen).is_none());
}

// ---------------------------------------------------------------------------
// Test 3: Full Live End-to-End Lifecycle Across ALL 3 REAL Harnesses
// (`claude`, `codex`, `agy`) against Local Mock SSE API Server!
// ---------------------------------------------------------------------------

async fn run_full_harness_lifecycle(rig: &LiveRexHarness, cli: &'static str, model: &str, effort: &str, extra_start_flags: &[&str]) {
    rig.mock.clear_queues();
    let name = format!("live-{cli}");
    let denied_marker = format!("DENIED_CMD_{cli}");
    let denied_exec = format!("DENIED_EXEC_{cli}");
    let approved_marker = format!("APPROVED_ONCE_{cli}_OK");
    let approved_exec = format!("APPROVED_EXEC_{cli}");
    let final_msg_1 = format!("TURN1_COMPLETE_{cli}: multi-line\nwith UTF-8 — “quotes” & marker {approved_marker}");
    let final_msg_2 = format!("TURN2_FOLLOWUP_{cli}_OK");
    let resumed_msg = format!("TURN3_RESUMED_{cli}_OK");

    // Script the mock LLM server:
    //   Req 1: Ask to run `sh -c "printf '%s_%s\n' DENIED_EXEC <cli>"` (compound shell call so even Claude
    //          Code 2.1's read-only auto-allow ALWAYS prompts, while still working inside Codex read-only
    //          sandbox; the literal `DENIED_EXEC_<cli>` string only appears if the shell actually executes!).
    //          We REJECT this with `approve --no --reason`!
    //   Req 2: After receiving denial, ask to run `sh -c "printf '%s_%s\n' APPROVED_EXEC <cli>"` -> we
    //          APPROVE ONCE with `approve --once`!
    //   Req 3: After receiving real stdout `APPROVED_EXEC_<cli>`, finish turn 1 with `final_msg_1`!
    //   Req 4: After `olo agent send --file` follow-up, finish turn 2 with `final_msg_2`!
    //   Req 5: After `stop --save` + `start --resume SID`, finish turn 3 with `resumed_msg`!
    rig.mock.push(cli, MockReply::RunCommand { command: format!("sh -c \"printf '%s_%s\\n' DENIED_EXEC {cli} # {denied_marker}\""), reason: "Test denial path".into() });
    rig.mock.push(cli, MockReply::RunCommand { command: format!("sh -c \"printf '%s_%s\\n' APPROVED_EXEC {cli} # {approved_marker}\""), reason: "Test approve-once path".into() });
    rig.mock.push(cli, MockReply::Text(final_msg_1.clone()));
    rig.mock.push(cli, MockReply::Text(final_msg_2.clone()));
    rig.mock.push(cli, MockReply::Text(resumed_msg.clone()));

    let prompt_file = rig.dir.join(format!("prompt-{cli}.txt"));
    std::fs::write(&prompt_file, format!("Start lifecycle test for {cli}.\nLine 2 with $HOME and 'quotes'.\n")).unwrap();

    // 1. Launch via real `olo agent start ... --prompt-file ... --json`!
    let mut start_args: Vec<String> = vec![
        "agent".into(),
        "start".into(),
        cli.into(),
        "--model".into(),
        model.into(),
        "--effort".into(),
        effort.into(),
        "--name".into(),
        name.clone(),
        "--trust".into(),
        "--cwd".into(),
        rig.work.display().to_string(),
        "--prompt-file".into(),
        prompt_file.display().to_string(),
        "--json".into(),
    ];
    for f in extra_start_flags {
        start_args.push((*f).to_owned());
    }
    start_args.extend(rig.cli_env_args(cli));
    let start_refs: Vec<&str> = start_args.iter().map(String::as_str).collect();
    let started = rig.run_olo_json(&start_refs).await;
    assert_eq!(started["ok"], json!(true), "{cli} start: {started}");
    assert_eq!(started["name"], json!(name));
    assert_eq!(started["desktop"], json!("none"));
    let started_at_ms = started["started_at"].as_i64().expect("started_at present");
    assert!(started_at_ms > 1_700_000_000_000, "{cli}: started_at must be Unix ms, got {started_at_ms}");
    let mut cursor = started["cursor"].as_u64().expect("start cursor");

    // 2. Event-driven `watch --all --since CURSOR --on approval` catches REAL approval #1!
    let cur_s = cursor.to_string();
    let w_ap1 = rig.run_olo_json(&["agent", "watch", "--all", "--since", &cur_s, "--on", "approval,error,exit", "--timeout", "25", "--json"]).await;
    assert_eq!(w_ap1["event"], json!("waiting_approval"), "{cli} expected waiting_approval #1, got: {w_ap1}");
    assert_eq!(w_ap1["name"], json!(name));
    cursor = w_ap1["cursor"].as_u64().unwrap();

    // 3. Inspect rich `status` on approval #1 — check safety invariant on EVERY choice + unified ms timestamps!
    let st1 = rig.run_olo_json(&["agent", "status", &name, "--json"]).await;
    assert_eq!(st1["state"], json!("waiting_approval"), "{cli} status #1: {st1}");
    assert!(st1["pid"].as_i64().is_some_and(|p| p > 0), "{cli} missing pid: {st1}");
    let st_started = st1["started_at"].as_i64().unwrap();
    let st_last_out = st1["last_output_at"].as_u64().unwrap();
    assert!(st_started > 1_700_000_000_000 && st_last_out > 1_700_000_000_000, "{cli} timestamp unit mismatch: started_at={st_started} last_output_at={st_last_out}");
    let choices1 = st1["approval"]["choices"].as_array().expect("approval choices array");
    let once_choices: Vec<&Value> = choices1.iter().filter(|c| c["kind"] == "once").collect();
    assert_eq!(once_choices.len(), 1, "{cli} MUST have strictly ONE `once` choice, got: {choices1:?}");
    for c in choices1 {
        let lbl = c["label"].as_str().unwrap_or("").to_ascii_lowercase();
        if lbl.contains("ask again") || lbl.contains("always") || lbl.contains("persist") || lbl.contains("session") || lbl.contains("auto mode") {
            assert_ne!(c["kind"], json!("once"), "{cli} persistent/session choice misclassified as `once`: {c}");
        }
    }
    let cmd1 = st1["approval"]["command"].as_str().unwrap_or("");
    assert!(cmd1.contains(&denied_marker), "{cli} approval #1 missing command `{denied_marker}`: {st1}");

    // 4. Reject approval #1 with `olo agent approve <name> --no --reason "..." --json`!
    // Deliberately include BOTH a leading digit `2` AND the letter `p` in `--reason` to torture-test C4
    // LIVE against real Claude (`2` = don't ask again), real Codex (`p` = don't ask again), and real Agy!
    let deny = rig.run_olo_json(&["agent", "approve", &name, "--no", "--reason", "2 checks failed; please use the approved command instead", "--json"]).await;
    assert_eq!(deny["ok"], json!(true), "{cli} approve --no failed: {deny}");
    assert_eq!(deny["chosen_kind"], json!("no"));

    // 5. Watch for approval #2 (`echo APPROVED_ONCE_...`)!
    let cur_s = cursor.to_string();
    let w_ap2 = rig.run_olo_json(&["agent", "watch", "--all", "--since", &cur_s, "--on", "approval,error,exit", "--timeout", "25", "--json"]).await;
    assert_eq!(w_ap2["event"], json!("waiting_approval"), "{cli} expected waiting_approval #2, got: {w_ap2}");
    cursor = w_ap2["cursor"].as_u64().unwrap();
    let st2 = rig.run_olo_json(&["agent", "status", &name, "--json"]).await;
    assert!(st2["approval"]["command"].as_str().unwrap_or("").contains(&approved_marker), "{cli} approval #2 command mismatch: {st2}");

    // Confirm the rejected command was NEVER executed (mock API request #2 must NOT contain `denied_marker` as stdout without denial)!
    let ap_once = rig.run_olo_json(&["agent", "approve", &name, "--once", "--json"]).await;
    assert_eq!(ap_once["ok"], json!(true), "{cli} approve --once failed: {ap_once}");
    assert_eq!(ap_once["chosen_kind"], json!("once"));

    // 6. Watch for turn 1 completion (`done` or `idle`) with full verbatim `last_message` & `cli_session_id`!
    let cur_s = cursor.to_string();
    let w_done1 = rig.run_olo_json(&["agent", "watch", "--all", "--since", &cur_s, "--on", "done,idle,error,exit", "--timeout", "25", "--json"]).await;
    assert!(matches!(w_done1["event"].as_str(), Some("done" | "idle")), "{cli} expected turn 1 completion, got: {w_done1}");
    cursor = w_done1["cursor"].as_u64().unwrap();

    // Verify `status` reports real `cli_session_id`, `transcript_path` on disk, and full `last_message`!
    let st_done1 = rig.run_olo_json(&["agent", "status", &name, "--json"]).await;
    let cli_sid = st_done1["cli_session_id"].as_str().filter(|s| !s.is_empty()).unwrap_or_else(|| panic!("{cli} missing cli_session_id: {st_done1}")).to_owned();
    assert!(st_done1["transcript_path"].as_str().is_some_and(|p| Path::new(p).is_file()), "{cli} missing transcript_path on disk: {st_done1}");
    let lm1 = st_done1["last_message"].as_str().unwrap_or("");
    assert!(lm1.contains(&approved_marker), "{cli} status last_message missing full text: {st_done1}");

    // Verify mathematically that:
    //   1) The denied command NEVER executed (`DENIED_EXEC_<cli>` never produced as stdout!)
    //   2) The approved-once command ACTUALLY executed and its real stdout `APPROVED_EXEC_<cli>` reached the mock API!
    let reqs = rig.mock.requests();
    assert!(!reqs.iter().any(|r| r.body.contains(&denied_exec)), "{cli} DENIED command actually executed! reqs={reqs:?}");
    assert!(reqs.iter().any(|r| r.body.contains(&approved_exec)), "{cli} approved command stdout `{approved_exec}` never reached mock API!");

    // 7. Test `olo agent send --file` (multiline follow-up) + verify turn 2 completes!
    let followup_file = rig.dir.join(format!("followup-{cli}.txt"));
    std::fs::write(&followup_file, format!("Follow-up line 1 for {cli}\nFollow-up line 2 with details.\n")).unwrap();
    let sent = rig.run_olo_json(&["agent", "send", &name, "--file", followup_file.to_str().unwrap(), "--enter", "--json"]).await;
    assert_eq!(sent["ok"], json!(true), "{cli} send --file failed: {sent}");

    let cur_s = cursor.to_string();
    let w_done2 = rig.run_olo_json(&["agent", "watch", "--all", "--since", &cur_s, "--on", "done,idle,error,exit", "--timeout", "25", "--json"]).await;
    assert!(matches!(w_done2["event"].as_str(), Some("done" | "idle")), "{cli} expected turn 2 completion, got: {w_done2}");
    cursor = w_done2["cursor"].as_u64().unwrap();

    // 8. Test `olo agent read --lines 40 --json`!
    let read_out = rig.run_olo_json(&["agent", "read", &name, "--lines", "40", "--json"]).await;
    assert_eq!(read_out["ok"], json!(true));
    assert!(!read_out["output"].as_str().unwrap_or("").is_empty(), "{cli} read returned empty output");

    // 9. Test server-side `--desktop none` isolation from this child's real `block_id`!
    let child_bid = started["block_id"].as_str().unwrap();
    let (iso_code, _, iso_err) = rig.run_olo_from(&["op", "whoami"], Some(&rig.sid), Some(child_bid)).await;
    assert_eq!(iso_code, 69, "{cli} --desktop none MUST refuse `olo op`, got code={iso_code} err={iso_err}");
    assert!(iso_err.contains("--desktop none"), "{cli} unexpected isolation stderr: {iso_err}");

    // 10. Test `olo agent stop <name> --save PATH --json` -> scrollback archived + `cli_session_id` preserved!
    let save_path = rig.dir.join(format!("archived-{cli}.txt"));
    let stopped = rig.run_olo_json(&["agent", "stop", &name, "--save", save_path.to_str().unwrap(), "--json"]).await;
    assert_eq!(stopped["ok"], json!(true), "{cli} stop --save failed: {stopped}");
    assert_eq!(stopped["cli_session_id"], json!(cli_sid));
    let archived = std::fs::read_to_string(&save_path).expect("saved scrollback file missing");
    assert!(!archived.trim().is_empty(), "{cli} saved scrollback was empty");

    // Watch `--since cursor --on exit` immediately sees `exited`!
    let cur_s = cursor.to_string();
    let w_exit = rig.run_olo_json(&["agent", "watch", "--all", "--since", &cur_s, "--on", "exit", "--timeout", "5", "--json"]).await;
    assert_eq!(w_exit["event"], json!("exited"), "{cli} missed exited event: {w_exit}");
    cursor = w_exit["cursor"].as_u64().unwrap();

    // 11. Test `olo agent start <cli> --resume <cli_sid> --name <name> --json`!
    let mut resume_args: Vec<String> = vec![
        "agent".into(),
        "start".into(),
        cli.into(),
        "--model".into(),
        model.into(),
        "--effort".into(),
        effort.into(),
        "--resume".into(),
        cli_sid.clone(),
        "--name".into(),
        name.clone(),
        "--trust".into(),
        "--cwd".into(),
        rig.work.display().to_string(),
        "--prompt".into(),
        format!("Resumed prompt for {cli}"),
        "--json".into(),
    ];
    for f in extra_start_flags {
        resume_args.push((*f).to_owned());
    }
    resume_args.extend(rig.cli_env_args(cli));
    let resume_refs: Vec<&str> = resume_args.iter().map(String::as_str).collect();
    let resumed = rig.run_olo_json(&resume_refs).await;
    assert_eq!(resumed["ok"], json!(true), "{cli} resume failed: {resumed}");
    assert_eq!(resumed["resume_id"], json!(cli_sid), "{cli} start --resume MUST report non-null resume_id: {resumed}");
    assert_eq!(resumed["cli_session_id"], json!(cli_sid));

    let st_res = rig.run_olo_json(&["agent", "status", &name, "--json"]).await;
    assert_eq!(st_res["resume_id"], json!(cli_sid), "{cli} status on resumed child MUST report non-null resume_id: {st_res}");

    let cur_s = cursor.to_string();
    let w_res = rig.run_olo_json(&["agent", "watch", "--all", "--since", &cur_s, "--on", "done,idle,error,exit", "--timeout", "25", "--json"]).await;
    assert!(matches!(w_res["event"].as_str(), Some("done" | "idle")), "{cli} resumed turn did not finish: {w_res}");

    // Clean stop + prune!
    let stop2 = rig.run_olo_json(&["agent", "stop", &name, "--force", "--json"]).await;
    assert_eq!(stop2["ok"], json!(true));
    let pruned = rig.run_olo_json(&["agent", "prune", "--json"]).await;
    assert!(pruned["pruned"].as_u64().unwrap_or(0) >= 2);
}

#[test]
fn live_mock_api_e2e_claude_harness() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("claude") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("claude").await;
        // Include variadic flags (`--disallowed-tools`, `--add-dir`) to prove they NEVER swallow `--prompt`!
        let extra_dir = rig.dir.join("extra-add-dir");
        std::fs::create_dir_all(&extra_dir).unwrap();
        let extra_str = extra_dir.to_string_lossy().into_owned();
        run_full_harness_lifecycle(
            &rig,
            "claude",
            "claude-opus-5-5",
            "max",
            &["--permission-mode", "default", "--strict-mcp-config", "--disallowed-tools", "WebFetch", "WebSearch", "--add-dir", &extra_str],
        )
        .await;
    });
}

#[test]
fn live_mock_api_e2e_codex_harness() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("codex") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("codex").await;
        run_full_harness_lifecycle(&rig, "codex", "gpt-5.4-codex", "high", &["--approval", "on-request", "--sandbox", "read-only", "--no-alt-screen", "--no-daemon"]).await;
    });
}

#[test]
fn live_mock_api_e2e_agy_harness() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("agy") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("agy").await;
        run_full_harness_lifecycle(&rig, "agy", "gemini-3-flash-preview", "high", &["--mode", "accept-edits"]).await;
    });
}

// ---------------------------------------------------------------------------
// Test 4: Concurrent 3-Harness Fuzz & Chaos Suite (Claude + Codex + Agy at once!)
// ---------------------------------------------------------------------------

#[test]
fn live_mock_api_concurrent_3_harness_fuzz() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("claude") || !have_program("codex") || !have_program("agy") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("fuzz3").await;
        let mut rng = Rng::new(0x2026_1126_F022);

        // Enqueue randomized multi-step scenarios for ALL 3 harnesses simultaneously!
        let fuzz_cmds = ["sh -c \"echo 'FUZZ_SINGLE_QUOTE' && pwd\"", "sh -c \"printf 'line1\\nline2\\nline3\\n' | wc -l\"", "env | head -n 2"];
        for cli in ["claude", "codex", "agy"] {
            let cmd = (*rng.pick(&fuzz_cmds)).to_owned();
            let do_approve = rng.range(2) == 0;
            rig.mock.push(cli, MockReply::RunCommand { command: cmd, reason: format!("Fuzz step for {cli}") });
            if !do_approve {
                // If denied, ask one more command and approve it!
                rig.mock.push(cli, MockReply::RunCommand { command: format!("sh -c \"echo RECOVERED_{cli}\""), reason: "Recovery".into() });
            }
            rig.mock.push(cli, MockReply::Text(format!("FUZZ_DONE_{cli}_SEED_{}", rng.next_u64() % 10000)));
        }

        // Launch ALL THREE CLIs CONCURRENTLY via real `olo agent start` (tests the concurrent-launch race too!).
        let start_one = |cli: &'static str, model: &'static str, effort: &'static str, extra: &'static [&'static str]| {
            let mut args: Vec<String> = vec![
                "agent".into(),
                "start".into(),
                cli.into(),
                "--model".into(),
                model.into(),
                "--effort".into(),
                effort.into(),
                "--name".into(),
                format!("fz-{cli}"),
                "--trust".into(),
                "--cwd".into(),
                rig.work.display().to_string(),
                "--prompt".into(),
                format!("Run fuzz workload for {cli}"),
                "--json".into(),
            ];
            for e in extra {
                args.push((*e).to_owned());
            }
            args.extend(rig.cli_env_args(cli));
            args
        };
        let a_claude = start_one("claude", "claude-opus-5-5", "max", &["--permission-mode", "default", "--strict-mcp-config"]);
        let a_codex = start_one("codex", "gpt-5.4-codex", "high", &["--approval", "on-request", "--sandbox", "read-only", "--no-alt-screen", "--no-daemon"]);
        let a_agy = start_one("agy", "gemini-3-flash-preview", "high", &["--mode", "accept-edits"]);

        let c_refs: Vec<&str> = a_claude.iter().map(String::as_str).collect();
        let x_refs: Vec<&str> = a_codex.iter().map(String::as_str).collect();
        let a_refs: Vec<&str> = a_agy.iter().map(String::as_str).collect();
        let (r_c, r_x, r_a) = tokio::join!(rig.run_olo_json(&c_refs), rig.run_olo_json(&x_refs), rig.run_olo_json(&a_refs));
        for (cli, r) in [("claude", &r_c), ("codex", &r_x), ("agy", &r_a)] {
            assert_eq!(r["ok"], json!(true), "concurrent start failed for {cli}: {r}");
        }

        // Verify ALL THREE children are alive in `list --json` (concurrent-launch race check!).
        let listed = rig.run_olo_json(&["agent", "list", "--json"]).await;
        let subs = listed["subagents"].as_array().unwrap();
        assert_eq!(subs.len(), 3, "concurrent launch lost a child! list={listed}");

        // Drive an event-driven orchestrator loop (`watch --all --since CURSOR`) until all 3 finish!
        let mut cursor: u64 = 0;
        let mut done_set: HashSet<String> = HashSet::new();
        let mut approvals_handled = 0usize;
        let deadline = Instant::now() + Duration::from_secs(75);

        while done_set.len() < 3 && Instant::now() < deadline {
            let cur_s = cursor.to_string();
            let ev = rig.run_olo_json(&["agent", "watch", "--all", "--since", &cur_s, "--on", "approval,question,done,idle,error,exit", "--timeout", "20", "--json"]).await;
            let ev_cur = ev["cursor"].as_u64().unwrap();
            assert!(ev_cur > cursor, "cursor did not advance: {ev_cur} <= {cursor}");
            cursor = ev_cur;
            let child_name = ev["name"].as_str().unwrap_or("").to_owned();
            match ev["event"].as_str().unwrap_or("") {
                "waiting_approval" => {
                    // Verify safety invariant on every live approval event!
                    if let Some(choices) = ev["approval"]["choices"].as_array() {
                        assert_eq!(choices.iter().filter(|c| c["kind"] == "once").count(), 1, "unsafe choices in watch event: {ev}");
                    }
                    let ap = rig.run_olo_json(&["agent", "approve", &child_name, "--once", "--json"]).await;
                    assert_eq!(ap["ok"], json!(true), "fuzz approve failed on {child_name}: {ap}");
                    approvals_handled += 1;
                }
                "done" | "idle" => {
                    let st = rig.run_olo_json(&["agent", "status", &child_name, "--json"]).await;
                    if st["last_message"].as_str().is_some_and(|m| m.contains("FUZZ_DONE_")) {
                        done_set.insert(child_name);
                    }
                }
                other => panic!("unexpected fuzz event `{other}` for {child_name}: {ev}"),
            }
        }
        assert_eq!(done_set.len(), 3, "not all 3 harnesses completed fuzz loop: done={done_set:?}, approvals={approvals_handled}");
        assert!(approvals_handled >= 3, "expected >=3 live approvals across 3 harnesses, got {approvals_handled}");

        // Stop all 3 cleanly!
        for cli in ["claude", "codex", "agy"] {
            let s = rig.run_olo_json(&["agent", "stop", &format!("fz-{cli}"), "--force", "--json"]).await;
            assert_eq!(s["ok"], json!(true));
        }
    });
}

// ---------------------------------------------------------------------------
// Test 5: Adversarial Security, Isolation, Subtree, Watch & Concurrency Suite
// (Covers ALL Opus 5.5 Audit §7 Categories C1-C6, H1-H18, M1-M26!)
// ---------------------------------------------------------------------------

#[test]
fn audit_security_isolation_subtree_watch_and_concurrency() {
    if !std::path::Path::new("/usr/bin/rex").exists() {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("audit").await;
        let ctrl = rig.inner.state.lock().unwrap().hosts.get("local").unwrap().control.clone().unwrap();
        let ep = rig.inner.state.lock().unwrap().hosts.get("local").unwrap().endpoint.clone().unwrap();
        let parent_wid = rig.inner.state.lock().unwrap().hosts.get("local").unwrap().sessions.get(&rig.sid).unwrap().windows[0].window_id.clone();

        // Spawn two real child tabs (idle shells so we can inject exact VT screens & test live IPC).
        let spawn_tab = |name: &str, pwid: &str, policy: crate::subagents::DesktopPolicy| {
            let ctrl = ctrl.clone();
            let ep = ep.clone();
            let inner = rig.inner.clone();
            let sid = rig.sid.clone();
            let pwid = pwid.to_owned();
            let name = name.to_owned();
            let parent_bid = rig.parent_bid.clone();
            async move {
                let opts = json!({ "command": ["sh", "-c", "printf '\\033[?2004hREADY\\n'; sleep 60"], "interactive": true });
                let created = ctrl.session_new_window_with_focus(&sid, Some(&name), Some(opts), false).await.unwrap().payload;
                let view = ctrl.session_view(&sid).await.unwrap().payload;
                let wid = created.get("window_id").and_then(Value::as_str).unwrap().to_owned();
                let bid = view.windows.iter().find(|w| w.window_id == wid).unwrap().all_blocks().next().unwrap().block_id.clone();
                let ent = crate::subagents::NativeSubagent {
                    id: crate::subagents::new_subagent_id(),
                    name: Some(name.clone()),
                    host_key: "local".into(),
                    session_id: sid.clone(),
                    window_id: wid,
                    block_id: bid,
                    parent_window_id: pwid,
                    parent_block_id: Some(parent_bid),
                    cli: crate::subagents::AgentCli::Claude,
                    model: "claude-opus-5-5".into(),
                    effort: "max".into(),
                    title: name,
                    role: None,
                    cwd: Some("/var/tmp".into()),
                    resume_id: None,
                    cli_session_id: None,
                    transcript_path: None,
                    desktop_policy: policy,
                    stream_mode: false,
                    started_at: crate::subagents::now_unix_ms() as i64 - 20_000,
                    exited_at: None,
                    exit_code: None,
                    last_pid: None,
                    created_rev: view.revision,
                    seen_live: true,
                    exited_by_sync: false,
                    session_confidence: None,
                    add_dirs: Vec::new(),
                };
                inner.state.lock().unwrap().subagents.register(ent.clone());
                inner.ingest_session("local", &ctrl, &ep, view, &[]).await;
                ent
            }
        };

        let child_a = spawn_tab("alpha", &parent_wid, crate::subagents::DesktopPolicy::None).await;
        let child_b = spawn_tab("beta", &parent_wid, crate::subagents::DesktopPolicy::parse("screens=qa-1").unwrap()).await;
        let grand_a = spawn_tab("grand", &child_a.window_id, crate::subagents::DesktopPolicy::None).await;
        tokio::time::sleep(Duration::from_millis(150)).await;

        // --- C1: Desktop isolation bypass attempts MUST ALL FAIL CLOSED (code 69)! ---
        for (label, sid, bid) in [
            ("missing REX_BLOCK", Some(rig.sid.as_str()), None),
            ("empty REX_BLOCK", Some(rig.sid.as_str()), Some("")),
            ("spoofed unknown REX_BLOCK", Some(rig.sid.as_str()), Some("blk-fake-999")),
            ("child_a own block", Some(rig.sid.as_str()), Some(child_a.block_id.as_str())),
        ] {
            let (code, _, err) = rig.run_olo_from(&["op", "whoami"], sid, bid).await;
            assert_eq!(code, 69, "C1 bypass succeeded for {label}: code={code} err={err}");
        }
        // Catch-all non-olo actions from restricted child MUST be denied!
        for catchall in ["agent.auto_approve_toggle", "olo.passwords.fill", "olo.install_remote_skill"] {
            let res = rig.inner.handle_client_perform("local", catchall.into(), None, Some(rig.sid.clone()), Some(child_a.block_id.clone())).await.unwrap();
            assert_eq!(res["ok"], json!(false), "C1 catch-all `{catchall}` was not blocked!");
        }

        // --- C2: Privilege escalation via grandchild launch MUST BE REFUSED! ---
        let (code_esc1, _, err_esc1) = rig
            .run_olo_from(
                &["agent", "start", "claude", "--model", "claude-opus-5-5", "--effort", "max", "--desktop", "inherit"],
                Some(&rig.sid),
                Some(&child_a.block_id),
            )
            .await;
        assert_eq!(code_esc1, 69, "C2 none->inherit escalation must fail 69: err={err_esc1}");
        let (code_esc2, _, err_esc2) = rig
            .run_olo_from(
                &["agent", "start", "claude", "--model", "claude-opus-5-5", "--effort", "max", "--desktop", "screens=qa-2"],
                Some(&rig.sid),
                Some(&child_b.block_id),
            )
            .await;
        assert_eq!(code_esc2, 69, "C2 screens=qa-1 -> screens=qa-2 escalation must fail 69: err={err_esc2}");

        // --- C3: Subtree confinement — child_a CANNOT act on self or sibling child_b, CAN act on strict descendant grand_a! ---
        for cmd in [
            &["agent", "read", "alpha"][..],
            &["agent", "read", "beta"],
            &["agent", "status", "beta"],
            &["agent", "stop", "beta", "--force"],
            &["agent", "focus", "beta"],
            &["agent", "approve", "alpha", "--once"],
            &["agent", "send", "alpha", "--key", "Enter"],
            &["agent", "prune"],
        ] {
            let (c, _, e) = rig.run_olo_from(cmd, Some(&rig.sid), Some(&child_a.block_id)).await;
            assert_ne!(c, 0, "C3 child_a MUST NOT run {cmd:?} on self/sibling (err={e})");
        }
        // child_a CAN read & inspect its own strict descendant `grand`!
        let (c_ok, out_ok, err_ok) = rig.run_olo_from(&["agent", "status", "grand", "--json"], Some(&rig.sid), Some(&child_a.block_id)).await;
        assert_eq!(c_ok, 0, "child_a should be allowed to inspect its own descendant grand: {err_ok}");
        assert!(out_ok.contains(&grand_a.id));

        // --- C6 & H18: `send` refused while approval dialog is active, and paste escape stripped ---
        {
            let bk = BlockKey { host_key: "local".into(), session_id: rig.sid.clone(), block_id: child_a.block_id.clone() };
            let br = rig.inner.state.lock().unwrap().blocks.get(&bk).unwrap().clone();
            let mut g = br.lock().unwrap();
            g.terminal.vt_write(b"\x1b[2J\x1b[H\x1b[?2004hBash command\r\n  rm -rf /tmp/x\r\nDo you want to proceed?\r\n \xe2\x9d\xaf 1. Yes\r\n   2. Yes, and don\xe2\x80\x99t ask again for: rm\r\n   3. No\r\n Esc to cancel\r\n");
            g.received_bytes += 256;
            g.subagent_cls_cache = None;
        }
        let (c_send_ref, _, e_send_ref) = rig.run_olo(&["agent", "send", "alpha", "please proceed", "--enter"]).await;
        assert_ne!(c_send_ref, 0, "C6 send must refuse typing into active approval dialog");
        assert!(e_send_ref.contains("dialog is active"), "unexpected C6 stderr: {e_send_ref}");

        // --- H5 & H7: A->B same-state approval transitions BOTH recorded without duplicates! ---
        let bk_a = BlockKey { host_key: "local".into(), session_id: rig.sid.clone(), block_id: child_a.block_id.clone() };
        rig.inner.record_subagent_transition(&bk_a);
        // Watch from cursor 0 catches approval A as soon as its stability+inspect completes!
        let ev_a = rig.run_olo_json(&["agent", "watch", "--all", "--since", "0", "--on", "approval", "--timeout", "5", "--json"]).await;
        assert!(ev_a["approval"]["command"].as_str().unwrap_or("").contains("rm -rf"), "missed approval A: {ev_a}");
        let cur_a = ev_a["cursor"].as_u64().unwrap();
        // Immediately replace dialog A with a DIFFERENT dialog B (no Working state in between!)
        {
            let br = rig.inner.state.lock().unwrap().blocks.get(&bk_a).unwrap().clone();
            let mut g = br.lock().unwrap();
            g.terminal.vt_write(b"\x1b[2J\x1b[HBash command\r\n  git push --force\r\nDo you want to proceed?\r\n \xe2\x9d\xaf 1. Yes\r\n   2. No\r\n Esc to cancel\r\n");
            g.received_bytes += 256;
            g.subagent_cls_cache = None;
        }
        rig.inner.record_subagent_transition(&bk_a);
        let ev_b = rig.run_olo_json(&["agent", "watch", "--all", "--since", &cur_a.to_string(), "--on", "approval", "--timeout", "5", "--json"]).await;
        assert!(ev_b["approval"]["command"].as_str().unwrap_or("").contains("git push"), "H5 dropped same-state approval B: {ev_b}");
        let cur_b = ev_b["cursor"].as_u64().unwrap();

        // --- H8: `since > latest_seq` immediately returns `{"event":"reset"}` instead of hanging! ---
        let reset = rig.run_olo_json(&["agent", "watch", "--all", "--since", "999999", "--timeout", "3", "--json"]).await;
        assert_eq!(reset["event"], json!("reset"), "H8 cursor reset failed: {reset}");

        // --- H11: `--match` MUST NOT fire on baseline text already on screen! ---
        let (mc, mout, _) = rig.run_olo(&["agent", "watch", "alpha", "--match", "git push", "--timeout", "1", "--json"]).await;
        assert_eq!(mc, 124, "H11 --match falsely matched pre-existing screen text: {mout}");

        // --- H4 & M11: Natural `InlineApc::ChildExited` on grandchild `grand` emits `exited` visible to top-level `watch --all`! ---
        let bk_g = BlockKey { host_key: "local".into(), session_id: rig.sid.clone(), block_id: grand_a.block_id.clone() };
        rig.inner.record_subagent_child_exit(&bk_g, 42);
        let ev_g_exit = rig.run_olo_json(&["agent", "watch", "--all", "--since", &cur_b.to_string(), "--on", "exit", "--timeout", "3", "--json"]).await;
        assert_eq!(ev_g_exit["id"], json!(grand_a.id), "H4/M11 missed grandchild natural exit: {ev_g_exit}");
        assert_eq!(ev_g_exit["event"], json!("exited"));

        // --- M10: `stop --force --keep-tab` refused with code 2! ---
        let (sc, _, serr) = rig.run_olo(&["agent", "stop", "alpha", "--force", "--keep-tab"]).await;
        assert_eq!(sc, 2, "M10 --force --keep-tab should exit 2: {serr}");

        // --- H1: 30 parallel registrations + out-of-order older view MUST NOT mark any live child exited! ---
        let mut reg = crate::subagents::SubagentRegistry::default();
        let mut live_all = HashSet::new();
        live_all.insert("w-parent".to_owned());
        for k in 0..30u64 {
            let wid = format!("w-par-{k}");
            live_all.insert(wid.clone());
            reg.register(crate::subagents::NativeSubagent {
                id: format!("sa-p{k:02}"),
                name: Some(format!("p-{k}")),
                host_key: "local".into(),
                session_id: "s1".into(),
                window_id: wid,
                block_id: format!("b-par-{k}"),
                parent_window_id: "w-parent".into(),
                parent_block_id: None,
                cli: crate::subagents::AgentCli::Claude,
                model: "m".into(),
                effort: "high".into(),
                title: format!("P{k}"),
                role: None,
                cwd: None,
                resume_id: None,
                cli_session_id: None,
                transcript_path: None,
                desktop_policy: crate::subagents::DesktopPolicy::None,
                stream_mode: false,
                started_at: crate::subagents::now_unix_ms() as i64,
                exited_at: None,
                exit_code: None,
                last_pid: None,
                created_rev: 100 + k,
                seen_live: false,
                exited_by_sync: false,
                session_confidence: None,
                add_dirs: Vec::new(),
            });
            // Simulate an older concurrent view arriving (`rev = 100`, missing later children):
            let partial: HashSet<String> = (0..=k.min(5)).map(|j| format!("w-par-{j}")).chain(std::iter::once("w-parent".into())).collect();
            let (_, ex) = reg.sync_live_windows("local", "s1", &partial, 100);
            assert!(ex.is_empty(), "H1 race falsely exited child during burst at k={k}: {ex:?}");
        }
        // Final view with all 30 live keeps every single child alive!
        let (_, ex_final) = reg.sync_live_windows("local", "s1", &live_all, 130);
        assert!(ex_final.is_empty());
        assert_eq!(reg.entries.iter().filter(|e| e.exited_at.is_none()).count(), 30);

        // Clean up live tabs.
        let _ = rig.run_olo(&["agent", "stop", "alpha", "--force", "--tree"]).await;
        let _ = rig.run_olo(&["agent", "stop", "beta", "--force"]).await;
    });
}

// ---------------------------------------------------------------------------
// Test 6: Live API Error / Mid-Stream Drop & Recovery Fuzz Across All 3 CLIs
// ---------------------------------------------------------------------------

#[test]
fn live_mock_api_error_midstream_drop_and_recovery() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("claude") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("errdrop").await;
        // Use a sticky HTTP 400 API error so every retry attempt fails deterministically until Claude
        // surfaces the error on screen; only clear sticky & enqueue the recovery text AFTER `error` is observed!
        rig.mock.set_sticky(
            "claude",
            Some(MockReply::HttpError { status: 400, body: r#"{"type":"error","error":{"type":"invalid_request_error","message":"Mock overload fault for fuzz test"}}"#.into() }),
        );

        let mut args: Vec<String> = vec![
            "agent".into(),
            "start".into(),
            "claude".into(),
            "--model".into(),
            "claude-opus-5-5".into(),
            "--effort".into(),
            "max".into(),
            "--name".into(),
            "err-claude".into(),
            "--trust".into(),
            "--permission-mode".into(),
            "default".into(),
            "--strict-mcp-config".into(),
            "--cwd".into(),
            rig.work.display().to_string(),
            "--prompt".into(),
            "Trigger mock API error".into(),
            "--json".into(),
        ];
        args.extend(rig.cli_env_args("claude"));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let started = rig.run_olo_json(&refs).await;
        let mut cursor = started["cursor"].as_u64().unwrap();

        // Watch MUST catch the `error` event!
        let cur_s = cursor.to_string();
        let w_err = rig.run_olo_json(&["agent", "watch", "err-claude", "--since", &cur_s, "--on", "error,exit", "--timeout", "25", "--json"]).await;
        assert_eq!(w_err["event"], json!("error"), "expected error event on HTTP 400: {w_err}");
        assert!(w_err["error"].as_str().is_some_and(|e| !e.is_empty()), "missing error message: {w_err}");
        cursor = w_err["cursor"].as_u64().unwrap();

        // Now clear remaining error responses and queue a healthy recovery reply for the follow-up turn!
        rig.mock.clear_queues();
        rig.mock.push("claude", MockReply::Text("RECOVERED_AFTER_API_ERROR_OK".into()));

        // Send a follow-up turn -> MUST recover to `done` with `error: null` (M2: stale error cleared!)
        let sent = rig.run_olo_json(&["agent", "send", "err-claude", "Try again now", "--enter", "--json"]).await;
        assert_eq!(sent["ok"], json!(true));
        let pre_cur = sent["cursor"].as_u64().unwrap().max(cursor);
        let w_rec = rig.run_olo_json(&["agent", "watch", "err-claude", "--since", &pre_cur.to_string(), "--on", "done,idle,error", "--timeout", "25", "--json"]).await;
        assert!(matches!(w_rec["event"].as_str(), Some("done" | "idle")), "recovery turn failed or reported stale error: {w_rec}");

        let st = rig.run_olo_json(&["agent", "status", "err-claude", "--json"]).await;
        assert_eq!(st["state"], json!("done"), "recovered status should be done: {st}");
        assert!(st["error"].is_null(), "stale error leaked into healthy recovery turn: {st}");
        assert!(st["last_message"].as_str().unwrap_or("").contains("RECOVERED_AFTER_API_ERROR_OK"), "missing recovery last_message: {st}");

        let _ = rig.run_olo_json(&["agent", "stop", "err-claude", "--force", "--json"]).await;
    });
}

// ---------------------------------------------------------------------------
// Test 7: Release Perf Measurement — 30 Streaming Children + 3 Watchers
// (Run with `perf stat --no-scale` on `--release` build per AGENTS.md & Audit §7 #8)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "perf measurement"]
fn perf_30_streaming_children_3_watchers() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let state = Arc::new(Mutex::new(HubState {
            hosts: HashMap::new(),
            host_order: vec!["local".into()],
            blocks: HashMap::new(),
            forwards: Vec::new(),
            kept_forwards: Vec::new(),
            kept_forward_errors: HashMap::new(),
            kept_forwards_starting: HashSet::new(),
            settings: crate::settings::Settings::default(),
            active_theme: TerminalTheme::default(),
            pools: None,
            repos: Vec::new(),
            carry_choices: Vec::new(),
            kept: Vec::new(),
            sessions_window: olo_rex_client::sessions::Window::default(),
            git_cache: GitCache::new(),
            local_bin_dir: None,
            subagents: crate::subagents::SubagentRegistry::default(),
            quick_look_cache: HashMap::new(),
        }));
        let inner = Arc::new(RexHubInner {
            state,
            ssh_pool: Arc::new(SshSessionPool::default()),
            askpass: None,
            notify: Arc::new(|_| {}),
            spares: Mutex::new(HashMap::new()),
            agent_socks: Mutex::new(HashMap::new()),
            bridge_calls: bridge::Calls::default(),
            run_jobs: crate::bridge_run::RunJobs::new(),
            run_approvals: crate::bridge_run::RunApprovals::new(|| {}, || {}),
            keeping: Mutex::new(HashMap::new()),
            subagent_events: Arc::new(tokio::sync::Notify::new()),
            subagent_log: Mutex::new(crate::subagents::SubagentEventLog::default()),
            subagent_text_watchers: std::sync::atomic::AtomicUsize::new(0),
        });
        let parent_wid = "w-perf-parent".to_owned();
        let theme = TerminalTheme::default();
        let mut block_keys = Vec::with_capacity(30);
        {
            let mut st = inner.state.lock().unwrap();
            for i in 0..30usize {
                let wid = format!("w-perf-{i:02}");
                let bid = format!("b-perf-{i:02}");
                let bk = BlockKey { host_key: "local".into(), session_id: "s-perf".into(), block_id: bid.clone() };
                let br = Arc::new(Mutex::new(BlockRuntime::new(120, 40, &theme)));
                st.blocks.insert(bk.clone(), br);
                st.subagents.register(crate::subagents::NativeSubagent {
                    id: format!("sa-perf-{i:02}"),
                    name: Some(format!("worker-{i:02}")),
                    host_key: "local".into(),
                    session_id: "s-perf".into(),
                    window_id: wid,
                    block_id: bid,
                    parent_window_id: parent_wid.clone(),
                    parent_block_id: Some("b-perf-parent".into()),
                    cli: crate::subagents::AgentCli::Claude,
                    model: "claude-opus-5-5".into(),
                    effort: "max".into(),
                    title: format!("Worker {i:02}"),
                    role: None,
                    cwd: Some("/var/tmp".into()),
                    resume_id: None,
                    cli_session_id: None,
                    transcript_path: None,
                    desktop_policy: crate::subagents::DesktopPolicy::None,
                    stream_mode: false,
                    started_at: crate::subagents::now_unix_ms() as i64 - 30_000,
                    exited_at: None,
                    exit_code: None,
                    last_pid: Some(10_000 + i as i32),
                    created_rev: 1,
                    seen_live: true,
                    exited_by_sync: false,
                    session_confidence: None,
                    add_dirs: Vec::new(),
                });
                block_keys.push(bk);
            }
        }

        // Spawn 3 concurrent event-driven state watchers waiting on `subagent_events` (zero polling!).
        let wake_count = Arc::new(AtomicUsize::new(0));
        let wake_bar = Arc::new(tokio::sync::Barrier::new(4));
        let mut watchers = Vec::with_capacity(3);
        for _ in 0..3 {
            let ev_notify = inner.subagent_events.clone();
            let log_inner = inner.clone();
            let wc = wake_count.clone();
            let bar = wake_bar.clone();
            watchers.push(tokio::spawn(async move {
                let notified = ev_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                bar.wait().await;
                notified.await;
                wc.fetch_add(1, Ordering::Relaxed);
                let got_at = Instant::now();
                let seq = log_inner.subagent_log.lock().unwrap().latest_seq();
                (got_at, seq)
            }));
        }
        wake_bar.wait().await;

        // Phase 1: Stream 30 children × 200 VT chunks (6,000 chunks total) through the exact block-data hot path.
        // Because `subagent_text_watchers == 0`, ZERO spurious watcher notifications fire during streaming!
        let t_stream = Instant::now();
        let chunk = b"  compiling olo-rex v0.1.0 (/workspace/olo/apps/rex) [chunk 0123456789]\r\n";
        for round in 0..200usize {
            for bk in &block_keys {
                let br = inner.state.lock().unwrap().blocks.get(bk).unwrap().clone();
                let mut g = br.lock().unwrap();
                g.terminal.vt_write(chunk);
                g.received_bytes += chunk.len() as u64;
                g.last_output = Instant::now();
                g.last_output_at = crate::subagents::now_unix_ms();
                g.subagent_cls_cache = None;
                drop(g);
                if inner.subagent_text_watchers.load(Ordering::Relaxed) > 0 {
                    inner.subagent_events.notify_waiters();
                }
            }
            if round % 50 == 49 {
                let entries: Vec<_> = inner.state.lock().unwrap().subagents.entries.clone();
                for ent in &entries {
                    let _ = inner.live_subagent_rich(ent);
                }
            }
        }
        let stream_ms = t_stream.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(wake_count.load(Ordering::Relaxed), 0, "state watchers must experience ZERO spurious wakeups during streaming!");

        // Phase 2: Measure 100 full cached `live_subagent_rich` sweeps across all 30 children (3,000 evaluations).
        let entries: Vec<_> = inner.state.lock().unwrap().subagents.entries.clone();
        for ent in &entries {
            let _ = inner.live_subagent_rich(ent);
        }
        let t_cached = Instant::now();
        for _ in 0..100 {
            for ent in &entries {
                let _ = inner.live_subagent_rich(ent);
            }
        }
        let cached_sweep_us = t_cached.elapsed().as_secs_f64() * 1_000_000.0 / 100.0;

        // Phase 3: Inject an approval event and measure end-to-end wakeup latency across all 3 watchers!
        let t_emit = Instant::now();
        let seq = inner.subagent_log.lock().unwrap().push(crate::subagents::SubagentEvent {
            seq: 0,
            timestamp_ms: crate::subagents::now_unix_ms(),
            host_key: "local".into(),
            session_id: "s-perf".into(),
            parent_window_id: parent_wid.clone(),
            ancestor_windows: vec![parent_wid.clone()],
            id: "sa-perf-00".into(),
            name: Some("worker-00".into()),
            window_id: "w-perf-00".into(),
            block_id: "b-perf-00".into(),
            resume_id: None,
            cli_session_id: None,
            event: "waiting_approval".into(),
            state: crate::subagents::SubagentLiveState::WaitingApproval,
            summary: Some("Approve command".into()),
            last_message: None,
            approval: None,
            error: None,
            tail: Vec::new(),
            dedupe_key: "cmd:perf".into(),
        });
        assert_eq!(seq, 1);
        inner.subagent_events.notify_waiters();
        let mut max_wake_us = 0.0f64;
        for w in watchers {
            let (t_got, seq) = w.await.unwrap();
            assert_eq!(seq, 1);
            max_wake_us = max_wake_us.max(t_got.saturating_duration_since(t_emit).as_secs_f64() * 1_000_000.0);
        }

        eprintln!(
            "PERF 30-streaming-children+3-watchers: 6000_chunks={stream_ms:.2}ms ({:.2}us/chunk), cached_30child_sweep={cached_sweep_us:.1}us, 3watcher_wake_max={max_wake_us:.1}us, spurious_wakes=0",
            (stream_ms * 1000.0) / 6000.0,
        );
    });
}
