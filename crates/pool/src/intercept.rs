//! The pool as the clients' proxy (`HTTPS_PROXY`), so they run just as on
//! their own subscription, signed in as they are and talking to their own
//! hosts: no other endpoint, no API mode. A connection to one of the
//! providers' hosts (`authority::HOSTS`) is opened with the pool's own
//! certificate for it (`authority`) and its model requests (Claude's
//! `/v1/messages`, Codex's `/backend-api/codex/…`) are served by the pool
//! as the gateway serves them: the account it picks, that account's
//! sign-in in place of the client's. Everything else on those hosts goes
//! through untouched (the client's own sign-in, every header and byte,
//! a WebSocket too), and every other host is a tunnel the pool never
//! looks into. A model request asking for a WebSocket (Codex's Responses
//! over one) is told 426 Upgrade Required, which Codex takes as "use
//! HTTPS" for the rest of its session: the pool serves an answer at a
//! time, each from the account best placed for it, and a socket kept for
//! a whole session would tie it to one.

use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    http::{
        HeaderMap, Method, StatusCode,
        header::{CONNECTION, UPGRADE},
    },
    response::{IntoResponse, Response},
};
use futures_util::TryStreamExt;
use http_body_util::BodyExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::{authority::Authority, gateway::Shared, model::Provider};

/// Where the proxy listens unless `OLO_POOL_PROXY_PORT` says otherwise:
/// beside the gateway.
pub const PORT: u16 = 17892;

pub fn port() -> u16 {
    std::env::var("OLO_POOL_PROXY_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(PORT)
}

/// The proxy's address, as the clients are told it.
pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The biggest request head taken (a CONNECT line and its headers).
const MAX_HEAD: usize = 16 << 10;

/// What the pool serves on a provider's host: the model requests, as
/// (provider, the path after the gateway's base, so `proxy` can build the
/// upstream address the same way).
pub fn pooled(host: &str, path: &str) -> Option<(Provider, String)> {
    match host {
        "api.anthropic.com" => {
            let rest = path.strip_prefix("/v1/")?;
            (rest == "messages" || rest.starts_with("messages/")).then(|| (Provider::Claude, rest.to_owned()))
        }
        "chatgpt.com" => {
            let rest = path.strip_prefix("/backend-api/codex/")?;
            (!rest.is_empty()).then(|| (Provider::Codex, rest.to_owned()))
        }
        _ => None,
    }
}

/// Which providers the pool answers for (their clients pointed at it).
pub type Serving = Arc<dyn Fn(Provider) -> bool + Send + Sync>;

/// Whom the proxy opens the providers' hosts for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    /// Only the providers' own clients (`peer`): a program that merely
    /// inherited their proxy setting goes straight through.
    Clients,
    /// Any program (tests that are the client themselves).
    Anyone,
}

/// Listen on `port` (loopback; 0: any), on the current runtime; where it
/// listens.
pub async fn serve(shared: Arc<Shared>, authority: Arc<Authority>, port: u16, serving: Serving, who: Who) -> Result<SocketAddr, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await.map_err(|e| format!("Can’t listen on {addr}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?;
    let tls = tokio_rustls::TlsAcceptor::from(authority.server_config()?);
    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else { continue };
            let (shared, tls, serving) = (shared.clone(), tls.clone(), serving.clone());
            tokio::spawn(async move {
                if let Err(e) = connection(stream, peer, shared, tls, serving, who).await {
                    tracing::debug!("pool proxy: {e}");
                }
            });
        }
    });
    Ok(bound)
}

/// One client connection: a CONNECT, opened (a provider's host, for its
/// client) or tunnelled (anything else).
async fn connection(mut client: TcpStream, peer: SocketAddr, shared: Arc<Shared>, tls: tokio_rustls::TlsAcceptor, serving: Serving, who: Who) -> Result<(), String> {
    let _ = client.set_nodelay(true);
    let (head, rest) = read_head(&mut client).await?;
    let text = String::from_utf8_lossy(&head);
    let mut parts = text.lines().next().unwrap_or("").split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "CONNECT" {
        let _ = client.write_all(b"HTTP/1.1 405 Method Not Allowed\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
        return Err(format!("not a CONNECT: {method}"));
    }
    let (host, port) = target.rsplit_once(':').map(|(h, p)| (h.trim_matches(['[', ']']).to_ascii_lowercase(), p.parse::<u16>().unwrap_or(443))).unwrap_or((target.to_ascii_lowercase(), 443));
    let provider = provider_of(&host);
    let mut opened = port == 443 && crate::authority::HOSTS.contains(&host.as_str()) && serving(provider);
    if opened && who == Who::Clients {
        // (Read from /proc, off the async threads.)
        let local = client.local_addr().map_err(|e| e.to_string())?;
        let program = tokio::task::spawn_blocking(move || crate::peer::program(peer, local)).await.ok().flatten();
        opened = program.as_ref().is_some_and(|p| p.is_client(provider));
        if !opened {
            tracing::debug!("pool proxy: {host} for {:?}, not {}: straight through", program.map(|p| p.exe), provider.client());
        }
    }
    if !opened {
        // (Tests keep off the network: `OLO_POOL_NO_TUNNEL`.)
        if std::env::var_os("OLO_POOL_NO_TUNNEL").is_some() {
            let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            return Err(format!("not tunnelling to {host}:{port} (OLO_POOL_NO_TUNNEL)"));
        }
        // Not ours to look into: straight through.
        let mut upstream = match tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host.as_str(), port))).await {
            Ok(Ok(s)) => s,
            _ => {
                let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
                return Err(format!("couldn’t reach {host}:{port}"));
            }
        };
        let _ = upstream.set_nodelay(true);
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.map_err(|e| e.to_string())?;
        if !rest.is_empty() {
            upstream.write_all(&rest).await.map_err(|e| e.to_string())?;
        }
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        return Ok(());
    }
    client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.map_err(|e| e.to_string())?;
    // (What came after the head, if the client didn't wait for the
    // answer, is the start of its TLS.)
    let stream = tls.accept(Prefixed { first: rest, at: 0, inner: client }).await.map_err(|e| format!("TLS with the client for {host}: {e}"))?;
    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let (shared, host) = (shared.clone(), host.clone());
        async move { Ok::<_, std::convert::Infallible>(request(shared, &host, req).await) }
    });
    hyper::server::conn::http1::Builder::new()
        .keep_alive(true)
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .with_upgrades()
        .await
        .map_err(|e| e.to_string())
}

/// A connection with bytes already read from it put back in front.
struct Prefixed {
    first: Vec<u8>,
    at: usize,
    inner: TcpStream,
}

impl tokio::io::AsyncRead for Prefixed {
    fn poll_read(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        if self.at < self.first.len() {
            let n = buf.remaining().min(self.first.len() - self.at);
            let at = self.at;
            buf.put_slice(&self.first[at..at + n]);
            self.at += n;
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for Prefixed {
    fn poll_write(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

fn provider_of(host: &str) -> Provider {
    if host == "chatgpt.com" { Provider::Codex } else { Provider::Claude }
}

/// Read up to the end of the request head; what came after it too.
async fn read_head(s: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err("request head too big".into());
        }
        let n = tokio::time::timeout(Duration::from_secs(30), s.read(&mut chunk)).await.map_err(|_| "no request".to_owned())?.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed before a request".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// One request on an opened connection to `host`: the pool's to serve, or
/// passed through as it is.
async fn request(shared: Arc<Shared>, host: &str, req: hyper::Request<hyper::body::Incoming>) -> Response {
    let (mut parts, body) = req.into_parts();
    let path = parts.uri.path().to_owned();
    let query = parts.uri.query().map(str::to_owned);
    if upgrading(&parts.headers) {
        if pooled(host, &path).is_some() {
            return (StatusCode::UPGRADE_REQUIRED, "The pool answers this over HTTPS.").into_response();
        }
        let Some(on) = parts.extensions.remove::<hyper::upgrade::OnUpgrade>() else {
            return (StatusCode::BAD_REQUEST, "This connection can’t switch protocols.").into_response();
        };
        return upgrade_through(&shared, host, &path, query.as_deref(), parts.method, parts.headers, on).await;
    }
    let body = match body.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("The request didn’t arrive whole: {e}")).into_response(),
    };
    match pooled(host, &path) {
        Some((provider, rest)) => crate::gateway::serve(shared, provider, rest, query, parts.method, parts.headers, body).await,
        None => through(&shared, host, &path, query.as_deref(), parts.method, parts.headers, body).await,
    }
}

/// Hop-by-hop headers, which belong to one connection, not the request.
fn hop(name: &str) -> bool {
    matches!(name, "connection" | "keep-alive" | "proxy-authenticate" | "proxy-authorization" | "te" | "trailer" | "transfer-encoding" | "upgrade" | "host" | "content-length")
}

/// A request the pool doesn't serve, sent on as it came (the client's
/// own sign-in, its encodings) and answered as it is.
async fn through(shared: &Shared, host: &str, path: &str, query: Option<&str>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let url = format!("{}{path}{}", shared.upstream_origin(host), query.map(|q| format!("?{q}")).unwrap_or_default());
    let mut req = shared.client().request(method, &url);
    for (k, v) in headers.iter().filter(|(k, _)| !hop(k.as_str())) {
        req = req.header(k, v);
    }
    match req.body(body).send().await {
        Ok(r) => answered(r),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("The pool couldn’t reach {host}: {e}")).into_response(),
    }
}

/// The host's answer, as it came.
fn answered(r: reqwest::Response) -> Response {
    let (status, headers) = (r.status(), r.headers().clone());
    let mut out = Response::new(Body::from_stream(r.bytes_stream().map_err(std::io::Error::other)));
    *out.status_mut() = status;
    for (k, v) in headers.iter().filter(|(k, _)| !hop(k.as_str())) {
        out.headers_mut().append(k.clone(), v.clone());
    }
    out
}

/// Whether a request asks to switch protocols (a WebSocket).
fn upgrading(h: &HeaderMap) -> bool {
    h.contains_key(UPGRADE) && h.get_all(CONNECTION).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
}

/// A request to switch protocols on a path the pool doesn't serve: sent
/// on as it came (its `connection` and `upgrade` too), and once the host
/// agrees (101), the two connections joined byte for byte until either
/// ends.
async fn upgrade_through(shared: &Shared, host: &str, path: &str, query: Option<&str>, method: Method, headers: HeaderMap, on: hyper::upgrade::OnUpgrade) -> Response {
    // HTTP/1.1 (HTTP/2 has no 101), and no read timeout: a socket may be
    // quiet for long.
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| reqwest::Client::builder().http1_only().connect_timeout(Duration::from_secs(20)).build().expect("HTTP client"));
    let url = format!("{}{path}{}", shared.upstream_origin(host), query.map(|q| format!("?{q}")).unwrap_or_default());
    let mut req = client.request(method, &url);
    for (k, v) in headers.iter().filter(|(k, _)| *k == CONNECTION || *k == UPGRADE || !hop(k.as_str())) {
        req = req.header(k, v);
    }
    let r = match req.send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("The pool couldn’t reach {host}: {e}")).into_response(),
    };
    if r.status() != StatusCode::SWITCHING_PROTOCOLS {
        return answered(r);
    }
    let mut out = Response::new(Body::empty());
    *out.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    for (k, v) in r.headers() {
        out.headers_mut().append(k.clone(), v.clone());
    }
    tokio::spawn(async move {
        match tokio::join!(on, r.upgrade()) {
            (Ok(theirs), Ok(mut host)) => {
                let _ = tokio::io::copy_bidirectional(&mut hyper_util::rt::TokioIo::new(theirs), &mut host).await;
            }
            (a, b) => tracing::debug!("pool proxy: switching protocols failed: {:?} {:?}", a.err(), b.err()),
        }
    });
    out
}
