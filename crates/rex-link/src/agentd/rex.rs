//! The Rex server of this host, as the agents service speaks to it: its
//! control connection over the server's own Unix socket
//! (`rex.control.v1`: requests, their answers, events), and a
//! terminal's stream (`rex.data.terminal.v1`), read only for *that*
//! something was written and for the word that its program ended. What
//! a terminal shows is asked of the server itself
//! ([`Rex::screen`]): the server's own reading of it, the one the Olo
//! desktop draws, never a second emulator's.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::http;

/// The API revision spoken (the server's own, 19, and whatever it still
/// takes below that).
const API_REVISION: u32 = 19;
const CONTROL: &str = "rex.control.v1";
const TERMINAL: &str = "rex.data.terminal.v1";
const ANSWER_WITHIN: Duration = Duration::from_secs(30);
/// The most of a terminal's text asked for at once: a screen is a few
/// kilobytes (the agents draw on the alternate screen, which has no
/// history), and what is longer is a main screen's history, of which
/// only the end is wanted ([`Rex::screen`]).
const SCREEN_MOST: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub enum RexErr {
    /// The server's own refusal: its code and what it said.
    Api {
        code: String,
        message: String,
    },
    Closed(String),
    Timeout,
    Protocol(String),
}

impl std::fmt::Display for RexErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api { code, message } => write!(f, "{message} ({code})"),
            Self::Closed(why) => write!(f, "the Rex server's connection ended: {why}"),
            Self::Timeout => f.write_str("the Rex server didn't answer in time"),
            Self::Protocol(what) => write!(f, "the Rex server said something unexpected: {what}"),
        }
    }
}

impl RexErr {
    /// Whether the server said the thing asked about is gone (a closed
    /// tab, an ended session).
    pub fn gone(&self) -> bool {
        matches!(self, Self::Api { code, .. } if matches!(code.as_str(), "not_found" | "session_not_found" | "block_not_found" | "window_not_found"))
    }
}

/// What the server says unasked.
#[derive(Debug, Clone)]
pub enum Event {
    /// Of a session: `window_closed`, `block_event`, … with its payload.
    Session { session: String, kind: String, payload: Value },
    /// Of the server: `session_created`, `session_destroyed`, …
    Server { kind: String, payload: Value },
    /// The connection ended.
    Closed(String),
}

enum Out {
    Call { id: String, body: String, reply: oneshot::Sender<Result<Value, RexErr>> },
    Forget(String),
}

/// The control connection.
pub struct Rex {
    tx: mpsc::UnboundedSender<Out>,
    next: AtomicU64,
    socket: PathBuf,
    max_message: usize,
}

async fn upgraded(socket: &Path, path: &str, protocol: &str) -> Result<WebSocketStream<UnixStream>, RexErr> {
    let stream = tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(socket)).await.map_err(|_| RexErr::Timeout)?.map_err(|e| RexErr::Closed(format!("{}: {e}", socket.display())))?;
    let req = http::Request::builder()
        .uri(format!("ws://localhost{path}"))
        .header("Host", "localhost")
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key())
        .header("Sec-WebSocket-Protocol", protocol)
        .body(())
        .map_err(|e| RexErr::Protocol(e.to_string()))?;
    let (ws, _) = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::client_async(req, stream)).await.map_err(|_| RexErr::Timeout)?.map_err(|e| RexErr::Closed(e.to_string()))?;
    Ok(ws)
}

impl Rex {
    /// Connects, says who this is (never taken for the Olo desktop:
    /// `olo` picks that among the server's clients by its kind and
    /// name) and begins: the answers to what is asked come back through
    /// the handle, everything else through the events.
    pub async fn connect(socket: &Path) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<Event>), RexErr> {
        let mut ws = upgraded(socket, "/api/control", CONTROL).await?;
        let hello = json!({
            "type": "hello",
            "api_revision": API_REVISION,
            "api_min_revision": 1,
            "client": { "kind": "service", "platform": "linux", "name": "Olo agents", "version": env!("CARGO_PKG_VERSION") },
        });
        ws.send(Message::Text(hello.to_string().into())).await.map_err(|e| RexErr::Closed(e.to_string()))?;
        let welcome = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(msg) = ws.next().await {
                match msg.map_err(|e| RexErr::Closed(e.to_string()))? {
                    Message::Text(text) => {
                        let v: Value = serde_json::from_str(&text).map_err(|e| RexErr::Protocol(e.to_string()))?;
                        if v["type"] != "welcome" {
                            return Err(RexErr::Protocol(format!("{} where its welcome was due", v["type"])));
                        }
                        return Ok(v);
                    }
                    Message::Ping(p) => ws.send(Message::Pong(p)).await.map_err(|e| RexErr::Closed(e.to_string()))?,
                    Message::Close(_) => return Err(RexErr::Closed("closed before its welcome".into())),
                    _ => {}
                }
            }
            Err(RexErr::Closed("ended before its welcome".into()))
        })
        .await
        .map_err(|_| RexErr::Timeout)??;
        if welcome["protocol"] != CONTROL {
            return Err(RexErr::Protocol(format!("protocol {}", welcome["protocol"])));
        }
        let max_message = welcome["limits"]["max_message_bytes"].as_u64().unwrap_or(1 << 20) as usize;
        let (tx, rx) = mpsc::unbounded_channel();
        let (events_tx, events) = mpsc::unbounded_channel();
        tokio::spawn(run(ws, rx, events_tx));
        Ok((Arc::new(Self { tx, next: AtomicU64::new(1), socket: socket.to_owned(), max_message }), events))
    }

    /// One request and its answer's payload.
    pub async fn call(&self, method: &str, payload: Value) -> Result<Value, RexErr> {
        self.call_within(method, payload, ANSWER_WITHIN).await
    }

    pub async fn call_within(&self, method: &str, payload: Value, within: Duration) -> Result<Value, RexErr> {
        let id = self.next.fetch_add(1, Ordering::Relaxed).to_string();
        let body = if payload.is_null() { json!({ "type": "request", "id": id, "method": method }) } else { json!({ "type": "request", "id": id, "method": method, "payload": payload }) }.to_string();
        if body.len() > self.max_message {
            return Err(RexErr::Protocol(format!("{method}: a request of {} bytes, more than the server takes ({})", body.len(), self.max_message)));
        }
        let (reply, answer) = oneshot::channel();
        self.tx.send(Out::Call { id: id.clone(), body, reply }).map_err(|_| RexErr::Closed("the connection's loop ended".into()))?;
        match tokio::time::timeout(within, answer).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(RexErr::Closed("the connection's loop ended".into())),
            Err(_) => {
                let _ = self.tx.send(Out::Forget(id));
                Err(RexErr::Timeout)
            }
        }
    }

    /// A method of a terminal (`com.superlogical.terminal.METHOD`).
    pub async fn terminal(&self, session: &str, block: &str, method: &str, args: Value) -> Result<Value, RexErr> {
        self.call(&format!("com.superlogical.terminal.{method}"), json!({ "session_id": session, "block_id": block, "args": args })).await
    }

    /// Types into a terminal.
    pub async fn write(&self, session: &str, block: &str, data: &str) -> Result<(), RexErr> {
        self.terminal(session, block, "write", json!({ "data": data })).await.map(|_| ())
    }

    /// What a terminal shows now, as text: the server's own reading of
    /// its screen (its last `rows` lines; all of it where `rows` is 0).
    /// One request where that is the alternate screen, as an agent's is.
    /// Of a main screen with a long history only the end is fetched:
    /// asked whole, the server would keep each answer for minutes.
    pub async fn screen(&self, session: &str, block: &str, rows: usize) -> Result<String, RexErr> {
        let first = self.terminal(session, block, "format", json!({ "format": "text", "trim": true, "max_bytes": SCREEN_MOST })).await?;
        let mut text = first["content"].as_str().unwrap_or_default().to_owned();
        if let Some(id) = first["id"].as_u64() {
            // (More than was sent: the end of it, from a place that
            // begins a character. The history before it isn't the
            // screen's.)
            let total = first["total"].as_u64().unwrap_or(0) as usize;
            let mut offset = total.saturating_sub(SCREEN_MOST / 4).max(text.len().min(total));
            let mut end = String::new();
            for _ in 0..8 {
                let piece = self.terminal(session, block, "blob_read", json!({ "id": id, "offset": offset })).await?;
                let data = piece["data"].as_str().unwrap_or_default();
                offset += data.len();
                end.push_str(data);
                if piece["eof"].as_bool().unwrap_or(true) || data.is_empty() {
                    break;
                }
            }
            // (From the first whole line of it.)
            text = end.split_once('\n').map(|(_, rest)| rest.to_owned()).unwrap_or(end);
        }
        Ok(last_lines(&text, rows))
    }

    /// All a terminal has, history too (`read --scrollback`, `stop
    /// --save`).
    pub async fn scrollback(&self, session: &str, block: &str, format: &str) -> Result<String, RexErr> {
        let most = self.max_message.saturating_sub(1024).clamp(1, 960 * 1024);
        let first = self.terminal(session, block, "format", json!({ "format": format, "trim": true, "max_bytes": most })).await?;
        let mut text = first["content"].as_str().unwrap_or_default().to_owned();
        if let Some(id) = first["id"].as_u64() {
            loop {
                let piece = self.terminal(session, block, "blob_read", json!({ "id": id, "offset": text.len() })).await?;
                let data = piece["data"].as_str().unwrap_or_default();
                text.push_str(data);
                if piece["eof"].as_bool().unwrap_or(true) || data.is_empty() {
                    break;
                }
            }
        }
        Ok(text)
    }

    /// A terminal's stream, for a session this connection has attached
    /// to (`session.attach`).
    pub async fn stream(&self, session: &str, block: &str) -> Result<TermStream, RexErr> {
        let ticket = self.call("data.connect", json!({ "session_id": session, "block_id": block, "data_ref_id": "terminal", "subprotocol": TERMINAL })).await?;
        let ticket = ticket["ticket"].as_str().ok_or_else(|| RexErr::Protocol("data.connect gave no ticket".into()))?;
        let raw = unbase64url(ticket).filter(|t| t.len() == 16).ok_or_else(|| RexErr::Protocol("a ticket that isn't 16 bytes".into()))?;
        let mut ws = upgraded(&self.socket, "/api/data", TERMINAL).await?;
        let mut hello = vec![0x01u8];
        hello.extend_from_slice(&raw);
        ws.send(Message::Binary(hello.into())).await.map_err(|e| RexErr::Closed(e.to_string()))?;
        let welcome = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(msg) = ws.next().await {
                match msg.map_err(|e| RexErr::Closed(e.to_string()))? {
                    Message::Binary(b) => return Ok(b),
                    Message::Ping(p) => ws.send(Message::Pong(p)).await.map_err(|e| RexErr::Closed(e.to_string()))?,
                    Message::Close(_) => return Err(RexErr::Closed("the stream was refused".into())),
                    _ => {}
                }
            }
            Err(RexErr::Closed("the stream ended before its welcome".into()))
        })
        .await
        .map_err(|_| RexErr::Timeout)??;
        if welcome.len() != 32 || welcome[0] != 0x81 {
            return Err(RexErr::Closed(format!("the stream was refused ({:02x?})", &welcome[..welcome.len().min(2)])));
        }
        let nonce = u64::from_le_bytes(welcome[24..32].try_into().expect("eight bytes"));
        Ok(TermStream { ws, skip: SnapshotSkip::default(), words: Words::new(nonce) })
    }
}

/// The last `rows` lines of `text` (all of it where `rows` is 0), its
/// trailing empty lines left out.
pub fn last_lines(text: &str, rows: usize) -> String {
    let text = text.trim_end_matches(['\n', ' ']);
    if rows == 0 {
        return text.to_owned();
    }
    match text.rmatch_indices('\n').nth(rows - 1) {
        Some((at, _)) => text[at + 1..].to_owned(),
        None => text.to_owned(),
    }
}

fn unbase64url(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u8);
    for c in text.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

async fn run(mut ws: WebSocketStream<UnixStream>, mut rx: mpsc::UnboundedReceiver<Out>, events: mpsc::UnboundedSender<Event>) {
    let mut pending: HashMap<String, oneshot::Sender<Result<Value, RexErr>>> = HashMap::new();
    let why = loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(Out::Call { id, body, reply }) => {
                    if reply.is_closed() {
                        continue;
                    }
                    pending.insert(id, reply);
                    if let Err(e) = ws.send(Message::Text(body.into())).await {
                        break format!("a request couldn't be sent: {e}");
                    }
                }
                Some(Out::Forget(id)) => {
                    pending.remove(&id);
                }
                None => {
                    let _ = ws.close(None).await;
                    break "closed here".to_owned();
                }
            },
            frame = ws.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    let Ok(mut v) = serde_json::from_str::<Value>(&text) else { break "a frame that isn't JSON".to_owned() };
                    match v["type"].as_str() {
                        Some("response") => {
                            let Some(reply) = v["id"].as_str().and_then(|id| pending.remove(id)) else { continue };
                            let answer = match v.get("error").filter(|e| !e.is_null()) {
                                Some(e) => Err(RexErr::Api { code: e["code"].as_str().unwrap_or("error").to_owned(), message: e["message"].as_str().unwrap_or_default().to_owned() }),
                                None => Ok(v["payload"].take()),
                            };
                            let _ = reply.send(answer);
                        }
                        Some("event") => {
                            let mut inner = v["payload"].take();
                            let kind = inner["type"].as_str().unwrap_or_default().to_owned();
                            let mut payload = inner["payload"].take();
                            let event = if kind == "session_event" {
                                let session = payload["session_id"].as_str().unwrap_or_default().to_owned();
                                let mut of = payload["event"].take();
                                Event::Session { session, kind: of["type"].as_str().unwrap_or_default().to_owned(), payload: of["payload"].take() }
                            } else {
                                Event::Server { kind, payload }
                            };
                            if events.send(event).is_err() {
                                break "nobody listens here".to_owned();
                            }
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Ping(p))) => {
                    if ws.send(Message::Pong(p)).await.is_err() {
                        break "a ping couldn't be answered".to_owned();
                    }
                }
                Some(Ok(Message::Close(c))) => break c.map(|c| format!("{} {}", c.code, c.reason)).unwrap_or_else(|| "closed".to_owned()),
                Some(Ok(_)) => {}
                Some(Err(e)) => break e.to_string(),
                None => break "the server went away".to_owned(),
            },
        }
    };
    for (_, reply) in pending.drain() {
        let _ = reply.send(Err(RexErr::Closed(why.clone())));
    }
    let _ = events.send(Event::Closed(why));
}

/// What a terminal's stream says, as far as the service reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// Its program wrote something (this many bytes).
    Output(usize),
    /// Its program ended, with this status.
    Exited(i32),
    /// Its size changed.
    Resized { rows: u16 },
    /// The stream is over: the terminal closed, or the server went.
    Closed,
}

/// A terminal's stream: the picture of the terminal as it stood (passed
/// over: what it shows is asked of the server), then what its program
/// writes, with the server's own words among it.
pub struct TermStream {
    ws: WebSocketStream<UnixStream>,
    skip: SnapshotSkip,
    words: Words,
}

impl TermStream {
    /// The next thing said; a frame of nothing but the first picture
    /// says nothing and is passed over.
    pub async fn next(&mut self) -> Said {
        loop {
            // (A word found in the last frame, after its output was said.)
            if let Some(word) = self.words.found.pop_front() {
                return word;
            }
            match self.ws.next().await {
                Some(Ok(Message::Binary(bytes))) => {
                    let live = &bytes[self.skip.feed(&bytes)..];
                    if live.is_empty() {
                        continue;
                    }
                    let output = self.words.feed(live);
                    if output > 0 {
                        return Said::Output(output);
                    }
                }
                Some(Ok(Message::Ping(p))) => {
                    if self.ws.send(Message::Pong(p)).await.is_err() {
                        return Said::Closed;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Said::Closed,
                Some(Ok(_)) => {}
            }
        }
    }
}

/// Finds where the first picture of a terminal ends (`GHOSTSNP`: an
/// envelope of ten bytes, then records, each a header of ten bytes that
/// says its kind and its payload's length, the last of kind 6), without
/// reading it: [`SnapshotSkip::feed`] says where in each frame what the
/// program writes begins.
#[derive(Default)]
pub struct SnapshotSkip {
    /// Bytes of the envelope or of a record's header had so far.
    head: Vec<u8>,
    /// Bytes of a record's payload still to pass.
    payload: usize,
    last: bool,
    enveloped: bool,
    done: bool,
}

impl SnapshotSkip {
    const ENVELOPE: &'static [u8] = b"GHOSTSNP\x01\x00";

    /// The offset in `bytes` where live output begins (`bytes.len()`
    /// where all of it is still the picture).
    pub fn feed(&mut self, bytes: &[u8]) -> usize {
        let mut at = 0;
        while !self.done && at < bytes.len() {
            if self.payload > 0 {
                let pass = self.payload.min(bytes.len() - at);
                self.payload -= pass;
                at += pass;
                if self.payload == 0 && self.last {
                    self.done = true;
                }
                continue;
            }
            self.head.push(bytes[at]);
            at += 1;
            if !self.enveloped {
                // (Not a picture at all: everything is the program's.)
                if self.head[..] != Self::ENVELOPE[..self.head.len()] {
                    self.done = true;
                    return at.saturating_sub(self.head.len());
                }
                if self.head.len() == Self::ENVELOPE.len() {
                    self.enveloped = true;
                    self.head.clear();
                }
            } else if self.head.len() == 10 {
                let tag = u16::from_le_bytes([self.head[0], self.head[1]]);
                self.payload = u32::from_le_bytes([self.head[2], self.head[3], self.head[4], self.head[5]]) as usize;
                self.last = tag == 6;
                self.head.clear();
                if self.payload == 0 && self.last {
                    self.done = true;
                }
            }
        }
        at
    }
}

/// The server's own words in a terminal's output (`ESC _ rex1;NONCE;JSON
/// ESC \`, the nonce this stream's alone, so nothing a program prints is
/// taken for one), found as the output passes, cut anywhere.
struct Words {
    prefix: Vec<u8>,
    state: WordState,
    body: Vec<u8>,
    found: std::collections::VecDeque<Said>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WordState {
    Plain,
    Esc,
    /// Matching the prefix: this many bytes of it had.
    Prefix(usize),
    Body,
    BodyEsc,
}

impl Words {
    fn new(nonce: u64) -> Self {
        Self { prefix: format!("rex1;{nonce};").into_bytes(), state: WordState::Plain, body: Vec::new(), found: Default::default() }
    }

    /// Takes `bytes` of live output, keeps the words found in it, and
    /// says how many of the bytes were the program's own.
    fn feed(&mut self, bytes: &[u8]) -> usize {
        let mut output = 0;
        for &b in bytes {
            self.state = match self.state {
                WordState::Plain if b == 0x1b => WordState::Esc,
                WordState::Plain => {
                    output += 1;
                    WordState::Plain
                }
                WordState::Esc if b == b'_' => WordState::Prefix(0),
                WordState::Esc => {
                    output += 2;
                    if b == 0x1b { WordState::Esc } else { WordState::Plain }
                }
                WordState::Prefix(n) if self.prefix.get(n) == Some(&b) => {
                    if n + 1 == self.prefix.len() {
                        self.body.clear();
                        WordState::Body
                    } else {
                        WordState::Prefix(n + 1)
                    }
                }
                // (Another's: the program's own bytes after all.)
                WordState::Prefix(n) => {
                    output += n + 3;
                    if b == 0x1b { WordState::Esc } else { WordState::Plain }
                }
                WordState::Body if b == 0x1b => WordState::BodyEsc,
                WordState::Body => {
                    if self.body.len() < 64 * 1024 {
                        self.body.push(b);
                    }
                    WordState::Body
                }
                WordState::BodyEsc if b == b'\\' => {
                    if let Ok(v) = serde_json::from_slice::<Value>(&self.body) {
                        match v["type"].as_str() {
                            Some("child_exited") => self.found.push_back(Said::Exited(v["exit_code"].as_i64().unwrap_or(-1) as i32)),
                            Some("resize") => self.found.push_back(Said::Resized { rows: v["rows"].as_u64().unwrap_or(24) as u16 }),
                            _ => {}
                        }
                    }
                    self.body.clear();
                    WordState::Plain
                }
                WordState::BodyEsc => {
                    self.body.push(0x1b);
                    self.body.push(b);
                    WordState::Body
                }
            };
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(records: &[(u16, usize)]) -> Vec<u8> {
        let mut out = SnapshotSkip::ENVELOPE.to_vec();
        for &(tag, len) in records {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&(len as u32).to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend(std::iter::repeat_n(0xAB, len));
        }
        out
    }

    #[test]
    fn the_first_picture_is_passed_over_cut_anywhere() {
        let mut whole = picture(&[(1, 4), (2, 12), (3, 300), (6, 0)]);
        let picture_len = whole.len();
        whole.extend_from_slice(b"hello");
        for cut in 1..whole.len() {
            let mut skip = SnapshotSkip::default();
            let (a, b) = whole.split_at(cut);
            let mut live = a[skip.feed(a)..].to_vec();
            live.extend_from_slice(&b[skip.feed(b)..]);
            assert_eq!(live, b"hello", "cut at {cut} of {picture_len}");
        }
        // (A byte at a time, too.)
        let mut skip = SnapshotSkip::default();
        let live: Vec<u8> = whole.iter().filter(|b| skip.feed(std::slice::from_ref(b)) == 0).copied().collect();
        assert_eq!(live, b"hello");
        // (What begins otherwise isn't a picture: all of it is output.)
        let mut skip = SnapshotSkip::default();
        assert_eq!(skip.feed(b"GHOSTLY bytes"), 0);
        assert_eq!(skip.feed(b"more"), 0);
    }

    #[test]
    fn the_servers_words_are_found_and_a_programs_own_are_not() {
        let mut said = b"abc\x1b_rex1;42;{\"type\":\"child_exited\",\"exit_code\":7}\x1b\\def".to_vec();
        said.extend_from_slice(b"\x1b_rex1;41;{\"type\":\"child_exited\",\"exit_code\":1}\x1b\\");
        said.extend_from_slice(b"\x1b[31mred\x1b_rex1;42;{\"type\":\"resize\",\"cols\":90,\"rows\":33}\x1b\\");
        for cut in 0..said.len() {
            let mut words = Words::new(42);
            let _ = words.feed(&said[..cut]);
            let _ = words.feed(&said[cut..]);
            assert_eq!(words.found, [Said::Exited(7), Said::Resized { rows: 33 }], "cut at {cut}");
        }
        let mut words = Words::new(42);
        // (Its own bytes counted: 6 letters, another's word whole, the colour and its word.)
        let other = "\x1b_rex1;41;{\"type\":\"child_exited\",\"exit_code\":1}\x1b\\".len();
        assert_eq!(words.feed(&said), 6 + other + "\x1b[31mred".len());
    }

    #[test]
    fn a_screen_is_the_last_lines() {
        assert_eq!(last_lines("a\nb\nc\n\n", 2), "b\nc");
        assert_eq!(last_lines("a\nb\nc", 5), "a\nb\nc");
        assert_eq!(last_lines("a\nb\nc", 0), "a\nb\nc");
        assert_eq!(last_lines("a\nb\nc", 1), "c");
        assert_eq!(last_lines("", 3), "");
    }

    #[test]
    fn a_ticket_is_sixteen_bytes() {
        assert_eq!(unbase64url("AAECAwQFBgcICQoLDA0ODw").unwrap(), (0..16).collect::<Vec<u8>>());
        assert!(unbase64url("not a ticket!").is_none());
    }
}
