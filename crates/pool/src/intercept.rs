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
//! looks into. Codex's Responses over a WebSocket are the pool's too, as
//! Codex sends them (`gateway::serve_socket`): a socket on the account
//! best placed for it, given up for another when it runs out.

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
/// Whether `path` is one of Antigravity's four model-generation operations
/// that the pool routes and authorizes (reused by `/agy/{*rest}` so auxiliary
/// operations like `claimBankedQuotaReset` can never bypass keeper gates).
pub fn is_agy_generation_path(path: &str) -> bool {
    let normalized = path.trim_start_matches('/');
    matches!(normalized, "v1internal:streamGenerateContent" | "v1internal:generateContent" | "v1internal:streamGenerateChat" | "v1internal:generateChat")
}

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
        "daily-cloudcode-pa.googleapis.com" | "cloudcode-pa.googleapis.com" => is_agy_generation_path(path).then(|| (Provider::Antigravity, path.trim_start_matches('/').to_owned())),
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
    // (The one the version before handed over, if it did.)
    let listener = crate::drain::listen(port).and_then(TcpListener::from_std).map_err(|e| format!("Can’t listen on {addr}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?;
    shared.drain.keep(bound.port(), &listener);
    let has_agy = authority.has_agy();
    let tls = tokio_rustls::TlsAcceptor::from(authority.server_config()?);
    tokio::spawn(async move {
        let mut stop = shared.drain.watch();
        loop {
            // An upgrade waiting: no more connections taken (they wait in
            // the queue for the new version, which takes this socket over).
            if *stop.borrow() {
                // (Until the upgrade, or its end if it doesn't come.)
                let _ = stop.wait_for(|s| !*s).await;
                continue;
            }
            let accepted = tokio::select! {
                a = listener.accept() => a,
                _ = stop.changed() => continue,
            };
            let Ok((stream, peer)) = accepted else { continue };
            let (shared, tls, serving) = (shared.clone(), tls.clone(), serving.clone());
            tokio::spawn(async move {
                if let Err(e) = connection(stream, peer, shared, tls, serving, who, has_agy).await {
                    tracing::debug!("pool proxy: {e}");
                }
            });
        }
    });
    Ok(bound)
}

/// Whether the proxy intercepts TLS for `host` given whether the active
/// certificate authority covers Antigravity hosts (`has_agy`).
pub fn intercepts_host(host: &str, has_agy: bool) -> bool {
    let hosts = if has_agy { crate::authority::HOSTS } else { crate::authority::LEGACY_HOSTS };
    hosts.contains(&host)
}

/// One client connection: a CONNECT, opened (a provider's host, for its
/// client) or tunnelled (anything else).
async fn connection(mut client: TcpStream, peer: SocketAddr, shared: Arc<Shared>, tls: tokio_rustls::TlsAcceptor, serving: Serving, who: Who, has_agy: bool) -> Result<(), String> {
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
    let mut opened = port == 443 && intercepts_host(&host, has_agy) && serving(provider);
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
        crate::drain::tunnel(&shared.drain, client, upstream).await;
        return Ok(());
    }
    client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.map_err(|e| e.to_string())?;
    // (What came after the head, if the client didn't wait for the
    // answer, is the start of its TLS.)
    // (What its handshake offers, written down: the pool offers the host
    // the same on its behalf, `relay::ClientOffer`.)
    // (The handshake within `HANDSHAKE_WITHIN`, as a client's is.)
    let handshake = async {
        let start = tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), Prefixed { first: rest, at: 0, inner: client }).await?;
        let offer = crate::relay::ClientOffer(start.client_hello().alpn().map(|a| a.map(|p| String::from_utf8_lossy(p).into_owned()).collect()).unwrap_or_default());
        Ok::<_, std::io::Error>((offer, start.into_stream(tls.config().clone()).await?))
    };
    let (offer, stream) = match tokio::time::timeout(HANDSHAKE_WITHIN, handshake).await {
        Ok(r) => r.map_err(|e| format!("TLS with the client for {host}: {e}"))?,
        Err(_) => return Err(format!("TLS with the client for {host}: no handshake")),
    };
    // Which client connection each request came on: the pool's own
    // connections upstream follow it, closed with it (`relay`).
    let conn = crate::relay::ClientConn::next();
    let _open = Open::new(shared.clone(), conn);
    let mut stop = shared.drain.watch();
    let service = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
        let (shared, host) = (shared.clone(), host.clone());
        req.extensions_mut().insert(offer.clone());
        req.extensions_mut().insert(conn);
        async move { Ok::<_, std::convert::Infallible>(request(shared, &host, req).await) }
    });
    // (Each header's name written down in the client's own case, for a
    // request passed on to be sent so: `relay`.)
    let conn = hyper::server::conn::http1::Builder::new()
        .keep_alive(true)
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(Duration::from_secs(30))
        .auto_date_header(false)
        .preserve_header_case(true)
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .with_upgrades();
    tokio::pin!(conn);
    // An upgrade waiting: the request under way finishes, and then the
    // connection closes (the client opens another for its next, which
    // waits for the new version).
    let mut closing = false;
    loop {
        if !closing && *stop.borrow() {
            closing = true;
            conn.as_mut().graceful_shutdown();
        }
        tokio::select! {
            r = conn.as_mut() => return r.map_err(|e| e.to_string()),
            _ = stop.changed(), if !closing => {}
        }
    }
}

/// How long a client has for its TLS handshake, and for the rest of a
/// request once its head has come.
const HANDSHAKE_WITHIN: Duration = Duration::from_secs(20);
const BODY_WITHIN: Duration = Duration::from_secs(60);
const BODY_MOST: usize = 64 << 20;

/// A client connection open (`relay::Exact::opened`) until this goes, as
/// it does however the connection ends.
struct Open {
    shared: Arc<Shared>,
    conn: crate::relay::ClientConn,
}

impl Open {
    fn new(shared: Arc<Shared>, conn: crate::relay::ClientConn) -> Open {
        shared.exact().opened(conn);
        Open { shared, conn }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.shared.exact().closed(self.conn);
    }
}

/// A connection with bytes already read from it put back in front.
pub(crate) struct Prefixed {
    first: Vec<u8>,
    at: usize,
    inner: TcpStream,
}

impl Prefixed {
    pub(crate) fn new(first: Vec<u8>, inner: TcpStream) -> Prefixed {
        Prefixed { first, at: 0, inner }
    }

    /// Puts `read`, just read from it, back in front of what's left.
    pub(crate) fn unread(&mut self, mut read: Vec<u8>) {
        read.extend_from_slice(&self.first[self.at..]);
        (self.first, self.at) = (read, 0);
    }
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
    match host {
        "chatgpt.com" => Provider::Codex,
        "daily-cloudcode-pa.googleapis.com" | "cloudcode-pa.googleapis.com" => Provider::Antigravity,
        _ => Provider::Claude,
    }
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
        let Some(on) = parts.extensions.remove::<hyper::upgrade::OnUpgrade>() else {
            return (StatusCode::BAD_REQUEST, "This connection can’t switch protocols.").into_response();
        };
        match pooled(host, &path).filter(|(p, _)| shared.routes(*p)) {
            // Codex's Responses over a WebSocket: the pool's, as it came
            // (under way until its 101 has gone; after, a tunnel).
            Some((Provider::Codex, ref rest)) if rest == "responses" && parts.method == Method::GET && parts.headers.get(UPGRADE).is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket")) => {
                let underway = crate::drain::Underway::new(&shared);
                let answer = crate::gateway::serve_socket(shared, path, query, parts.headers, parts.extensions, on).await;
                let (parts, body) = answer.into_parts();
                return Response::from_parts(parts, Body::new(crate::drain::Counted { inner: body, underway }));
            }
            Some(_) => return (StatusCode::UPGRADE_REQUIRED, "The pool answers this over HTTPS.").into_response(),
            None => return upgrade_through(&shared, host, &path, query.as_deref(), parts.method, parts.headers, on).await,
        }
    }
    // Under way until its answer has gone (an upgrade waits for it).
    let underway = crate::drain::Underway::new(&shared);
    // (No more than `BODY_MOST`: twice what Anthropic takes.)
    let body = match tokio::time::timeout(BODY_WITHIN, http_body_util::Limited::new(body, BODY_MOST).collect()).await {
        Ok(Ok(b)) => b.to_bytes(),
        Ok(Err(e)) if e.is::<http_body_util::LengthLimitError>() => return (StatusCode::PAYLOAD_TOO_LARGE, "The request is larger than the pool takes.").into_response(),
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, format!("The request didn’t arrive whole: {e}")).into_response(),
        Err(_) => return (StatusCode::REQUEST_TIMEOUT, "The request didn’t arrive whole.").into_response(),
    };
    // The pool's, unless its client is turned off (or it has no account
    // for it): then the client's own, passed through. Decided for each
    // request, so turning a client off or on needs no restart of it.
    let answer = match pooled(host, &path) {
        Some((provider, rest)) if shared.routes(provider) => {
            crate::gateway::serve_host(shared, provider, Some(host.to_owned()), rest, query, parts.method, parts.headers, parts.extensions, body).await
        }
        _ => {
            // (A usage report asked by a client the pool serves: what it
            // is told of its usage, as its answers' headers are. `told`.
            // Antigravity's quota summary and model list are POSTs.)
            let report = crate::told::is_report(host, &path).filter(|p| {
                let expected = if *p == Provider::Antigravity { Method::POST } else { Method::GET };
                parts.method == expected && shared.routes(*p)
            });
            let answer = through(&shared, host, &path, query.as_deref(), parts.method, parts.headers, parts.extensions, body).await;
            match report {
                Some(provider) => told(&shared, provider, answer).await,
                None => answer,
            }
        }
    };
    let (parts, body) = answer.into_parts();
    Response::from_parts(parts, Body::new(crate::drain::Counted { inner: body, underway }))
}

/// The most of a usage report read to say it anew (they are a few KB).
const REPORT_MOST: usize = 1 << 20;

/// A usage report's answer as the client is told it: with every meter at
/// nothing used, for as long as the pool has more to draw from than the
/// account it would choose now (`Shared::more_to_draw`); else, and where
/// it isn't a report the pool can read (not a 2xx, too big, a coding or
/// a shape it doesn't know), as it came.
async fn told(shared: &Shared, provider: Provider, answer: Response) -> Response {
    use futures_util::StreamExt as _;
    if !answer.status().is_success() {
        return answer;
    }
    if provider != Provider::Antigravity && !shared.more_to_draw(provider, "", None) {
        return answer;
    }
    let (mut parts, body) = answer.into_parts();
    // Read up to REPORT_MOST bytes; if the response exceeds that bound, stream
    // the buffered prefix + remainder straight through untouched rather than
    // failing with 502.
    let mut stream = body.into_data_stream();
    let mut buf = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => {
                if buf.len().saturating_add(chunk.len()) > REPORT_MOST {
                    let prefix = futures_util::stream::iter(vec![Ok(Bytes::from(buf)), Ok(chunk)]);
                    return Response::from_parts(parts, Body::from_stream(prefix.chain(stream)));
                }
                buf.extend_from_slice(&chunk);
            }
            Err(_) => {
                return (StatusCode::BAD_GATEWAY, "The usage report didn’t arrive whole.").into_response();
            }
        }
    }
    let bytes = Bytes::from(buf);
    let coding = parts.headers.get("content-encoding").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    let said = crate::gateway::decoded_bounded(&coding, &bytes, REPORT_MOST)
        .and_then(|plain| serde_json::from_slice::<serde_json::Value>(&plain).ok())
        .and_then(|mut report| crate::told::report_for(provider, &mut report, |fam| shared.more_to_draw(provider, fam, None)).then(|| report.to_string()));
    match said {
        Some(text) => {
            // (Said plain: its coding and its length were the other's.)
            parts.headers.remove("content-encoding");
            parts.headers.remove("transfer-encoding");
            parts.headers.insert("content-length", text.len().into());
            Response::from_parts(parts, Body::from(text))
        }
        None => Response::from_parts(parts, Body::from(bytes)),
    }
}

/// Hop-by-hop headers, which belong to one connection, not the request.
fn hop(name: &str) -> bool {
    matches!(name, "connection" | "keep-alive" | "proxy-authenticate" | "proxy-authorization" | "te" | "trailer" | "transfer-encoding" | "upgrade" | "host" | "content-length")
}

/// A request the pool doesn't serve, sent on as it came (the client's
/// own sign-in, its encodings) and answered as it is.
#[allow(clippy::too_many_arguments)]
async fn through(shared: &Shared, host: &str, path: &str, query: Option<&str>, method: Method, headers: HeaderMap, ext: hyper::http::Extensions, body: Bytes) -> Response {
    // Anthropic's: exactly as it came, each name in its case, over
    // HTTP/1.1 as Claude Code speaks it there, on connections of the
    // client's own sign-in's (`relay`).
    if host == "api.anthropic.com" {
        let target = format!("{path}{}", query.map(|q| format!("?{q}")).unwrap_or_default());
        let mut b = hyper::Request::builder().method(method).uri(target);
        for (k, v) in headers.iter().filter(|(k, _)| !matches!(k.as_str(), "proxy-connection" | "proxy-authorization" | "te" | "trailer" | "upgrade")) {
            b = b.header(k, v);
        }
        let alpn: Vec<String> = ext.get::<crate::relay::ClientOffer>().map(|o| o.0.clone()).unwrap_or_default();
        let alpn: Vec<&str> = alpn.iter().map(String::as_str).collect();
        let conn = ext.get::<crate::relay::ClientConn>().copied().unwrap_or_default();
        return match b.body(http_body_util::Full::new(body)) {
            Ok(mut req) => {
                *req.extensions_mut() = ext;
                match shared.exact().send(&own_key(req.headers()), conn, &shared.upstream_origin(host), &alpn, req).await {
                    Ok(a) => answered_exact(a),
                    Err(e) => (StatusCode::BAD_GATEWAY, format!("The pool couldn’t reach {host}: {e}")).into_response(),
                }
            }
            Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        };
    }
    // (On a client of the sign-in's own: ChatGPT's HTTP/2 would carry two
    // sign-ins' requests on one connection.)
    let url = format!("{}{path}{}", shared.upstream_origin(host), query.map(|q| format!("?{q}")).unwrap_or_default());
    let mut req = shared.own_client(&own_key(&headers)).request(method, &url);
    for (k, v) in headers.iter().filter(|(k, _)| !hop(k.as_str())) {
        req = req.header(k, v);
    }
    match req.body(body).send().await {
        Ok(r) => answered(r),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("The pool couldn’t reach {host}: {e}")).into_response(),
    }
}

/// Whose own a request passed through is: its sign-in, as a hash of its
/// credentials (kept in memory alone), so no connection carries two
/// sign-ins' requests (Codex's third look: two on one).
fn own_key(headers: &HeaderMap) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for name in ["authorization", "x-api-key", "chatgpt-account-id"] {
        headers.get(name).map(|v| v.as_bytes()).hash(&mut h);
    }
    format!("{}{:016x}", crate::relay::OWN, h.finish())
}

/// An exact relay's answer, as it came: each name in its case, its status
/// line's reason, its length (its connection kept once it's read).
fn answered_exact(mut a: crate::relay::Answer) -> Response {
    let (status, headers, ext) = (a.status(), a.headers().clone(), a.take_extensions());
    let mut out = Response::new(Body::from_stream(a.into_stream()));
    *out.status_mut() = status;
    *out.extensions_mut() = ext;
    let transfer_coded = headers.contains_key("transfer-encoding");
    for (k, v) in headers.iter().filter(|(k, _)| (!transfer_coded && k.as_str() == "content-length") || !hop(k.as_str())) {
        out.headers_mut().append(k.clone(), v.clone());
    }
    out
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
async fn upgrade_through(shared: &Arc<Shared>, host: &str, path: &str, query: Option<&str>, method: Method, headers: HeaderMap, on: hyper::upgrade::OnUpgrade) -> Response {
    // HTTP/1.1 (HTTP/2 has no 101), and no read timeout: a socket may be
    // quiet for long. (Made again for a new network: `olo_net`.)
    static CLIENT: OnceLock<olo_net::PerNetwork<reqwest::Client>> = OnceLock::new();
    // (Nothing kept between upgrades: a connection a refused upgrade left
    // would carry the next client's, another sign-in's.)
    let client = CLIENT
        .get_or_init(|| {
            olo_net::PerNetwork::new(|| {
                reqwest::Client::builder()
                    .http1_only()
                    .redirect(reqwest::redirect::Policy::none())
                    .pool_max_idle_per_host(0)
                    .connect_timeout(Duration::from_secs(20))
                    .no_gzip()
                    .no_deflate()
                    .no_brotli()
                    .no_zstd()
                    .build()
                    .expect("HTTP client")
            })
        })
        .get();
    let url = format!("{}{path}{}", shared.upstream_origin(host), query.map(|q| format!("?{q}")).unwrap_or_default());
    let mut req = client.request(method, &url);
    for (k, v) in headers.iter().filter(|(k, _)| *k == CONNECTION || *k == UPGRADE || !hop(k.as_str())) {
        req = req.header(k, v);
    }
    // (Its answer within a minute; once switched, as quiet as it likes.)
    let r = match tokio::time::timeout(Duration::from_secs(60), req.send()).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return (StatusCode::BAD_GATEWAY, format!("The pool couldn’t reach {host}: {e}")).into_response(),
        Err(_) => return (StatusCode::GATEWAY_TIMEOUT, format!("{host} didn’t answer.")).into_response(),
    };
    if r.status() != StatusCode::SWITCHING_PROTOCOLS {
        return answered(r);
    }
    let mut out = Response::new(Body::empty());
    *out.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    for (k, v) in r.headers() {
        out.headers_mut().append(k.clone(), v.clone());
    }
    let shared = shared.clone();
    tokio::spawn(async move {
        match tokio::join!(on, r.upgrade()) {
            (Ok(theirs), Ok(host)) => {
                // (A tunnel: an upgrade waits while it carries anything.)
                crate::drain::tunnel(&shared.drain, hyper_util::rt::TokioIo::new(theirs), host).await;
            }
            (a, b) => tracing::debug!("pool proxy: switching protocols failed: {:?} {:?}", a.err(), b.err()),
        }
    });
    out
}
