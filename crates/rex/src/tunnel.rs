//! Port forwarding and reverse tunnel relay over `com.superlogical.tunnel`
//! (`dial` / `-L` and `listen` / `-R`) using `rex.data.bytes.v1`.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};
use tokio_tungstenite::tungstenite::Message;

use crate::{
    control::ControlClient,
    data::BytesDataStream,
    error::RexError,
    model::DATA_BYTES_SUBPROTOCOL,
    transport::{RexEndpoint, RexStream},
};

/// Active local-to-remote (`-L` / `com.superlogical.tunnel.dial`) forward handle.
pub struct LocalForwardHandle {
    pub session_id: String,
    pub block_id: String,
    pub local_addr: SocketAddr,
    pub remote_host: String,
    pub remote_port: u16,
    pub active_connections: Arc<AtomicU64>,
    pub total_connections: Arc<AtomicU64>,
    _stop: oneshot::Sender<()>,
    restore: mpsc::Sender<Restore>,
    /// Set by [`LocalForwardHandle::leave`]: the forward's block is left
    /// open on the server as it stops here.
    left: Arc<std::sync::atomic::AtomicBool>,
}

struct Restore {
    control: ControlClient,
    endpoint: RexEndpoint,
    result: oneshot::Sender<Result<(), RexError>>,
}

impl LocalForwardHandle {
    /// After bootstrap has attached this session on the replacement connection,
    /// reclaim the forward's block and resume accepting connections on the same
    /// local port. The future owns its inputs and can be awaited outside a UI lock.
    /// A missing session/block is an error. Existing TCP streams are never replayed.
    pub fn reconnect(&self, control: ControlClient, endpoint: RexEndpoint) -> impl Future<Output = Result<(), RexError>> + Send + 'static {
        let restore = self.restore.clone();
        async move {
            let (result, done) = oneshot::channel();
            let stopped = || RexError::Closed("forward removed".into());
            restore.send(Restore { control, endpoint, result }).await.map_err(|_| stopped())?;
            done.await.map_err(|_| stopped())?
        }
    }

    pub fn stop(self) {
        drop(self);
    }

    /// Stops listening here and leaves the forward's block open on the
    /// server, as a client that ends without closing it does (Rex
    /// itself across an upgrade in place): another client takes it up
    /// again with [`resume_local_forward`].
    pub fn leave(self) {
        self.left.store(true, Ordering::Relaxed);
        drop(self);
    }
}

/// Create a `com.superlogical.tunnel.dial` block on the Rex server, bind a local
/// loopback TCP listener on `127.0.0.1:<local_port>` (0 picks an ephemeral port),
/// and relay every accepted TCP connection through `rex.data.bytes.v1`.
pub async fn start_local_forward(control: ControlClient, endpoint: RexEndpoint, session_id: String, remote_host: String, remote_port: u16, local_port: u16) -> Result<LocalForwardHandle, RexError> {
    let listener = TcpListener::bind(("127.0.0.1", local_port)).await?;
    let local_addr = listener.local_addr()?;

    let created = control
        .session_new_block(
            &session_id,
            "com.superlogical.tunnel.dial",
            Some(json!({
                "host": remote_host,
                "port": remote_port,
                "client_address": local_addr.to_string(),
            })),
            Some(&format!("L:{local_port}->{remote_host}:{remote_port}")),
        )
        .await?;
    let block_id = created.payload.get("block_id").and_then(|v| v.as_str()).ok_or_else(|| RexError::Protocol("session.new_block missing block_id".to_owned()))?.to_owned();
    if let Err(e) = control.session_own_block(&session_id, &block_id).await {
        let _ = control.block_close(&session_id, &block_id).await;
        return Err(e);
    }
    Ok(run_local_forward(listener, local_addr, control, endpoint, session_id, block_id, remote_host, remote_port))
}

/// Takes up a forward's block that another client made and left open
/// (Rex before an upgrade in place: its process ended without closing
/// it, and the server lists it as orphaned), listening again on
/// `127.0.0.1:<local_port>`. An error where the block is gone or isn't
/// a forward's to own; the caller makes a new one then.
pub async fn resume_local_forward(
    control: ControlClient,
    endpoint: RexEndpoint,
    session_id: String,
    block_id: String,
    remote_host: String,
    remote_port: u16,
    local_port: u16,
) -> Result<LocalForwardHandle, RexError> {
    let listener = TcpListener::bind(("127.0.0.1", local_port)).await?;
    let local_addr = listener.local_addr()?;
    control.session_own_block(&session_id, &block_id).await?;
    Ok(run_local_forward(listener, local_addr, control, endpoint, session_id, block_id, remote_host, remote_port))
}

/// A forward's work once its block is owned: each connection accepted on
/// `listener` relayed through the block, until the handle is dropped.
#[allow(clippy::too_many_arguments)]
fn run_local_forward(
    listener: TcpListener,
    local_addr: SocketAddr,
    mut control: ControlClient,
    mut endpoint: RexEndpoint,
    session_id: String,
    block_id: String,
    remote_host: String,
    remote_port: u16,
) -> LocalForwardHandle {
    let active_connections = Arc::new(AtomicU64::new(0));
    let total_connections = Arc::new(AtomicU64::new(0));
    let (stop, mut stopping) = oneshot::channel();
    let (restore, mut restores) = mpsc::channel::<Restore>(1);

    let active_c = active_connections.clone();
    let total_c = total_connections.clone();
    let sid_c = session_id.clone();
    let bid_c = block_id.clone();
    let left = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let left_c = left.clone();

    tokio::spawn(async move {
        let mut relays = tokio::task::JoinSet::new();
        let mut connected = true;
        loop {
            let tcp = tokio::select! {
                biased;
                _ = &mut stopping => break,
                _ = control.disconnected(), if connected => {
                    connected = false;
                    relays.shutdown().await;
                    active_c.store(0, Ordering::Relaxed);
                    continue;
                }
                restore = restores.recv() => {
                    let Some(restore) = restore else { break };
                    if control.same_connection(&restore.control) && connected {
                        let _ = restore.result.send(Ok(()));
                        continue;
                    }
                    let owned = tokio::select! {
                        _ = &mut stopping => break,
                        result = restore.control.session_own_block(&sid_c, &bid_c) => result,
                    };
                    match owned {
                        Ok(_) => {
                            relays.shutdown().await;
                            active_c.store(0, Ordering::Relaxed);
                            control = restore.control;
                            endpoint = restore.endpoint;
                            connected = true;
                            let _ = restore.result.send(Ok(()));
                        }
                        Err(error) => { let _ = restore.result.send(Err(error)); }
                    }
                    continue;
                }
                _ = relays.join_next(), if !relays.is_empty() => {
                    active_c.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                accepted = listener.accept() => match accepted {
                    Ok((tcp, _)) => tcp,
                    Err(_) => break,
                }
            };
            if !connected {
                continue;
            }
            let _ = tcp.set_nodelay(true);
            let ctrl = control.clone();
            let ep = endpoint.clone();
            let sid = sid_c.clone();
            let bid = bid_c.clone();

            total_c.fetch_add(1, Ordering::Relaxed);
            active_c.fetch_add(1, Ordering::Relaxed);
            relays.spawn(async move {
                let conn_ref = format!("conn:{}", uuid::Uuid::new_v4());
                let res = async {
                    let ticket = ctrl.data_connect(&sid, &bid, &conn_ref, DATA_BYTES_SUBPROTOCOL).await?.payload.ticket;
                    let (stream, host_hdr) = ep.connect_stream().await?;
                    let bytes_ws = BytesDataStream::connect_hello(stream, &host_hdr, &ticket).await?;
                    relay_stream_with_bytes_ws(tcp, bytes_ws).await
                }
                .await;
                if let Err(e) = res {
                    tracing::debug!("dial tunnel connection ended: {e}");
                }
            });
        }
        drop(listener);
        relays.shutdown().await;
        active_c.store(0, Ordering::Relaxed);
        if !left_c.load(Ordering::Relaxed) {
            let _ = control.block_close(&sid_c, &bid_c).await;
        }
    });

    LocalForwardHandle { session_id, block_id, local_addr, remote_host, remote_port, active_connections, total_connections, _stop: stop, restore, left }
}

/// Relay one accepted reverse-tunnel (`-R` / `com.superlogical.tunnel.listen`)
/// `connection_requested` event (`ref = conn:<name>`) to a local TCP address.
pub async fn relay_reverse_to_tcp(control: ControlClient, endpoint: RexEndpoint, session_id: &str, block_id: &str, conn_ref: &str, local_target: &str) -> Result<(), RexError> {
    let ticket = control.data_connect(session_id, block_id, conn_ref, DATA_BYTES_SUBPROTOCOL).await?.payload.ticket;
    let (stream, host_hdr) = endpoint.connect_stream().await?;
    let bytes_ws = BytesDataStream::connect_hello(stream, &host_hdr, &ticket).await?;
    let tcp = TcpStream::connect(local_target).await?;
    let _ = tcp.set_nodelay(true);
    relay_stream_with_bytes_ws(tcp, bytes_ws).await
}

/// Full-duplex bidirectional relay between any `AsyncRead + AsyncWrite` stream
/// (TCP socket, Unix socket, or in-process `tokio::io::DuplexStream`) and a
/// `rex.data.bytes.v1` [`BytesDataStream`] honoring 0-byte binary frame half-close.
pub async fn relay_stream_with_bytes_ws<L, R>(local: L, bytes_ws: BytesDataStream<R>) -> Result<(), RexError>
where
    L: AsyncRead + AsyncWrite + Unpin,
    R: AsyncRead + AsyncWrite + Unpin,
{
    let (mut lr, mut lw) = tokio::io::split(local);
    let (mut ws_tx, mut ws_rx) = bytes_ws.into_inner().split();

    let local_to_ws = async {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let n = lr.read(&mut buf).await?;
            if n == 0 {
                // 0-byte binary frame signals half-close EOF on rex.data.bytes.v1.
                ws_tx.send(Message::Binary(Vec::new().into())).await?;
                break;
            }
            ws_tx.send(Message::Binary(buf[..n].to_vec().into())).await?;
        }
        Ok::<(), RexError>(())
    };

    let (eof_tx, eof_rx) = oneshot::channel();
    let ws_to_local = async {
        let mut eof = Some(eof_tx);
        while let Some(msg) = ws_rx.next().await {
            match msg? {
                Message::Binary(b) if b.is_empty() => {
                    if let Some(tx) = eof.take() {
                        lw.shutdown().await?;
                        let _ = tx.send(());
                    }
                }
                Message::Binary(b) if eof.is_some() => lw.write_all(&b).await?,
                Message::Binary(_) => return Err(RexError::Protocol("tunnel received data after EOF".into())),
                Message::Close(Some(f)) if f.code == tungstenite::protocol::frame::coding::CloseCode::Normal => {
                    lw.shutdown().await?;
                    return Ok(());
                }
                Message::Close(frame) => return Err(RexError::Closed(format!("tunnel closed: {frame:?}"))),
                _ => {}
            }
        }
        Err(RexError::Closed("tunnel ended before EOF".into()))
    };

    // Keep reading after the peer half-closes. A subsequent full close must
    // release the relay even when the local application has nothing to write.
    tokio::select! {
        result = ws_to_local => result?,
        result = async {
            local_to_ws.await?;
            eof_rx.await.map_err(|_| RexError::Closed("tunnel ended before EOF".into()))
        } => result?,
    }
    Ok(())
}

/// Helper for opening a `BytesDataStream` on a `RexEndpoint` from an issued ticket.
pub async fn open_bytes_stream(endpoint: &RexEndpoint, ticket: &str) -> Result<BytesDataStream<RexStream>, RexError> {
    let (stream, host_hdr) = endpoint.connect_stream().await?;
    BytesDataStream::connect_hello(stream, &host_hdr, ticket).await
}
