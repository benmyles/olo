//! The pool's own requests, sent as the client itself sends them: built
//! from what the account's client was captured sending (`capture`), the
//! account's own values in place of the placeholders, each header in its
//! own case and order, the body as the client writes it, over the protocol
//! the client would get (HTTP/1.1 written byte for byte, or HTTP/2 where
//! its offer includes it and the host agrees), with the client's offer in
//! the TLS handshake (its ALPN list).

use std::{io::Read, sync::Arc, time::Duration};

use bytes::Bytes;
use http_body_util::BodyExt;
use rustls::pki_types::ServerName;
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};

use crate::{
    capture::{self, Capture, Sent},
    model::{Credential, Provider},
    upstream::Upstreams,
};

/// What the pool sends for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    /// The quota read.
    Usage,
    /// The reset credits read.
    Credits,
    /// A reset's claim.
    Claim,
    /// A sign-in's renewal.
    Renewal,
}

/// An account's capture and its own values: what the pool's requests for
/// it are made from.
#[derive(Debug, Clone)]
pub struct Like {
    pub capture: Arc<Capture>,
    pub org: Option<String>,
    pub email: Option<String>,
    /// The cookies the server set for this account's requests, which its
    /// client keeps and sends back (Codex's cookie store): the pool's jar.
    pub jar: Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

/// The runs to take a request from, the session's first: what a person's
/// own client sends.
const RUNS: &[&str] = &["session", "wall", "print", "app-server", "renewal"];

impl Like {
    pub fn fill(&self, c: &Credential) -> Fill {
        let jar = self.jar.lock().unwrap();
        let cookie = (!jar.is_empty()).then(|| jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "));
        Fill { access: c.access.clone(), refresh: c.refresh.clone(), account_id: c.account_id.clone(), org: self.org.clone(), account_uuid: None, email: self.email.clone(), cookie }
    }

    /// What an answer's `set-cookie`s set, kept (a name's value replaced;
    /// one set to expire, gone).
    pub fn remember(&self, headers: &[(String, String)]) {
        let mut jar = self.jar.lock().unwrap();
        for (k, v) in headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")) {
            let _ = k;
            let mut parts = v.split(';');
            let Some((name, value)) = parts.next().and_then(|nv| nv.split_once('=')).map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned())) else { continue };
            let gone = value.is_empty() || parts.any(|a| a.trim().eq_ignore_ascii_case("max-age=0"));
            jar.retain(|(n, _)| *n != name);
            if !gone {
                jar.push((name, value));
            }
        }
    }

    /// The captured request `what` is made from, if this account's client
    /// was captured making it (or the one it's made like, below).
    pub fn request(&self, what: What) -> Option<Sent> {
        let c = &self.capture;
        match (c.provider, what) {
            (Provider::Claude, What::Usage | What::Credits) => c.find("api.anthropic.com", "GET", "/api/oauth/usage", RUNS).cloned(),
            (Provider::Claude, What::Claim) => {
                let path = format!("/api/organizations/{}/reset_rate_limits", capture::ORG);
                c.find("api.anthropic.com", "POST", &path, RUNS).cloned().or_else(|| claude_claim(c.find("api.anthropic.com", "GET", "/api/oauth/usage", RUNS)?))
            }
            (Provider::Claude, What::Renewal) => c.find("platform.claude.com", "POST", "/v1/oauth/token", RUNS).or_else(|| c.find("console.anthropic.com", "POST", "/v1/oauth/token", RUNS)).cloned(),
            (Provider::Codex, What::Usage) => c.find("chatgpt.com", "GET", "/backend-api/wham/usage", RUNS).cloned(),
            (Provider::Codex, What::Credits) => c.find("chatgpt.com", "GET", "/backend-api/wham/rate-limit-reset-credits", RUNS).cloned(),
            (Provider::Codex, What::Claim) => {
                let mut claim = c.find("chatgpt.com", "POST", "/backend-api/wham/rate-limit-reset-credits/consume", RUNS)?.clone();
                // (Captured through its app server, which names itself
                // otherwise: the same backend client, so a session's claim is
                // this with the session's own name, as its credits read has it.)
                if claim.run != "session"
                    && let Some(ua) = c.find("chatgpt.com", "GET", "/backend-api/wham/rate-limit-reset-credits", &["session"]).filter(|s| s.run == "session").and_then(|s| s.header("user-agent"))
                {
                    let ua = ua.to_owned();
                    for (k, v) in claim.headers.iter_mut() {
                        if k.eq_ignore_ascii_case("user-agent") {
                            *v = ua.clone();
                        }
                    }
                }
                Some(claim)
            }
            (Provider::Codex, What::Renewal) => c.find("auth.openai.com", "POST", "/oauth/token", RUNS).cloned(),
        }
    }
}

/// Claude Code's claim of a reset, which it makes with the same API client
/// as its quota read (`Xt.post` with `"Content-Type": "application/json"`,
/// the program, grant and request ids, in that order), where its interface
/// doesn't offer one to capture: that read's head as a POST, with the
/// `Content-Length` that client puts before `Accept-Encoding` (as its
/// sign-in renewal, a POST of the same client, shows).
fn claude_claim(usage: &Sent) -> Option<Sent> {
    let mut headers = Vec::with_capacity(usage.headers.len() + 1);
    let mut placed = false;
    for (k, v) in &usage.headers {
        if !placed && k.eq_ignore_ascii_case("accept-encoding") {
            headers.push(("Content-Length".to_owned(), "0".to_owned()));
            placed = true;
        }
        headers.push((k.clone(), v.clone()));
    }
    if !placed {
        return None;
    }
    Some(Sent {
        run: format!("{}+claim", usage.run),
        host: usage.host.clone(),
        method: "POST".into(),
        target: format!("/api/organizations/{}/reset_rate_limits", capture::ORG),
        headers,
        body: r#"{"program":"cedar_ember","grant_id":"","request_id":""}"#.into(),
        body_len: 0,
        alpn: usage.alpn.clone(),
        suites: usage.suites.clone(),
        schemes: usage.schemes.clone(),
        passed_on: false,
    })
}

/// The account's own values for a captured request's placeholders.
#[derive(Debug, Default, Clone)]
pub struct Fill {
    pub access: String,
    pub refresh: Option<String>,
    pub account_id: Option<String>,
    pub org: Option<String>,
    pub account_uuid: Option<String>,
    pub email: Option<String>,
    pub cookie: Option<String>,
}

impl Fill {
    fn pairs(&self) -> Vec<(&'static str, Option<&str>)> {
        vec![
            (capture::ACCESS, Some(self.access.as_str())),
            (capture::REFRESH, self.refresh.as_deref()),
            (capture::ACCOUNT_ID, self.account_id.as_deref()),
            (capture::ORG, self.org.as_deref()),
            (capture::ACCOUNT_UUID, self.account_uuid.as_deref()),
            (capture::EMAIL, self.email.as_deref()),
        ]
    }

    /// `text` with every placeholder filled; an error for one this
    /// account has no value for (never sent with a placeholder in it).
    pub fn fill(&self, text: &str) -> Result<String, String> {
        let mut out = text.to_owned();
        for (placeholder, value) in self.pairs() {
            if out.contains(placeholder) {
                let v = value.ok_or_else(|| format!("no value for {placeholder}"))?;
                out = out.replace(placeholder, v);
            }
        }
        Ok(out)
    }
}

/// A request ready to send: its head as it goes on the wire (HTTP/1.1),
/// its body, and how to connect.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub host: String,
    pub method: String,
    pub target: String,
    /// The headers, in order, as written (the `host` one among them).
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub alpn: Vec<String>,
}

impl Built {
    /// The head as HTTP/1.1 writes it.
    pub fn head(&self) -> String {
        let mut h = format!("{} {} HTTP/1.1\r\n", self.method, self.target);
        for (k, v) in &self.headers {
            h.push_str(k);
            h.push_str(": ");
            h.push_str(v);
            h.push_str("\r\n");
        }
        h.push_str("\r\n");
        h
    }
}

/// `s` as the account's request: `target` and `body` in place of its own
/// where given, its placeholders filled, its `Content-Length` the body's
/// (where it had one; a body where it had none gets one at the end, as
/// HTTP/1.1 needs it).
pub fn build(s: &Sent, fill: &Fill, target: Option<&str>, body: Option<&[u8]>) -> Result<Built, String> {
    let target = fill.fill(target.unwrap_or(&s.target))?;
    let body: Vec<u8> = match body {
        Some(b) => b.to_vec(),
        None => fill.fill(&s.body)?.into_bytes(),
    };
    let mut headers = Vec::with_capacity(s.headers.len() + 1);
    let mut had_length = false;
    for (k, v) in &s.headers {
        if v == capture::COOKIE {
            // (Sent where the client sends them, when there are any.)
            if let Some(c) = fill.cookie.as_deref().filter(|c| !c.is_empty()) {
                headers.push((k.clone(), c.to_owned()));
            }
        } else if k.eq_ignore_ascii_case("content-length") {
            had_length = true;
            headers.push((k.clone(), body.len().to_string()));
        } else if k.eq_ignore_ascii_case("transfer-encoding") {
            // (Sent whole, its length said.)
            had_length = true;
            headers.push(("Content-Length".to_owned(), body.len().to_string()));
        } else {
            headers.push((k.clone(), fill.fill(v)?));
        }
    }
    if !had_length && !body.is_empty() {
        headers.push(("Content-Length".to_owned(), body.len().to_string()));
    }
    Ok(Built { host: s.host.clone(), method: s.method.clone(), target, headers, body, alpn: s.alpn.clone() })
}

/// A captured JSON body with some of its fields set to the pool's own
/// values (a claim's grant and request ids), in the client's order and
/// form (compact, as it wrote it).
pub fn with_fields(body: &str, fields: &[(&str, Value)]) -> Result<Vec<u8>, String> {
    let mut v: Value = serde_json::from_str(body).map_err(|e| format!("the captured body isn’t JSON: {e}"))?;
    let obj = v.as_object_mut().ok_or("the captured body isn’t an object")?;
    for (k, value) in fields {
        match obj.get_mut(*k) {
            Some(slot) => *slot = value.clone(),
            None => {
                obj.insert((*k).to_owned(), value.clone());
            }
        }
    }
    serde_json::to_vec(&v).map_err(|e| e.to_string())
}

/// An answer: its status, headers and body (decoded).
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn json(&self) -> Result<Value, String> {
        serde_json::from_slice(&self.body).map_err(|e| e.to_string())
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).chars().take(200).collect()
    }
}

/// Where a captured host is reached: the pool's upstreams (the providers'
/// own, or a test's).
fn origin(host: &str, up: &Upstreams) -> Result<(bool, String, u16), String> {
    let base = match host {
        "api.anthropic.com" => &up.claude,
        "chatgpt.com" => &up.chatgpt,
        "platform.claude.com" | "console.anthropic.com" => &up.claude_oauth,
        "auth.openai.com" => &up.codex_oauth,
        other => return Err(format!("not a host the pool talks to: {other}")),
    };
    let (tls, rest) = if let Some(r) = base.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = base.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("not an address: {base}"));
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let (h, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), p.parse().map_err(|_| format!("not a port: {p}"))?),
        None => (authority.to_owned(), if tls { 443 } else { 80 }),
    };
    Ok((tls, h, port))
}

/// Send `b` (on its own connection, which closes after: the clients'
/// one-off reads and claims do the same, or are no different for it).
pub async fn send(b: &Built, up: &Upstreams) -> Result<Answer, String> {
    let (tls, host, port) = origin(&b.host, up)?;
    let tcp = tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host.as_str(), port))).await.map_err(|_| format!("{host}: no connection"))?.map_err(|e| format!("{host}: {e}"))?;
    let _ = tcp.set_nodelay(true);
    if !tls {
        return tokio::time::timeout(Duration::from_secs(60), http1(tcp, b)).await.map_err(|_| format!("{host}: no answer"))?;
    }
    let crypto = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config =
        rustls::ClientConfig::builder_with_provider(crypto).with_safe_default_protocol_versions().map_err(|e| e.to_string())?.with_root_certificates(capture::roots()).with_no_client_auth();
    config.alpn_protocols = b.alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    let name = ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
    let stream = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map_err(|e| format!("{host}: {e}"))?;
    let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2".as_slice());
    let work = async { if h2 { http2(stream, b).await } else { http1(stream, b).await } };
    tokio::time::timeout(Duration::from_secs(60), work).await.map_err(|_| format!("{host}: no answer"))?
}

/// HTTP/1.1: the head and body written as they are; the answer read to
/// its end.
async fn http1<S: AsyncRead + AsyncWrite + Unpin>(mut s: S, b: &Built) -> Result<Answer, String> {
    s.write_all(b.head().as_bytes()).await.map_err(|e| e.to_string())?;
    s.write_all(&b.body).await.map_err(|e| e.to_string())?;
    s.flush().await.map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16384];
    let end = loop {
        if let Some(e) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break e + 4;
        }
        let n = s.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("the connection closed before an answer".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).ok_or("not an HTTP answer")?;
    let headers: Vec<(String, String)> = lines.filter(|l| !l.is_empty()).filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))).collect();
    let get = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str());
    let mut body = buf[end..].to_vec();
    if get("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked")) {
        while !body.windows(5).any(|w| w == b"0\r\n\r\n") {
            let n = s.read(&mut chunk).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body = unchunk(&body);
    } else if let Some(len) = get("content-length").and_then(|l| l.trim().parse::<usize>().ok()) {
        while body.len() < len {
            let n = s.read(&mut chunk).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body.truncate(len);
    } else if !(status == 204 || status == 304 || b.method == "HEAD") {
        // (To the connection's end.)
        let _ = s.read_to_end(&mut body).await;
    }
    let body = decode(get("content-encoding").unwrap_or(""), body)?;
    Ok(Answer { status, headers, body })
}

/// A chunked body's chunks, joined.
fn unchunk(raw: &[u8]) -> Vec<u8> {
    let mut plain = Vec::new();
    let mut at = 0;
    while let Some(eol) = raw[at..].windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(String::from_utf8_lossy(&raw[at..at + eol]).split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        at += eol + 2;
        if size == 0 || at + size > raw.len() {
            break;
        }
        plain.extend_from_slice(&raw[at..at + size]);
        at += size + 2;
    }
    plain
}

/// A body decoded as its `content-encoding` says (the clients ask for
/// gzip, deflate, br and zstd).
fn decode(coding: &str, body: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    match coding.trim().to_ascii_lowercase().as_str() {
        "" | "identity" => return Ok(body),
        "gzip" | "x-gzip" => flate2::read::GzDecoder::new(&body[..]).read_to_end(&mut out).map_err(|e| e.to_string())?,
        "deflate" => flate2::read::ZlibDecoder::new(&body[..]).read_to_end(&mut out).map_err(|e| e.to_string())?,
        "br" => brotli_decompressor::Decompressor::new(&body[..], 4096).read_to_end(&mut out).map_err(|e| e.to_string())?,
        "zstd" => return zstd::stream::decode_all(&body[..]).map_err(|e| e.to_string()),
        other => return Err(format!("an encoding the pool can’t read: {other}")),
    };
    Ok(out)
}

/// HTTP/2 (where the client's offer has it and the host chose it): the
/// headers in the client's order (its `host` as the authority).
async fn http2<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(s: S, b: &Built) -> Result<Answer, String> {
    let (mut sender, conn) = hyper::client::conn::http2::handshake(hyper_util::rt::TokioExecutor::new(), hyper_util::rt::TokioIo::new(s)).await.map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let authority = b.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("host")).map(|(_, v)| v.clone()).unwrap_or_else(|| b.host.clone());
    let mut req = hyper::Request::builder().method(b.method.as_str()).uri(format!("https://{authority}{}", b.target));
    for (k, v) in &b.headers {
        // (What HTTP/2 says otherwise: the authority, the length, the connection.)
        if matches!(k.to_ascii_lowercase().as_str(), "host" | "connection" | "keep-alive" | "transfer-encoding" | "upgrade" | "proxy-connection") {
            continue;
        }
        req = req.header(k.to_ascii_lowercase(), v);
    }
    let req = req.body(http_body_util::Full::new(Bytes::from(b.body.clone()))).map_err(|e| e.to_string())?;
    let r = sender.send_request(req).await.map_err(|e| e.to_string())?;
    let status = r.status().as_u16();
    let headers: Vec<(String, String)> = r.headers().iter().map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned())).collect();
    let coding = r.headers().get("content-encoding").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    let body = r.into_body().collect().await.map_err(|e| e.to_string())?.to_bytes().to_vec();
    Ok(Answer { status, headers, body: decode(&coding, body)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(headers: &[(&str, &str)], body: &str) -> Sent {
        Sent {
            run: "print".into(),
            host: "api.anthropic.com".into(),
            method: "POST".into(),
            target: "/api/organizations/{{organization_uuid}}/reset_rate_limits".into(),
            headers: headers.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
            body: body.into(),
            body_len: body.len(),
            alpn: vec![],
            suites: vec![],
            schemes: vec![],
            passed_on: false,
        }
    }

    /// A request is the client's, byte for byte, but for the account's own
    /// values and the length of the body it carries.
    #[test]
    fn built_as_captured() {
        let s = sent(
            &[
                ("Accept", "application/json, text/plain, */*"),
                ("Content-Type", "application/json"),
                ("User-Agent", "claude-cli/2.1.329 (external, cli)"),
                ("Authorization", "Bearer {{access_token}}"),
                ("anthropic-beta", "oauth-2025-04-20"),
                ("Content-Length", "71"),
                ("Accept-Encoding", "gzip, compress, deflate, br"),
                ("Host", "api.anthropic.com"),
                ("Connection", "close"),
            ],
            r#"{"program":"cedar_ember","grant_id":"captured-grant","request_id":"captured-request"}"#,
        );
        let fill = Fill { access: "tok".into(), org: Some("org-1".into()), ..Fill::default() };
        let body = with_fields(&s.body, &[("grant_id", Value::from("g-2")), ("request_id", Value::from("r-3"))]).unwrap();
        let b = build(&s, &fill, None, Some(&body)).unwrap();
        assert_eq!(
            b.head(),
            "POST /api/organizations/org-1/reset_rate_limits HTTP/1.1\r\nAccept: application/json, text/plain, */*\r\nContent-Type: application/json\r\nUser-Agent: claude-cli/2.1.329 (external, cli)\r\nAuthorization: Bearer tok\r\nanthropic-beta: oauth-2025-04-20\r\nContent-Length: 61\r\nAccept-Encoding: gzip, compress, deflate, br\r\nHost: api.anthropic.com\r\nConnection: close\r\n\r\n"
        );
        assert_eq!(String::from_utf8(b.body).unwrap(), r#"{"program":"cedar_ember","grant_id":"g-2","request_id":"r-3"}"#);
    }

    /// A placeholder the account has no value for is never sent.
    #[test]
    fn nothing_sent_with_a_placeholder() {
        let s = sent(&[("Authorization", "Bearer {{access_token}}")], "");
        assert!(build(&s, &Fill { access: "tok".into(), ..Fill::default() }, None, None).is_err());
    }

    #[test]
    fn bodies_decoded() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, b"{\"a\":1}").unwrap();
        assert_eq!(decode("gzip", gz.finish().unwrap()).unwrap(), b"{\"a\":1}");
        assert_eq!(decode("", b"x".to_vec()).unwrap(), b"x");
        assert_eq!(unchunk(b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"), b"abcde");
    }
}
