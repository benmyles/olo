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
    error::{ApiError, RexError},
    model::{
        ActionResult, AttachResponse, BlobReadResponse, BootstrapResponse, CONTROL_SUBPROTOCOL, ClientIdentity, ConnectedClient, ControlEvent, ControlEventEnvelope, DataConnectResponse,
        DataTicketRequest, DirListing, DownloadBeginResponse, HostRecord, HostsListResponse, InputMode, KeyEvent, KeymapAction, KeymapLayer, KeymapResponse, PortsSnapshot, ProgramStatusRecord,
        REX_API_MIN_REVISION, REX_API_REVISION, ServerStatus, ServerVersion, SessionSummary, SessionView, TailscaleStatus, TerminalProcessSnapshot, TunnelInfo, UploadBeginResponse, WelcomeMessage,
    },
};

const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(120);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Result of an RPC call paired with the server's `seq` watermark.
#[derive(Debug, Clone)]
pub struct RpcResult<T> {
    pub seq: u64,
    pub payload: T,
}

struct PendingCall {
    tx: oneshot::Sender<Result<RpcResult<Value>, RexError>>,
}

enum OutboundCmd {
    Request { id: String, body: String, reply: oneshot::Sender<Result<RpcResult<Value>, RexError>> },
    Close,
}

/// Live connection to a Rex server's `/api/control` WebSocket (`rex.control.v1`).
#[derive(Clone)]
pub struct ControlClient {
    cmd_tx: mpsc::UnboundedSender<OutboundCmd>,
    next_id: Arc<AtomicU64>,
    pub welcome: Arc<WelcomeMessage>,
    pub max_message_bytes: usize,
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

        let (mut ws, _resp) = tokio_tungstenite::client_async(req, stream).await?;

        // Build uncompressed < 512 B `hello` message.
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
        let hello_text = serde_json::to_string(&hello)?;
        if hello_text.len() >= 512 {
            // Trim optional label if needed so hello is strictly < 512 bytes.
            let mut trimmed = json!({
                "type": "hello",
                "api_revision": REX_API_REVISION,
                "api_min_revision": REX_API_MIN_REVISION,
                "client": {
                    "kind": "app",
                    "platform": "linux",
                    "name": "Olo Rex",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            });
            if let Some(tok) = reconnect_token
                && !tok.is_empty()
            {
                trimmed["reconnect_token"] = Value::String(tok.to_owned());
            }
            let t = serde_json::to_string(&trimmed)?;
            ws.send(Message::Text(t.into())).await?;
        } else {
            ws.send(Message::Text(hello_text.into())).await?;
        }

        // Wait up to 5 seconds for `welcome`.
        let welcome = time::timeout(Duration::from_secs(5), read_welcome(&mut ws)).await.map_err(|_| RexError::Timeout)??;
        if welcome.protocol != CONTROL_SUBPROTOCOL {
            return Err(RexError::Protocol(format!("unexpected control protocol: {}", welcome.protocol)));
        }

        let max_message_bytes = welcome.limits.max_message_bytes.max(65_536);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (evt_tx, evt_rx) = mpsc::unbounded_channel();

        tokio::spawn(run_control_loop(ws, cmd_rx, evt_tx));

        Ok((Self { cmd_tx, next_id: Arc::new(AtomicU64::new(1)), welcome: Arc::new(welcome), max_message_bytes }, evt_rx))
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
        self.cmd_tx.send(OutboundCmd::Request { id, body, reply: tx }).map_err(|_| RexError::Closed("control loop stopped".to_owned()))?;
        time::timeout(timeout, rx).await.map_err(|_| RexError::Timeout)?.map_err(|_| RexError::Closed("control loop dropped reply".to_owned()))?
    }

    /// Typed RPC call deserializing `payload` into `T`.
    pub async fn call<T: DeserializeOwned>(&self, method: &str, payload: Option<Value>) -> Result<RpcResult<T>, RexError> {
        let res = self.call_raw(method, payload).await?;
        let typed = serde_json::from_value(res.payload)?;
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
        self.call("server.config_reload", None).await
    }

    pub async fn server_stop(&self) -> Result<RpcResult<Value>, RexError> {
        self.call("server.stop", None).await
    }

    pub async fn hosts_list(&self) -> Result<RpcResult<HostsListResponse>, RexError> {
        self.call("hosts.list", None).await
    }

    pub async fn hosts_put(&self, host: &HostRecord) -> Result<RpcResult<Value>, RexError> {
        self.call("hosts.put", Some(json!({ "host": host }))).await
    }

    pub async fn hosts_remove(&self, id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("hosts.remove", Some(json!({ "id": id }))).await
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
            p.insert("tags".into(), json!(t));
        }
        if let Some(u) = control_url {
            p.insert("control_url".into(), json!(u));
        }
        self.call("server.tailscale.enable", Some(Value::Object(p))).await
    }

    pub async fn tailscale_disable(&self) -> Result<RpcResult<TailscaleStatus>, RexError> {
        self.call("server.tailscale.disable", None).await
    }

    pub async fn tailscale_login(&self) -> Result<RpcResult<TailscaleStatus>, RexError> {
        self.call("server.tailscale.login", None).await
    }

    pub async fn tailscale_logout(&self) -> Result<RpcResult<TailscaleStatus>, RexError> {
        self.call("server.tailscale.logout", None).await
    }

    pub async fn tailscale_rename(&self, hostname: &str) -> Result<RpcResult<TailscaleStatus>, RexError> {
        self.call("server.tailscale.rename", Some(json!({ "hostname": hostname }))).await
    }

    // ---- Session & Layout Methods ----

    pub async fn session_bootstrap(
        &self,
        event_scope: &str,
        preferred_session_id: Option<&str>,
        attach_session_ids: Option<&[String]>,
        data_tickets: &[DataTicketRequest],
    ) -> Result<RpcResult<BootstrapResponse>, RexError> {
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
        self.call("session.bootstrap", Some(p)).await
    }

    pub async fn session_list(&self) -> Result<RpcResult<Vec<SessionSummary>>, RexError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(default)]
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
        self.call("session.create", Some(p)).await
    }

    pub async fn session_attach(&self, session_id: &str) -> Result<RpcResult<AttachResponse>, RexError> {
        self.call("session.attach", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_detach(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.detach", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_destroy(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.destroy", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_view(&self, session_id: &str) -> Result<RpcResult<SessionView>, RexError> {
        self.call("session.view", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_set_label(&self, session_id: &str, label: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.set_label", Some(json!({ "session_id": session_id, "label": label }))).await
    }

    pub async fn session_move(&self, session_id: &str, before_session_id: Option<&str>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id });
        if let Some(b) = before_session_id {
            p["before_session_id"] = json!(b);
        }
        self.call("session.move", Some(p)).await
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
        self.call("session.new_window", Some(p)).await
    }

    pub async fn session_close_window(&self, session_id: &str, window_id: Option<&str>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id });
        if let Some(w) = window_id {
            p["window_id"] = json!(w);
        }
        self.call("session.close_window", Some(p)).await
    }

    pub async fn session_set_window_label(&self, session_id: &str, window_id: &str, label: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.set_window_label", Some(json!({ "session_id": session_id, "window_id": window_id, "label": label }))).await
    }

    pub async fn session_focus_window(&self, session_id: &str, window_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.focus_window", Some(json!({ "session_id": session_id, "window_id": window_id }))).await
    }

    pub async fn session_move_window(&self, session_id: &str, window_id: &str, before_window_id: Option<&str>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id, "window_id": window_id });
        if let Some(b) = before_window_id {
            p["before_window_id"] = json!(b);
        }
        self.call("session.move_window", Some(p)).await
    }

    pub async fn session_new_split(&self, session_id: &str, target_block_id: Option<&str>, direction: &str, shell_options: Option<Value>) -> Result<RpcResult<Value>, RexError> {
        let mut block = json!({ "flavor": "com.superlogical.terminal.shell" });
        if let Some(opts) = shell_options {
            block["options"] = opts;
        }
        let mut p = json!({
            "session_id": session_id,
            "direction": direction,
            "block": block
        });
        if let Some(b) = target_block_id {
            p["block_id"] = json!(b);
        }
        self.call("session.new_split", Some(p)).await
    }

    pub async fn session_resize_pane(&self, session_id: &str, split_id: &str, ratio: f64) -> Result<RpcResult<Value>, RexError> {
        self.call("session.resize_pane", Some(json!({ "session_id": session_id, "split_id": split_id, "ratio": ratio }))).await
    }

    pub async fn session_focus_block(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.focus_block", Some(json!({ "session_id": session_id, "block_id": block_id }))).await
    }

    pub async fn session_focus_direction(&self, session_id: &str, direction: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.focus_direction", Some(json!({ "session_id": session_id, "direction": direction }))).await
    }

    pub async fn session_focus_next_pane(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.focus_next_pane", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_focus_previous_pane(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.focus_previous_pane", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_focus_next_window(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.focus_next_window", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_focus_previous_window(&self, session_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.focus_previous_window", Some(json!({ "session_id": session_id }))).await
    }

    pub async fn session_zoom_block(&self, session_id: &str, block_id: Option<&str>, zoom: Option<bool>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id });
        if let Some(b) = block_id {
            p["block_id"] = json!(b);
        }
        if let Some(z) = zoom {
            p["zoom"] = json!(z);
        }
        self.call("session.zoom_block", Some(p)).await
    }

    pub async fn session_swap_blocks(&self, session_id: &str, block_id: &str, other_block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.swap_blocks", Some(json!({ "session_id": session_id, "block_id": block_id, "other_block_id": other_block_id }))).await
    }

    pub async fn session_move_block(&self, session_id: &str, block_id: &str, target_block_id: &str, direction: &str) -> Result<RpcResult<Value>, RexError> {
        self.call(
            "session.move_block",
            Some(json!({
                "session_id": session_id,
                "block_id": block_id,
                "target_block_id": target_block_id,
                "direction": direction,
            })),
        )
        .await
    }

    pub async fn session_own_block(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.own_block", Some(json!({ "session_id": session_id, "block_id": block_id }))).await
    }

    pub async fn session_disown_block(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.disown_block", Some(json!({ "session_id": session_id, "block_id": block_id }))).await
    }

    pub async fn session_set_block_label(&self, session_id: &str, block_id: &str, label: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.set_block_label", Some(json!({ "session_id": session_id, "block_id": block_id, "label": label }))).await
    }

    pub async fn session_new_block(&self, session_id: &str, flavor: &str, options: Option<Value>, label: Option<&str>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id, "flavor": flavor });
        if let Some(o) = options {
            p["options"] = o;
        }
        if let Some(l) = label {
            p["label"] = json!(l);
        }
        self.call("session.new_block", Some(p)).await
    }

    pub async fn block_close(&self, session_id: &str, block_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("block.close", Some(json!({ "session_id": session_id, "block_id": block_id }))).await
    }

    // ---- Floating Layer Methods ----

    pub async fn session_float_block(&self, session_id: &str, block_id: &str, bounds: Option<[f64; 4]>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id, "block_id": block_id });
        if let Some(b) = bounds {
            p["bounds"] = json!(b);
        }
        self.call("session.float_block", Some(p)).await
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
        self.call("session.new_layer", Some(p)).await
    }

    pub async fn session_resize_layer(&self, session_id: &str, layer_id: &str, bounds: [f64; 4]) -> Result<RpcResult<Value>, RexError> {
        self.call("session.resize_layer", Some(json!({ "session_id": session_id, "layer_id": layer_id, "bounds": bounds }))).await
    }

    pub async fn session_set_layer_state(&self, session_id: &str, layer_id: &str, state: &str, focus: bool) -> Result<RpcResult<Value>, RexError> {
        self.call("session.set_layer_state", Some(json!({ "session_id": session_id, "layer_id": layer_id, "state": state, "focus": focus }))).await
    }

    pub async fn session_raise_layer(&self, session_id: &str, layer_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.raise_layer", Some(json!({ "session_id": session_id, "layer_id": layer_id }))).await
    }

    pub async fn session_lower_layer(&self, session_id: &str, layer_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.lower_layer", Some(json!({ "session_id": session_id, "layer_id": layer_id }))).await
    }

    pub async fn session_close_layer(&self, session_id: &str, layer_id: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("session.close_layer", Some(json!({ "session_id": session_id, "layer_id": layer_id }))).await
    }

    // ---- Focus History & Client / Keymap / Prompts ----

    pub async fn focus_history_reverse(&self, session_id: &str, scope: Option<&str>) -> Result<RpcResult<Value>, RexError> {
        let mut p = json!({ "session_id": session_id });
        if let Some(s) = scope {
            p["scope"] = json!(s);
        }
        self.call("focus_history.reverse", Some(p)).await
    }

    pub async fn client_list(&self) -> Result<RpcResult<Vec<ConnectedClient>>, RexError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(default)]
            clients: Vec<ConnectedClient>,
        }
        let r: RpcResult<Resp> = self.call("client.list", None).await?;
        Ok(RpcResult { seq: r.seq, payload: r.payload.clients })
    }

    pub async fn client_set_label(&self, label: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("client.set_label", Some(json!({ "label": label }))).await
    }

    pub async fn client_key(&self, session_id: &str, block_id: Option<&str>, key: &KeyEvent, prefix: Option<&[String]>) -> Result<RpcResult<bool>, RexError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            handled: bool,
        }
        let mut p = json!({ "session_id": session_id, "key": key });
        if let Some(b) = block_id {
            p["block_id"] = json!(b);
        }
        if let Some(pre) = prefix
            && !pre.is_empty()
        {
            p["prefix"] = json!(pre);
        }
        let r: RpcResult<Resp> = self.call("client.key", Some(p)).await?;
        Ok(RpcResult { seq: r.seq, payload: r.payload.handled })
    }

    pub async fn client_keymap(&self, actions: &[KeymapAction], defaults: &[KeymapLayer], overrides: &[KeymapLayer]) -> Result<RpcResult<KeymapResponse>, RexError> {
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
        self.call("client.keymap", Some(p)).await
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
        self.call("client.reply", Some(json!({ "call_id": call_id, "result": result }))).await
    }

    pub async fn client_reply_err(&self, call_id: &str, code: &str, message: &str) -> Result<RpcResult<Value>, RexError> {
        self.call("client.reply", Some(json!({ "call_id": call_id, "error": { "code": code, "message": message } }))).await
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
        }
        let r: RpcResult<Resp> = self.call_block("com.superlogical.terminal.format", session_id, block_id, Some(json!({ "format": format, "trim": true }))).await?;
        Ok(RpcResult { seq: r.seq, payload: r.payload.content })
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

async fn read_welcome<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>) -> Result<WelcomeMessage, RexError> {
    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Text(txt) => {
                let v: Value = serde_json::from_str(&txt)?;
                if v.get("type").and_then(Value::as_str) != Some("welcome") {
                    return Err(RexError::Protocol(format!("expected welcome, got {txt}")));
                }
                return Ok(serde_json::from_value(v)?);
            }
            Message::Ping(p) => {
                let _ = ws.send(Message::Pong(p)).await;
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

async fn run_control_loop<S: AsyncRead + AsyncWrite + Unpin>(mut ws: WebSocketStream<S>, mut cmd_rx: mpsc::UnboundedReceiver<OutboundCmd>, evt_tx: mpsc::UnboundedSender<ControlEventEnvelope>) {
    let mut pending: HashMap<String, PendingCall> = HashMap::new();
    let mut keepalive = time::interval(KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    keepalive.tick().await; // consume initial immediate tick

    let close_reason = loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if let Err(e) = ws.send(Message::Ping(Vec::new().into())).await {
                    break format!("keepalive send failed: {e}");
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(OutboundCmd::Request { id, body, reply }) => {
                        pending.insert(id.clone(), PendingCall { tx: reply });
                        if let Err(e) = ws.send(Message::Text(body.into())).await {
                            if let Some(call) = pending.remove(&id) {
                                let _ = call.tx.send(Err(RexError::WebSocket(e)));
                            }
                            break "send failed".to_owned();
                        }
                    }
                    Some(OutboundCmd::Close) | None => {
                        let _ = ws.close(Some(tungstenite::protocol::CloseFrame {
                            code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                            reason: "".into(),
                        })).await;
                        break "closed by client".to_owned();
                    }
                }
            }
            frame = ws.next() => {
                match frame {
                    Some(Ok(Message::Text(txt))) => {
                        let Ok(val) = serde_json::from_str::<Value>(&txt) else { continue };
                        match val.get("type").and_then(Value::as_str) {
                            Some("response") => {
                                let Some(id) = val.get("id").and_then(Value::as_str) else { continue };
                                let seq = val.get("seq").and_then(Value::as_u64).unwrap_or(0);
                                if let Some(call) = pending.remove(id) {
                                    if let Some(err_val) = val.get("error").filter(|v| !v.is_null()) {
                                        let api_err: ApiError = serde_json::from_value(err_val.clone())
                                            .unwrap_or(ApiError {
                                                code: "internal".to_owned(),
                                                message: err_val.to_string(),
                                                status: 500,
                                            });
                                        let _ = call.tx.send(Err(RexError::Api(api_err)));
                                    } else {
                                        let payload = val.get("payload").cloned().unwrap_or(Value::Object(Default::default()));
                                        let _ = call.tx.send(Ok(RpcResult { seq, payload }));
                                    }
                                }
                            }
                            Some("event") => {
                                let seq = val.get("seq").and_then(Value::as_u64).unwrap_or(0);
                                if let Some(inner) = val.get("payload") {
                                    let kind = inner.get("type").and_then(Value::as_str).unwrap_or_default();
                                    let payload = inner.get("payload").cloned().unwrap_or(Value::Null);
                                    let ev = ControlEvent::parse(kind, payload);
                                    let _ = evt_tx.send(ControlEventEnvelope { seq, event: ev });
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = ws.send(Message::Pong(p)).await;
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
