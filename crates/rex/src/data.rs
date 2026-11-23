//! Rex binary data plane (`GET /api/data`) for both:
//! - `rex.data.terminal.v1`: server-to-client Ghostty snapshot + live VT bytes +
//!   inline nonce-authenticated `ESC _ rex1;<nonce>;<json> ESC \` APCs, with
//!   exact byte counting for lossless 60 s stream resume.
//! - `rex.data.bytes.v1`: bidirectional raw bytes with 0-byte binary frame
//!   half-close EOF for uploads, downloads, and `-L`/`-R` port-forwarding tunnels.

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

use crate::{
    error::{DataRejectReason, RexError},
    model::{DATA_BYTES_SUBPROTOCOL, DATA_TERMINAL_SUBPROTOCOL, InlineApc},
};

const MSG_HELLO: u8 = 0x01;
const MSG_RESUME: u8 = 0x02;
const MSG_WELCOME: u8 = 0x81;
const MSG_REJECT: u8 = 0x82;

/// Decoded 32-byte `WELCOME` frame from `/api/data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataWelcome {
    pub reconnect_token: [u8; 16],
    pub resume_ttl_ms: u32,
    pub channel_nonce: u64,
}

/// Encode a 17-byte `HELLO` binary handshake frame from a base64url ticket.
pub fn encode_hello(ticket_b64url: &str) -> Result<[u8; 17], RexError> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(ticket_b64url.trim_end_matches('=')).map_err(|e| RexError::Protocol(format!("invalid base64url ticket: {e}")))?;
    if raw.len() != 16 {
        return Err(RexError::Protocol(format!("expected 16-byte ticket, got {}", raw.len())));
    }
    let mut frame = [0u8; 17];
    frame[0] = MSG_HELLO;
    frame[1..17].copy_from_slice(&raw);
    Ok(frame)
}

/// Encode a 25-byte `RESUME` binary handshake frame.
pub fn encode_resume(reconnect_token: &[u8; 16], bytes_consumed: u64) -> [u8; 25] {
    let mut frame = [0u8; 25];
    frame[0] = MSG_RESUME;
    frame[1..17].copy_from_slice(reconnect_token);
    frame[17..25].copy_from_slice(&bytes_consumed.to_le_bytes());
    frame
}

/// Parse a `WELCOME` (32 B) or `REJECT` (2 B) handshake answer.
pub fn decode_welcome_or_reject(frame: &[u8]) -> Result<DataWelcome, RexError> {
    if frame.is_empty() {
        return Err(RexError::Protocol("empty data handshake response".to_owned()));
    }
    match frame[0] {
        MSG_WELCOME => {
            if frame.len() != 32 {
                return Err(RexError::Protocol(format!("expected 32-byte WELCOME, got {}", frame.len())));
            }
            let mut reconnect_token = [0u8; 16];
            reconnect_token.copy_from_slice(&frame[1..17]);
            let resume_ttl_ms = u32::from_le_bytes(frame[20..24].try_into().unwrap());
            let channel_nonce = u64::from_le_bytes(frame[24..32].try_into().unwrap());
            if channel_nonce == 0 {
                return Err(RexError::Protocol("WELCOME channel_nonce must be non-zero".to_owned()));
            }
            Ok(DataWelcome { reconnect_token, resume_ttl_ms, channel_nonce })
        }
        MSG_REJECT => {
            let reason = frame.get(1).copied().map(DataRejectReason::from_byte).unwrap_or(DataRejectReason::Unknown);
            Err(RexError::DataRejected(reason))
        }
        other => Err(RexError::Protocol(format!("unexpected data handshake opcode 0x{other:02x}"))),
    }
}

/// Perform the WebSocket upgrade to `/api/data` with `subprotocol` (never compressed).
pub async fn upgrade_data_ws<S>(stream: S, host_header: &str, subprotocol: &str) -> Result<WebSocketStream<S>, RexError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let req = http::Request::builder()
        .uri(format!("ws://{host_header}/api/data"))
        .header("Host", host_header)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tungstenite::handshake::client::generate_key())
        .header("Sec-WebSocket-Protocol", subprotocol)
        .body(())
        .map_err(|e| RexError::Protocol(format!("request build failed: {e}")))?;

    let (ws, _resp) = tokio::time::timeout(std::time::Duration::from_secs(5), tokio_tungstenite::client_async(req, stream)).await.map_err(|_| RexError::Timeout)??;
    Ok(ws)
}

async fn perform_data_handshake<S>(ws: &mut WebSocketStream<S>, handshake_bytes: &[u8]) -> Result<DataWelcome, RexError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tokio::time::timeout(std::time::Duration::from_secs(5), exchange_data_hello(ws, handshake_bytes)).await.map_err(|_| RexError::Timeout)?
}

async fn exchange_data_hello<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>, handshake_bytes: &[u8]) -> Result<DataWelcome, RexError> {
    ws.send(Message::Binary(handshake_bytes.to_vec().into())).await?;
    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Binary(b) => return decode_welcome_or_reject(&b),
            Message::Ping(p) => {
                ws.send(Message::Pong(p)).await?;
            }
            Message::Pong(_) => {}
            Message::Close(cf) => {
                let reason = cf.map(|c| format!("{} {}", c.code, c.reason)).unwrap_or_else(|| "closed".to_owned());
                return Err(RexError::Closed(reason));
            }
            _ => {}
        }
    }
    Err(RexError::Closed("eof during data handshake".to_owned()))
}

/// Server-to-client terminal data stream (`rex.data.terminal.v1`) with exact byte
/// counting for lossless resume.
pub struct TerminalDataStream<S> {
    ws: WebSocketStream<S>,
    pub welcome: DataWelcome,
    pub bytes_received: u64,
}

/// Event yielded from reading a [`TerminalDataStream`].
#[derive(Debug, Clone)]
pub enum TerminalStreamFrame {
    Bytes(Vec<u8>),
    Closed { code: u16, reason: String },
}

/// A terminal's stream opened as far as its upgrade
/// ([`TerminalDataStream::upgrade`]): the ticket that says which
/// terminal's it is goes in the hello that follows
/// ([`Self::hello`]), so the stream can be opened before the ticket is
/// had, while it is being asked for: a round trip of the link saved
/// (three over SSH, where a stream is a channel opened first), which
/// on a slow link is most of what a terminal waits to show.
pub struct TerminalUpgrade<S> {
    ws: WebSocketStream<S>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> TerminalUpgrade<S> {
    /// The stream becomes the terminal's whose single-use ticket this is.
    pub async fn hello(mut self, ticket: &str) -> Result<TerminalDataStream<S>, RexError> {
        let hello = encode_hello(ticket)?;
        let welcome = perform_data_handshake(&mut self.ws, &hello).await?;
        Ok(TerminalDataStream { ws: self.ws, welcome, bytes_received: 0 })
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> TerminalDataStream<S> {
    /// Open a fresh terminal stream using a single-use base64url ticket.
    pub async fn connect_hello(stream: S, host_header: &str, ticket: &str) -> Result<Self, RexError> {
        Self::upgrade(stream, host_header).await?.hello(ticket).await
    }

    /// A terminal stream opened as far as its upgrade, for a ticket
    /// still to come ([`TerminalUpgrade`]).
    pub async fn upgrade(stream: S, host_header: &str) -> Result<TerminalUpgrade<S>, RexError> {
        Ok(TerminalUpgrade { ws: upgrade_data_ws(stream, host_header, DATA_TERMINAL_SUBPROTOCOL).await? })
    }

    /// Resume an interrupted terminal stream from `bytes_received` using the
    /// latest reconnect token.
    pub async fn connect_resume(stream: S, host_header: &str, reconnect_token: &[u8; 16], bytes_received: u64) -> Result<Self, RexError> {
        let mut ws = upgrade_data_ws(stream, host_header, DATA_TERMINAL_SUBPROTOCOL).await?;
        let resume = encode_resume(reconnect_token, bytes_received);
        let welcome = perform_data_handshake(&mut ws, &resume).await?;
        Ok(Self { ws, welcome, bytes_received })
    }

    /// Read the next stream chunk (auto-answering pings) and advance `bytes_received`.
    pub async fn next_frame(&mut self) -> Result<TerminalStreamFrame, RexError> {
        while let Some(msg) = self.ws.next().await {
            match msg? {
                Message::Binary(b) => {
                    self.bytes_received += b.len() as u64;
                    return Ok(TerminalStreamFrame::Bytes(b.to_vec()));
                }
                Message::Ping(p) => {
                    self.ws.send(Message::Pong(p)).await?;
                }
                Message::Pong(_) => {}
                Message::Close(cf) => {
                    let (code, reason) = cf.map(|c| (u16::from(c.code), c.reason.to_string())).unwrap_or((1000, String::new()));
                    return Ok(TerminalStreamFrame::Closed { code, reason });
                }
                _ => {}
            }
        }
        Ok(TerminalStreamFrame::Closed { code: 1006, reason: "eof".to_owned() })
    }

    /// Close cleanly with status 1000 (tells the server to end the stream immediately).
    pub async fn close_clean(mut self) {
        let _ = self.ws.close(Some(tungstenite::protocol::CloseFrame { code: tungstenite::protocol::frame::coding::CloseCode::Normal, reason: "".into() })).await;
    }
}

/// Output segment produced by [`InlineApcDemuxer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DemuxItem {
    /// Raw VT bytes to feed directly to `GhosttyTerminal::vt_write`.
    Vt(Vec<u8>),
    /// Authenticated inline Rex APC (`resize`, `theme`, `child_exited`).
    Apc(InlineApc),
}

/// Streaming demultiplexer for post-snapshot `rex.data.terminal.v1` bytes that
/// extracts nonce-authenticated `ESC _ rex1;<decimal_nonce>;<json> ESC \`
/// sequences while passing all other bytes (including spoofed/unmatched APCs)
/// cleanly through to the VT parser.
pub struct InlineApcDemuxer {
    expected_prefix: Vec<u8>,
    state: DemuxState,
    buf: Vec<u8>,
}

enum DemuxState {
    Normal,
    SawEsc,
    InApcMatchingPrefix,
    InApcBody,
    InApcBodySawEsc,
}

impl InlineApcDemuxer {
    pub fn new(channel_nonce: u64) -> Self {
        let expected_prefix = format!("rex1;{channel_nonce};").into_bytes();
        Self { expected_prefix, state: DemuxState::Normal, buf: Vec::with_capacity(256) }
    }

    pub fn push(&mut self, input: &[u8]) -> Vec<DemuxItem> {
        let mut out = Vec::new();
        let mut vt = Vec::new();

        let flush_vt = |vt: &mut Vec<u8>, out: &mut Vec<DemuxItem>| {
            if !vt.is_empty() {
                out.push(DemuxItem::Vt(std::mem::take(vt)));
            }
        };

        for &b in input {
            match self.state {
                DemuxState::Normal => {
                    if b == 0x1b {
                        self.state = DemuxState::SawEsc;
                    } else {
                        vt.push(b);
                    }
                }
                DemuxState::SawEsc => {
                    if b == b'_' {
                        self.buf.clear();
                        self.state = DemuxState::InApcMatchingPrefix;
                    } else if b == 0x1b {
                        vt.push(0x1b);
                        // remain in SawEsc
                    } else {
                        vt.push(0x1b);
                        vt.push(b);
                        self.state = DemuxState::Normal;
                    }
                }
                DemuxState::InApcMatchingPrefix => {
                    let idx = self.buf.len();
                    if idx < self.expected_prefix.len() && b == self.expected_prefix[idx] {
                        self.buf.push(b);
                        if self.buf.len() == self.expected_prefix.len() {
                            self.buf.clear();
                            self.state = DemuxState::InApcBody;
                        }
                    } else {
                        // Not a matching Rex APC: replay `ESC _` + buffered prefix bytes + current byte.
                        vt.push(0x1b);
                        vt.push(b'_');
                        vt.extend_from_slice(&self.buf);
                        self.buf.clear();
                        if b == 0x1b {
                            self.state = DemuxState::SawEsc;
                        } else {
                            vt.push(b);
                            self.state = DemuxState::Normal;
                        }
                    }
                }
                DemuxState::InApcBody => {
                    if b == 0x1b {
                        self.state = DemuxState::InApcBodySawEsc;
                    } else if self.buf.len() < 1024 * 1024 {
                        self.buf.push(b);
                    } else {
                        // Guard against malformed runaway APC; flush as VT.
                        vt.push(0x1b);
                        vt.push(b'_');
                        vt.extend_from_slice(&self.expected_prefix);
                        vt.extend_from_slice(&self.buf);
                        vt.push(b);
                        self.buf.clear();
                        self.state = DemuxState::Normal;
                    }
                }
                DemuxState::InApcBodySawEsc => {
                    if b == b'\\' {
                        flush_vt(&mut vt, &mut out);
                        if let Some(apc) = parse_inline_apc(&self.buf) {
                            out.push(DemuxItem::Apc(apc));
                        }
                        self.buf.clear();
                        self.state = DemuxState::Normal;
                    } else {
                        self.buf.push(0x1b);
                        if b == 0x1b {
                            // stay in InApcBodySawEsc
                        } else {
                            self.buf.push(b);
                            self.state = DemuxState::InApcBody;
                        }
                    }
                }
            }
        }

        flush_vt(&mut vt, &mut out);
        out
    }
}

fn parse_inline_apc(json_bytes: &[u8]) -> Option<InlineApc> {
    let val: Value = serde_json::from_slice(json_bytes).ok()?;
    let kind = val.get("type").and_then(Value::as_str)?;
    match kind {
        "resize" => Some(InlineApc::Resize {
            cols: val.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16,
            rows: val.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16,
            cell_width_px: val.get("cell_width_px").and_then(Value::as_u64).unwrap_or(0) as u32,
            cell_height_px: val.get("cell_height_px").and_then(Value::as_u64).unwrap_or(0) as u32,
            owner: val.get("owner").and_then(Value::as_str).map(str::to_owned),
        }),
        "theme" => {
            let palette = val.get("palette").and_then(Value::as_array).map(|arr| arr.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
            Some(InlineApc::Theme {
                foreground: val.get("foreground").and_then(Value::as_str).map(str::to_owned),
                background: val.get("background").and_then(Value::as_str).map(str::to_owned),
                cursor: val.get("cursor").and_then(Value::as_str).map(str::to_owned),
                palette,
                scheme: val.get("scheme").and_then(Value::as_str).map(str::to_owned),
            })
        }
        "child_exited" => Some(InlineApc::ChildExited { exit_code: val.get("exit_code").and_then(Value::as_i64).unwrap_or(-1) as i32 }),
        _ => Some(InlineApc::Unknown(val)),
    }
}

/// Bidirectional raw byte stream (`rex.data.bytes.v1`) with 0-byte binary frame
/// half-close EOF semantics, used for uploads, downloads, and tunnels.
pub struct BytesDataStream<S> {
    read_finished: bool,
    ws: WebSocketStream<S>,
    pub welcome: DataWelcome,
}

impl<S: AsyncRead + AsyncWrite + Unpin> BytesDataStream<S> {
    pub async fn connect_hello(stream: S, host_header: &str, ticket: &str) -> Result<Self, RexError> {
        let mut ws = upgrade_data_ws(stream, host_header, DATA_BYTES_SUBPROTOCOL).await?;
        let hello = encode_hello(ticket)?;
        let welcome = perform_data_handshake(&mut ws, &hello).await?;
        Ok(Self { ws, welcome, read_finished: false })
    }

    async fn send_chunk(&mut self, data: Vec<u8>) -> Result<(), RexError> {
        tokio::time::timeout(std::time::Duration::from_secs(30), self.ws.send(Message::Binary(data.into()))).await.map_err(|_| RexError::Timeout)??;
        Ok(())
    }

    /// Write raw bytes in chunks up to 64 KiB.
    pub async fn write_all(&mut self, data: &[u8]) -> Result<(), RexError> {
        for chunk in data.chunks(65_536) {
            if !chunk.is_empty() {
                self.send_chunk(chunk.to_vec()).await?;
            }
        }
        Ok(())
    }

    /// Signal half-close EOF on the write direction (0-byte binary frame).
    pub async fn finish_write(&mut self) -> Result<(), RexError> {
        self.send_chunk(Vec::new()).await
    }

    /// Read next raw byte chunk. Returns `Ok(None)` on 0-byte EOF or clean close.
    pub async fn read_chunk(&mut self) -> Result<Option<Vec<u8>>, RexError> {
        tokio::time::timeout(std::time::Duration::from_secs(30), self.read_one_chunk()).await.map_err(|_| RexError::Timeout)?
    }

    async fn read_one_chunk(&mut self) -> Result<Option<Vec<u8>>, RexError> {
        if self.read_finished {
            return Ok(None);
        }
        while let Some(msg) = self.ws.next().await {
            match msg? {
                Message::Binary(b) => {
                    if b.is_empty() {
                        self.read_finished = true;
                        return Ok(None);
                    }
                    return Ok(Some(b.to_vec()));
                }
                Message::Ping(p) => {
                    self.ws.send(Message::Pong(p)).await?;
                }
                Message::Pong(_) => {}
                Message::Close(Some(frame)) if frame.code == tungstenite::protocol::frame::coding::CloseCode::Normal => {
                    self.read_finished = true;
                    return Ok(None);
                }
                Message::Close(frame) => {
                    return Err(RexError::Closed(frame.map(|f| format!("{} {}", f.code, f.reason)).unwrap_or_else(|| "byte stream closed without a status".into())));
                }
                _ => {}
            }
        }
        Err(RexError::Closed("eof before the end of the byte stream".into()))
    }

    /// Split into independent sink/stream halves for full-duplex tunnel relaying.
    pub fn into_inner(self) -> WebSocketStream<S> {
        self.ws
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tungstenite::protocol::{CloseFrame, Role, frame::coding::CloseCode};

    #[tokio::test]
    async fn full_tunnel_close_after_half_close_releases_idle_writer() {
        let (client, peer) = tokio::io::duplex(1024);
        let ws = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        let bytes = BytesDataStream { ws, welcome: DataWelcome { reconnect_token: [0; 16], resume_ttl_ms: 0, channel_nonce: 1 }, read_finished: false };
        let (socket, mut local) = tokio::io::duplex(256);
        let relay = tokio::spawn(crate::relay_stream_with_bytes_ws(socket, bytes));
        let mut server = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
        server.send(Message::Binary(Vec::new().into())).await.unwrap();
        let mut byte = [0];
        assert_eq!(tokio::io::AsyncReadExt::read(&mut local, &mut byte).await.unwrap(), 0);
        server.close(Some(CloseFrame { code: CloseCode::Away, reason: "stopping".into() })).await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), relay).await.unwrap().unwrap();
        assert!(matches!(result, Err(RexError::Closed(_))));
    }

    #[tokio::test]
    async fn stalled_transfer_times_out_in_both_directions() {
        async fn stream() -> (BytesDataStream<tokio::io::DuplexStream>, tokio::io::DuplexStream) {
            let (client, peer) = tokio::io::duplex(256);
            let ws = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
            (BytesDataStream { ws, welcome: DataWelcome { reconnect_token: [0; 16], resume_ttl_ms: 0, channel_nonce: 1 }, read_finished: false }, peer)
        }
        let (mut reader, _read_peer) = stream().await;
        let (mut writer, _write_peer) = stream().await;
        let data = vec![0; 65536];
        let (read, write) = tokio::time::timeout(std::time::Duration::from_secs(35), async { tokio::join!(reader.read_chunk(), writer.write_all(&data)) }).await.unwrap();
        assert!(matches!(read, Err(RexError::Timeout)));
        assert!(matches!(write, Err(RexError::Timeout)));
    }

    #[tokio::test]
    async fn interrupted_byte_stream_is_an_error_after_partial_data() {
        let (local, peer) = tokio::io::duplex(1024);
        let ws = WebSocketStream::from_raw_socket(local, Role::Client, None).await;
        let mut server = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
        let mut bytes = BytesDataStream { ws, welcome: DataWelcome { reconnect_token: [0; 16], resume_ttl_ms: 0, channel_nonce: 1 }, read_finished: false };
        server.send(Message::Binary(b"first".to_vec().into())).await.unwrap();
        server.close(Some(CloseFrame { code: CloseCode::Away, reason: "data provider canceled".into() })).await.unwrap();
        assert_eq!(bytes.read_chunk().await.unwrap().unwrap(), b"first");
        assert!(matches!(bytes.read_chunk().await, Err(RexError::Closed(s)) if s.contains("data provider canceled")));
    }
}
