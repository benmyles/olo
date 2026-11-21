//! Port forwarding and reverse tunnel relay over `com.superlogical.tunnel`
//! (`dial` / `-L` and `listen` / `-R`) using `rex.data.bytes.v1`.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
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
    stop: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl LocalForwardHandle {
    pub fn stop(self) {
        self.stop.store(true, Ordering::Release);
        self.task.abort();
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
    let _ = control.session_own_block(&session_id, &block_id).await;

    let active_connections = Arc::new(AtomicU64::new(0));
    let total_connections = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    let active_c = active_connections.clone();
    let total_c = total_connections.clone();
    let stop_c = stop.clone();
    let sid_c = session_id.clone();
    let bid_c = block_id.clone();

    let task = tokio::spawn(async move {
        while !stop_c.load(Ordering::Acquire) {
            let Ok((tcp, _peer)) = listener.accept().await else { break };
            let _ = tcp.set_nodelay(true);
            let ctrl = control.clone();
            let ep = endpoint.clone();
            let sid = sid_c.clone();
            let bid = bid_c.clone();
            let active = active_c.clone();
            let total = total_c.clone();

            total.fetch_add(1, Ordering::Relaxed);
            active.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
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
                active.fetch_sub(1, Ordering::Relaxed);
            });
        }
    });

    Ok(LocalForwardHandle { session_id, block_id, local_addr, remote_host, remote_port, active_connections, total_connections, stop, task })
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
                let _ = ws_tx.send(Message::Binary(Vec::new().into())).await;
                break;
            }
            ws_tx.send(Message::Binary(buf[..n].to_vec().into())).await?;
        }
        Ok::<(), RexError>(())
    };

    let ws_to_local = async {
        while let Some(msg) = ws_rx.next().await {
            match msg? {
                Message::Binary(b) => {
                    if b.is_empty() {
                        let _ = lw.shutdown().await;
                        break;
                    }
                    lw.write_all(&b).await?;
                }
                Message::Close(_) => {
                    let _ = lw.shutdown().await;
                    break;
                }
                _ => {}
            }
        }
        Ok::<(), RexError>(())
    };

    let _ = tokio::join!(local_to_ws, ws_to_local);
    Ok(())
}

/// Helper for opening a `BytesDataStream` on a `RexEndpoint` from an issued ticket.
pub async fn open_bytes_stream(endpoint: &RexEndpoint, ticket: &str) -> Result<BytesDataStream<RexStream>, RexError> {
    let (stream, host_hdr) = endpoint.connect_stream().await?;
    BytesDataStream::connect_hello(stream, &host_hdr, ticket).await
}
