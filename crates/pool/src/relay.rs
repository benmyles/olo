//! Requests passed on exactly as the client sent them (Ben, 2026-11-19:
//! "all requests from the pool need to be exactly the same as if they came
//! from claude code / codex"): over a connection of the account's own (a
//! client has one account, so none of its connections ever carries
//! another's requests), HTTP/1.1 as Claude Code speaks it to Anthropic
//! (its SDK offers no more), each header in the client's own case and
//! order (hyper's server writes down each name's case, its client writes
//! it back: `preserve_header_case` on both, the case map in the request's
//! extensions, which the proxy carries over), kept alive between requests
//! as the client keeps its own.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStreamExt};
use http_body_util::{BodyExt, Full};
use hyper::{HeaderMap, StatusCode, client::conn::http1::SendRequest};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;

/// What the client's TLS handshake offered the proxy (its ALPN list), on
/// each request of that connection: what the pool offers the host on its
/// behalf (Claude Code's API client offers none, its SDK `http/1.1`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientOffer(pub Vec<String>);

/// An answer from upstream: reqwest's, or one from an account's own
/// connection (`Exact`), which goes back to its pool once read to its end.
pub enum Answer {
    Reqwest(reqwest::Response),
    Exact(hyper::Response<hyper::body::Incoming>, Option<Back>),
}

/// A connection to put back in its account's pool when its answer has
/// been read whole.
pub struct Back {
    pool: Arc<Exact>,
    account: String,
    sender: SendRequest<Full<Bytes>>,
}

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

    /// Its body as it comes (its connection back in its pool once it has
    /// all come; a body left unread closes it).
    pub fn into_stream(self) -> std::pin::Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> {
        match self {
            Answer::Reqwest(r) => Box::pin(r.bytes_stream().map_err(std::io::Error::other)),
            Answer::Exact(r, back) => {
                let mut back = back;
                let body = http_body_util::BodyStream::new(r.into_body()).filter_map(|f| async move {
                    match f {
                        Ok(frame) => frame.into_data().ok().map(Ok),
                        Err(e) => Some(Err(std::io::Error::other(e))),
                    }
                });
                let done = futures_util::stream::poll_fn(move |_| {
                    if let Some(b) = back.take() {
                        b.pool.put(&b.account, b.sender);
                    }
                    std::task::Poll::Ready(None)
                });
                Box::pin(body.chain(done))
            }
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

/// Each account's idle HTTP/1.1 connections to a host, by account.
#[derive(Default)]
pub struct Exact {
    idle: Mutex<HashMap<String, Vec<SendRequest<Full<Bytes>>>>>,
}

/// The pool's key for connections carrying a client's own sign-in (its
/// requests the pool passes through), apart from every account's.
pub const OWN: &str = "\u{0}own";

/// How many idle connections an account keeps (a client keeps a few).
const IDLE_MOST: usize = 4;

impl Exact {
    fn put(&self, account: &str, sender: SendRequest<Full<Bytes>>) {
        if sender.is_closed() {
            return;
        }
        let mut idle = self.idle.lock().unwrap();
        let list = idle.entry(account.to_owned()).or_default();
        if list.len() < IDLE_MOST {
            list.push(sender);
        }
    }

    /// Every account's connections closed (the network changed).
    pub fn clear(&self) {
        self.idle.lock().unwrap().clear();
    }

    /// One account's connections closed (it left the pool).
    pub fn forget(&self, account: &str) {
        self.idle.lock().unwrap().remove(account);
    }

    /// Send `req` for `account` to `base` (the host's origin: `https://…`,
    /// or a test's `http://…`), on an idle connection of the account's or a
    /// new one; `alpn`, what its TLS handshake offers (Claude Code's SDK:
    /// `http/1.1`).
    pub async fn send(self: &Arc<Self>, account: &str, base: &str, alpn: &[&str], req: hyper::Request<Full<Bytes>>) -> Result<Answer, SendError> {
        let mut sender = loop {
            let idle = self.idle.lock().unwrap().get_mut(account).and_then(|l| l.pop());
            match idle {
                Some(mut s) if !s.is_closed() => {
                    if s.ready().await.is_ok() {
                        break s;
                    }
                }
                Some(_) => continue,
                None => break connect(base, alpn).await?,
            }
        };
        match sender.try_send_request(req).await {
            Ok(r) => Ok(Answer::Exact(r, Some(Back { pool: self.clone(), account: account.to_owned(), sender }))),
            // Never sent (a kept connection the host had closed): again on
            // a new one. Once it may have gone, never again.
            Err(mut e) => match e.take_message() {
                Some(req) => {
                    let mut fresh = connect(base, alpn).await?;
                    match fresh.send_request(req).await {
                        Ok(r) => Ok(Answer::Exact(r, Some(Back { pool: self.clone(), account: account.to_owned(), sender: fresh }))),
                        Err(e) => Err(SendError::Other(e.to_string())),
                    }
                }
                None => Err(SendError::Other(e.into_error().to_string())),
            },
        }
    }
}

/// A new HTTP/1.1 connection to `base`'s host: TLS (with `alpn`) for
/// `https`, plain for a test's `http`.
async fn connect(base: &str, alpn: &[&str]) -> Result<SendRequest<Full<Bytes>>, SendError> {
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
    let tcp = tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| SendError::Connect(format!("{host}: no connection")))?
        .map_err(|e| SendError::Connect(format!("{host}: {e}")))?;
    let _ = tcp.set_nodelay(true);
    let builder = || {
        let mut b = hyper::client::conn::http1::Builder::new();
        b.preserve_header_case(true);
        b
    };
    if !tls {
        let (sender, conn) = builder().handshake(hyper_util::rt::TokioIo::new(tcp)).await.map_err(|e| SendError::Connect(e.to_string()))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        return Ok(sender);
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
    let (sender, conn) = builder().handshake(hyper_util::rt::TokioIo::new(stream)).await.map_err(|e| SendError::Connect(e.to_string()))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(sender)
}

/// A body read whole (an answer the pool reads itself).
pub async fn read_all(a: Answer) -> Result<Bytes, std::io::Error> {
    match a {
        Answer::Reqwest(r) => r.bytes().await.map_err(std::io::Error::other),
        Answer::Exact(r, back) => {
            let body = r.into_body().collect().await.map_err(std::io::Error::other)?.to_bytes();
            if let Some(b) = back {
                b.pool.put(&b.account, b.sender);
            }
            Ok(body)
        }
    }
}
