//! Full `rex.control.v1` WebSocket client with uncompressed `< 512 B` `hello`,
//! `welcome` negotiation, keepalive pings, concurrent + ordered request
//! correlation, event streaming, and typed RPC methods for all Rex rev-19 calls.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
    time,
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

use crate::{
    error::RexError,
    model::{
        ActionResult, AttachResponse, BlobReadResponse, BootstrapResponse, CONTROL_SUBPROTOCOL, ClientIdentity, ConnectedClient, ControlEvent, ControlEventEnvelope, DataConnectResponse,
        DataTicketRequest, DirListing, DownloadBeginResponse, HostRecord, HostsListResponse, InputMode, KeyEvent, KeymapAction, KeymapLayer, KeymapResponse, ListedBlocks, PortsSnapshot,
        ProgramStatusRecord, REX_API_MIN_REVISION, REX_API_REVISION, ServerStatus, ServerVersion, SessionSummary, SessionView, TailscaleStatus, TerminalProcessSnapshot, TunnelInfo,
        UploadBeginResponse, WelcomeMessage,
    },
};

const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(120);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a probe ([`ControlClient::probe`]) waits for anything at all
/// to come from the server before the connection is taken for dead.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Result of an RPC call paired with the server's `seq` watermark.
#[derive(Debug, Clone)]
pub struct RpcResult<T> {
    pub seq: u64,
    pub payload: T,
}

#[derive(serde::Deserialize)]
struct TailscaleReply {
    tailscale: TailscaleStatus,
}

struct PendingCall {
    method: String,
    tx: oneshot::Sender<Result<RpcResult<Value>, RexError>>,
}

enum OutboundCmd {
    Request { id: String, method: String, body: String, reply: oneshot::Sender<Result<RpcResult<Value>, RexError>> },
    Forget(String),
    Probe,
    Close,
}

struct PendingGuard<'a> {
    sender: &'a mpsc::UnboundedSender<OutboundCmd>,
    id: Option<String>,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let _ = self.sender.send(OutboundCmd::Forget(id));
        }
    }
}

/// [`PendingGuard`] for an answer waited for apart from its request
/// ([`ControlClient::send_raw`]): it owns its way to the loop.
struct SentGuard {
    sender: mpsc::UnboundedSender<OutboundCmd>,
    id: Option<String>,
}

impl Drop for SentGuard {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let _ = self.sender.send(OutboundCmd::Forget(id));
        }
    }
}

/// The answer to a request sent with the hello
/// ([`ControlClient::connect_asking`]), to be waited for.
pub type Asked = std::pin::Pin<Box<dyn Future<Output = Result<RpcResult<Value>, RexError>> + Send>>;

/// An answer as the request's own method gives it (`method`, for what
/// an answer that can't be read says): [`Asked`]'s, typed.
pub fn answered<T: DeserializeOwned>(method: &str, answer: Result<RpcResult<Value>, RexError>) -> Result<RpcResult<T>, RexError> {
    let answer = answer?;
    Ok(RpcResult { seq: answer.seq, payload: decode(method, answer.payload)? })
}

/// The hello a client begins with: under 512 bytes, its label left out
/// where that is what makes it longer.
fn hello_text(identity: &ClientIdentity, reconnect_token: Option<&str>) -> Result<String, RexError> {
    let mut hello = json!({
        "type": "hello",
        "api_revision": REX_API_REVISION,
        "api_min_revision": REX_API_MIN_REVISION,
        "client": identity,
    });
    if let Some(tok) = reconnect_token
        && !tok.is_empty()
    {
        hello["reconnect_token"] = Value::String(tok.to_owned());
    }
    let text = serde_json::to_string(&hello)?;
    if text.len() < 512 {
        return Ok(text);
    }
    hello["client"].as_object_mut().unwrap().remove("label");
    let text = serde_json::to_string(&hello)?;
    if text.len() >= 512 {
        return Err(RexError::MessageTooLarge { size: text.len(), limit: 511 });
    }
    Ok(text)
}

/// A text message in one frame, as a client sends it: under a mask of
/// chance (the WebSocket library's own are; over SSH the mask is taken
/// off again, `plain`).
fn client_text_frame(text: &[u8]) -> Vec<u8> {
    let mut out = vec![0x81];
    match text.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n < 65536 => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    let chance = uuid::Uuid::new_v4();
    let mask = &chance.as_bytes()[..4];
    out.extend_from_slice(mask);
    out.extend(text.iter().enumerate().map(|(i, byte)| byte ^ mask[i % 4]));
    out
}

/// Live connection to a Rex server's `/api/control` WebSocket (`rex.control.v1`).
#[derive(Clone)]
pub struct ControlClient {
    cmd_tx: mpsc::UnboundedSender<OutboundCmd>,
    next_id: Arc<AtomicU64>,
    pub welcome: Arc<WelcomeMessage>,
    pub max_message_bytes: usize,
    /// What Olo's helper on the host does for this connection, as it
    /// said in the upgrade's answer (`olo_rex_link::VERSION`); 0 where
    /// the connection doesn't go through one.
    link: u32,
    /// The number of the last request sent by [`Self::send_raw`]: the
    /// last thing typed ([`Self::last_typed`]).
    typed: Arc<AtomicU64>,
}

impl ControlClient {
    /// Upgrade `stream` to `GET /api/control`, perform the `hello`/`welcome`
    /// handshake, and spawn the background reader/writer/keepalive task.
    ///
    /// Returns the cloneable [`ControlClient`] handle and a channel receiving
    /// ordered [`ControlEventEnvelope`] events from the server.
    pub async fn connect<S>(stream: S, host_header: &str, identity: ClientIdentity, reconnect_token: Option<&str>) -> Result<(Self, mpsc::UnboundedReceiver<ControlEventEnvelope>), RexError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::handshake(stream, host_header, identity, reconnect_token, true).await
    }

    /// Local sockets report peer death directly. Respond to the server's pings
    /// without running a client-side keepalive timer.
    pub async fn connect_unix(stream: tokio::net::UnixStream, identity: ClientIdentity) -> Result<(Self, mpsc::UnboundedReceiver<ControlEventEnvelope>), RexError> {
        Self::handshake(stream, "localhost", identity, None, false).await
    }

    async fn handshake<S>(
        stream: S,
        host_header: &str,
        identity: ClientIdentity,
        reconnect_token: Option<&str>,
        keepalive: bool,
    ) -> Result<(Self, mpsc::UnboundedReceiver<ControlEventEnvelope>), RexError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let req = http::Request::builder()
            .uri(format!("ws://{host_header}/api/control"))
            .header("Host", host_header)
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", tungstenite::handshake::client::generate_key())
            .header("Sec-WebSocket-Protocol", CONTROL_SUBPROTOCOL)
            .body(())
            .map_err(|e| RexError::Protocol(format!("control request build failed: {e}")))?;

        let (mut ws, resp) = time::timeout(Duration::from_secs(5), tokio_tungstenite::client_async(req, stream)).await.map_err(|_| RexError::Timeout)??;
        let link = resp.headers().get(olo_rex_link::HEADER).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        send_control(&mut ws, Message::Text(hello_text(&identity, reconnect_token)?.into())).await?;
        let (client, events) = Self::welcomed(ws, link, keepalive, Vec::new()).await?;
        Ok((client, events))
    }

    /// The server's welcome is waited for and held to what this client
    /// speaks, and the connection's loop begun; `asked`: requests sent
    /// already, whose answers the loop is to know ([`Self::
    /// connect_asking`]).
    async fn welcomed<S>(mut ws: WebSocketStream<S>, link: u32, keepalive: bool, asked: Vec<(String, PendingCall)>) -> Result<(Self, mpsc::UnboundedReceiver<ControlEventEnvelope>), RexError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        // Wait up to 5 seconds for `welcome`.
        let welcome = time::timeout(Duration::from_secs(5), read_welcome(&mut ws)).await.map_err(|_| RexError::Timeout)??;
        if welcome.protocol != CONTROL_SUBPROTOCOL {
            return Err(RexError::Protocol(format!("unexpected control protocol: {}", welcome.protocol)));
        }

        if !(REX_API_MIN_REVISION..=REX_API_REVISION).contains(&welcome.api_revision) || welcome.api_min_revision > welcome.api_revision {
            return Err(RexError::Protocol(format!(
                "incompatible Rex API revision {} (server minimum {}, client supports {REX_API_MIN_REVISION}–{REX_API_REVISION})",
                welcome.api_revision, welcome.api_min_revision
            )));
        }
        let max_message_bytes = welcome.limits.max_message_bytes;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (evt_tx, evt_rx) = mpsc::unbounded_channel();
        let next_id = asked.len() as u64 + 1;

        tokio::spawn(run_control_loop(ws, cmd_rx, evt_tx, keepalive, asked));

        Ok((Self { cmd_tx, next_id: Arc::new(AtomicU64::new(next_id)), welcome: Arc::new(welcome), max_message_bytes, link, typed: Arc::new(AtomicU64::new(0)) }, evt_rx))
    }

    /// [`Self::connect`], with `asked` (requests: each a method and its
    /// payload) sent in the same flight as the upgrade and the hello,
    /// none waiting for the one before: the upgrade's request, the
    /// hello and the requests leave together, and the server's yes, its
    /// welcome and its answers come back together, **one round trip of
    /// the link where they were one each**. The answers, in the order
    /// asked, are given back to be waited for ([`Asked`]; typed with
    /// [`answered`]).
    ///
    /// The Rex server takes it so (tried by hand against one,
    /// 2026-11-23: the yes, the welcome and two answers in a
    /// millisecond), and so do Olo's helper and `rex __internal
    /// dial-stdio` in front of it. Only for a stream with nothing
    /// between that reads HTTP (a socket, an SSH channel): a proxy may
    /// not pass on what comes behind an upgrade's request before the
    /// answer to it. A server that doesn't take it fails this (the
    /// stream is spent): connect again the plain way.
    pub async fn connect_asking<S>(
        mut stream: S,
        host_header: &str,
        identity: ClientIdentity,
        reconnect_token: Option<&str>,
        asked: Vec<(&'static str, Option<Value>)>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<ControlEventEnvelope>, Vec<Asked>), RexError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let key = tungstenite::handshake::client::generate_key();
        let mut flight = format!(
            "GET /api/control HTTP/1.1\r\nHost: {host_header}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Protocol: {CONTROL_SUBPROTOCOL}\r\n\r\n"
        )
        .into_bytes();
        flight.extend(client_text_frame(hello_text(&identity, reconnect_token)?.as_bytes()));
        let (mut pending, mut answers) = (Vec::new(), Vec::new());
        for (i, (method, payload)) in asked.into_iter().enumerate() {
            let id = (i + 1).to_string();
            let body = serde_json::to_string(&match payload {
                Some(p) => json!({ "type": "request", "id": id, "method": method, "payload": p }),
                None => json!({ "type": "request", "id": id, "method": method }),
            })?;
            flight.extend(client_text_frame(body.as_bytes()));
            let (tx, rx) = oneshot::channel();
            pending.push((id, PendingCall { method: method.to_owned(), tx }));
            let answer: Asked =
                Box::pin(async move { time::timeout(DEFAULT_REQUEST_TIMEOUT, rx).await.map_err(|_| RexError::Timeout)?.map_err(|_| RexError::Closed("control loop stopped".to_owned()))? });
            answers.push(answer);
        }
        // The server's answer to the upgrade, read here (the WebSocket
        // library's own client waits for it before anything else is
        // sent, which is the round trip this saves): a yes, to the key
        // that was sent.
        let upgraded = async {
            stream.write_all(&flight).await?;
            stream.flush().await?;
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            let end = loop {
                if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
                if got.len() > 16 * 1024 {
                    return Err(RexError::Protocol("the upgrade's answer has no end".to_owned()));
                }
                match stream.read(&mut buf).await? {
                    0 => return Err(RexError::Closed("closed before the upgrade was answered".to_owned())),
                    n => got.extend_from_slice(&buf[..n]),
                }
            };
            let rest = got.split_off(end);
            Ok::<_, RexError>((got, rest, stream))
        };
        let (head, rest, stream) = time::timeout(Duration::from_secs(5), upgraded).await.map_err(|_| RexError::Timeout)??;
        let head = String::from_utf8_lossy(&head);
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap_or_default();
        let header = |name: &str| head.split("\r\n").skip(1).filter_map(|line| line.split_once(':')).find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_owned());
        if !(status.starts_with("HTTP/1.1 101") || status.starts_with("HTTP/1.0 101")) {
            return Err(RexError::Protocol(format!("the upgrade was answered {status:?}")));
        }
        if header("Sec-WebSocket-Accept").as_deref() != Some(tungstenite::handshake::derive_accept_key(key.as_bytes()).as_str()) {
            return Err(RexError::Protocol("the upgrade's answer isn't to this request".to_owned()));
        }
        let link = header(olo_rex_link::HEADER).and_then(|v| v.parse().ok()).unwrap_or(0);
        let ws = WebSocketStream::from_partially_read(stream, rest, tungstenite::protocol::Role::Client, None).await;
        let (client, events) = Self::welcomed(ws, link, true, pending).await?;
        Ok((client, events, answers))
    }

    /// Whether what is typed may be sent as it is typed, each request
    /// without waiting for the last one's answer
    /// ([`Self::send_terminal_write`], [`Self::send_client_key`]):
    /// Olo's helper on the host stands in front of the server and
    /// gives it one at a time (`olo-rex-link`). Without it the server
    /// takes two at once, and what was typed can come out shuffled.
    pub fn keeps_typing_in_order(&self) -> bool {
        self.link >= 1
    }

    /// The id of the last request typed on this connection as it is
    /// typed ([`Self::send_terminal_write`], [`Self::send_client_key`]),
    /// as a number; 0 before any. Olo's helper on the host says how a
    /// terminal reads unasked, and after which typed request it looked
    /// (`after`, in its `input_mode_changed`: `olo_rex_link::Teller`):
    /// its word is of the terminal as everything typed left it where
    /// that is this number still, and of before something typed
    /// otherwise.
    pub fn last_typed(&self) -> u64 {
        self.typed.load(Ordering::Relaxed)
    }

    pub fn client_id(&self) -> Option<&str> {
        self.welcome.client.as_ref().map(|c| c.id.as_str())
    }

    pub fn reconnect_token(&self) -> Option<&str> {
        self.welcome.client.as_ref().map(|c| c.reconnect_token.as_str())
    }

    pub fn api_revision(&self) -> u32 {
        self.welcome.api_revision
    }

    /// Resolves when this connection's control loop closes its command receiver.
    /// From that point, new RPCs on any clone fail. Already closed connections
    /// resolve immediately. Dropping this wait leaves the connection running.
    pub async fn disconnected(&self) {
        self.cmd_tx.closed().await;
    }

    /// True when both handles belong to the same live or closed control loop.
    /// Clones compare equal. A reconnect creates a distinct loop, even when
    /// the server resumes the same client ID and reconnect token.
    pub fn same_connection(&self, other: &Self) -> bool {
        self.cmd_tx.same_channel(&other.cmd_tx)
    }

    /// Ask whether the server is still there, now: a ping is sent, and if
    /// nothing at all comes back within five seconds (`PROBE_TIMEOUT`)
    /// the connection ends as lost ([`ControlClient::disconnected`]
    /// resolves, pending calls fail). For when the person does something
    /// that should be answered at once (a key typed into a pane of a
    /// remote host): a link that died without a word is otherwise found
    /// only by the keepalive, minutes on (120 s apart, three missed) or
    /// by SSH's own (30 s apart, three missed), and the person types into
    /// nothing meanwhile. Nothing is sent while a probe is already out,
    /// so it may be called at every key; and none at all while nobody
    /// calls it: an idle connection costs what it did.
    pub fn probe(&self) {
        let _ = self.cmd_tx.send(OutboundCmd::Probe);
    }

    /// Close the control connection cleanly (tells server to end client immediately).
    pub fn close(&self) {
        let _ = self.cmd_tx.send(OutboundCmd::Close);
    }

    /// Low-level RPC call returning raw JSON payload + `seq`.
    pub async fn call_raw(&self, method: &str, payload: Option<Value>) -> Result<RpcResult<Value>, RexError> {
        self.call_raw_timeout(method, payload, DEFAULT_REQUEST_TIMEOUT).await
    }

    pub async fn call_raw_timeout(&self, method: &str, payload: Option<Value>, timeout: Duration) -> Result<RpcResult<Value>, RexError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let req = match payload {
            Some(p) => json!({ "type": "request", "id": id, "method": method, "payload": p }),
            None => json!({ "type": "request", "id": id, "method": method }),
        };
        let body = serde_json::to_string(&req)?;
        if body.len() > self.max_message_bytes {
            return Err(RexError::MessageTooLarge { size: body.len(), limit: self.max_message_bytes });
        }
        let (tx, rx) = oneshot::channel();
        self.cmd_tx.send(OutboundCmd::Request { id: id.clone(), method: method.to_owned(), body, reply: tx }).map_err(|_| RexError::Closed("control loop stopped".to_owned()))?;
        // A cancelled prompt or a caller's deadline must free the pending
        // entry even when the server never sends another frame.
        let mut guard = PendingGuard { sender: &self.cmd_tx, id: Some(id) };
        let result = time::timeout(timeout, rx).await.map_err(|_| RexError::Timeout)?;
        guard.id = None;
        result.map_err(|_| RexError::Closed("control loop stopped".to_owned()))?
    }

    /// Sends a request **now**, in the order this is called, and gives
    /// back its answer, to be waited for apart. The answer must be
    /// waited for, or at least kept until it comes: a request whose
    /// answer nobody holds isn't sent.
    ///
    /// **Not how Rex types.** Requests reach the server in the order
    /// they are sent here (one writer, one connection), but the server
    /// takes two that arrive together at once, and may do the second
    /// first: forty letters sent so over a real link came back
    /// "acbdefgjhilknm…" (2026-11-23, with Nagle's algorithm putting two
    /// requests in one packet; set apart by a tenth of a second each
    /// they kept their order, and each letter's echo took one round trip
    /// of the link, 183 to 237 ms, where one write at a time takes about
    /// one and a half, 255 to 355 ms). What is typed therefore goes one
    /// write at a time, what is typed meanwhile gathered into the next
    /// (the app's `typed_together`), unless Olo's helper is on the host
    /// to keep the order there ([`Self::keeps_typing_in_order`]);
    /// otherwise this is for what has no order to keep, and for
    /// measuring (`tests/host.rs`).
    pub fn send_raw(&self, method: &str, payload: Option<Value>) -> Result<impl Future<Output = Result<RpcResult<Value>, RexError>> + Send + 'static + use<>, RexError> {
        let number = self.next_id.fetch_add(1, Ordering::Relaxed);
        // (Said typed before it leaves: a word of the helper's that
        // names an earlier one is of before this from here on.)
        self.typed.fetch_max(number, Ordering::Relaxed);
        let id = number.to_string();
        let req = match payload {
            Some(p) => json!({ "type": "request", "id": id, "method": method, "payload": p }),
            None => json!({ "type": "request", "id": id, "method": method }),
        };
        let body = serde_json::to_string(&req)?;
        if body.len() > self.max_message_bytes {
            return Err(RexError::MessageTooLarge { size: body.len(), limit: self.max_message_bytes });
        }
        let (tx, rx) = oneshot::channel();
        self.cmd_tx.send(OutboundCmd::Request { id: id.clone(), method: method.to_owned(), body, reply: tx }).map_err(|_| RexError::Closed("control loop stopped".to_owned()))?;
        let guard = SentGuard { sender: self.cmd_tx.clone(), id: Some(id) };
        Ok(async move {
            // (Whole, into the answer's keeping: let go unanswered, it
            // has the loop forget the request.)
            let mut guard = guard;
            let result = time::timeout(DEFAULT_REQUEST_TIMEOUT, rx).await.map_err(|_| RexError::Timeout)?;
            guard.id = None;
            result.map_err(|_| RexError::Closed("control loop stopped".to_owned()))?
        })
    }

    /// Typed RPC call deserializing `payload` into `T`.
    pub async fn call<T: DeserializeOwned>(&self, method: &str, payload: Option<Value>) -> Result<RpcResult<T>, RexError> {
        let res = self.call_raw(method, payload).await?;
        let typed = decode(method, res.payload)?;
        Ok(RpcResult { seq: res.seq, payload: typed })
    }

    /// Call a block method (`<creator>.<method>`) wrapping `args`.
    pub async fn call_block<T: DeserializeOwned>(&self, method: &str, session_id: &str, block_id: &str, args: Option<Value>) -> Result<RpcResult<T>, RexError> {
        let mut payload = json!({
            "session_id": session_id,
            "block_id": block_id,
        });
        if let Some(a) = args {
            payload["args"] = a;
        }
        self.call(method, Some(payload)).await
    }

    // ---- Server & Host Methods ----

    pub async fn server_status(&self) -> Result<RpcResult<ServerStatus>, RexError> {
        self.call("server.status", None).await
    }

    pub async fn server_version(&self) -> Result<RpcResult<ServerVersion>, RexError> {
        self.call("server.version", None).await
    }

    pub async fn server_config_reload(&self) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("server.config_reload", None).await
    }

    pub async fn server_stop(&self) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("server.stop", None).await
    }

    pub async fn hosts_list(&self) -> Result<RpcResult<HostsListResponse>, RexError> {
        self.call("hosts.list", None).await
    }

    pub async fn hosts_put(&self, host: &HostRecord) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("hosts.put", Some(json!({ "host": host }))).await
    }

    pub async fn hosts_remove(&self, id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("hosts.remove", Some(json!({ "id": id }))).await
    }

    async fn tailscale_change(&self, method: &str, payload: Option<Value>) -> Result<RpcResult<TailscaleStatus>, RexError> {
        let result: RpcResult<TailscaleReply> = self.call(method, payload).await?;
        Ok(RpcResult { seq: result.seq, payload: result.payload.tailscale })
    }

    pub async fn tailscale_enable(
        &self,
        hostname: Option<&str>,
        allow_tailnet_members: Option<bool>,
        tags: Option<&[String]>,
        control_url: Option<&str>,
    ) -> Result<RpcResult<TailscaleStatus>, RexError> {
        let mut p = serde_json::Map::new();
        if let Some(h) = hostname {
            p.insert("hostname".into(), json!(h));
        }
        if let Some(a) = allow_tailnet_members {
            p.insert("allow_tailnet_members".into(), json!(a));
        }
        if let Some(t) = tags {
            p.insert("advertise_tags".into(), json!(t));
        }
        if let Some(u) = control_url {
            p.insert("control_url".into(), json!(u));
        }
        self.tailscale_change("server.tailscale.enable", Some(Value::Object(p))).await
    }

    pub async fn tailscale_disable(&self) -> Result<RpcResult<TailscaleStatus>, RexError> {
        self.tailscale_change("server.tailscale.disable", None).await
    }

    pub async fn tailscale_login(&self) -> Result<RpcResult<TailscaleStatus>, RexError> {
        self.tailscale_change("server.tailscale.login", None).await
    }

    pub async fn tailscale_logout(&self) -> Result<RpcResult<TailscaleStatus>, RexError> {
        self.tailscale_change("server.tailscale.logout", None).await
    }

    pub async fn tailscale_rename(&self, hostname: &str) -> Result<RpcResult<TailscaleStatus>, RexError> {
        self.tailscale_change("server.tailscale.rename", Some(json!({ "hostname": hostname }))).await
    }

    // ---- Session & Layout Methods ----

    pub async fn session_bootstrap(
        &self,
        event_scope: &str,
        preferred_session_id: Option<&str>,
        attach_session_ids: Option<&[String]>,
        data_tickets: &[DataTicketRequest],
    ) -> Result<RpcResult<BootstrapResponse>, RexError> {
        self.call("session.bootstrap", Some(Self::bootstrap_request(event_scope, preferred_session_id, attach_session_ids, data_tickets)?)).await
    }

    /// What [`Self::session_bootstrap`] asks (`session.bootstrap`'s
    /// payload), for a request sent with the hello
    /// ([`Self::connect_asking`]).
    pub fn bootstrap_request(event_scope: &str, preferred_session_id: Option<&str>, attach_session_ids: Option<&[String]>, data_tickets: &[DataTicketRequest]) -> Result<Value, RexError> {
        let mut p = json!({ "event_scope": event_scope });
        if let Some(pref) = preferred_session_id {
            p["preferred_session_id"] = json!(pref);
        }
        if let Some(ids) = attach_session_ids {
            p["attach_session_ids"] = json!(ids);
        }
        if !data_tickets.is_empty() {
            p["data_tickets"] = serde_json::to_value(data_tickets)?;
        }
        Ok(p)
    }

    pub async fn session_list(&self) -> Result<RpcResult<Vec<SessionSummary>>, RexError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(deserialize_with = "crate::model::list_or_empty")]
            sessions: Vec<SessionSummary>,
        }
        let r: RpcResult<Resp> = self.call("session.list", None).await?;
        Ok(RpcResult { seq: r.seq, payload: r.payload.sessions })
    }

    pub async fn session_create(&self, label: Option<&str>, shell_options: Option<Value>) -> Result<RpcResult<Value>, RexError> {
        let mut block = json!({ "flavor": "com.superlogical.terminal.shell" });
        if let Some(opts) = shell_options {
            block["options"] = opts;
        }
        let mut p = json!({
            "initial_windows": [{ "layout": { "block": block } }]
        });
        if let Some(l) = label
            && !l.is_empty()
        {
            p["label"] = json!(l);
        }
        self.call_raw("session.create", Some(p)).await
    }

    pub async fn session_attach(&self, session_id: &str) -> Result<RpcResult<AttachResponse>, RexError> {
        self.call("session.attach", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_detach(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.detach", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_destroy(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.destroy", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_view(&self, session_id: &str) -> Result<RpcResult<SessionView>, RexError> {
        self.call("session.view", Some(json!({ "session_id": session_id }))).await
    }

    /// Every block of the session, those in no window too (a forward's
    /// tunnel, which `session_view` leaves out).
    pub async fn session_list_blocks(&self, session_id: &str) -> Result<RpcResult<ListedBlocks>, RexError> {
        self.call("session.list_blocks", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_set_label(&self, session_id: &str, label: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.set_label", Some(json!({ "session_id": session_id, "label": label }))).await
    }

    pub async fn session_move(&self, session_id: &str, before_session_id: Option<&str>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id });
        if let Some(b) = before_session_id {
            p["before_session_id"] = json!(b);
        }
        self.call_raw("session.move", Some(p)).await
    }

    pub async fn session_new_window(&self, session_id: &str, window_label: Option<&str>, shell_options: Option<Value>) -> Result<RpcResult<Value>, RexError> {
        let mut block = json!({ "flavor": "com.superlogical.terminal.shell" });
        if let Some(opts) = shell_options {
            block["options"] = opts;
        }
        let mut p = json!({
            "session_id": session_id,
            "layout": { "block": block }
        });
        if let Some(l) = window_label
            && !l.is_empty()
        {
            p["window_label"] = json!(l);
        }
        self.call_raw("session.new_window", Some(p)).await
    }

    pub async fn session_close_window(&self, session_id: &str, window_id: Option<&str>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id });
        if let Some(w) = window_id {
            p["window_id"] = json!(w);
        }
        self.call_raw("session.close_window", Some(p)).await
    }

    pub async fn session_set_window_label(&self, session_id: &str, window_id: &str, label: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.set_window_label", Some(json!({ "session_id": session_id, "window_id": window_id, "label": label }))).await
    }

    pub async fn session_focus_window(&self, session_id: &str, window_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.focus_window", Some(json!({ "session_id": session_id, "window_id": window_id }))).await
    }

    pub async fn session_move_window(&self, session_id: &str, window_id: &str, before_window_id: Option<&str>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id, "window_id": window_id });
        if let Some(b) = before_window_id {
            p["before_window_id"] = json!(b);
        }
        self.call_raw("session.move_window", Some(p)).await
    }

    pub async fn session_new_split(&self, session_id: &str, target_block_id: Option<&str>, direction: &str, shell_options: Option<Value>) -> Result<RpcResult<Value>, RexError> {
        let mut block = json!({ "flavor": "com.superlogical.terminal.shell" });
        if let Some(opts) = shell_options {
            block["options"] = opts;
        }
        let (direction, side) = split_placement(direction);
        let mut p = json!({
            "session_id": session_id,
            "direction": direction,
            "side": side,
            "layout": { "block": block },
            "focus": true,
        });
        if let Some(b) = target_block_id {
            p["anchor_id"] = json!(b);
        }
        self.call_raw("session.new_split", Some(p)).await
    }

    pub async fn session_resize_pane(&self, session_id: &str, split_id: &str, ratio: f64) -> Result<RpcResult<Value>, RexError> {
        let view = self.session_view(session_id).await?.payload;
        let split = view
            .windows
            .iter()
            .flat_map(|w| &w.layers)
            .filter_map(|l| l.layout.as_ref())
            .find_map(|layout| layout.find_split(split_id))
            .ok_or_else(|| RexError::Protocol(format!("session.resize_pane: split {split_id} is no longer in the session")))?;
        let before = split.before.first_block_id();
        let after = split.after.first_block_id();
        let (Some(before), Some(after)) = (before, after) else {
            return Err(RexError::Protocol("session.resize_pane: split has no block on one side".into()));
        };
        self.call_raw("session.resize_pane", Some(json!({ "session_id": session_id, "direction": split.direction, "before_block_id": before, "after_block_id": after, "ratio": ratio }))).await
    }

    pub async fn session_focus_block(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.focus_block", Some(json!({ "session_id": session_id, "block_id": block_id }))).await
    }

    pub async fn session_focus_direction(&self, session_id: &str, direction: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.focus_direction", Some(json!({ "session_id": session_id, "direction": direction }))).await
    }

    pub async fn session_focus_next_pane(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.focus_next_pane", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_focus_previous_pane(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.focus_previous_pane", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_focus_next_window(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.focus_next_window", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_focus_previous_window(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.focus_previous_window", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_zoom_block(&self, session_id: &str, block_id: Option<&str>, zoom: Option<bool>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id });
        if let Some(b) = block_id {
            p["block_id"] = json!(b);
        }
        if let Some(z) = zoom {
            p["zoom"] = json!(z);
        }
        self.call_raw("session.zoom_block", Some(p)).await
    }

    pub async fn session_swap_blocks(&self, session_id: &str, block_id: &str, other_block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.swap_blocks", Some(json!({ "session_id": session_id, "block_id": block_id, "other_block_id": other_block_id }))).await
    }

    pub async fn session_move_block(&self, session_id: &str, block_id: &str, target_block_id: &str, direction: &str) -> Result<RpcResult<Value>, RexError> {
        let (direction, side) = split_placement(direction);
        self.call_raw(
            "session.move_block",
            Some(json!({
                "session_id": session_id,
                "block_id": block_id,
                "anchor_block_id": target_block_id,
                "side": side,
                "direction": direction,
            })),
        )
        .await
    }

    pub async fn session_own_block(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.own_block", Some(json!({ "session_id": session_id, "block_ids": [block_id] }))).await
    }

    pub async fn session_disown_block(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.disown_block", Some(json!({ "session_id": session_id, "block_ids": [block_id] }))).await
    }

    pub async fn session_set_block_label(&self, session_id: &str, block_id: &str, label: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.set_block_label", Some(json!({ "session_id": session_id, "block_id": block_id, "label": label }))).await
    }

    pub async fn session_new_block(&self, session_id: &str, flavor: &str, options: Option<Value>, label: Option<&str>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id, "flavor": flavor });
        if let Some(o) = options {
            p["options"] = o;
        }
        if let Some(l) = label {
            p["label"] = json!(l);
        }
        self.call_raw("session.new_block", Some(p)).await
    }

    pub async fn block_close(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("block.close", Some(json!({ "session_id": session_id, "block_id": block_id }))).await
    }

    // ---- Floating Layer Methods ----

    pub async fn session_float_block(&self, session_id: &str, block_id: &str, bounds: Option<[f64; 4]>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id, "block_id": block_id });
        if let Some(b) = bounds {
            p["bounds"] = json!(b);
        }
        self.call_raw("session.float_block", Some(p)).await
    }

    pub async fn session_new_layer(&self, session_id: &str, window_id: Option<&str>, bounds: [f64; 4], shell_options: Option<Value>) -> Result<RpcResult<Value>, RexError> {
        let mut block = json!({ "flavor": "com.superlogical.terminal.shell" });
        if let Some(o) = shell_options {
            block["options"] = o;
        }
        let mut p = json!({
            "session_id": session_id,
            "bounds": bounds,
            "layout": { "block": block },
        });
        if let Some(w) = window_id {
            p["window_id"] = json!(w);
        }
        self.call_raw("session.new_layer", Some(p)).await
    }

    pub async fn session_resize_layer(&self, session_id: &str, layer_id: &str, bounds: [f64; 4]) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.resize_layer", Some(json!({ "session_id": session_id, "layer_id": layer_id, "bounds": bounds }))).await
    }

    pub async fn session_set_layer_state(&self, session_id: &str, layer_id: &str, state: &str, focus: bool) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.set_layer_state", Some(json!({ "session_id": session_id, "layer_id": layer_id, "state": state, "focus": focus }))).await
    }

    pub async fn session_raise_layer(&self, session_id: &str, layer_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.raise_layer", Some(json!({ "session_id": session_id, "layer_id": layer_id }))).await
    }

    pub async fn session_lower_layer(&self, session_id: &str, layer_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.lower_layer", Some(json!({ "session_id": session_id, "layer_id": layer_id }))).await
    }

    pub async fn session_close_layer(&self, session_id: &str, layer_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("session.close_layer", Some(json!({ "session_id": session_id, "layer_id": layer_id }))).await
    }

    // ---- Focus History & Client / Keymap / Prompts ----

    pub async fn focus_history_reverse(&self, session_id: &str, cursor: Option<u64>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id });
        if let Some(cursor) = cursor {
            p["cursor"] = json!(cursor);
        }
        self.call_raw("focus_history.reverse", Some(p)).await
    }

    pub async fn client_list(&self) -> Result<RpcResult<Vec<ConnectedClient>>, RexError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(deserialize_with = "crate::model::list_or_empty")]
            clients: Vec<ConnectedClient>,
        }
        let r: RpcResult<Resp> = self.call("client.list", None).await?;
        Ok(RpcResult { seq: r.seq, payload: r.payload.clients })
    }

    pub async fn client_set_label(&self, label: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("client.set_label", Some(json!({ "label": label }))).await
    }

    pub async fn client_key(&self, session_id: &str, block_id: Option<&str>, key: &KeyEvent, prefix: Option<&[String]>) -> Result<RpcResult<bool>, RexError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            handled: bool,
        }
        let r: RpcResult<Resp> = self.call("client.key", Some(Self::key_payload(session_id, block_id, key, prefix))).await?;
        Ok(RpcResult { seq: r.seq, payload: r.payload.handled })
    }

    fn key_payload(session_id: &str, block_id: Option<&str>, key: &KeyEvent, prefix: Option<&[String]>) -> Value {
        let mut p = json!({ "session_id": session_id, "key": key });
        if let Some(b) = block_id {
            p["block_id"] = json!(b);
        }
        if let Some(pre) = prefix
            && !pre.is_empty()
        {
            p["prefix"] = json!(pre);
        }
        p
    }

    /// [`Self::client_key`], sent now and its answer given back to be
    /// waited for apart ([`Self::send_raw`]): for a connection that
    /// [`Self::keeps_typing_in_order`].
    pub fn send_client_key(&self, session_id: &str, block_id: Option<&str>, key: &KeyEvent) -> Result<impl Future<Output = Result<RpcResult<Value>, RexError>> + Send + 'static + use<>, RexError> {
        self.send_raw("client.key", Some(Self::key_payload(session_id, block_id, key, None)))
    }

    pub async fn client_keymap(&self, actions: &[KeymapAction], defaults: &[KeymapLayer], overrides: &[KeymapLayer]) -> Result<RpcResult<KeymapResponse>, RexError> {
        self.call("client.keymap", Some(Self::keymap_request(actions, defaults, overrides)?)).await
    }

    /// What [`Self::client_keymap`] asks (`client.keymap`'s payload),
    /// for a request sent with the hello ([`Self::connect_asking`]).
    pub fn keymap_request(actions: &[KeymapAction], defaults: &[KeymapLayer], overrides: &[KeymapLayer]) -> Result<Value, RexError> {
        let mut p = json!({ "server_actions": "all" });
        if !actions.is_empty() {
            p["actions"] = serde_json::to_value(actions)?;
        }
        if !defaults.is_empty() {
            p["defaults"] = serde_json::to_value(defaults)?;
        }
        if !overrides.is_empty() {
            p["overrides"] = serde_json::to_value(overrides)?;
        }
        Ok(p)
    }

    pub async fn client_action(
        &self,
        action: &str,
        args: Option<Value>,
        origin: &str,
        session_id: Option<&str>,
        block_id: Option<&str>,
        servers: Option<&[(String, String)]>,
        server: Option<&str>,
    ) -> Result<RpcResult<ActionResult>, RexError> {
        let mut p = json!({ "action": action, "origin": origin });
        if let Some(a) = args {
            p["args"] = a;
        }
        if let Some(s) = session_id {
            p["session_id"] = json!(s);
        }
        if let Some(b) = block_id {
            p["block_id"] = json!(b);
        }
        if let Some(sv) = servers
            && !sv.is_empty()
        {
            p["servers"] = Value::Array(sv.iter().map(|(name, id)| json!({ "name": name, "id": id })).collect());
        }
        if let Some(s) = server {
            p["server"] = json!(s);
        }
        self.call("client.action", Some(p)).await
    }

    pub async fn client_reply_ok(&self, call_id: &str, result: Value) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("client.reply", Some(json!({ "call_id": call_id, "result": result }))).await
    }

    pub async fn client_reply_err(&self, call_id: &str, code: &str, message: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_raw("client.reply", Some(json!({ "call_id": call_id, "error": { "code": code, "message": message } }))).await
    }

    pub async fn data_connect(&self, session_id: &str, block_id: &str, data_ref_id: &str, subprotocol: &str) -> Result<RpcResult<DataConnectResponse>, RexError> {
        self.call(
            "data.connect",
            Some(json!({
                "session_id": session_id,
                "block_id": block_id,
                "data_ref_id": data_ref_id,
                "subprotocol": subprotocol,
            })),
        )
        .await
    }

    // ---- Terminal Block Methods ----

    pub async fn terminal_write(&self, session_id: &str, block_id: &str, data: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_block("com.superlogical.terminal.write", session_id, block_id, Some(json!({ "data": data }))).await
    }

    /// [`Self::terminal_write`], sent now and its answer given back to
    /// be waited for apart ([`Self::send_raw`], and why typing doesn't
    /// go this way).
    pub fn send_terminal_write(&self, session_id: &str, block_id: &str, data: &str) -> Result<impl Future<Output = Result<RpcResult<Value>, RexError>> + Send + 'static + use<>, RexError> {
        self.send_raw("com.superlogical.terminal.write", Some(json!({ "session_id": session_id, "block_id": block_id, "args": { "data": data } })))
    }

    pub async fn terminal_resize(&self, session_id: &str, block_id: &str, columns: u16, rows: u16, cell_width_px: Option<u16>, cell_height_px: Option<u16>) -> Result<RpcResult<Value>, RexError> {
        let mut args = json!({ "columns": columns.max(1), "rows": rows.max(1) });
        if let (Some(cw), Some(ch)) = (cell_width_px, cell_height_px)
            && cw > 0
            && ch > 0
        {
            args["cell_width_px"] = json!(cw);
            args["cell_height_px"] = json!(ch);
        }
        self.call_block("com.superlogical.terminal.resize", session_id, block_id, Some(args)).await
    }

    pub async fn terminal_set_theme(
        &self,
        session_id: &str,
        block_id: &str,
        foreground: &str,
        background: &str,
        cursor: Option<&str>,
        palette: &[String],
        scheme: &str,
    ) -> Result<RpcResult<Value>, RexError> {
        let mut args = json!({
            "foreground": foreground,
            "background": background,
            "palette": palette,
            "scheme": scheme,
        });
        if let Some(c) = cursor {
            args["cursor"] = json!(c);
        }
        self.call_block("com.superlogical.terminal.set_theme", session_id, block_id, Some(args)).await
    }

    pub async fn terminal_clear(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_block("com.superlogical.terminal.clear", session_id, block_id, None).await
    }

    pub async fn terminal_reset(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_block("com.superlogical.terminal.reset", session_id, block_id, None).await
    }

    pub async fn terminal_ports(&self, session_id: &str, block_id: &str) -> Result<RpcResult<PortsSnapshot>, RexError> {
        self.call_block("com.superlogical.terminal.ports", session_id, block_id, None).await
    }

    pub async fn terminal_process(&self, session_id: &str, block_id: &str) -> Result<RpcResult<TerminalProcessSnapshot>, RexError> {
        self.call_block("com.superlogical.terminal.process", session_id, block_id, None).await
    }

    pub async fn terminal_program_status(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Vec<ProgramStatusRecord>>, RexError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(default)]
            records: Vec<ProgramStatusRecord>,
        }
        let r: RpcResult<Resp> = self.call_block("com.superlogical.terminal.program_status", session_id, block_id, None).await?;
        Ok(RpcResult { seq: r.seq, payload: r.payload.records })
    }

    pub async fn terminal_input_mode(&self, session_id: &str, block_id: &str) -> Result<RpcResult<InputMode>, RexError> {
        self.call_block("com.superlogical.terminal.input_mode", session_id, block_id, None).await
    }

    pub async fn terminal_list_dir(&self, session_id: &str, block_id: &str, path: &str, prefix: Option<&str>, files: bool) -> Result<RpcResult<DirListing>, RexError> {
        let mut args = json!({ "path": path, "files": files });
        if let Some(pre) = prefix
            && !pre.is_empty()
        {
            args["prefix"] = json!(pre);
        }
        self.call_block("com.superlogical.terminal.list_dir", session_id, block_id, Some(args)).await
    }

    pub async fn terminal_clipboard_read(&self, session_id: &str, block_id: &str, id: Option<u64>, offset: usize) -> Result<RpcResult<BlobReadResponse>, RexError> {
        let mut args = json!({ "offset": offset });
        if let Some(i) = id {
            args["id"] = json!(i);
        }
        self.call_block("com.superlogical.terminal.clipboard_read", session_id, block_id, Some(args)).await
    }

    pub async fn terminal_blob_read(&self, session_id: &str, block_id: &str, id: u64, offset: usize) -> Result<RpcResult<BlobReadResponse>, RexError> {
        self.call_block("com.superlogical.terminal.blob_read", session_id, block_id, Some(json!({ "id": id, "offset": offset }))).await
    }

    pub async fn terminal_download_begin(&self, session_id: &str, block_id: &str, data_ref: &str, path: &str, if_none_match: Option<&str>) -> Result<RpcResult<DownloadBeginResponse>, RexError> {
        let mut args = json!({ "ref": data_ref, "path": path });
        if let Some(v) = if_none_match {
            args["if_none_match"] = json!(v);
        }
        self.call_block("com.superlogical.terminal.download_begin", session_id, block_id, Some(args)).await
    }

    pub async fn terminal_upload_begin(
        &self,
        session_id: &str,
        block_id: &str,
        data_ref: &str,
        name: &str,
        size: u64,
        batch: Option<(&str, usize, usize)>,
    ) -> Result<RpcResult<UploadBeginResponse>, RexError> {
        let mut args = json!({ "ref": data_ref, "name": name, "size": size });
        if let Some((b_id, idx, count)) = batch {
            args["batch"] = json!(b_id);
            args["index"] = json!(idx);
            args["count"] = json!(count);
        }
        self.call_block("com.superlogical.terminal.upload_begin", session_id, block_id, Some(args)).await
    }

    pub async fn terminal_format(&self, session_id: &str, block_id: &str, format: &str) -> Result<RpcResult<String>, RexError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            content: String,
            id: Option<u64>,
        }
        let limit = self.max_message_bytes.saturating_sub(1024).clamp(1, 960 * 1024);
        let r: RpcResult<Resp> = self.call_block("com.superlogical.terminal.format", session_id, block_id, Some(json!({ "format": format, "trim": true, "max_bytes": limit }))).await?;
        let mut content = r.payload.content;
        if let Some(id) = r.payload.id {
            loop {
                let chunk = self.terminal_blob_read(session_id, block_id, id, content.len()).await?.payload;
                if chunk.id != id || (!chunk.eof && chunk.data.is_empty()) {
                    return Err(RexError::Protocol("com.superlogical.terminal.blob_read returned an inconsistent id or empty data before eof".into()));
                }
                content.push_str(&chunk.data);
                if chunk.eof {
                    break;
                }
            }
        }
        Ok(RpcResult { seq: r.seq, payload: content })
    }

    // ---- Tunnel & Onboarding Block Methods ----

    pub async fn tunnel_info(&self, session_id: &str, block_id: &str) -> Result<RpcResult<TunnelInfo>, RexError> {
        self.call_block("com.superlogical.tunnel.info", session_id, block_id, None).await
    }

    pub async fn onboarding_state(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_block("com.superlogical.onboarding.state", session_id, block_id, None).await
    }

    pub async fn onboarding_set_step(&self, session_id: &str, block_id: &str, step: &str) -> Result<RpcResult<Value>, RexError> {
        self.call_block("com.superlogical.onboarding.set_step", session_id, block_id, Some(json!({ "step": step }))).await
    }
}

fn split_placement(direction: &str) -> (&str, &str) {
    match direction {
        "left" => ("horizontal", "before"),
        "right" => ("horizontal", "after"),
        "above" | "up" => ("vertical", "before"),
        "below" | "down" => ("vertical", "after"),
        axis => (axis, "after"),
    }
}

fn decode<T: DeserializeOwned>(method: &str, value: Value) -> Result<T, RexError> {
    serde_path_to_error::deserialize(value).map_err(|e| RexError::Protocol(format!("{method} payload at {}: {}", e.path(), e.inner())))
}

async fn send_control<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>, msg: Message) -> Result<(), RexError> {
    time::timeout(DEFAULT_REQUEST_TIMEOUT, ws.send(msg)).await.map_err(|_| RexError::Timeout)??;
    Ok(())
}

fn response(method: &str, frame: &mut Value) -> Result<RpcResult<Value>, RexError> {
    let seq = frame.get("seq").and_then(Value::as_u64).ok_or_else(|| RexError::Protocol(format!("{method} response missing integer seq")))?;
    if let Some(error) = frame.get_mut("error").filter(|v| !v.is_null()) {
        return Err(RexError::Api(decode(method, error.take())?));
    }
    let payload = frame.get_mut("payload").ok_or_else(|| RexError::Protocol(format!("{method} response missing payload")))?;
    Ok(RpcResult { seq, payload: payload.take() })
}

async fn read_welcome<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>) -> Result<WelcomeMessage, RexError> {
    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Text(txt) => {
                let v: Value = serde_json::from_str(&txt)?;
                if v.get("type").and_then(Value::as_str) != Some("welcome") {
                    return Err(RexError::Protocol(format!("expected welcome, got frame type {:?}", v.get("type"))));
                }
                return decode("welcome", v);
            }
            Message::Ping(p) => {
                send_control(ws, Message::Pong(p)).await?;
            }
            Message::Close(cf) => {
                let reason = cf.map(|c| format!("{} {}", c.code, c.reason)).unwrap_or_else(|| "closed".to_owned());
                return Err(RexError::Closed(reason));
            }
            _ => {}
        }
    }
    Err(RexError::Closed("eof waiting for welcome".to_owned()))
}

async fn run_control_loop<S: AsyncRead + AsyncWrite + Unpin>(
    mut ws: WebSocketStream<S>,
    mut cmd_rx: mpsc::UnboundedReceiver<OutboundCmd>,
    evt_tx: mpsc::UnboundedSender<ControlEventEnvelope>,
    send_keepalives: bool,
    asked: Vec<(String, PendingCall)>,
) {
    // (Requests sent with the hello are waited for from the start.)
    let mut pending: HashMap<String, PendingCall> = asked.into_iter().collect();
    let mut last_frame = time::Instant::now();
    let mut keepalive = time::interval_at(last_frame + KEEPALIVE_INTERVAL, KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    // A probe that is out: when it gives up. (Any frame from the server
    // answers it.)
    let mut probe: Option<time::Instant> = None;

    let close_reason = loop {
        tokio::select! {
            _ = async { time::sleep_until(probe.unwrap_or_else(time::Instant::now)).await }, if probe.is_some() => {
                break "no reply to probe".to_owned();
            }
            _ = keepalive.tick(), if send_keepalives => {
                if last_frame.elapsed() >= KEEPALIVE_INTERVAL * 3 {
                    break "no reply to keepalive".to_owned();
                }
                if let Err(e) = send_control(&mut ws, Message::Ping(Vec::new().into())).await {
                    break format!("keepalive send failed: {e}");
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(OutboundCmd::Request { id, method, body, reply }) => {
                        if reply.is_closed() { continue; }
                        pending.insert(id, PendingCall { method, tx: reply });
                        if let Err(e) = send_control(&mut ws, Message::Text(body.into())).await {
                            break format!("send failed: {e}");
                        }
                    }
                    Some(OutboundCmd::Forget(id)) => { pending.remove(&id); }
                    Some(OutboundCmd::Probe) => {
                        if probe.is_none() {
                            if let Err(e) = send_control(&mut ws, Message::Ping(Vec::new().into())).await {
                                break format!("probe send failed: {e}");
                            }
                            probe = Some(time::Instant::now() + PROBE_TIMEOUT);
                        }
                    }
                    Some(OutboundCmd::Close) | None => {
                        let _ = send_control(&mut ws, Message::Close(Some(tungstenite::protocol::CloseFrame {
                            code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                            reason: "".into(),
                        }))).await;
                        break "closed by client".to_owned();
                    }
                }
            }
            frame = ws.next() => {
                last_frame = time::Instant::now();
                probe = None;
                match frame {
                    Some(Ok(Message::Text(txt))) => {
                        let mut val = match serde_json::from_str::<Value>(&txt) {
                            Ok(val) => val,
                            Err(e) => break format!("invalid control frame: {e}"),
                        };
                        match val.get("type").and_then(Value::as_str) {
                            Some("response") => {
                                let Some(id) = val.get("id").and_then(Value::as_str) else { break "response missing string id".into() };
                                if let Some(call) = pending.remove(id) {
                                    let _ = call.tx.send(response(&call.method, &mut val));
                                }
                            }
                            Some("event") => {
                                let seq = val.get("seq").and_then(Value::as_u64).unwrap_or(0);
                                if let Some(inner) = val.get_mut("payload") {
                                    let payload = inner.get_mut("payload").map(Value::take).unwrap_or(Value::Null);
                                    let kind = inner.get("type").and_then(Value::as_str).unwrap_or_default();
                                    let ev = ControlEvent::parse(kind, payload);
                                    let _ = evt_tx.send(ControlEventEnvelope { seq, event: ev });
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        if let Err(e) = send_control(&mut ws, Message::Pong(p)).await { break format!("pong failed: {e}") }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(cf))) => {
                        let r = cf.map(|c| format!("{} {}", c.code, c.reason)).unwrap_or_else(|| "closed".to_owned());
                        break r;
                    }
                    Some(Err(e)) => break format!("websocket error: {e}"),
                    None => break "eof".to_owned(),
                    _ => {}
                }
            }
        }
    };

    for (_, call) in pending.drain() {
        let _ = call.tx.send(Err(RexError::Closed(close_reason.clone())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_error_identifies_method_and_nested_field() {
        let e = decode::<ServerStatus>("server.status", json!({"pid":1,"instance_id":"test","tailscale":{"state":4}})).unwrap_err();
        assert!(e.to_string().contains("server.status payload at tailscale.state"), "{e}");
    }

    #[test]
    fn revision_19_build_fields_and_future_additions() {
        let v = decode::<ServerVersion>("server.version", json!({"rex_version":"0.1.0-snapshot.2506","built_at":"2026-11-21T15:05:34Z","platform":"linux_amd64","dirty":false,"future":true})).unwrap();
        assert_eq!(v.version, "0.1.0-snapshot.2506");
        assert_eq!(v.build_date, "2026-11-21T15:05:34Z");
        assert_eq!(v.platform, "linux_amd64");
        let actions = decode::<ActionResult>("client.action", json!({"client_actions":null})).unwrap();
        assert!(actions.client_actions.is_empty());
    }

    #[test]
    fn unknown_events_retain_the_whole_payload_and_session() {
        let v = json!({"new":["fields"]});
        assert!(matches!(ControlEvent::parse("future",v.clone()),ControlEvent::Unknown{kind,payload} if kind=="future" && payload==v));
        let nested = json!({"session_id":"s","event":{"type":"new_kind","payload":v},"extra":true});
        assert!(matches!(ControlEvent::parse("session_event",nested.clone()),ControlEvent::Unknown{kind,payload} if kind=="session_event.new_kind" && payload==nested));
        // (What rev 19 sends when a client begins and stops following a
        // session, as `rex events -s` shows them: known, not "unknown".)
        let joined = json!({"session_id":"s","event":{"type":"client_connected","payload":{"client_id":"client:1","principal":{"kind":"peer","transport":"unix","uid":1000},"connected_at":"2026-11-22T03:21:52-08:00"}}});
        assert!(matches!(ControlEvent::parse("session_event", joined), ControlEvent::SessionClientConnected { session_id, client_id } if session_id == "s" && client_id == "client:1"));
        let left = json!({"session_id":"s","event":{"type":"client_disconnected","payload":{"client_id":"client:1"}}});
        assert!(matches!(ControlEvent::parse("session_event", left), ControlEvent::SessionClientDisconnected { session_id, client_id } if session_id == "s" && client_id == "client:1"));
        let status = json!({"state":"starting","allow_tailnet_members":false,"https":{"state":"pending"}});
        for state in ["starting", "needs_login", "disabled"] {
            let mut value = status.clone();
            value["state"] = json!(state);
            if state == "disabled" {
                value.as_object_mut().unwrap().remove("https");
            }
            let reply: TailscaleReply = decode("server.tailscale.enable", json!({"tailscale":value})).unwrap();
            assert_eq!(reply.tailscale.state, state);
        }
        assert!(matches!(ControlEvent::parse("tailscale_status_changed",status),ControlEvent::TailscaleStatusChanged(s) if s.state=="starting"));
    }
}
