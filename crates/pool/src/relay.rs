//! Requests passed on exactly as the client sent them (Ben, 2026-11-19:
//! "all requests from the pool need to be exactly the same as if they came
//! from claude code / codex"): each header in the client's own case and
//! order (hyper's server writes down each name's case, its client writes it
//! back: `preserve_header_case` on both, the case map in the request's
//! extensions, which the proxy carries over), the body as it came, over
//! connections that follow the client's own. For each connection the
//! client opens to the proxy, the pool opens one of its own to the host for
//! each account it passes that connection's requests on for (never one
//! carrying two accounts', nor two sign-ins'), offering in its handshake
//! what the client offered, kept alive as long as the client keeps its
//! own, and closed with it. The answer comes back as it came, each name in
//! its case, its status line's reason too.
//!
//! What isn't kept: a header's spacing, and a header repeated with another
//! between (hyper groups a name's values); neither is what a client sends.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures_util::Stream;
use http_body_util::{BodyExt, Full};
use hyper::{
    HeaderMap, StatusCode,
    client::conn::{TrySendError, http1, http2},
};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;

/// What the client's TLS handshake offered the proxy (its ALPN list), on
/// each request of that connection: what the pool offers the host on its
/// behalf (Claude Code's API client offers none, its SDK `http/1.1`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientOffer(pub Vec<String>);

/// The client's connection a request came on (the proxy's numbering; 0:
/// none known, a request to the gateway's own address), on each of its
/// requests: the pool's connections upstream follow it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ClientConn(pub u64);

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

impl ClientConn {
    /// A new client connection's number.
    pub fn next() -> ClientConn {
        ClientConn(NEXT_CONN.fetch_add(1, Ordering::Relaxed))
    }
}

/// An answer from upstream: reqwest's, or one from a connection of the
/// pool's that follows the client's (`Exact`), which goes back to be used
/// again once read to its end.
pub enum Answer {
    Reqwest(reqwest::Response),
    Exact(hyper::Response<hyper::body::Incoming>, Option<Back>),
}

/// A connection to keep for its key's next request, once its answer has
/// been read whole.
pub struct Back {
    pool: Arc<Exact>,
    key: Key,
    made: (u64, u64),
    sender: Sender,
}

impl Back {
    fn put(self) {
        let pool = self.pool.clone();
        pool.put(self);
    }
}

/// How long the host has to answer what the pool asks of it: a connection
/// made (TCP, TLS and the protocol's own start, together), a kept one ready
/// again, the head of an answer (a model thinking at length before a
/// single answer: the client gives up first, as reqwest's read timeout
/// let it), and each next piece of an answer.
const CONNECT_WITHIN: Duration = Duration::from_secs(20);
const READY_WITHIN: Duration = Duration::from_secs(5);
const HEAD_WITHIN: Duration = Duration::from_secs(15 * 60);
const PIECE_WITHIN: Duration = Duration::from_secs(15 * 60);

/// How long a connection the pool made for no client connection (the
/// gateway's own address) is kept idle.
const IDLE_FOR: Duration = Duration::from_secs(60);

/// How many idle connections a key keeps (one for a client connection's,
/// whose requests come one at a time).
const IDLE_MOST: usize = 4;

impl Answer {
    pub fn status(&self) -> StatusCode {
        match self {
            Answer::Reqwest(r) => r.status(),
            Answer::Exact(r, _) => r.status(),
        }
    }

    pub fn headers(&self) -> &HeaderMap {
        match self {
            Answer::Reqwest(r) => r.headers(),
            Answer::Exact(r, _) => r.headers(),
        }
    }

    /// What hyper wrote down of the answer beside its headers (each name's
    /// case, a status line's own reason), for the proxy's server to write
    /// it back so.
    pub fn take_extensions(&mut self) -> hyper::http::Extensions {
        match self {
            Answer::Reqwest(_) => Default::default(),
            Answer::Exact(r, _) => std::mem::take(r.extensions_mut()),
        }
    }

    /// Its body as it comes (its connection kept once it has all come; a
    /// body left unread, broken off or stalled closes it).
    pub fn into_stream(self) -> std::pin::Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> {
        use futures_util::TryStreamExt;
        match self {
            Answer::Reqwest(r) => Box::pin(r.bytes_stream().map_err(std::io::Error::other)),
            Answer::Exact(r, back) if http_body::Body::is_end_stream(r.body()) => {
                if let Some(b) = back {
                    b.put();
                }
                Box::pin(futures_util::stream::empty())
            }
            Answer::Exact(r, back) => Box::pin(futures_util::stream::unfold(Some((r.into_body(), back)), |state| async move {
                let (mut body, mut back) = state?;
                loop {
                    match tokio::time::timeout(PIECE_WITHIN, body.frame()).await {
                        Err(_) => return Some((Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the answer stopped coming")), None)),
                        Ok(None) => {
                            if let Some(b) = back.take() {
                                b.put();
                            }
                            return None;
                        }
                        Ok(Some(Ok(frame))) => {
                            // (Trailers, which no answer here has, aside.)
                            if let Ok(data) = frame.into_data() {
                                // Its last byte, by its length: kept now
                                // (a server passing on an answer of a known
                                // length reads no further).
                                if http_body::Body::is_end_stream(&body) {
                                    if let Some(b) = back.take() {
                                        b.put();
                                    }
                                    return Some((Ok(data), None));
                                }
                                return Some((Ok(data), Some((body, back))));
                            }
                        }
                        Ok(Some(Err(e))) => return Some((Err(std::io::Error::other(e)), None)),
                    }
                }
            })),
        }
    }
}

/// Why a send failed: no connection (it never reached the host: try again
/// freely), or after it may have.
#[derive(Debug)]
pub enum SendError {
    Connect(String),
    Other(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Connect(e) | SendError::Other(e) => f.write_str(e),
        }
    }
}

/// The pool's key for connections carrying a client's own sign-in (its
/// requests the pool passes through), apart from every account's.
pub const OWN: &str = "\u{0}own";

/// Whose a connection is: an account's (or the client's own sign-in's,
/// `OWN`), for one client connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    account: String,
    conn: u64,
}

/// A connection of the pool's: HTTP/1.1, or HTTP/2 where the client
/// offered it and the host chose it (requests then name their origin, as
/// HTTP/2 has them do).
enum Sender {
    H1(http1::SendRequest<Full<Bytes>>),
    H2(http2::SendRequest<Full<Bytes>>, String),
}

impl Sender {
    fn is_closed(&self) -> bool {
        match self {
            Sender::H1(s) => s.is_closed(),
            Sender::H2(s, _) => s.is_closed(),
        }
    }

    async fn ready(&mut self) -> hyper::Result<()> {
        match self {
            Sender::H1(s) => s.ready().await,
            Sender::H2(s, _) => s.ready().await,
        }
    }

    #[allow(clippy::result_large_err)]
    async fn send(&mut self, req: hyper::Request<Full<Bytes>>) -> Result<hyper::Response<hyper::body::Incoming>, TrySendError<hyper::Request<Full<Bytes>>>> {
        match self {
            Sender::H1(s) => s.try_send_request(req).await,
            Sender::H2(s, origin) => s.try_send_request(as_h2(req, origin)).await,
        }
    }
}

/// A request as HTTP/2 sends it: its origin in its target (`:scheme`,
/// `:authority`, from its `Host`), no `Host` or connection headers.
fn as_h2(req: hyper::Request<Full<Bytes>>, origin: &str) -> hyper::Request<Full<Bytes>> {
    let (mut parts, body) = req.into_parts();
    let authority = parts.headers.get(hyper::header::HOST).and_then(|h| h.to_str().ok()).map(str::to_owned).unwrap_or_else(|| origin.to_owned());
    let nominated: Vec<hyper::header::HeaderName> =
        parts.headers.get_all(hyper::header::CONNECTION).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).filter_map(|v| v.trim().parse().ok()).collect();
    for name in nominated {
        parts.headers.remove(name);
    }
    for h in ["host", "connection", "keep-alive", "proxy-connection", "transfer-encoding", "upgrade"] {
        parts.headers.remove(h);
    }
    let target = parts.uri.path_and_query().map(|p| p.as_str().to_owned()).unwrap_or_else(|| "/".into());
    if let Ok(uri) = format!("https://{authority}{target}").parse() {
        parts.uri = uri;
    }
    parts.version = hyper::Version::HTTP_2;
    hyper::Request::from_parts(parts, body)
}

struct Idle {
    sender: Sender,
    since: Instant,
}

/// The pool's connections that follow clients', idle between requests.
#[derive(Default)]
pub struct Exact {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    idle: HashMap<Key, Vec<Idle>>,
    /// The client connections open now: one of the pool's for a client
    /// connection that has closed is closed too, never kept.
    open: HashSet<u64>,
    /// Each account's connections' generation (a new one as it leaves the
    /// pool or signs in again), and every connection's (the network
    /// changed): one made before isn't kept.
    gens: HashMap<String, u64>,
    next_gen: u64,
    all: u64,
    /// The network the connections were made on (`olo_net::generation`):
    /// another, and none is kept.
    net: u64,
}

impl Inner {
    fn made(&mut self, account: &str) -> (u64, u64) {
        let generation = match self.gens.get(account) {
            Some(&generation) => generation,
            None => {
                self.next_gen += 1;
                self.gens.insert(account.to_owned(), self.next_gen);
                self.next_gen
            }
        };
        (self.all, generation)
    }

    fn current(&self, account: &str, made: (u64, u64)) -> bool {
        self.all == made.0 && self.gens.get(account) == Some(&made.1)
    }
}

impl Exact {
    fn put(&self, back: Back) {
        if back.sender.is_closed() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        if !inner.current(&back.key.account, back.made) || (back.key.conn != 0 && !inner.open.contains(&back.key.conn)) {
            return;
        }
        let list = inner.idle.entry(back.key).or_default();
        if list.len() < IDLE_MOST {
            list.push(Idle { sender: back.sender, since: Instant::now() });
        }
    }

    /// A client connection opened (its requests' connections are kept for
    /// it until `closed`).
    pub fn opened(&self, conn: ClientConn) {
        self.inner.lock().unwrap().open.insert(conn.0);
    }

    /// A client connection closed: the pool's that followed it close too.
    pub fn closed(&self, conn: ClientConn) {
        let mut inner = self.inner.lock().unwrap();
        inner.open.remove(&conn.0);
        inner.idle.retain(|k, _| k.conn != conn.0);
        // (A client's own sign-ins come and go, renewed: one with no
        // connection kept any more is forgotten.)
        let Inner { idle, gens, .. } = &mut *inner;
        gens.retain(|account, _| !account.starts_with(OWN) || idle.keys().any(|k| k.account == *account));
    }

    /// Every connection closed, and none made before kept (the network
    /// changed).
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.all += 1;
        inner.idle.clear();
    }

    /// One account's connections closed, and none made before kept (it
    /// left the pool, or signed in again, perhaps as another account).
    pub fn forget(&self, account: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.gens.remove(account);
        inner.idle.retain(|k, _| k.account != account);
    }

    /// How many idle connections are kept, all told (for tests).
    pub fn idle(&self) -> usize {
        self.inner.lock().unwrap().idle.values().map(Vec::len).sum()
    }

    /// Send `req` for `account` (or `OWN`) on client connection `conn`'s
    /// behalf to `base` (the host's origin: `https://…`, or a test's
    /// `http://…`): on the connection kept for them, or a new one, whose
    /// handshake offers `alpn` (the client's own offer). A request that
    /// never went (a kept connection the host had closed) goes once more
    /// on a new one; one that may have gone, never.
    pub async fn send(self: &Arc<Self>, account: &str, conn: ClientConn, base: &str, alpn: &[&str], req: hyper::Request<Full<Bytes>>) -> Result<Answer, SendError> {
        let key = Key { account: account.to_owned(), conn: conn.0 };
        let made = {
            let mut inner = self.inner.lock().unwrap();
            let net = olo_net::generation();
            if inner.net != net {
                inner.net = net;
                inner.all += 1;
                inner.idle.clear();
            }
            inner.made(account)
        };
        let mut sender = loop {
            let idle = {
                let mut inner = self.inner.lock().unwrap();
                // (Let go meanwhile, its account gone or the network
                // changed: never sent, so free to send again.)
                if !inner.current(account, made) {
                    return Err(SendError::Connect("its connections were let go before it was sent".into()));
                }
                let list = inner.idle.get_mut(&key);
                list.and_then(|l| {
                    // (One the gateway's own address made, idle long, goes.)
                    if key.conn == 0 {
                        l.retain(|i| i.since.elapsed() < IDLE_FOR);
                    }
                    l.pop()
                })
            };
            match idle {
                Some(mut i) if !i.sender.is_closed() => {
                    if matches!(tokio::time::timeout(READY_WITHIN, i.sender.ready()).await, Ok(Ok(()))) {
                        break i.sender;
                    }
                }
                Some(_) => continue,
                None => break connect(base, alpn).await?,
            }
        };
        let back = |sender| Back { pool: self.clone(), key: key.clone(), made, sender };
        // Hyper returns the converted request on an unsent HTTP/2 failure.
        // A replacement connection can negotiate a different protocol.
        let original = matches!(sender, Sender::H2(..)).then(|| req.clone());
        match tokio::time::timeout(HEAD_WITHIN, sender.send(req)).await {
            Err(_) => Err(SendError::Other("no answer came".into())),
            Ok(Ok(r)) => Ok(Answer::Exact(r, Some(back(sender)))),
            Ok(Err(mut e)) => match e.take_message() {
                Some(req) => {
                    let mut fresh = connect(base, alpn).await?;
                    match tokio::time::timeout(HEAD_WITHIN, fresh.send(original.unwrap_or(req))).await {
                        Err(_) => Err(SendError::Other("no answer came".into())),
                        Ok(Ok(r)) => Ok(Answer::Exact(r, Some(back(fresh)))),
                        Ok(Err(e)) => Err(SendError::Other(e.into_error().to_string())),
                    }
                }
                None => Err(SendError::Other(e.into_error().to_string())),
            },
        }
    }
}

/// A connection's stream, plain (a test's) or TLS.
trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}

/// Send `req`, which switches protocols (a WebSocket's handshake), to
/// `base`'s host on a connection of its own, offering `alpn` less `h2`
/// (HTTP/2 has no 101): the answer, its connection the socket's once it
/// says 101 (`hyper::upgrade::on`). Within `CONNECT_WITHIN` to connect,
/// a minute for the answer.
pub async fn upgrade(base: &str, alpn: &[&str], req: hyper::Request<Full<Bytes>>) -> Result<hyper::Response<hyper::body::Incoming>, SendError> {
    let alpn: Vec<&str> = alpn.iter().copied().filter(|p| *p != "h2").collect();
    let io: Box<dyn Io> = match tokio::time::timeout(CONNECT_WITHIN, open(base, &alpn)).await {
        Ok(r) => r?,
        Err(_) => return Err(SendError::Connect(format!("{base}: no connection"))),
    };
    let mut b = http1::Builder::new();
    b.preserve_header_case(true);
    let (mut sender, conn) = b.handshake(hyper_util::rt::TokioIo::new(io)).await.map_err(|e| SendError::Connect(e.to_string()))?;
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    match tokio::time::timeout(Duration::from_secs(60), sender.send_request(req)).await {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(e)) => Err(SendError::Other(e.to_string())),
        Err(_) => Err(SendError::Other("no answer came".into())),
    }
}

/// A stream to `base`'s host: TLS offering `alpn` for `https`, plain for a
/// test's `http`.
async fn open(base: &str, alpn: &[&str]) -> Result<Box<dyn Io>, SendError> {
    let (tls, rest) = if let Some(r) = base.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = base.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(SendError::Connect(format!("not an address: {base}")));
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), p.parse::<u16>().map_err(|_| SendError::Connect(format!("not a port: {p}")))?),
        None => (authority.to_owned(), if tls { 443 } else { 80 }),
    };
    let tcp = TcpStream::connect((host.as_str(), port)).await.map_err(|e| SendError::Connect(format!("{host}: {e}")))?;
    let _ = tcp.set_nodelay(true);
    if !tls {
        return Ok(Box::new(tcp));
    }
    let crypto = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(crypto)
        .with_safe_default_protocol_versions()
        .map_err(|e| SendError::Connect(e.to_string()))?
        .with_root_certificates(crate::capture::roots())
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    let name = ServerName::try_from(host.clone()).map_err(|e| SendError::Connect(e.to_string()))?;
    let stream = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map_err(|e| SendError::Connect(format!("{host}: {e}")))?;
    Ok(Box::new(stream))
}

/// A new connection to `base`'s host: TLS (offering `alpn`) for `https`,
/// plain for a test's `http`; HTTP/2 if the host chose it, else HTTP/1.1.
/// All of it within `CONNECT_WITHIN`.
async fn connect(base: &str, alpn: &[&str]) -> Result<Sender, SendError> {
    match tokio::time::timeout(CONNECT_WITHIN, connect_now(base, alpn)).await {
        Ok(r) => r,
        Err(_) => Err(SendError::Connect(format!("{base}: no connection"))),
    }
}

async fn connect_now(base: &str, alpn: &[&str]) -> Result<Sender, SendError> {
    let (tls, rest) = if let Some(r) = base.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = base.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(SendError::Connect(format!("not an address: {base}")));
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), p.parse::<u16>().map_err(|_| SendError::Connect(format!("not a port: {p}")))?),
        None => (authority.to_owned(), if tls { 443 } else { 80 }),
    };
    let tcp = TcpStream::connect((host.as_str(), port)).await.map_err(|e| SendError::Connect(format!("{host}: {e}")))?;
    let _ = tcp.set_nodelay(true);
    let h1 = || {
        let mut b = http1::Builder::new();
        b.preserve_header_case(true);
        b
    };
    if !tls {
        let (sender, conn) = h1().handshake(hyper_util::rt::TokioIo::new(tcp)).await.map_err(|e| SendError::Connect(e.to_string()))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        return Ok(Sender::H1(sender));
    }
    let crypto = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(crypto)
        .with_safe_default_protocol_versions()
        .map_err(|e| SendError::Connect(e.to_string()))?
        .with_root_certificates(crate::capture::roots())
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    let name = ServerName::try_from(host.clone()).map_err(|e| SendError::Connect(e.to_string()))?;
    let stream = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map_err(|e| SendError::Connect(format!("{host}: {e}")))?;
    if stream.get_ref().1.alpn_protocol() == Some(b"h2") {
        let (sender, conn) = http2::Builder::new(hyper_util::rt::TokioExecutor::new()).handshake(hyper_util::rt::TokioIo::new(stream)).await.map_err(|e| SendError::Connect(e.to_string()))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        return Ok(Sender::H2(sender, authority.to_owned()));
    }
    let (sender, conn) = h1().handshake(hyper_util::rt::TokioIo::new(stream)).await.map_err(|e| SendError::Connect(e.to_string()))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(Sender::H1(sender))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A host answering each request on a kept connection.
    async fn host() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    loop {
                        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            match s.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        }
                        buf.clear();
                        if s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok").await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        format!("http://{at}")
    }

    fn get() -> hyper::Request<Full<Bytes>> {
        hyper::Request::builder().uri("/").header("host", "example").body(Full::new(Bytes::new())).unwrap()
    }

    async fn read(a: Answer) {
        use futures_util::StreamExt;
        let mut s = a.into_stream();
        while let Some(piece) = s.next().await {
            piece.unwrap();
        }
    }

    /// A connection is kept for its account's next request once its answer
    /// is read; not one whose account left (or signed in again) or whose
    /// client connection closed while its answer was still coming, nor any
    /// made before every connection was let go (Codex's review: the old one
    /// came back, a new sign-in sent on the old account's connection).
    #[tokio::test]
    async fn nothing_made_before_is_kept() {
        let base = host().await;
        let pool = Arc::new(Exact::default());
        let conn = ClientConn::next();
        pool.opened(conn);
        read(pool.send("a", conn, &base, &[], get()).await.unwrap()).await;
        assert_eq!(pool.idle(), 1);
        let answer = pool.send("a", conn, &base, &[], get()).await.unwrap();
        pool.forget("a");
        read(answer).await;
        assert_eq!(pool.idle(), 0, "kept after its account left");
        let answer = pool.send("a", conn, &base, &[], get()).await.unwrap();
        pool.clear();
        read(answer).await;
        assert_eq!(pool.idle(), 0, "kept after every connection was let go");
        let answer = pool.send("a", conn, &base, &[], get()).await.unwrap();
        pool.closed(conn);
        read(answer).await;
        assert_eq!(pool.idle(), 0, "kept after its client connection closed");
        // (The gateway's own address: no client connection to follow.)
        read(pool.send("a", ClientConn::default(), &base, &[], get()).await.unwrap()).await;
        assert_eq!(pool.idle(), 1);
    }
}

#[cfg(test)]
mod more {
    use super::*;

    /// A request sent over HTTP/2 names its origin in its target and has
    /// no `Host`, nor any header its `Connection` named (Codex's third look:
    /// those went on).
    #[tokio::test]
    async fn over_http2_as_http2_has_it() {
        use hyper_util::rt::{TokioExecutor, TokioIo};
        let (client, server) = tokio::io::duplex(32768);
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(|req: hyper::Request<hyper::body::Incoming>| async move {
                let said = format!("{:?} {} host={} hop={}", req.version(), req.uri(), req.headers().contains_key("host"), req.headers().contains_key("x-test-hop"));
                Ok::<_, std::convert::Infallible>(hyper::Response::new(Full::new(Bytes::from(said))))
            });
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(server), svc).await;
        });
        let (sender, driver) = http2::Builder::new(TokioExecutor::new()).handshake(TokioIo::new(client)).await.unwrap();
        tokio::spawn(driver);
        let mut sender = Sender::H2(sender, "fixture.invalid".into());
        let req = hyper::Request::builder()
            .method("POST")
            .uri("/resource?q=1")
            .header("host", "fixture.invalid")
            .header("connection", "x-test-hop")
            .header("x-test-hop", "local")
            .body(Full::new(Bytes::from_static(b"{}")))
            .unwrap();
        let said = sender.send(req).await.unwrap().into_body().collect().await.unwrap().to_bytes();
        assert_eq!(String::from_utf8_lossy(&said), "HTTP/2.0 https://fixture.invalid/resource?q=1 host=false hop=false");
    }

    /// Nothing is kept of client connections that closed, nor of accounts
    /// forgotten (Codex's third look: a generation stayed for each).
    #[test]
    fn nothing_kept_of_what_went() {
        let pool = Exact::default();
        for i in 0..128 {
            let c = ClientConn::next();
            pool.opened(c);
            pool.closed(c);
            pool.forget(&format!("fixture-{i}"));
        }
        pool.clear();
        let inner = pool.inner.lock().unwrap();
        assert_eq!((inner.open.len(), inner.idle.len(), inner.gens.len()), (0, 0, 0));
    }
}
