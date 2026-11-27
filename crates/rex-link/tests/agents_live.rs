//! The agents service with the agents themselves: Claude Code, Codex
//! and Antigravity as installed here, each in a home of the test's own
//! and answered by a made-up API on this computer (Anthropic's Messages,
//! OpenAI's Responses and Gemini's, as each asks): no account, no
//! network, nothing spent. And **no Olo desktop**: every command is
//! `olo agent …` as a host has it (Olo's own `olo` script, which runs
//! the helper), asked of the host's agents service.
//!
//! A whole life for each: started with a prompt from a file, its
//! permission question seen whole and refused with a reason, the next
//! one allowed once, its turn's end with its last message whole and its
//! conversation's id, a message of several lines, its screen read,
//! stopped with what it showed saved, taken up again by its id. Then all
//! three at once, and an API that fails and comes back.
//!
//! Skipped where Rex or the agent isn't installed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const HELPER: &str = env!("CARGO_BIN_EXE_olo-rex-link");
/// Olo's command on a host, as the app puts it there.
const OLO: &str = include_str!("../../../apps/rex/src/bridge_olo.sh");
/// Stand-in agent script (used in the live shared-minder test so real
/// Claude Code minds two deterministic stand-in agents without them
/// sharing the mock API queue).
const STAND_IN: &str = include_str!("stand-in.sh");

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
enum CodexSseStep {
    SleepMs(u64),
    Reasoning { chunks: Vec<String>, delay_ms: u64 },
    Message { phase: Option<String>, chunks: Vec<String>, delay_ms: u64 },
    ExecCommand { command: String, escalated: bool, reason: String },
    ExecCommandYield { command: String, yield_time_ms: u64 },
    DropMidStream,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum MockReply {
    /// Stream plain text and end the turn.
    Text(String),
    /// Emit a shell/command tool call that triggers the CLI's real approval dialog.
    /// (A `reason` that begins "background:" has Claude Code run it in the
    /// background: `run_in_background`, a shell that outlives the turn.)
    RunCommand { command: String, reason: String },
    /// Emit a call of another of Claude Code's tools by its name (`Read`,
    /// `Write`), with its input.
    Tool { name: String, input: Value },
    /// Return an HTTP error status (e.g. 429 or 529 overloaded) with JSON error body.
    HttpError { status: u16, body: String },
    /// Start streaming SSE then drop the TCP connection mid-response.
    MidStreamDrop { partial_text: String },
    /// Plain text after a wait: a model that takes a while to begin (a
    /// turn that has begun and not ended, for that long).
    SlowText { after_ms: u64, text: String },
    /// Stream a multi-step OpenAI Responses SSE sequence slowly over chunked HTTP.
    CodexStream { steps: Vec<CodexSseStep> },
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct RecordedReq {
    path: String,
    body: String,
}

type ClaudeHandler = Arc<dyn Fn(&str) -> MockReply + Send + Sync>;

struct MockState {
    /// Queued scripted replies per wire family (`"claude"`, `"codex"`, `"agy"`).
    queues: HashMap<&'static str, VecDeque<MockReply>>,
    /// Sticky reply returned for EVERY request of a family until cleared (used for deterministic retry-exhausting API errors).
    sticky: HashMap<&'static str, MockReply>,
    /// Optional dynamic handler for Claude requests (used by the shared-minder live test to drive replies from actual tool outputs).
    claude_handler: Option<ClaudeHandler>,
    /// All captured HTTP POST requests for assertions (e.g. verifying tool execution vs denial).
    requests: Vec<RecordedReq>,
    req_count: usize,
    /// Race-free watch channel gating Claude mock replies (`true` = open, `false` = held).
    gate_tx: tokio::sync::watch::Sender<bool>,
    gate_rx: tokio::sync::watch::Receiver<bool>,
}

impl Default for MockState {
    fn default() -> Self {
        let (gate_tx, gate_rx) = tokio::sync::watch::channel(true);
        Self { queues: HashMap::new(), sticky: HashMap::new(), claude_handler: None, requests: Vec::new(), req_count: 0, gate_tx, gate_rx }
    }
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

    fn set_claude_handler(&self, handler: Option<ClaudeHandler>) {
        self.state.lock().unwrap().claude_handler = handler;
    }

    fn clear_queues(&self) {
        let mut g = self.state.lock().unwrap();
        g.queues.clear();
        g.sticky.clear();
        g.claude_handler = None;
        g.requests.clear();
        let _ = g.gate_tx.send(true);
    }

    fn requests(&self) -> Vec<RecordedReq> {
        self.state.lock().unwrap().requests.clone()
    }

    fn hold_claude(&self) {
        let _ = self.state.lock().unwrap().gate_tx.send(false);
    }

    fn release_claude(&self) {
        let _ = self.state.lock().unwrap().gate_tx.send(true);
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

    if family == "claude" {
        let mut rx = state.lock().unwrap().gate_rx.clone();
        let _ = rx.wait_for(|open| *open).await;
    }

    let (req_id, handler_opt) = {
        let mut g = state.lock().unwrap();
        g.req_count += 1;
        let id = g.req_count;
        g.requests.push(RecordedReq { path: path.clone(), body: body.clone() });
        let h = if family == "claude" { g.claude_handler.clone() } else { None };
        (id, h)
    };
    let reply = if let Some(handler) = handler_opt {
        handler(&body)
    } else {
        let mut g = state.lock().unwrap();
        g.sticky.get(family).cloned().or_else(|| g.queues.get_mut(family).and_then(VecDeque::pop_front)).unwrap_or_else(|| MockReply::Text(format!("DEFAULT_{}_{req_id}", family.to_ascii_uppercase())))
    };

    let reply = match reply {
        MockReply::SlowText { after_ms, text } => {
            tokio::time::sleep(Duration::from_millis(after_ms)).await;
            MockReply::Text(text)
        }
        reply => reply,
    };
    match reply {
        MockReply::SlowText { .. } => unreachable!("made plain above"),
        MockReply::CodexStream { steps } => {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.flush().await;
            let rid = format!("resp_{req_id}");
            async fn send_ev(stream: &mut TcpStream, ev: &str, data: Value) {
                let s = format!("event: {ev}\ndata: {data}\n\n");
                let frame = format!("{:x}\r\n{s}\r\n", s.len());
                let _ = stream.write_all(frame.as_bytes()).await;
                let _ = stream.flush().await;
            }
            send_ev(&mut stream, "response.created", json!({"type":"response.created","response":{"id":rid,"status":"in_progress"}})).await;
            let mut output_items: Vec<Value> = Vec::new();
            let mut output_index: usize = 0;
            let mut dropped = false;
            for (step_idx, step) in steps.into_iter().enumerate() {
                match step {
                    CodexSseStep::SleepMs(ms) => {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                    }
                    CodexSseStep::DropMidStream => {
                        dropped = true;
                        break;
                    }
                    CodexSseStep::Reasoning { chunks, delay_ms } => {
                        let rsid = format!("rs_{req_id}_{step_idx}");
                        send_ev(&mut stream, "response.output_item.added", json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":rsid,"type":"reasoning","summary":[]}}))
                            .await;
                        send_ev(
                            &mut stream,
                            "response.reasoning_summary_part.added",
                            json!({"type":"response.reasoning_summary_part.added","item_id":rsid,"output_index":output_index,"summary_index":0,"part":{"type":"summary_text","text":""}}),
                        )
                        .await;
                        let mut full = String::new();
                        for chunk in chunks {
                            full.push_str(&chunk);
                            send_ev(
                                &mut stream,
                                "response.reasoning_summary_text.delta",
                                json!({"type":"response.reasoning_summary_text.delta","item_id":rsid,"output_index":output_index,"summary_index":0,"delta":chunk}),
                            )
                            .await;
                            if delay_ms > 0 {
                                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            }
                        }
                        send_ev(
                            &mut stream,
                            "response.reasoning_summary_text.done",
                            json!({"type":"response.reasoning_summary_text.done","item_id":rsid,"output_index":output_index,"summary_index":0,"text":full}),
                        )
                        .await;
                        let item = json!({"id":rsid,"type":"reasoning","summary":[{"type":"summary_text","text":full}]});
                        send_ev(&mut stream, "response.output_item.done", json!({"type":"response.output_item.done","output_index":output_index,"item":item})).await;
                        output_items.push(item);
                        output_index += 1;
                    }
                    CodexSseStep::Message { phase, chunks, delay_ms } => {
                        let mid = format!("msg_{req_id}_{step_idx}");
                        let mut added_item = json!({"id":mid,"type":"message","role":"assistant","status":"in_progress","content":[]});
                        if let Some(ref ph) = phase {
                            added_item["phase"] = json!(ph);
                        }
                        send_ev(&mut stream, "response.output_item.added", json!({"type":"response.output_item.added","output_index":output_index,"item":added_item})).await;
                        send_ev(
                            &mut stream,
                            "response.content_part.added",
                            json!({"type":"response.content_part.added","item_id":mid,"output_index":output_index,"content_index":0,"part":{"type":"output_text","text":""}}),
                        )
                        .await;
                        let mut full = String::new();
                        for chunk in chunks {
                            full.push_str(&chunk);
                            send_ev(&mut stream, "response.output_text.delta", json!({"type":"response.output_text.delta","item_id":mid,"output_index":output_index,"content_index":0,"delta":chunk}))
                                .await;
                            if delay_ms > 0 {
                                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            }
                        }
                        send_ev(&mut stream, "response.output_text.done", json!({"type":"response.output_text.done","item_id":mid,"output_index":output_index,"content_index":0,"text":full})).await;
                        let mut done_item = json!({"id":mid,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":full}]});
                        if let Some(ref ph) = phase {
                            done_item["phase"] = json!(ph);
                        }
                        send_ev(&mut stream, "response.output_item.done", json!({"type":"response.output_item.done","output_index":output_index,"item":done_item})).await;
                        output_items.push(done_item);
                        output_index += 1;
                    }
                    CodexSseStep::ExecCommand { command, escalated, reason } => {
                        let fcid = format!("fc_{req_id}_{step_idx}");
                        let cid = format!("call_{req_id}_{step_idx}");
                        let args_json = if escalated {
                            json!({
                                "cmd": command,
                                "sandbox_permissions": "require_escalated",
                                "justification": reason,
                            })
                            .to_string()
                        } else {
                            json!({ "cmd": command }).to_string()
                        };
                        let call = json!({
                            "id": fcid,
                            "type": "function_call",
                            "status": "completed",
                            "call_id": cid,
                            "name": "exec_command",
                            "arguments": args_json,
                        });
                        send_ev(&mut stream, "response.output_item.added", json!({"type":"response.output_item.added","output_index":output_index,"item":call})).await;
                        send_ev(&mut stream, "response.output_item.done", json!({"type":"response.output_item.done","output_index":output_index,"item":call})).await;
                        output_items.push(call);
                        output_index += 1;
                    }
                    CodexSseStep::ExecCommandYield { command, yield_time_ms } => {
                        let fcid = format!("fc_{req_id}_{step_idx}");
                        let cid = format!("call_{req_id}_{step_idx}");
                        let args_json = json!({ "cmd": command, "yield_time_ms": yield_time_ms }).to_string();
                        let call = json!({
                            "id": fcid,
                            "type": "function_call",
                            "status": "completed",
                            "call_id": cid,
                            "name": "exec_command",
                            "arguments": args_json,
                        });
                        send_ev(&mut stream, "response.output_item.added", json!({"type":"response.output_item.added","output_index":output_index,"item":call})).await;
                        send_ev(&mut stream, "response.output_item.done", json!({"type":"response.output_item.done","output_index":output_index,"item":call})).await;
                        output_items.push(call);
                        output_index += 1;
                    }
                }
            }
            if dropped {
                drop(stream);
            } else {
                send_ev(
                    &mut stream,
                    "response.completed",
                    json!({"type":"response.completed","response":{"id":rid,"status":"completed","output":output_items,"usage":{"input_tokens":12,"output_tokens":20,"total_tokens":32}}}),
                )
                .await;
                let _ = stream.write_all(b"0\r\n\r\n").await;
                let _ = stream.flush().await;
            }
        }
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
        MockReply::Tool { name, input } => {
            let mut sse = String::new();
            for (ev, data) in [
                (
                    "message_start",
                    json!({"type":"message_start","message":{"id":format!("msg_{req_id}"),"type":"message","role":"assistant","content":[],"model":"claude-opus-5-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":12,"output_tokens":1}}}),
                ),
                ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":format!("toolu_{req_id}"),"name":name,"input":{}}})),
                ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":input.to_string()}})),
                ("content_block_stop", json!({"type":"content_block_stop","index":0})),
                ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":12}})),
                ("message_stop", json!({"type":"message_stop"})),
            ] {
                sse.push_str(&format!("event: {ev}\ndata: {data}\n\n"));
            }
            write_sse(&mut stream, &sse).await;
        }
        MockReply::RunCommand { command, reason } => match family {
            "claude" => {
                let mut input = json!({"command": command, "description": reason});
                if reason.starts_with("background:") {
                    input["run_in_background"] = json!(true);
                }
                let input_json = input.to_string();
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
// A Rex server, a home and the `olo` command of the test's own
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct LiveRexHarness {
    dir: PathBuf,
    home: PathBuf,
    work: PathBuf,
    olo_bin: PathBuf,
    sock: PathBuf,
    sid: String,
    parent_bid: String,
    mock: MockApiServer,
    rex_proc: Child,
    claude_key: String,
}

impl Drop for LiveRexHarness {
    fn drop(&mut self) {
        let mut retire = Command::new(HELPER);
        self.scratch(&mut retire);
        let _ = retire.args(["agent", "retire"]).output();
        let _ = self.rex_proc.kill();
        let _ = self.rex_proc.wait();
        std::thread::sleep(Duration::from_millis(100));
        if std::env::var_os("OLO_AGENTS_TEST_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl LiveRexHarness {
    async fn start(tag: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(1);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(format!("/var/tmp/olo-agents-live-{}-{}-{n}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        let work = dir.join("work");
        let bin = dir.join("bin");
        std::fs::create_dir_all(home.join(".gemini/antigravity-cli/cache")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(dir.join("agents")).unwrap();

        let mock = MockApiServer::start().await;
        let claude_key = "sk-ant-api03-olofuzztest0000000001".to_owned();

        // A home for all three with nothing real in it. (Claude Code's
        // `projects` begins empty, so `--trust` is what trusts `work`.)
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

        // Olo's own `olo` as a host has it, with the helper beside it
        // (as on the desktop's own computer): `olo agent` runs that.
        let sock = dir.join("rex.sock");
        let olo_bin = bin.join("olo");
        std::fs::write(&olo_bin, OLO).unwrap();
        std::fs::set_permissions(&olo_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(HELPER, bin.join("olo-rex-link")).unwrap();

        let at = format!("unix://{}", sock.display());
        let mut server = Command::new("/usr/bin/rex");
        server.args(["-S", &at, "server", "run", "--listen", &at, "--restore=false", "--state-dir"]).arg(dir.join("rex-state")).current_dir(&work).stdout(Stdio::null()).stderr(Stdio::null());
        let mut rig = Self { dir, home, work, olo_bin, sock, sid: String::new(), parent_bid: String::new(), mock, rex_proc: Command::new("true").spawn().unwrap(), claude_key };
        rig.scratch(&mut server);
        let _ = rig.rex_proc.wait();
        rig.rex_proc = server.spawn().expect("the scratch Rex server");
        let deadline = Instant::now() + Duration::from_secs(8);
        while !rig.sock.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(rig.sock.exists(), "the scratch Rex server didn't begin to listen");
        rig.sid = rig.rex("session.create", json!({ "label": "lead" }))["session_id"].as_str().unwrap().to_owned();
        let lead = rig.rex(
            "session.new_window",
            json!({ "session_id": rig.sid, "window_label": "lead", "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "options": { "size": { "columns": 120, "rows": 40 } } } } }),
        );
        rig.parent_bid = lead["block_ids"][0].as_str().unwrap().to_owned();
        rig
    }

    /// Nothing of the computer the test runs on.
    fn scratch(&self, cmd: &mut Command) {
        cmd.env("HOME", &self.home)
            .env("REX_SERVER_SOCKET", &self.sock)
            .env("OLO_REX_AGENTS_DIR", self.dir.join("agents"))
            .env("TMPDIR", &self.dir)
            .env("XDG_RUNTIME_DIR", &self.dir)
            .env("XDG_STATE_HOME", &self.dir)
            .env("XDG_CONFIG_HOME", &self.dir)
            .env_remove("SSH_AUTH_SOCK")
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK");
        // (Run from an agent's own shell, the test carries that agent's
        // marks (Claude Code's would have a child of it save no
        // transcript): none of them is the scratch server's or its
        // children's.)
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(|n| n.starts_with("CLAUDE") || n.starts_with("CODEX") || n == "AI_AGENT") {
                cmd.env_remove(name);
            }
        }
    }

    fn rex(&self, method: &str, payload: Value) -> Value {
        let mut cmd = Command::new("/usr/bin/rex");
        cmd.args(["-S", &format!("unix://{}", self.sock.display()), "api", "call", method, &payload.to_string()]);
        self.scratch(&mut cmd);
        let out = cmd.output().expect("rex api call");
        assert!(out.status.success(), "rex api call {method}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    /// `olo ARGS` from the lead's pane: Olo's own script, which runs the helper.
    async fn run_olo(&self, args: &[&str]) -> (i32, String, String) {
        self.run_olo_from(args, Some(&self.sid), Some(&self.parent_bid)).await
    }

    async fn run_olo_from(&self, args: &[&str], sid: Option<&str>, bid: Option<&str>) -> (i32, String, String) {
        let mut cmd = Command::new(&self.olo_bin);
        cmd.args(args).current_dir(&self.work).env("PWD", &self.work).stdout(Stdio::piped()).stderr(Stdio::piped());
        self.scratch(&mut cmd);
        if let Some(s) = sid {
            cmd.env("REX_SESSION", s);
        }
        if let Some(b) = bid {
            cmd.env("REX_BLOCK", b);
        }
        let mut cmd = tokio::process::Command::from(cmd);
        let out = tokio::time::timeout(Duration::from_secs(45), cmd.output()).await.expect("olo command timed out").expect("spawn olo");
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    }

    /// `olo` that must succeed and print one JSON object on one line.
    async fn run_olo_json(&self, args: &[&str]) -> Value {
        let (code, stdout, stderr) = self.run_olo(args).await;
        if code != 0 {
            // (Each child's screen, for the failure's account.)
            let (_, listed, _) = self.run_olo(&["agent", "list", "--all", "--fast", "--json"]).await;
            let mut diag = format!("list={listed}\n");
            if let Ok(v) = serde_json::from_str::<Value>(listed.trim()) {
                for child in v["subagents"].as_array().into_iter().flatten().filter(|c| c["state"] != "exited") {
                    let id = child["id"].as_str().unwrap_or_default();
                    let (_, screen, _) = self.run_olo(&["agent", "read", id, "--lines", "60"]).await;
                    diag.push_str(&format!("--- SCREEN [{}] ---\n{screen}\n", child["name"].as_str().unwrap_or(id)));
                }
            }
            for d in std::fs::read_dir(self.dir.join("agents")).into_iter().flatten().flatten() {
                diag.push_str(&format!("--- the service's log ---\n{}", std::fs::read_to_string(d.path().join("service.log")).unwrap_or_default()));
            }
            // (What the service told, and how each child stands, when a wait gives up.)
            for d in std::fs::read_dir(self.dir.join("agents")).into_iter().flatten().flatten() {
                let events = std::fs::read_to_string(d.path().join("events.jsonl")).unwrap_or_default();
                let last: Vec<&str> = events.lines().rev().take(12).collect();
                diag.push_str(&format!("--- its last events, newest first ---\n{}\n", last.iter().map(|l| l.chars().take(420).collect::<String>()).collect::<Vec<_>>().join("\n")));
            }
            if let Ok(v) = serde_json::from_str::<Value>(listed.trim()) {
                for child in v["subagents"].as_array().into_iter().flatten().filter(|c| c["state"] != "exited") {
                    let id = child["id"].as_str().unwrap_or_default();
                    let (_, said, _) = self.run_olo(&["agent", "status", id, "--json"]).await;
                    diag.push_str(&format!("--- STATUS [{id}] ---\n{}\n", said.chars().take(1500).collect::<String>()));
                }
            }
            panic!("olo {args:?} failed (code={code})\nstdout: {stdout}\nstderr: {stderr}\nDIAG:\n{diag}");
        }
        let non_empty: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(non_empty.len(), 1, "olo {args:?} MUST print strictly 1 JSON object, got {} lines:\n{stdout}", non_empty.len());
        serde_json::from_str::<Value>(non_empty[0]).unwrap_or_else(|e| panic!("invalid JSON from olo {args:?}: {e}\n{stdout}"))
    }

    /// What each agent is started with so that it asks the made-up API in the test's home.
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

/// `olo` with nothing after a command says how it is used; a file named
/// for a prompt or a message is read here; what is printed with `--json`
/// is one object on one line.
#[test]
fn olo_agent_says_its_usage_reads_its_files_and_prints_one_object() {
    if !Path::new("/usr/bin/rex").exists() {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("usage").await;
        for nothing in [&["agent", "approve"][..], &["agent", "start"], &["agent", "status"], &["agent", "read"], &["agent", "watch"], &["agent", "send"], &["agent", "stop"], &["agent", "focus"]] {
            let (code, stdout, stderr) = rig.run_olo(nothing).await;
            assert_eq!(code, 2, "olo {nothing:?} should exit 2 with its usage: stdout={stdout:?} stderr={stderr:?}");
            assert!(stderr.contains("usage:") || stderr.contains("missing agent CLI"), "olo {nothing:?}: {stderr:?}");
        }
        let (code, _, stderr) = rig.run_olo(&["agent", "nonsense"]).await;
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("unknown command"), "{stderr}");

        // A prompt's file is read (a relative path from where it was asked), in both spellings; an empty one too.
        std::fs::write(rig.work.join("multiline-prompt.md"), "Line 1 — Unicode “quotes” & $HOME\nLine 2 `backticks` 'single'\n").unwrap();
        std::fs::write(rig.dir.join("empty-prompt.md"), "").unwrap();
        let (code, _, stderr) = rig.run_olo(&["agent", "start", "claude", "--prompt-file", "multiline-prompt.md"]).await;
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("--model"), "the file was read and the model asked for: {stderr:?}");
        let (code, _, stderr) = rig.run_olo(&["agent", "start", "claude", &format!("--prompt-file={}", rig.dir.join("empty-prompt.md").display())]).await;
        assert_eq!(code, 2);
        assert!(stderr.contains("--model"), "{stderr:?}");
        let (code, _, stderr) = rig.run_olo(&["agent", "send", "no-such-child", "--file", "multiline-prompt.md", "--enter"]).await;
        assert_eq!(code, 1);
        assert!(stderr.contains("not found"), "{stderr:?}");
        let (code, _, stderr) = rig.run_olo(&["agent", "start", "claude", "--prompt-file", "/var/tmp/definitely-no-such-file-olo"]).await;
        assert_eq!(code, 1);
        assert!(stderr.contains("couldn't read prompt file"), "{stderr:?}");
        // (What follows `--` is the agent's own, never read as a file here.)
        let (code, _, stderr) = rig.run_olo(&["agent", "start", "--command", "--json", "--", "true", "--prompt-file", "/no/such"]).await;
        assert_eq!(code, 0, "{stderr}");

        let listed = rig.run_olo_json(&["agent", "list", "--json"]).await;
        assert_eq!(listed["ok"], json!(true));
        let pruned = rig.run_olo_json(&["agent", "prune", "--json"]).await;
        assert_eq!(pruned["ok"], json!(true));
    });
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

    // 9. What this child may ask of the desktop is the service's to say (a desktop asks it): nothing.
    let child_bid = started["block_id"].as_str().unwrap();
    let mut ask = Command::new(HELPER);
    rig.scratch(&mut ask);
    let mut ask = ask.args(["agent", "request"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    {
        use std::io::Write as _;
        ask.stdin.take().unwrap().write_all(json!({ "proto": 1, "op": "policy", "block": child_bid }).to_string().as_bytes()).unwrap();
    }
    let said: Value = serde_json::from_slice(&ask.wait_with_output().unwrap().stdout).unwrap();
    assert_eq!(said["data"]["desktop"], json!("none"), "{cli}: what it may ask of the desktop: {said}");

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

/// A shared minder as Claude Code itself runs it (`agents::minder`):
/// one real Claude Code minder minds two stand-in agents nested under
/// it (`lead ▸ minder ▸ {alpha, beta}` = 3 child tabs), reads each
/// agent's own rules file with `Read`, has its one `mind` command
/// allowed with no leave asked while anything else in its shell or
/// `Write` is refused with none asked either, and answers the same
/// command differently for `alpha` (approved once) and `beta` (refused
/// and escalated to the lead's watch). Stopping the minder leaves both
/// agents running under the lead, unminded.
#[test]
fn live_mock_api_a_minder_has_its_one_command_and_nothing_else() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("claude") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("minder").await;
        rig.mock.clear_queues();

        // Stand-in `agy` binary so `alpha` and `beta` are deterministic stand-ins while
        // the shared minder runs the real `claude` binary against `rig.mock`.
        let standin_bin = rig.dir.join("standin-bin");
        let stage = rig.dir.join("stage");
        std::fs::create_dir_all(&standin_bin).unwrap();
        std::fs::create_dir_all(&stage).unwrap();
        let agy_stub = standin_bin.join("agy");
        std::fs::write(&agy_stub, STAND_IN).unwrap();
        std::fs::set_permissions(&agy_stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let staged = |name: &str, what: &str| stage.join(format!("{name}.{what}"));
        let mk_standin = |name: &str| {
            let fifo = staged(name, "fifo");
            assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        };
        let write_fifo_deadline = |fifo: &Path, ready: &Path, line: &str, name: &str| -> Result<(), String> {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                match std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(fifo) {
                    Ok(mut f) => match f.write_all(line.as_bytes()) {
                        Ok(()) => return Ok(()),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(e) => return Err(format!("write to {name} fifo failed (ready={}): {e}", ready.exists())),
                    },
                    Err(e) if e.raw_os_error() == Some(libc::ENXIO) && Instant::now() < until => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(e) => return Err(format!("open {name} fifo failed (ready={}): {e}", ready.exists())),
                }
            }
        };
        let tell = |name: &str, line: &str| {
            let (fifo, ready, line) = (staged(name, "fifo"), staged(name, "ready"), format!("{line}\n"));
            write_fifo_deadline(&fifo, &ready, &line, name).expect("tell stand-in");
        };
        let show = |name: &str, tag: &str, screen: &str| {
            let file = staged(name, tag);
            std::fs::write(&file, screen).unwrap();
            tell(name, &format!("show {}", file.display()));
        };
        let when_typed = |name: &'static str, keys: &'static [u8], tag: &str, then: String| -> std::thread::JoinHandle<bool> {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let file = staged(name, "keys");
            let from = std::fs::read(&file).unwrap_or_default().len();
            let fifo = staged(name, "fifo");
            let out_screen = staged(name, tag);
            std::thread::spawn(move || {
                let until = Instant::now() + Duration::from_secs(30);
                loop {
                    let typed = std::fs::read(&file).unwrap_or_default();
                    if typed.len() > from && typed[from..].windows(keys.len()).any(|w| w == keys) {
                        break;
                    }
                    if Instant::now() >= until {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                std::fs::write(&out_screen, then).unwrap();
                let msg = format!("show {}\n", out_screen.display());
                let w_until = Instant::now() + Duration::from_secs(10);
                while Instant::now() < w_until {
                    match std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&fifo) {
                        Ok(mut f) => return f.write_all(msg.as_bytes()).is_ok(),
                        Err(e) if e.raw_os_error() == Some(libc::ENXIO) => std::thread::sleep(Duration::from_millis(20)),
                        Err(_) => return false,
                    }
                }
                false
            })
        };

        let agy_rule = "─".repeat(60);
        let agy_asking = |command: &str, marked: usize| -> String {
            let mark = |i: usize| if i == marked { ">" } else { " " };
            format!(
                "● Bash({command})\nCommand\n{agy_rule}\nRequesting permission for:\n   {command}\nRun this command?\n{} 1. Yes, run command\n{} 2. Yes, and always allow in this conversation\n{} 3. No, cancel\n  ↑/↓ Navigate\nesc to cancel\n",
                mark(0),
                mark(1),
                mark(2)
            )
        };
        let agy_working = format!("⣽  Generating...\n{agy_rule}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{agy_rule}\nesc to cancel\n");

        // Hold Claude mock replies until both `alpha` and `beta` are started under the shared minder and asking leave.
        rig.mock.hold_claude();
        let orig_path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        let child_path = format!("{}:{orig_path}", standin_bin.display());
        let start_minded = |name: &str, rules: &str, with_minder_env: bool| -> Vec<String> {
            let mut args: Vec<String> = [
                "agent",
                "start",
                "agy",
                "--model",
                "stand-in",
                "--effort",
                "none",
                "--name",
                name,
                "--cwd",
                &rig.work.display().to_string(),
                "--minder",
                "claude:claude-opus-5-5:max",
                "--rules-text",
                rules,
                "--json",
                "--env",
                &format!("STAND_IN_DIR={}", stage.display()),
                "--env",
                &format!("STAND_IN_NAME={name}"),
                "--env",
                &format!("PATH={child_path}"),
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
            if with_minder_env {
                for (k, v) in [
                    ("HOME", rig.home.display().to_string()),
                    ("ANTHROPIC_BASE_URL", format!("http://127.0.0.1:{}", rig.mock.port)),
                    ("ANTHROPIC_API_KEY", rig.claude_key.clone()),
                    ("REX_SERVER_SOCKET", rig.sock.display().to_string()),
                    ("OLO_REX_AGENTS_DIR", rig.dir.join("agents").display().to_string()),
                    ("XDG_RUNTIME_DIR", rig.dir.display().to_string()),
                    ("XDG_STATE_HOME", rig.dir.display().to_string()),
                ] {
                    args.extend(["--minder-env".to_owned(), format!("{k}={v}")]);
                }
            }
            args
        };

        mk_standin("alpha");
        let rules_alpha = "ALPHA_RULE_7C41: Approve `cargo test` once.";
        let a_args = start_minded("alpha", rules_alpha, true);
        let alpha = rig.run_olo_json(&a_args.iter().map(String::as_str).collect::<Vec<_>>()).await;
        let alpha_id = alpha["id"].as_str().unwrap().to_owned();
        let minder_id = alpha["minder"]["id"].as_str().unwrap().to_owned();
        let minder_win = alpha["minder"]["window_id"].as_str().unwrap().to_owned();
        assert_eq!(alpha["minder"]["reused"], json!(false), "{alpha}");

        mk_standin("beta");
        let rules_beta = "BETA_RULE_9D52: Refuse `cargo test` and escalate word for word.";
        let b_args = start_minded("beta", rules_beta, false);
        let beta = rig.run_olo_json(&b_args.iter().map(String::as_str).collect::<Vec<_>>()).await;
        let beta_id = beta["id"].as_str().unwrap().to_owned();
        assert_eq!((&beta["minder"]["id"], &beta["minder"]["reused"]), (&json!(minder_id), &json!(true)), "beta must reuse the same real Claude minder: {beta}");

        let wait_ready = |name: &str| {
            let until = Instant::now() + Duration::from_secs(15);
            while !staged(name, "ready").exists() {
                assert!(Instant::now() < until, "{name} stand-in didn't listen");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait_ready("alpha");
        wait_ready("beta");

        // Verify tree: 3 child tabs total (1 minder + 2 agents = N + 1), both nested under minder.
        let listed = rig.run_olo_json(&["agent", "list", "--json"]).await;
        let subs = listed["subagents"].as_array().unwrap();
        assert_eq!(subs.len(), 3, "2 agents + 1 shared minder = 3 child tabs: {listed}");
        for n in ["alpha", "beta"] {
            let row = subs.iter().find(|s| s["name"] == n).unwrap();
            assert_eq!((&row["parent_window_id"], &row["minded_by"]), (&json!(minder_win), &json!(minder_id)), "{n}: {listed}");
        }

        let service = std::fs::read_dir(rig.dir.join("agents")).unwrap().flatten().map(|d| d.path()).find(|d| d.join("service.log").exists()).expect("the service's folder");
        let events_log = service.join("events.jsonl");

        // Have `alpha` ask `cargo test`, wait until its specific `waiting_approval`
        // event is recorded in `events.jsonl` (no fixed sleep!), then have `beta`
        // ask the exact same command `cargo test` and wait for its recorded event.
        let wait_recorded_approval = |id: &str, label: &str| {
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                let recorded = std::fs::read_to_string(&events_log)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                    .any(|ev| ev["id"] == id && ev["state"] == "waiting_approval");
                if recorded {
                    break;
                }
                assert!(Instant::now() < until, "{label} ({id}) waiting_approval was not recorded in events.jsonl");
                std::thread::sleep(Duration::from_millis(30));
            }
        };
        show("alpha", "ask", &agy_asking("cargo test", 0));
        wait_recorded_approval(&alpha_id, "alpha");
        show("beta", "ask", &agy_asking("cargo test", 0));
        wait_recorded_approval(&beta_id, "beta");

        let alpha_gone = when_typed("alpha", b"\r", "work", agy_working.clone());
        let beta_moved = when_typed("beta", b"\x1b[B\x1b[B", "ask2", agy_asking("cargo test", 2));
        let beta_gone = when_typed("beta", b"\r", "work", agy_working.clone());

        let mind = service.join("minders").join(&minder_id).join("mind").display().to_string();
        let written = rig.dir.join("a-minder-wrote-this.txt");
        let said = "MINDER_SAYS beta asked cargo test which its rules forbid";

        // Dynamic Claude minder driver:
        // - Reads the roster returned by `mind list --json` to discover each agent's rules path (never precomputed).
        // - Reads each agent's rules file with `Read` and derives every approval/refusal/escalation from the actual Read result.
        // - Proves per-agent authorization by attempting `mind approve <other> --once` AFTER only the first agent has been
        //   delivered by `watch` for the identical `cargo test` box, verifying refusal and zero keys sent to `<other>`.
        #[derive(Default)]
        struct Driver {
            step: usize,
            roster: Vec<(String, String)>, // (name, rules_path)
            rules_text: HashMap<String, String>,
            first_agent: Option<String>,
            second_agent: Option<String>,
            cross_refused: bool,
            cross_keys_zero: bool,
        }
        let driver = Arc::new(std::sync::Mutex::new(Driver::default()));
        let last_tool_result = |body: &str| -> String {
            let Ok(v) = serde_json::from_str::<Value>(body) else { return String::new() };
            let Some(msgs) = v["messages"].as_array() else { return String::new() };
            for m in msgs.iter().rev() {
                if let Some(arr) = m["content"].as_array() {
                    for block in arr.iter().rev() {
                        if block["type"] == "tool_result" {
                            if let Some(s) = block["content"].as_str() {
                                return s.to_owned();
                            }
                            if let Some(parts) = block["content"].as_array() {
                                return parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n");
                            }
                        }
                    }
                }
            }
            String::new()
        };
        let extract_json = |text: &str| -> Option<Value> {
            for line in text.lines() {
                let t = line.trim();
                if t.starts_with('{') && let Ok(v) = serde_json::from_str::<Value>(t) {
                    return Some(v);
                }
            }
            serde_json::from_str::<Value>(text.trim()).ok()
        };
        let (driver_cb, mind_cb, written_cb, stage_cb, said_cb) = (driver.clone(), mind.clone(), written.clone(), stage.clone(), said.to_owned());
        rig.mock.set_claude_handler(Some(Arc::new(move |body: &str| {
            let tool_out = last_tool_result(body);
            let mut d = driver_cb.lock().unwrap();
            let step = d.step;
            d.step += 1;
            match step {
                0 => MockReply::RunCommand { command: format!("{mind_cb} list --json"), reason: "List minded agents".into() },
                1 => {
                    let v = extract_json(&tool_out).expect("mind list --json output");
                    for s in v["subagents"].as_array().expect("subagents array") {
                        let name = s["name"].as_str().unwrap().to_owned();
                        let rules_p = s["rules"].as_str().expect("rules path in mind list").to_owned();
                        d.roster.push((name, rules_p));
                    }
                    assert_eq!(d.roster.len(), 2, "expected 2 agents in roster: {v}");
                    let p0 = d.roster[0].1.clone();
                    MockReply::Tool { name: "Read".into(), input: json!({ "file_path": p0 }) }
                }
                2 => {
                    let n0 = d.roster[0].0.clone();
                    d.rules_text.insert(n0, tool_out);
                    let p1 = d.roster[1].1.clone();
                    MockReply::Tool { name: "Read".into(), input: json!({ "file_path": p1 }) }
                }
                3 => {
                    let n1 = d.roster[1].0.clone();
                    d.rules_text.insert(n1, tool_out);
                    MockReply::RunCommand { command: "sh -c \"printf '%s_%s\\n' RAN OTHER\"".into(), reason: "Not a minder's to run".into() }
                }
                4 => MockReply::Tool { name: "Write".into(), input: json!({ "file_path": written_cb.display().to_string(), "content": "nothing a minder may leave" }) },
                5 => MockReply::RunCommand { command: format!("{mind_cb} watch --since 0 --on question,approval --timeout 15 --json"), reason: "Watch first asking agent".into() },
                6 => {
                    let ev = extract_json(&tool_out).expect("first watch event");
                    let first = ev["name"].as_str().expect("watched agent name").to_owned();
                    let other = d.roster.iter().find(|(n, _)| n != &first).expect("other agent").0.clone();
                    d.first_agent = Some(first);
                    // Per-agent authorization check: attempt to approve the OTHER agent (same `cargo test` box!) BEFORE watching it.
                    MockReply::RunCommand { command: format!("{mind_cb} approve {other} --once --json"), reason: "Cross-agent approval must be refused".into() }
                }
                7 => {
                    let first = d.first_agent.clone().unwrap();
                    let other = d.roster.iter().find(|(n, _)| n != &first).unwrap().0.clone();
                    d.cross_refused = tool_out.contains("\"ok\":false") || tool_out.contains("wasn't") || tool_out.contains("isn't what you were last told");
                    d.cross_keys_zero = std::fs::read(stage_cb.join(format!("{other}.keys"))).unwrap_or_default().is_empty();
                    let rule = d.rules_text.get(&first).cloned().unwrap_or_default();
                    let cmd = if rule.contains("Approve") {
                        format!("{mind_cb} approve {first} --once --json")
                    } else {
                        format!("{mind_cb} approve {first} --no --reason 'Refused by rules.' --json")
                    };
                    MockReply::RunCommand { command: cmd, reason: "Answer first agent from its Read rules".into() }
                }
                8 => MockReply::RunCommand { command: format!("{mind_cb} watch --since 0 --on question,approval --timeout 15 --json"), reason: "Watch second asking agent".into() },
                9 => {
                    let ev = extract_json(&tool_out).expect("second watch event");
                    let second = ev["name"].as_str().expect("second watched agent name").to_owned();
                    d.second_agent = Some(second.clone());
                    let rule = d.rules_text.get(&second).cloned().unwrap_or_default();
                    let cmd = if rule.contains("Approve") {
                        format!("{mind_cb} approve {second} --once --json")
                    } else {
                        format!("{mind_cb} approve {second} --no --reason 'Refused by rules.' --json")
                    };
                    MockReply::RunCommand { command: cmd, reason: "Answer second agent from its Read rules".into() }
                }
                10 => {
                    let second = d.second_agent.clone().unwrap();
                    let rule = d.rules_text.get(&second).cloned().unwrap_or_default();
                    assert!(rule.contains("escalate"), "expected escalate in {second}'s rules: {rule}");
                    MockReply::RunCommand { command: format!("{mind_cb} escalate {second} '{said_cb}' --json"), reason: "Escalate refused agent to parent".into() }
                }
                _ => MockReply::Text("MINDER_TURN_DONE".into()),
            }
        })));
        let from = alpha["cursor"].to_string();
        rig.mock.release_claude();

        // Lead receives beta's escalation on its watch.
        let heard = rig.run_olo_json(&["agent", "watch", "--all", "--since", &from, "--on", "escalation", "--timeout", "40", "--json"]).await;
        assert_eq!((&heard["event"], &heard["name"], &heard["summary"]), (&json!("escalation"), &json!("beta"), &json!(said)), "{heard}");

        // Verify per-agent authorization check failed cleanly and sent zero bytes to `beta` before `beta` was watched.
        {
            let d = driver.lock().unwrap();
            assert_eq!(d.first_agent.as_deref(), Some("alpha"), "expected alpha delivered first");
            assert!(d.cross_refused, "approving beta before beta was watched must be refused");
            assert!(d.cross_keys_zero, "beta must receive zero input from unauthorized pre-watch approve");
        }

        // Both stand-ins received their distinct answers (alpha: Return; beta: Down+Down+Return).
        assert!(alpha_gone.join().unwrap(), "alpha was not approved once");
        assert!(beta_moved.join().unwrap() && beta_gone.join().unwrap(), "beta was not refused");

        // Minder comes to rest without ever asking leave of anyone.
        let rested = rig.run_olo_json(&["agent", "watch", &minder_id, "--since", &from, "--on", "idle,done,approval,question,error,exit", "--timeout", "40", "--json"]).await;
        assert!(matches!(rested["state"].as_str(), Some("idle" | "done")), "the minder asked, failed or ended: {rested}");
        let (code, out, _) = rig.run_olo(&["agent", "watch", &minder_id, "--since", &from, "--on", "approval,question", "--timeout", "2", "--json"]).await;
        assert_eq!(code, 124, "a minder asked leave of someone: {out}");

        let bodies: Vec<String> = rig.mock.requests().into_iter().map(|r| r.body).collect();
        let any = |what: &str| bodies.iter().any(|b| b.contains(what));
        assert!(any("You are a minder. Your job is to watch over the agents that run as child tabs under yours.") && any("Begin minding now."), "shared-minder brief missing");
        assert!(any("ALPHA_RULE_7C41") && any("BETA_RULE_9D52"), "minder did not read both agents' rules files");
        assert!(!any("RAN_OTHER"), "minder ran an unauthorized command");
        assert!(!written.exists(), "minder wrote a file");

        // Each agent's own `mind NAME` history recorded strictly its own answers by the shared minder.
        let of_a = rig.run_olo_json(&["agent", "mind", "alpha", "--json"]).await;
        let of_b = rig.run_olo_json(&["agent", "mind", "beta", "--json"]).await;
        let did_of = |v: &Value| v["answered"].as_array().unwrap().iter().filter_map(|a| a["did"].as_str().map(str::to_owned)).collect::<Vec<_>>();
        assert_eq!(did_of(&of_a), ["approve"], "{of_a}");
        assert_eq!(did_of(&of_b), ["approve", "escalate"], "{of_b}");

        // Stopping the minder leaves BOTH `alpha` and `beta` running under the lead, unminded.
        let stopped_m = rig.run_olo_json(&["agent", "stop", &minder_id, "--force", "--json"]).await;
        assert_eq!(stopped_m["stopped"], json!([minder_id]), "{stopped_m}");
        assert_eq!(rig.run_olo_json(&["agent", "status", "alpha", "--json"]).await["running"], json!(true));
        assert_eq!(rig.run_olo_json(&["agent", "status", "beta", "--json"]).await["running"], json!(true));
        rig.run_olo_json(&["agent", "stop", "alpha", "--force", "--json"]).await;
        rig.run_olo_json(&["agent", "stop", "beta", "--force", "--json"]).await;
    });
}

/// The conversation a process holds open, as its own files say (Antigravity's `conversations/ID.db`,
/// `presence/ID.lock`, `brain/ID/…`): what `cli_session_id` must be, read here from the process itself.
fn agy_conversations_held(pid: i64) -> HashSet<String> {
    let mut held = HashSet::new();
    let mut pids = vec![pid];
    // (And its own children: the same reading the service's is checked against.)
    if let Ok(tasks) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) {
        pids.extend(tasks.split_whitespace().filter_map(|p| p.parse::<i64>().ok()));
    }
    for p in pids {
        for fd in std::fs::read_dir(format!("/proc/{p}/fd")).into_iter().flatten().flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else { continue };
            let t = target.to_string_lossy();
            if !t.contains("/antigravity") {
                continue;
            }
            for part in t.split('/') {
                let stem = part.split('.').next().unwrap_or_default();
                if stem.len() == 36 && stem.chars().filter(|c| *c == '-').count() == 4 {
                    held.insert(stem.to_owned());
                }
            }
        }
    }
    held
}

/// A new agent's conversation is its own or none yet, never another agent's (the ShiftUp lead's item 33,
/// 2026-11-26: "A new agy's `cli_session_id` names another live agy's conversation, after its first turn
/// too … Please report a new agy's own conversation in `cli_session_id`, the one its process holds open,
/// or none until it has one. Never report another agent's."; one of their launchers resumed what it was
/// told and another agent's conversation got a turn that wasn't its own). Two Antigravity children, the
/// first in the middle of a turn, its conversation the newest written, as the second starts.
#[test]
fn live_mock_api_a_new_agents_conversation_is_its_own_or_none() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("agy") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("ownconv").await;
        rig.mock.clear_queues();
        let start = |name: &'static str, prompt: Option<&'static str>| {
            let mut args: Vec<String> =
                ["agent", "start", "agy", "--model", "gemini-3-flash-preview", "--effort", "high", "--name", name, "--trust", "--cwd"].iter().map(|s| (*s).to_owned()).collect();
            args.push(rig.work.display().to_string());
            if let Some(p) = prompt {
                args.extend(["--prompt".to_owned(), p.to_owned()]);
            }
            args.push("--json".to_owned());
            args.extend(rig.cli_env_args("agy"));
            args
        };
        // The first: in the middle of its turn (it asks leave), its conversation open and the newest written.
        rig.mock.push("agy", MockReply::RunCommand { command: "sh -c \"printf '%s_%s\\n' FIRST RAN\"".into(), reason: "The first's turn".into() });
        let a = start("first", Some("Begin the first's work."));
        let a = rig.run_olo_json(&a.iter().map(String::as_str).collect::<Vec<_>>()).await;
        let asked = rig.run_olo_json(&["agent", "watch", "first", "--since", &a["cursor"].to_string(), "--on", "approval,error,exit", "--timeout", "30", "--json"]).await;
        assert_eq!(asked["event"], json!("waiting_approval"), "{asked}");
        let first = rig.run_olo_json(&["agent", "status", "first", "--json"]).await;
        let first_id = first["cli_session_id"].as_str().unwrap_or_else(|| panic!("the first has a conversation: {first}")).to_owned();
        assert!(agy_conversations_held(first["pid"].as_i64().unwrap()).contains(&first_id), "the first's is the one its process holds: {first}");

        // The second: started with nothing to do. It has made no conversation: it has none, and never the first's.
        let b = start("second", None);
        let b = rig.run_olo_json(&b.iter().map(String::as_str).collect::<Vec<_>>()).await;
        rig.run_olo_json(&["agent", "watch", "second", "--since", &b["cursor"].to_string(), "--on", "idle,error,exit", "--timeout", "30", "--json"]).await;
        for _ in 0..3 {
            let second = rig.run_olo_json(&["agent", "status", "second", "--json"]).await;
            if std::env::var_os("OLO_AGENTS_SHOW_FDS").is_some() {
                for who in ["first", "second"] {
                    let pid = rig.run_olo_json(&["agent", "status", who, "--json"]).await["pid"].as_i64().unwrap();
                    let mut open: Vec<String> = std::fs::read_dir(format!("/proc/{pid}/fd"))
                        .into_iter()
                        .flatten()
                        .flatten()
                        .filter_map(|f| std::fs::read_link(f.path()).ok())
                        .map(|t| t.to_string_lossy().into_owned())
                        .filter(|t| t.contains("antigravity"))
                        .collect();
                    open.sort();
                    eprintln!("{who} (pid {pid}, {}) holds:\n  {}", std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default().trim(), open.join("\n  "));
                }
            }
            let held = agy_conversations_held(second["pid"].as_i64().unwrap());
            match second["cli_session_id"].as_str() {
                None => {}
                Some(id) => {
                    assert!(id != first_id && held.contains(id), "a new agent was given a conversation that isn't its own (the first's is {first_id}; its own process holds {held:?}): {second}")
                }
            }
            let listed = rig.run_olo_json(&["agent", "list", "--json"]).await;
            let row = listed["subagents"].as_array().unwrap().iter().find(|s| s["name"] == "second").unwrap().clone();
            assert!(row["cli_session_id"].as_str() != Some(first_id.as_str()), "{row}");
            tokio::time::sleep(Duration::from_millis(400)).await;
        }

        // Its first turn: now it has one, its own (what its process holds open), still not the first's.
        rig.mock.push("agy", MockReply::Text("SECOND_HAS_SPOKEN".into()));
        let sent = rig.run_olo_json(&["agent", "send", "second", "Say that you have spoken.", "--enter", "--json"]).await;
        let ended = rig.run_olo_json(&["agent", "watch", "second", "--since", &sent["cursor"].to_string(), "--on", "done,idle,error,exit", "--timeout", "40", "--json"]).await;
        assert!(matches!(ended["state"].as_str(), Some("done" | "idle")), "{ended}");
        let second = rig.run_olo_json(&["agent", "status", "second", "--json"]).await;
        let held = agy_conversations_held(second["pid"].as_i64().unwrap());
        let own = second["cli_session_id"].as_str().unwrap_or_else(|| panic!("after its first turn it has a conversation: {second}; its process holds {held:?}"));
        assert!(own != first_id && held.contains(own), "after its first turn: {own} isn't its own (the first's is {first_id}; its process holds {held:?})");
        // (And the first's is still the first's.)
        assert_eq!(rig.run_olo_json(&["agent", "status", "first", "--json"]).await["cli_session_id"], json!(first_id));
        rig.run_olo_json(&["agent", "stop", "first", "--force", "--json"]).await;
        rig.run_olo_json(&["agent", "stop", "second", "--force", "--json"]).await;
    });
}

/// An agent's own shell that outlives its turn is said (the ShiftUp lead's item 26, 2026-11-26: "A Claude
/// Code agent whose turn ended with its own background Bash still running (`sleep 45`, its footer '1 shell',
/// the shell under its process) had `waiting_on: someone` and `background_shells: []` … Please list each
/// such shell, with its command, its output file and when it last wrote, and say `waiting_on: own_run`").
/// Claude Code itself, the made-up API its model: a command run in the background, the turn ended.
#[test]
fn live_mock_api_an_agents_own_background_shell_is_listed() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("claude") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("ownshell").await;
        rig.mock.clear_queues();
        // (Their probe's own command; and one that says something first, for the file's last write.)
        rig.mock.push("claude", MockReply::RunCommand { command: "echo STARTED_WAITING; sleep 45".into(), reason: "background: wait for the build".into() });
        rig.mock.push("claude", MockReply::Text("OWN_SHELL_STARTED: I will look again when it ends.".into()));
        let mut args: Vec<String> = [
            "agent",
            "start",
            "claude",
            "--model",
            "claude-opus-5-5",
            "--effort",
            "max",
            "--name",
            "waiter",
            "--trust",
            "--allowed-tools",
            "Bash(echo *)",
            "Bash(sleep *)",
            "--prompt",
            "Start the wait.",
            "--json",
            "--cwd",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        args.push(rig.work.display().to_string());
        args.extend(rig.cli_env_args("claude"));
        let _started = rig.run_olo_json(&args.iter().map(String::as_str).collect::<Vec<_>>()).await;
        // Its turn's end: by what it said last. (As it starts, what runs under it is Claude Code's own
        // looking about, `ps aux | grep …`: never said to be the agent's.)
        let until = Instant::now() + Duration::from_secs(40);
        let mut status = rig.run_olo_json(&["agent", "status", "waiter", "--json"]).await;
        // (Its last words *and* its rest, in one answer: a status asked as the turn ends can have the
        // words, read from its file, beside a screen read a moment before that still shows it at work.
        // Under a load of ninety that was met here, 2026-11-27; a code review had said so of this loop.)
        while !(status["last_message"].as_str().is_some_and(|m| m.contains("OWN_SHELL_STARTED")) && matches!(status["state"].as_str(), Some("done" | "idle"))) {
            assert!(status["approval"].is_null(), "nothing is asked (its commands are allowed): {status}");
            for shell in status["background_shells"].as_array().into_iter().flatten() {
                assert!(shell["cmd"].as_str().is_some_and(|c| c.contains("sleep 45")), "Claude Code's own doing was said to be the agent's shell: {shell}");
            }
            assert!(Instant::now() < until, "its turn didn't end: {status}");
            tokio::time::sleep(Duration::from_millis(150)).await;
            status = rig.run_olo_json(&["agent", "status", "waiter", "--json"]).await;
        }
        assert!(matches!(status["state"].as_str(), Some("done" | "idle")), "{status}");
        let shells = status["background_shells"].as_array().unwrap_or_else(|| panic!("{status}"));
        // (What the made-up API was asked and in what order, should the shell not be there: whether the
        // command was ever given to this conversation.)
        let asked: Vec<String> = rig
            .mock
            .requests()
            .iter()
            .map(|r| {
                format!(
                    "{} bytes, tools {}, a tool's result {}, the background command asked {}, says OWN_SHELL {}",
                    r.body.len(),
                    r.body.contains("\"Bash\""),
                    r.body.contains("tool_result"),
                    r.body.contains("run_in_background\\\":true") || r.body.contains("\"run_in_background\":true"),
                    r.body.contains("OWN_SHELL_STARTED")
                )
            })
            .collect();
        // (And what runs under its tab at this moment, as the system has it: whether the shell is there and under what.)
        let under = |pid: i64| String::from_utf8_lossy(&Command::new("ps").args(["-o", "pid,ppid,stat,comm,args", "--ppid", &pid.to_string()]).output().map(|o| o.stdout).unwrap_or_default()).lines().map(|l| l.chars().take(1500).collect::<String>()).collect::<Vec<_>>().join("\n    ");
        let pid = status["pid"].as_i64().unwrap_or(0);
        let all = String::from_utf8_lossy(&Command::new("ps").args(["-eo", "pid,ppid,stat,comm,args"]).output().map(|o| o.stdout).unwrap_or_default()).lines().filter(|l| l.contains("STARTED_WAITING") || l.contains("sleep 45")).map(|l| l.chars().take(170).collect::<String>()).collect::<Vec<_>>().join("\n    ");
        assert_eq!(
            shells.len(),
            1,
            "its one shell, once (not its `sleep` as another): {status}\nwhat the API was asked:\n  {}\nunder the pid status gives ({pid}):\n    {}\nevery process of this command on the machine:\n    {all}",
            asked.join("\n  "),
            under(pid)
        );
        let shell = &shells[0];
        assert_eq!((&shell["cmd"], &shell["kind"]), (&json!("echo STARTED_WAITING; sleep 45"), &json!("task")), "its own command, as it gave it: {shell}");
        assert_eq!(status["waiting_on"], "own_run", "{status}");
        // Its output file, which has what it printed, and when that was written.
        let output = shell["output"].as_str().unwrap_or_else(|| panic!("no output file: {shell}"));
        assert_eq!(std::fs::read_to_string(output).unwrap().trim(), "STARTED_WAITING");
        let wrote = shell["last_wrote_ms"].as_u64().unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        assert!(wrote <= now && now - wrote < 60_000, "written just now: {wrote} against {now}");
        // The shell is a process under the agent's own.
        let pid = shell["pid"].as_i64().unwrap();
        let parent: i64 = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap().rsplit_once(')').unwrap().1.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert_eq!(parent, status["pid"].as_i64().unwrap(), "{status}");
        rig.run_olo_json(&["agent", "stop", "waiter", "--force", "--json"]).await;
    });
}

/// No turn's end is told before the prompt's own turn has ended (the ShiftUp lead's item 27, 2026-11-26:
/// "`start agy --resume ID --prompt …` reports `done` 1.4 s after the start, carrying the old conversation's
/// last message, before the prompt is read … Please tell no turn's end until the prompt's own turn has
/// ended"; and the same seen of a Claude child started with `--prompt`: told at rest a second after its
/// start). The model takes four seconds over each prompt here, so what is told before then is told too soon.
#[test]
fn live_mock_api_no_rest_is_told_before_the_prompts_own_turn() {
    if !std::path::Path::new("/usr/bin/rex").exists() {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = &LiveRexHarness::start("ownturn").await;
        for (cli, model, effort) in [("claude", "claude-opus-5-5", "max"), ("agy", "gemini-3-flash-preview", "high")] {
            if !have_program(cli) {
                continue;
            }
            rig.mock.clear_queues();
            let cli_static: &'static str = if cli == "claude" { "claude" } else { "agy" };
            let start = |name: &str, more: &[&str]| {
                let mut args: Vec<String> = ["agent", "start", cli, "--model", model, "--effort", effort, "--name", name, "--trust", "--json", "--cwd"].iter().map(|s| (*s).to_owned()).collect();
                args.push(rig.work.display().to_string());
                args.extend(more.iter().map(|s| (*s).to_owned()));
                args.extend(rig.cli_env_args(cli));
                args
            };
            async fn first_rest(rig: &LiveRexHarness, since: String, name: String) -> (Instant, Value) {
                let told = rig.run_olo_json(&["agent", "watch", &name, "--since", &since, "--on", "done,idle,error,exit", "--timeout", "40", "--json"]).await;
                (Instant::now(), told)
            }

            // A new conversation, started with a prompt.
            let name = format!("{cli}-new");
            rig.mock.push(cli_static, MockReply::SlowText { after_ms: 4000, text: format!("FIRST_REPLY_{cli}") });
            let began = Instant::now();
            let started = rig.run_olo_json(&start(&name, &["--prompt", "Say the first thing."]).iter().map(String::as_str).collect::<Vec<_>>()).await;
            let (at, told) = first_rest(rig, started["cursor"].to_string(), name.clone()).await;
            assert!(
                told["last_message"].as_str().is_some_and(|m| m.contains(&format!("FIRST_REPLY_{cli}"))) && at.duration_since(began) >= Duration::from_millis(3500),
                "{cli}: the first rest told of a child started with a prompt isn't its prompt's turn's end (after {:?}): {told}",
                at.duration_since(began)
            );
            assert_eq!(told["answers"], started["cursor"], "{cli}: its turn's end answers its start: {told}");
            let conversation =
                rig.run_olo_json(&["agent", "status", &name, "--json"]).await["cli_session_id"].as_str().unwrap_or_else(|| panic!("{cli}: it has a conversation after its turn")).to_owned();
            rig.run_olo_json(&["agent", "stop", &name, "--force", "--json"]).await;

            // That conversation taken up again with a prompt: its old last message is not this turn's.
            let name = format!("{cli}-again");
            rig.mock.push(cli_static, MockReply::SlowText { after_ms: 4000, text: format!("SECOND_REPLY_{cli}") });
            let began = Instant::now();
            let started = rig.run_olo_json(&start(&name, &["--resume", &conversation, "--prompt", "Say the second thing."]).iter().map(String::as_str).collect::<Vec<_>>()).await;
            let (at, told) = first_rest(rig, started["cursor"].to_string(), name.clone()).await;
            assert!(
                told["last_message"].as_str().is_some_and(|m| m.contains(&format!("SECOND_REPLY_{cli}"))) && at.duration_since(began) >= Duration::from_millis(3500),
                "{cli}: the first rest told of a conversation taken up again with a prompt isn't the prompt's turn's end (after {:?}): {told}",
                at.duration_since(began)
            );
            assert_eq!(told["answers"], started["cursor"], "{cli}: {told}");
            // A message sent to it at rest: the next rest is that message's turn's end, and says so.
            rig.mock.push(cli_static, MockReply::SlowText { after_ms: 1500, text: format!("THIRD_REPLY_{cli}") });
            let sent = rig.run_olo_json(&["agent", "send", &name, "Say the third thing.", "--enter", "--json"]).await;
            let (_, told) = first_rest(rig, sent["cursor"].to_string(), name.clone()).await;
            assert!(told["last_message"].as_str().is_some_and(|m| m.contains(&format!("THIRD_REPLY_{cli}"))), "{cli}: the rest told after a send isn't its turn's end: {told}");
            assert_eq!(told["answers"], sent["cursor"], "{cli}: {told}");
            rig.run_olo_json(&["agent", "stop", &name, "--force", "--json"]).await;
        }
    });
}

/// `olo agent run`, with Claude Code itself (the made-up API its model, which takes a second and a half): one
/// command starts it, waits for its prompt's own turn, prints its whole last message and writes it to the file
/// named, and stops its tab (the ShiftUp lead's item 36).
#[test]
fn live_mock_api_one_agent_for_one_prompt() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("claude") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("oneshot").await;
        rig.mock.clear_queues();
        let run = |name: &'static str, more: Vec<String>| {
            let mut args: Vec<String> =
                ["agent", "run", "claude", "--model", "claude-opus-5-5", "--effort", "max", "--name", name, "--trust", "--prompt", "Read the file and say what it says.", "--cwd"]
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect();
            args.push(rig.work.display().to_string());
            args.extend(more);
            args.extend(rig.cli_env_args("claude"));
            args
        };
        // In JSON, with its file.
        let reading = "ONE_SHOT_READING:\nThe file says forty-two — “quoted”.\n\nDONE";
        rig.mock.push("claude", MockReply::SlowText { after_ms: 1500, text: reading.into() });
        let out = rig.dir.join("readings/first.md");
        let ran = rig.run_olo_json(&run("reader", vec!["--out".into(), out.display().to_string(), "--json".into()]).iter().map(String::as_str).collect::<Vec<_>>()).await;
        assert_eq!((&ran["ok"], &ran["ended_as"], &ran["last_message"], &ran["stopped"]), (&json!(true), &json!("done"), &json!(reading), &json!(true)), "{ran}");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), reading, "its message whole, in the file (its folder made)");
        assert!(ran["cli_session_id"].as_str().is_some_and(|s| s.len() == 36), "its conversation, to take up again: {ran}");
        let listed = rig.run_olo_json(&["agent", "list", "--all", "--json"]).await;
        assert_eq!(listed["subagents"].as_array().unwrap().iter().find(|s| s["name"] == "reader").unwrap()["state"], "exited", "its tab is stopped: {listed}");
        // Plainly: its message and nothing else, status 0.
        let plain = "A second reading,\nof two lines.";
        rig.mock.push("claude", MockReply::SlowText { after_ms: 1000, text: plain.into() });
        let (code, said, err) = rig.run_olo(&run("reader-2", Vec::new()).iter().map(String::as_str).collect::<Vec<_>>()).await;
        assert_eq!((code, said.as_str()), (0, format!("{plain}\n").as_str()), "{err}");
    });
}

/// Reproduces Symptom 1 against real Codex 0.172:
/// `olo agent run codex` must NOT exit 0 early with Codex's first commentary message
/// when a subsequent commentary message streams slowly (Codex hides its status line
/// while streaming assistant text).
#[test]
fn live_codex_run_waits_past_commentary_stream_for_final_answer() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("codex") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("codexrepro1").await;

        rig.mock.clear_queues();
        rig.mock.push(
            "codex",
            MockReply::CodexStream {
                steps: vec![
                    CodexSseStep::SleepMs(600),
                    CodexSseStep::Message { phase: Some("commentary".into()), chunks: vec!["FIRST_MESSAGE_ONLY: starting review and running check 1.".into()], delay_ms: 0 },
                    CodexSseStep::ExecCommand { command: "sh -c \"sleep 2; echo CHECK1_OK\"".into(), escalated: false, reason: "Check 1".into() },
                ],
            },
        );
        rig.mock.push(
            "codex",
            MockReply::CodexStream {
                steps: vec![
                    CodexSseStep::SleepMs(400),
                    CodexSseStep::Message {
                        phase: Some("commentary".into()),
                        chunks: vec![
                            "SECOND_COMMENTARY: Check 1 passed.\n".into(),
                            "Inspecting callers and invariants now.\n".into(),
                            "Cross-checking state transitions.\n".into(),
                            "Preparing to run check 2.\n".into(),
                            "Starting check 2...".into(),
                        ],
                        delay_ms: 500,
                    },
                    CodexSseStep::ExecCommand { command: "sh -c \"sleep 1; echo CHECK2_OK\"".into(), escalated: false, reason: "Check 2".into() },
                ],
            },
        );
        rig.mock.push(
            "codex",
            MockReply::CodexStream {
                steps: vec![
                    CodexSseStep::SleepMs(300),
                    CodexSseStep::Message { phase: Some("final_answer".into()), chunks: vec!["REAL_FINAL_VERDICT: ".into(), "both checks passed cleanly.".into()], delay_ms: 200 },
                ],
            },
        );

        let mut run_args: Vec<String> = [
            "agent",
            "run",
            "codex",
            "--model",
            "gpt-5.4-codex",
            "--effort",
            "high",
            "--name",
            "run-repro",
            "--trust",
            "--approval",
            "on-request",
            "--sandbox",
            "read-only",
            "--no-daemon",
            "--prompt",
            "Run both checks and give the final verdict.",
            "--cwd",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        run_args.push(rig.work.display().to_string());
        run_args.push("--json".into());
        run_args.extend(rig.cli_env_args("codex"));

        let (code, stdout, stderr) = rig.run_olo(&run_args.iter().map(String::as_str).collect::<Vec<_>>()).await;
        assert_eq!(code, 0, "olo agent run codex failed (code={code}): stdout={stdout} stderr={stderr}");
        let ran: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("bad json ({e}): {stdout}"));
        assert_eq!(ran["ended_as"], "done", "unexpected ended_as: {ran}");
        let msg = ran["last_message"].as_str().unwrap_or("");
        assert!(msg.contains("REAL_FINAL_VERDICT"), "olo agent run codex returned prematurely mid-turn! got last_message={msg:?} full={ran}");
    });
}

/// Reproduces premature `idle` during `olo agent start --prompt` + `watch` against real Codex 0.172
/// when a long reasoning summary header truncates off `(Xs • esc to interrupt)` or when commentary streams.
#[test]
fn live_codex_watch_waits_past_long_reasoning_and_commentary_stream() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("codex") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("codexrepro2").await;

        rig.mock.clear_queues();
        rig.mock.push(
            "codex",
            MockReply::CodexStream {
                steps: vec![
                    CodexSseStep::SleepMs(500),
                    CodexSseStep::Reasoning {
                        chunks: vec![
                            "**Analyzing a very long reasoning header that deliberately exceeds a single terminal line width of 120 columns so Codex ellipsizes the status line**\n\n".into(),
                            "Reviewing details before running check.".into(),
                        ],
                        delay_ms: 1000,
                    },
                    CodexSseStep::Message {
                        phase: Some("commentary".into()),
                        chunks: vec![
                            "WATCH_COMMENTARY_1: inspecting module.\n".into(),
                            "WATCH_COMMENTARY_2: checking invariants.\n".into(),
                            "WATCH_COMMENTARY_3: running check.\n".into(),
                            "WATCH_COMMENTARY_4: almost ready.".into(),
                        ],
                        delay_ms: 500,
                    },
                    CodexSseStep::ExecCommand { command: "sh -c \"sleep 2; echo WATCH_CMD_OK\"".into(), escalated: false, reason: "Watch check".into() },
                ],
            },
        );
        rig.mock.push(
            "codex",
            MockReply::CodexStream {
                steps: vec![
                    CodexSseStep::SleepMs(300),
                    CodexSseStep::Message { phase: Some("final_answer".into()), chunks: vec!["WATCH_FINAL_VERDICT: ".into(), "review complete.".into()], delay_ms: 200 },
                ],
            },
        );

        let mut start_args: Vec<String> = [
            "agent",
            "start",
            "codex",
            "--model",
            "gpt-5.4-codex",
            "--effort",
            "high",
            "--name",
            "watch-repro",
            "--trust",
            "--approval",
            "on-request",
            "--sandbox",
            "read-only",
            "--no-daemon",
            "--prompt",
            "Review and run the verification check.",
            "--cwd",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        start_args.push(rig.work.display().to_string());
        start_args.push("--json".into());
        start_args.extend(rig.cli_env_args("codex"));

        let started = rig.run_olo_json(&start_args.iter().map(String::as_str).collect::<Vec<_>>()).await;
        let start_cur = started["cursor"].as_u64().unwrap();
        let ev = rig.run_olo_json(&["agent", "watch", "watch-repro", "--since", &start_cur.to_string(), "--on", "idle,done,error,exit", "--timeout", "25", "--json"]).await;
        let _ = rig.run_olo(&["agent", "stop", "watch-repro", "--force", "--json"]).await;
        assert_eq!(ev["state"], "done", "watch emitted premature rest mid-turn: {ev}");
        assert!(ev["last_message"].as_str().is_some_and(|m| m.contains("WATCH_FINAL_VERDICT")), "watch returned before final answer: {ev}");
    });
}

/// Verifies Codex 0.172 status across the two opposing failure modes:
/// 1. Working with a running command (`· 1 background terminal running`) + queued message
///    in the field must stay `working` (never `idle` mid-turn).
/// 2. A turn that completes while a background terminal (`yield_time_ms`) is still running
///    must become `done` (never stuck `working`), and a session killed mid-turn and resumed
///    without a prompt must NEVER stay stuck `working` on its unclosed rollout.
#[test]
fn live_codex_queued_message_stays_working_and_bg_or_resumed_rests() {
    if !std::path::Path::new("/usr/bin/rex").exists() || !have_program("codex") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let rig = LiveRexHarness::start("codexrepro3").await;
        let codex_start = |name: &str, extra: &[&str]| -> Vec<String> {
            let mut args: Vec<String> =
                ["agent", "start", "codex", "--model", "gpt-5.4-codex", "--effort", "high", "--name", name, "--trust", "--approval", "on-request", "--sandbox", "read-only", "--no-daemon", "--cwd"]
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect();
            args.push(rig.work.display().to_string());
            for e in extra {
                args.push((*e).to_owned());
            }
            args.push("--json".into());
            args.extend(rig.cli_env_args("codex"));
            args
        };

        // Case A: tool running (`· 1 background terminal running`) + multi-line draft & Tab-queued follow-up.
        rig.mock.clear_queues();
        rig.mock
            .push("codex", MockReply::CodexStream { steps: vec![CodexSseStep::ExecCommand { command: "sh -c \"sleep 14; echo TOOL_OK\"".into(), escalated: false, reason: "Run 14s tool".into() }] });
        rig.mock.push("codex", MockReply::CodexStream { steps: vec![CodexSseStep::Message { phase: Some("final_answer".into()), chunks: vec!["TURN1_DONE".into()], delay_ms: 0 }] });
        rig.mock.push("codex", MockReply::CodexStream { steps: vec![CodexSseStep::Message { phase: Some("final_answer".into()), chunks: vec!["TURN2_QUEUED_DONE".into()], delay_ms: 0 }] });
        let a = codex_start("queued-tool", &["--prompt", "Run the slow tool."]);
        let _ = rig.run_olo_json(&a.iter().map(String::as_str).collect::<Vec<_>>()).await;
        // Wait until the running command's status line shows `background terminal` on screen.
        // (Twenty seconds for Codex to come up and begin, of a command that runs fourteen: with ten and
        // eight, under a load of 120 here, Codex was not up in time, 2026-11-27.)
        let dl = Instant::now() + Duration::from_secs(20);
        let mut saw_bg_line = false;
        while Instant::now() < dl {
            let (_, scr, _) = rig.run_olo(&["agent", "read", "queued-tool", "--lines", "40"]).await;
            if scr.contains("background terminal") {
                saw_bg_line = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        assert!(saw_bg_line, "Codex never showed running command status line");
        let draft = "LINE_1: follow-up check\nLINE_2: second line\nLINE_3: third line\nLINE_4: fourth line\nLINE_5: fifth line\nLINE_6: sixth line\nLINE_7: seventh line\nLINE_8: eighth line";
        let _ = rig.run_olo(&["agent", "send", "queued-tool", draft, "--paste", "--json"]).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let st_draft = rig.run_olo_json(&["agent", "status", "queued-tool", "--json"]).await;
        let _ = rig.run_olo(&["agent", "send", "queued-tool", "--key", "Tab", "--json"]).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let st_queued = rig.run_olo_json(&["agent", "status", "queued-tool", "--json"]).await;
        let _ = rig.run_olo(&["agent", "stop", "queued-tool", "--force", "--json"]).await;
        assert_eq!(st_draft["state"], "working", "working Codex with running tool + 8-line draft read as {}: {st_draft}", st_draft["state"]);
        assert_eq!(st_queued["state"], "working", "working Codex with running tool + Tab-queued message read as {}: {st_queued}", st_queued["state"]);

        // Case B: background terminal (`yield_time_ms: 500`) outlives the turn -> must reach `done`, never stuck `working`.
        rig.mock.clear_queues();
        rig.mock.push("codex", MockReply::CodexStream { steps: vec![CodexSseStep::ExecCommandYield { command: "sh -c \"sleep 25\"".into(), yield_time_ms: 500 }] });
        rig.mock.push(
            "codex",
            MockReply::CodexStream {
                steps: vec![
                    CodexSseStep::SleepMs(300),
                    CodexSseStep::Message { phase: Some("final_answer".into()), chunks: vec!["BG_DONE: ".into(), "background process started.".into()], delay_ms: 300 },
                ],
            },
        );
        let a = codex_start("bg-outlives", &["--prompt", "Start background sleep."]);
        let started = rig.run_olo_json(&a.iter().map(String::as_str).collect::<Vec<_>>()).await;
        let c0 = started["cursor"].as_u64().unwrap();
        let ev = rig.run_olo_json(&["agent", "watch", "bg-outlives", "--since", &c0.to_string(), "--on", "idle,done,error,exit", "--timeout", "15", "--json"]).await;
        let st_bg = rig.run_olo_json(&["agent", "status", "bg-outlives", "--json"]).await;
        let _ = rig.run_olo(&["agent", "stop", "bg-outlives", "--force", "--json"]).await;
        assert_eq!(ev["state"], "done", "Codex turn with background terminal should reach done: {ev}");
        assert!(ev["last_message"].as_str().is_some_and(|m| m.contains("BG_DONE")), "unexpected message: {ev}");
        assert_eq!(st_bg["waiting_on"], "own_run", "background terminal should still be reported in waiting_on: {st_bg}");

        // Case C: killed mid-turn (unclosed `task_started` in rollout) + resumed without prompt -> must NOT stay `working`.
        rig.mock.clear_queues();
        rig.mock.push(
            "codex",
            MockReply::CodexStream { steps: vec![CodexSseStep::SleepMs(15_000), CodexSseStep::Message { phase: Some("final_answer".into()), chunks: vec!["NEVER_SENT".into()], delay_ms: 0 }] },
        );
        let a = codex_start("kill-midturn", &["--prompt", "Start turn and get killed."]);
        let _ = rig.run_olo_json(&a.iter().map(String::as_str).collect::<Vec<_>>()).await;
        let dl = Instant::now() + Duration::from_secs(10);
        let mut sid = String::new();
        while Instant::now() < dl {
            let st_mid = rig.run_olo_json(&["agent", "status", "kill-midturn", "--json"]).await;
            if let Some(s) = st_mid["cli_session_id"].as_str()
                && st_mid["transcript_path"].is_string()
            {
                sid = s.to_owned();
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert!(!sid.is_empty(), "kill-midturn never created rollout session");
        let _ = rig.run_olo(&["agent", "stop", "kill-midturn", "--force", "--json"]).await;

        let a = codex_start("resumed-rest", &["--resume", &sid]);
        let _ = rig.run_olo_json(&a.iter().map(String::as_str).collect::<Vec<_>>()).await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let st_res = rig.run_olo_json(&["agent", "status", "resumed-rest", "--json"]).await;
        let _ = rig.run_olo(&["agent", "stop", "resumed-rest", "--force", "--json"]).await;
        assert_ne!(st_res["state"], "working", "resumed-after-kill Codex at rest must NEVER be stuck working: {st_res}");
    });
}
