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
    /// The account profile read (Claude organization uuid).
    Profile,
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
    /// The cookies each host set for this account's requests, which its
    /// client keeps and sends back to that host (Codex's cookie store):
    /// the pool's jar, (host, name, value).
    pub jar: Jar,
}

/// The runs to take Claude's requests from, the session's first: what a
/// person's own Claude Code sends.
const CLAUDE_RUNS: &[&str] = &["session", "wall", "print", "renewal"];
/// Codex's: its app server's first, whose reads are the passive ones a
/// background poll makes (its TUI's ask for Reserve, which the pool can't
/// use: Codex's review), and whose claim is Codex's own.
const CODEX_RUNS: &[&str] = &["app-server", "session", "print", "renewal"];
/// Antigravity's runs.
const AGY_RUNS: &[&str] = &["session", "models", "renewal"];

/// An account's cookies: what each host set, sent back as its client's
/// cookie store sends them (Codex's: RFC 6265's rules, as a browser's),
/// never another account's, nor the client's own. Each kept with the
/// domain it's for (its host alone, or a domain and its subdomains), the
/// path it's for, and when it expires (Codex's third review: kept by host
/// alone, every cookie went with every request, an expired one too).
pub type Jar = Arc<std::sync::Mutex<Vec<Kept>>>;

/// A cookie in a [`Jar`], in the order they were first set.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    pub name: String,
    pub value: String,
    /// The host that set it (`host_only`), or the domain it was set for.
    pub domain: String,
    pub host_only: bool,
    pub path: String,
    /// When it expires (Unix seconds): never, for the pool's life.
    pub expires: Option<i64>,
}

/// Whether `host` is `domain` or one of its subdomains (RFC 6265, 5.1.3).
fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|h| h.ends_with('.')) && host.parse::<std::net::IpAddr>().is_err()
}

/// Whether a request to `path` is within a cookie's `cookie_path` (5.1.4).
fn path_matches(path: &str, cookie_path: &str) -> bool {
    path == cookie_path || path.starts_with(cookie_path) && (cookie_path.ends_with('/') || path[cookie_path.len()..].starts_with('/'))
}

/// A cookie's path where it names none: the request's, up to its last `/`
/// (5.1.4).
fn default_path(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) if path.starts_with('/') => path[..i].into(),
        _ => "/".into(),
    }
}

/// The `Cookie` a request to `host`'s `path` carries from `jar`, if any:
/// those whose domain and path it's within and that haven't expired, the
/// longer paths first, then the older (5.4).
pub fn cookie(jar: &Jar, host: &str, path: &str) -> Option<String> {
    let now = crate::model::now();
    let mut jar = jar.lock().unwrap();
    jar.retain(|c| c.expires.is_none_or(|e| e > now));
    let mut theirs: Vec<(usize, &Kept)> =
        jar.iter().enumerate().filter(|(_, c)| if c.host_only { c.domain == host } else { domain_matches(host, &c.domain) }).filter(|(_, c)| path_matches(path, &c.path)).collect();
    theirs.sort_by(|(i, a), (j, b)| b.path.len().cmp(&a.path.len()).then(i.cmp(j)));
    (!theirs.is_empty()).then(|| theirs.iter().map(|(_, c)| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; "))
}

/// What an answer from `host` to a request for `path` set (each
/// `set-cookie` value), kept in `jar` (5.3): its Domain if `host` is
/// within it (else the cookie isn't kept), its Path or the request's, its
/// Max-Age or else its Expires; one of the same name, domain and path
/// replaced (keeping its place), one set to expire gone.
pub fn keep<'a>(jar: &Jar, host: &str, path: &str, set: impl IntoIterator<Item = &'a str>) {
    let now = crate::model::now();
    let mut jar = jar.lock().unwrap();
    'cookies: for v in set {
        let mut parts = v.split(';');
        let Some((name, value)) = parts.next().and_then(|nv| nv.split_once('=')).map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned())) else { continue };
        if name.is_empty() {
            continue;
        }
        let (mut domain, mut cookie_path, mut max_age, mut expires) = (None, None, None, None);
        for a in parts {
            let (k, v) = a.split_once('=').map_or((a.trim(), ""), |(k, v)| (k.trim(), v.trim()));
            match k.to_ascii_lowercase().as_str() {
                "domain" if !v.is_empty() => domain = Some(v.trim_start_matches('.').to_ascii_lowercase()),
                "path" if v.starts_with('/') => cookie_path = Some(v.to_owned()),
                "max-age" => max_age = v.parse::<i64>().ok(),
                "expires" => expires = chrono::DateTime::parse_from_rfc2822(&v.replace('-', " ")).ok().map(|t| t.timestamp()),
                _ => {}
            }
        }
        let (domain, host_only) = match domain {
            // (Only a domain the host is within, and not a bare top level.)
            Some(d) if d.contains('.') && domain_matches(host, &d) => (d, false),
            Some(_) => continue 'cookies,
            None => (host.to_owned(), true),
        };
        let path = cookie_path.unwrap_or_else(|| default_path(path));
        let expires = match max_age {
            Some(n) => Some(now.saturating_add(n)),
            None => expires,
        };
        let kept = Kept { name, value, domain, host_only, path, expires };
        let gone = kept.expires.is_some_and(|e| e <= now);
        match jar.iter().position(|c| c.name == kept.name && c.domain == kept.domain && c.path == kept.path) {
            Some(i) if gone => {
                jar.remove(i);
            }
            Some(i) => jar[i] = kept,
            None if gone => {}
            None => jar.push(kept),
        }
    }
}

impl Like {
    /// The account's own values, for a request to `host`'s `path`.
    pub fn fill(&self, c: &Credential, host: &str, path: &str) -> Fill {
        let cookie = cookie(&self.jar, host, path);
        Fill { access: c.access.clone(), refresh: c.refresh.clone(), account_id: c.account_id.clone(), org: self.org.clone(), account_uuid: None, email: self.email.clone(), cookie }
    }

    /// What an answer from `host` to a request for `path` set
    /// (`set-cookie`), kept as its client keeps it.
    pub fn remember(&self, host: &str, path: &str, headers: &[(String, String)]) {
        keep(&self.jar, host, path, headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")).map(|(_, v)| v.as_str()));
    }

    /// The captured request `what` is made from, if this account's client
    /// was captured making it (or the one it's made like, below).
    pub fn request(&self, what: What) -> Option<Sent> {
        let c = &self.capture;
        match (c.provider, what) {
            (Provider::Claude, What::Usage | What::Credits) => c.find("api.anthropic.com", "GET", "/api/oauth/usage", CLAUDE_RUNS).cloned().map(as_session),
            // Profile uses `claude-code/<ver>` with no `anthropic-beta`; never
            // synthesize it from `/api/oauth/usage` (which uses `claude-cli/...`).
            (Provider::Claude, What::Profile) => c.find("api.anthropic.com", "GET", "/api/oauth/profile", CLAUDE_RUNS).cloned(),
            (Provider::Claude, What::Claim) => {
                let path = format!("/api/organizations/{}/reset_rate_limits", capture::ORG);
                c.find("api.anthropic.com", "POST", &path, CLAUDE_RUNS).cloned().or_else(|| claude_claim(c.find("api.anthropic.com", "GET", "/api/oauth/usage", CLAUDE_RUNS)?)).map(as_session)
            }
            (Provider::Claude, What::Renewal) => {
                c.find("platform.claude.com", "POST", "/v1/oauth/token", &["renewal"]).or_else(|| c.find("console.anthropic.com", "POST", "/v1/oauth/token", &["renewal"])).cloned()
            }
            // (The reads Codex makes on its own, never its TUI's, which asks
            // for Reserve: Codex's review. None captured, the pool's own.)
            (Provider::Codex, What::Usage) => passive(c, "/backend-api/wham/usage"),
            (Provider::Codex, What::Credits) => passive(c, "/backend-api/wham/rate-limit-reset-credits"),
            (Provider::Codex, What::Claim) => c.find("chatgpt.com", "POST", "/backend-api/wham/rate-limit-reset-credits/consume", CODEX_RUNS).cloned(),
            (Provider::Codex, What::Renewal) => c.find("auth.openai.com", "POST", "/oauth/token", &["renewal"]).cloned(),
            (Provider::Codex, What::Profile) => None,
            (Provider::Antigravity, What::Usage | What::Credits) => c
                .find("daily-cloudcode-pa.googleapis.com", "POST", "/v1internal:retrieveUserQuotaSummary", AGY_RUNS)
                .or_else(|| c.find("cloudcode-pa.googleapis.com", "POST", "/v1internal:retrieveUserQuotaSummary", AGY_RUNS))
                .cloned(),
            (Provider::Antigravity, What::Profile) => c
                .find("daily-cloudcode-pa.googleapis.com", "POST", "/v1internal:loadCodeAssist", AGY_RUNS)
                .or_else(|| c.find("cloudcode-pa.googleapis.com", "POST", "/v1internal:loadCodeAssist", AGY_RUNS))
                .cloned(),
            (Provider::Antigravity, What::Claim) => c
                .find("daily-cloudcode-pa.googleapis.com", "POST", "/v1internal:claimBankedQuotaReset", AGY_RUNS)
                .or_else(|| c.find("cloudcode-pa.googleapis.com", "POST", "/v1internal:claimBankedQuotaReset", AGY_RUNS))
                .cloned()
                .or_else(|| {
                    c.find("daily-cloudcode-pa.googleapis.com", "POST", "/v1internal:retrieveUserQuotaSummary", AGY_RUNS)
                        .or_else(|| c.find("cloudcode-pa.googleapis.com", "POST", "/v1internal:retrieveUserQuotaSummary", AGY_RUNS))
                        .or_else(|| c.find("daily-cloudcode-pa.googleapis.com", "POST", "/v1internal:loadCodeAssist", AGY_RUNS))
                        .or_else(|| c.find("cloudcode-pa.googleapis.com", "POST", "/v1internal:loadCodeAssist", AGY_RUNS))
                        .map(agy_claim)
                }),
            (Provider::Antigravity, What::Renewal) => c.find("oauth2.googleapis.com", "POST", "/token", &["renewal"]).cloned(),
        }
    }
}

/// Synthesize Antigravity's `claimBankedQuotaReset` request from a captured
/// Cloud Code POST (`retrieveUserQuotaSummary` or `loadCodeAssist`) when no
/// claim has been captured directly yet, preserving `agy`'s exact headers & ALPN.
fn agy_claim(base: &Sent) -> Sent {
    let mut headers = Vec::with_capacity(base.headers.len() + 1);
    let mut has_ct = false;
    for (k, v) in &base.headers {
        if k.eq_ignore_ascii_case("content-type") {
            has_ct = true;
            headers.push((k.clone(), "application/json".to_owned()));
        } else {
            headers.push((k.clone(), v.clone()));
        }
    }
    if !has_ct {
        headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    }
    Sent {
        run: format!("{}+claim", base.run),
        host: base.host.clone(),
        method: "POST".into(),
        target: "/v1internal:claimBankedQuotaReset".into(),
        headers,
        body: r#"{"project":"","resetId":"","requestId":""}"#.into(),
        body_len: 0,
        alpn: base.alpn.clone(),
        suites: base.suites.clone(),
        schemes: base.schemes.clone(),
        passed_on: false,
    }
}

/// The last read of ChatGPT's `path` Codex made on its own (its app
/// server's, its `exec`'s), not its TUI's: that one asks for Reserve
/// (`x-openai-codex-luna-reserve`), which a poll of the pool's mustn't.
fn passive(c: &crate::capture::Capture, path: &str) -> Option<Sent> {
    ["app-server", "print", "renewal"]
        .iter()
        .find_map(|run| {
            c.requests
                .iter()
                .rev()
                .find(|s| s.run == *run && s.host == "chatgpt.com" && s.method == "GET" && s.path() == path && !s.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("x-openai-codex-luna-reserve")))
        })
        .cloned()
}

/// A Claude Code request as its session sends it: print mode's names its
/// entrypoint `sdk-cli` where a session says `cli`, the only difference
/// between the two's quota reads as captured, and Claude answers the credit
/// read of print mode's as the wrong surface (no credits). Used where a
/// capture's session missed its read.
fn as_session(mut s: Sent) -> Sent {
    if s.run != "session" && s.run != "wall" {
        for (k, v) in s.headers.iter_mut() {
            if k.eq_ignore_ascii_case("user-agent") && v.contains("(external, sdk-cli") {
                *v = v.replace("(external, sdk-cli", "(external, cli");
            }
        }
    }
    s
}

/// Claude Code's claim of a reset, which it makes with the same API client
/// as its quota read (`Xt.post` with `"Content-Type": "application/json"`,
/// the program, grant and request ids, in that order), where its interface
/// doesn't offer one to capture: that read's head as a POST, with the
/// `Content-Length` that client puts before `Accept-Encoding` (as its
/// sign-in renewal, a POST of the same client, shows).
fn claude_claim(usage: &Sent) -> Option<Sent> {
    let mut headers = Vec::with_capacity(usage.headers.len() + 2);
    let has_content_type = usage.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    let has_content_length = usage.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
    let mut placed_ct = has_content_type;
    let mut placed_cl = has_content_length;
    for (k, v) in &usage.headers {
        if k.eq_ignore_ascii_case("content-type") {
            headers.push((k.clone(), "application/json".to_owned()));
            continue;
        }
        if k.eq_ignore_ascii_case("content-length") {
            headers.push((k.clone(), "0".to_owned()));
            continue;
        }
        if !placed_cl && k.eq_ignore_ascii_case("accept-encoding") {
            if !placed_ct {
                headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
                placed_ct = true;
            }
            headers.push(("Content-Length".to_owned(), "0".to_owned()));
            placed_cl = true;
        }
        headers.push((k.clone(), v.clone()));
        if !placed_ct && k.eq_ignore_ascii_case("accept") {
            headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
            placed_ct = true;
        }
    }
    if !placed_cl {
        return None;
    }
    if !placed_ct {
        headers.insert(0, ("Content-Type".to_owned(), "application/json".to_owned()));
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

    /// As [`Self::fill`], percent-encoding substituted values for
    /// `application/x-www-form-urlencoded` bodies (e.g. Google OAuth refresh
    /// tokens containing `/` or `+`).
    pub fn fill_form(&self, text: &str) -> Result<String, String> {
        let mut out = text.to_owned();
        for (placeholder, value) in self.pairs() {
            if out.contains(placeholder) {
                let v = value.ok_or_else(|| format!("no value for {placeholder}"))?;
                out = out.replace(placeholder, &crate::upstream::url_encode(v));
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
    let is_form = s.header("content-type").is_some_and(|ct| ct.to_ascii_lowercase().starts_with("application/x-www-form-urlencoded"));
    let body: Vec<u8> = match body {
        Some(b) => b.to_vec(),
        None if is_form => fill.fill_form(&s.body)?.into_bytes(),
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
        "daily-cloudcode-pa.googleapis.com" => &up.agy,
        "cloudcode-pa.googleapis.com" => &up.agy_prod,
        "platform.claude.com" | "console.anthropic.com" => &up.claude_oauth,
        "auth.openai.com" => &up.codex_oauth,
        "oauth2.googleapis.com" => &up.agy_oauth,
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

/// Whether a transport error happened strictly before any request bytes left
/// this host (so a reset claim definitely never reached the provider) or after
/// transmission may have started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    BeforeSend(String),
    AfterSend(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::BeforeSend(s) | SendError::AfterSend(s) => f.write_str(s),
        }
    }
}

/// Send `b` (on its own connection, which closes after: the clients'
/// one-off reads and claims do the same, or are no different for it).
pub async fn send(b: &Built, up: &Upstreams) -> Result<Answer, SendError> {
    let (tls, host, port) = origin(&b.host, up).map_err(SendError::BeforeSend)?;
    let tcp = tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| SendError::BeforeSend(format!("{host}: no connection")))?
        .map_err(|e| SendError::BeforeSend(format!("{host}: {e}")))?;
    let _ = tcp.set_nodelay(true);
    if !tls {
        return tokio::time::timeout(Duration::from_secs(60), http1(tcp, b)).await.map_err(|_| SendError::AfterSend(format!("{host}: no answer")))?.map_err(SendError::AfterSend);
    }
    let crypto = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(crypto)
        .with_safe_default_protocol_versions()
        .map_err(|e| SendError::BeforeSend(e.to_string()))?
        .with_root_certificates(capture::roots())
        .with_no_client_auth();
    config.alpn_protocols = b.alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    let name = ServerName::try_from(host.clone()).map_err(|e| SendError::BeforeSend(e.to_string()))?;
    // TLS handshake happens before any HTTP bytes are written; failures here
    // cannot have delivered the request.
    let stream = tokio::time::timeout(Duration::from_secs(20), tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp))
        .await
        .map_err(|_| SendError::BeforeSend(format!("{host}: TLS handshake timed out")))?
        .map_err(|e| SendError::BeforeSend(format!("{host}: {e}")))?;
    let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2".as_slice());
    let work = async { if h2 { http2(stream, b).await } else { http1(stream, b).await } };
    tokio::time::timeout(Duration::from_secs(60), work).await.map_err(|_| SendError::AfterSend(format!("{host}: no answer")))?.map_err(SendError::AfterSend)
}

/// Maximum response headers and body size read by control-plane requests
/// (`likeness`), both on the wire and after decompression.
const MAX_HEAD: usize = 64 << 10;
const MAX_BODY: usize = 2 << 20;

/// HTTP/1.1: the head and body written as they are; the answer read to
/// its end (bounded to `MAX_HEAD` / `MAX_BODY`).
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
        if buf.len() > MAX_HEAD {
            return Err(format!("response headers exceeded {MAX_HEAD} bytes"));
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
    if body.len() > MAX_BODY {
        return Err(format!("response body exceeded {MAX_BODY} bytes"));
    }
    if get("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked")) {
        while !body.windows(5).any(|w| w == b"0\r\n\r\n") {
            let n = s.read(&mut chunk).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            if body.len().saturating_add(n) > MAX_BODY {
                return Err(format!("response body exceeded {MAX_BODY} bytes"));
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body = unchunk(&body);
    } else if let Some(len) = get("content-length").and_then(|l| l.trim().parse::<usize>().ok()) {
        if len > MAX_BODY {
            return Err(format!("response body exceeded {MAX_BODY} bytes"));
        }
        while body.len() < len {
            let n = s.read(&mut chunk).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            let take = n.min(len.saturating_sub(body.len()));
            body.extend_from_slice(&chunk[..take]);
        }
        body.truncate(len);
    } else if !(status == 204 || status == 304 || b.method == "HEAD") {
        // (To the connection's end, bounded to MAX_BODY.)
        let remaining = (MAX_BODY + 1).saturating_sub(body.len());
        let mut tail = Vec::new();
        let _ = s.take(remaining as u64).read_to_end(&mut tail).await;
        body.extend_from_slice(&tail);
        if body.len() > MAX_BODY {
            return Err(format!("response body exceeded {MAX_BODY} bytes"));
        }
    }
    let body = decode(get("content-encoding").unwrap_or(""), body)?;
    Ok(Answer { status, headers, body })
}

/// Where a chunked body that starts `raw` ends, as its framing says: past
/// its last chunk (size 0), its trailers and the blank line after them.
/// None while more is to come; an error for framing that isn't chunked
/// (a size that isn't hex, a chunk not ended by its CRLF). Read by its
/// sizes, not by looking for the last chunk's bytes (Codex's third
/// review: a chunk's own data could hold them).
pub(crate) fn chunked_end(raw: &[u8]) -> Option<Result<usize, ()>> {
    let mut at = 0;
    loop {
        let eol = raw[at..].windows(2).position(|w| w == b"\r\n")?;
        let line = std::str::from_utf8(&raw[at..at + eol]).map_err(|_| ());
        let size = line.and_then(|l| usize::from_str_radix(l.split(';').next().unwrap_or("").trim(), 16).map_err(|_| ()));
        let size = match size {
            Ok(n) => n,
            Err(()) => return Some(Err(())),
        };
        at += eol + 2;
        if size == 0 {
            // Trailers, each a line, then a blank one.
            loop {
                let eol = raw[at..].windows(2).position(|w| w == b"\r\n")?;
                at += eol + 2;
                if eol == 0 {
                    return Some(Ok(at));
                }
            }
        }
        let Some(end) = at.checked_add(size) else { return Some(Err(())) };
        if raw.len() < end + 2 {
            return None;
        }
        if &raw[end..end + 2] != b"\r\n" {
            return Some(Err(()));
        }
        at = end + 2;
    }
}

/// A chunked body's chunks, joined.
pub(crate) fn unchunk(raw: &[u8]) -> Vec<u8> {
    let mut plain = Vec::new();
    let mut at = 0;
    while let Some(eol) = raw[at..].windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(String::from_utf8_lossy(&raw[at..at + eol]).split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        at += eol + 2;
        // (A size past what came, or past what a size can say, ends it.)
        let Some(end) = at.checked_add(size).filter(|&e| size > 0 && e <= raw.len()) else { break };
        plain.extend_from_slice(&raw[at..end]);
        at = end.saturating_add(2).min(raw.len());
    }
    plain
}

/// A body decoded as its `content-encoding` says (the clients ask for
/// gzip, deflate, br and zstd), bounded to `MAX_BODY` decoded bytes.
fn decode(coding: &str, body: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let reader: Box<dyn Read + '_> = match coding.trim().to_ascii_lowercase().as_str() {
        "" | "identity" => {
            if body.len() > MAX_BODY {
                return Err(format!("response body exceeded {MAX_BODY} bytes"));
            }
            return Ok(body);
        }
        "gzip" | "x-gzip" => Box::new(flate2::read::GzDecoder::new(&body[..])),
        "deflate" => Box::new(flate2::read::ZlibDecoder::new(&body[..])),
        "br" => Box::new(brotli_decompressor::Decompressor::new(&body[..], 4096)),
        "zstd" => Box::new(zstd::stream::read::Decoder::new(&body[..]).map_err(|e| e.to_string())?),
        other => return Err(format!("an encoding the pool can’t read: {other}")),
    };
    reader.take((MAX_BODY + 1) as u64).read_to_end(&mut out).map_err(|e| e.to_string())?;
    if out.len() > MAX_BODY {
        return Err(format!("decoded response body exceeded {MAX_BODY} bytes"));
    }
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
    let body = http_body_util::Limited::new(r.into_body(), MAX_BODY).collect().await.map_err(|e| e.to_string())?.to_bytes().to_vec();
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

    /// Codex's quota read for the pool's polls is one it makes on its own
    /// (its app server's), never its TUI's, which asks for Reserve (Codex's
    /// review: a capture with only the TUI's had the pool poll with it).
    #[test]
    fn codex_polls_with_a_passive_read() {
        let read = |run: &str, reserve: bool| {
            let mut s = sent(&[("user-agent", "codex")], "");
            (s.run, s.host, s.method, s.target) = (run.into(), "chatgpt.com".into(), "GET".into(), "/backend-api/wham/usage".into());
            if reserve {
                s.headers.push(("x-openai-codex-luna-reserve".into(), "1".into()));
            }
            s
        };
        let like = |requests: Vec<Sent>| Like {
            capture: Arc::new(Capture { provider: Provider::Codex, version: "0".into(), installed: String::new(), account: "a".into(), at: 0, requests, account_uuid: None, hellos: vec![] }),
            org: None,
            email: None,
            jar: Default::default(),
        };
        assert!(like(vec![read("session", true)]).request(What::Usage).is_none());
        assert_eq!(like(vec![read("app-server", false), read("session", true)]).request(What::Usage).map(|s| s.run), Some("app-server".to_owned()));
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

    /// Cookies kept and sent as Codex's cookie store keeps and sends
    /// them (RFC 6265): a domain's to its subdomains, a host's to it
    /// alone; within their path; gone when they expire; replaced by name,
    /// domain and path; the longer path first.
    #[test]
    fn cookies_as_a_browser_keeps_them() {
        let jar: Jar = Default::default();
        let later = chrono::DateTime::from_timestamp(crate::model::now() + 3600, 0).unwrap().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        keep(
            &jar,
            "chatgpt.com",
            "/backend-api/codex/responses",
            [
                "__cf_bm=one; path=/; expires=".to_owned() + &later + "; domain=.chatgpt.com; HttpOnly; Secure",
                "_cfuvid=two; path=/; domain=.chatgpt.com".to_owned(),
                "here=three".to_owned(),
                "codex=four; Path=/backend-api/codex".to_owned(),
                "stale=five; Max-Age=0".to_owned(),
                "elsewhere=six; Domain=example.com".to_owned(),
                "top=seven; Domain=com".to_owned(),
            ]
            .iter()
            .map(String::as_str),
        );
        let got = |host: &str, path: &str| cookie(&jar, host, path).unwrap_or_default();
        assert_eq!(got("chatgpt.com", "/backend-api/codex/responses"), "here=three; codex=four; __cf_bm=one; _cfuvid=two");
        assert_eq!(got("chatgpt.com", "/backend-api/wham/usage"), "__cf_bm=one; _cfuvid=two");
        assert_eq!(got("ab.chatgpt.com", "/backend-api/codex/x"), "__cf_bm=one; _cfuvid=two");
        assert_eq!(got("notchatgpt.com", "/"), "");
        // Replaced in its place; set to expire, gone; an expired one never sent.
        keep(&jar, "chatgpt.com", "/", ["__cf_bm=one-again; path=/; domain=chatgpt.com", "_cfuvid=; path=/; domain=chatgpt.com; Max-Age=-1"]);
        jar.lock().unwrap().push(Kept { name: "old".into(), value: "x".into(), domain: "chatgpt.com".into(), host_only: true, path: "/".into(), expires: Some(crate::model::now() - 1) });
        assert_eq!(got("chatgpt.com", "/backend-api/wham/usage"), "__cf_bm=one-again");
        assert_eq!(default_path("/backend-api/codex/responses"), "/backend-api/codex");
        assert_eq!(default_path("/v1"), "/");
        assert!(path_matches("/backend-api/codex/x", "/backend-api/codex") && !path_matches("/backend-api/codexes", "/backend-api/codex"));
    }

    /// A chunked body ends where its framing says: its last chunk, its
    /// trailers, the blank line; not where a chunk's own data holds what
    /// a last chunk looks like.
    #[test]
    fn a_chunked_body_ends_by_its_sizes() {
        let tricky = b"9\r\n0\r\n\r\nabcd\r\n3\r\nxyz\r\n0\r\n\r\nNEXT";
        assert_eq!(chunked_end(tricky), Some(Ok(tricky.len() - 4)));
        assert_eq!(unchunk(&tricky[..tricky.len() - 4]), b"0\r\n\r\nabcdxyz");
        let trailers = b"3;ext=1\r\nabc\r\n0\r\nx-checksum: 1\r\n\r\n";
        assert_eq!(chunked_end(trailers), Some(Ok(trailers.len())));
        for more in [&b"9\r\n0\r\n\r\nab"[..], b"3\r\nabc\r\n", b"3\r\nabc\r\n0\r\nx: 1\r\n", b""] {
            assert_eq!(chunked_end(more), None, "{:?}", String::from_utf8_lossy(more));
        }
        for broken in [&b"zz\r\n"[..], b"3\r\nabcXY0\r\n\r\n"] {
            assert_eq!(chunked_end(broken), Some(Err(())), "{:?}", String::from_utf8_lossy(broken));
        }
    }

    #[test]
    fn a_chunk_too_big_ends_it() {
        assert_eq!(unchunk(b"2\r\nok\r\nffffffffffffffff\r\nrest"), b"ok");
    }

    /// A Claude claim derived from a `"print"` capture rewrites
    /// `(external, sdk-cli...)` to `(external, cli)`, normalizes `Content-Type`
    /// to `application/json`, and never duplicates `Content-Length`.
    /// `What::Profile` is only taken from an actual `/api/oauth/profile` capture
    /// (never synthesized from `/api/oauth/usage`).
    #[test]
    fn claude_claim_and_profile_from_print_capture_use_cli_surface() {
        for ua in ["claude-cli/2.1.332 (external, sdk-cli)", "claude-cli/2.1.332 (external, sdk-cli, workload/agent)"] {
            let mut usage = sent(
                &[
                    ("Accept", "application/json, text/plain, */*"),
                    ("User-Agent", ua),
                    ("Authorization", "Bearer {{access_token}}"),
                    ("anthropic-beta", "oauth-2025-04-20"),
                    ("Content-Length", "0"),
                    ("Accept-Encoding", "gzip, compress, deflate, br"),
                    ("Host", "api.anthropic.com"),
                ],
                "",
            );
            usage.method = "GET".into();
            usage.target = "/api/oauth/usage".into();
            let like = Like {
                capture: Arc::new(Capture {
                    provider: Provider::Claude,
                    version: "2.1.332".into(),
                    installed: String::new(),
                    account: "a".into(),
                    at: 0,
                    requests: vec![usage],
                    account_uuid: None,
                    hellos: vec![],
                }),
                org: Some("org-1".into()),
                email: None,
                jar: Default::default(),
            };
            assert!(like.request(What::Profile).is_none(), "profile must not be synthesized from usage capture");
            for what in [What::Credits, What::Claim] {
                let s = like.request(what).expect("request built");
                let got_ua = s.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("user-agent")).map(|(_, v)| v.as_str()).unwrap();
                assert!(got_ua.contains("(external, cli"), "{what:?}: {got_ua}");
                assert!(!got_ua.contains("sdk-cli"), "{what:?}: {got_ua}");
                if matches!(what, What::Claim) {
                    assert_eq!(s.method, "POST");
                    assert!(s.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "application/json"));
                    assert_eq!(s.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("content-length")).count(), 1, "no duplicate Content-Length");
                }
            }
        }
    }

    #[test]
    fn bodies_decoded() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, b"{\"a\":1}").unwrap();
        assert_eq!(decode("gzip", gz.finish().unwrap()).unwrap(), b"{\"a\":1}");
        assert_eq!(decode("", b"x".to_vec()).unwrap(), b"x");
        assert_eq!(unchunk(b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"), b"abcde");
        // (Cut off anywhere: what came, never a panic.)
        let whole = b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        for n in 0..whole.len() {
            let _ = unchunk(&whole[..n]);
        }
    }

    #[test]
    fn antigravity_form_renewal_and_claim_synthesis() {
        // Form-urlencoded renewal body percent-encodes `{{refresh_token}}` containing `/` and `+`.
        let mut renew = sent(
            &[("Content-Type", "application/x-www-form-urlencoded"), ("Host", "oauth2.googleapis.com"), ("Content-Length", "10")],
            "client_id=cid&refresh_token={{refresh_token}}&grant_type=refresh_token",
        );
        renew.host = "oauth2.googleapis.com".into();
        renew.target = "/token".into();
        let fill = Fill { refresh: Some("1//0abc+def/ghi".into()), ..Fill::default() };
        let built = build(&renew, &fill, None, None).unwrap();
        assert_eq!(String::from_utf8(built.body).unwrap(), "client_id=cid&refresh_token=1%2F%2F0abc%2Bdef%2Fghi&grant_type=refresh_token");

        // Synthesizing What::Claim from a captured Antigravity retrieveUserQuotaSummary request.
        let mut usage = sent(
            &[
                ("Authorization", "Bearer {{access_token}}"),
                ("Content-Type", "application/json"),
                ("User-Agent", "Antigravity/1.5.18 (linux; amd64)"),
                ("Host", "daily-cloudcode-pa.googleapis.com"),
                ("Content-Length", "2"),
            ],
            "{}",
        );
        usage.run = "models".into();
        usage.host = "daily-cloudcode-pa.googleapis.com".into();
        usage.target = "/v1internal:retrieveUserQuotaSummary".into();
        let like = Like {
            capture: Arc::new(Capture {
                provider: Provider::Antigravity,
                version: "1.5.18".into(),
                installed: String::new(),
                account: "agy-1".into(),
                at: 0,
                requests: vec![usage],
                account_uuid: None,
                hellos: vec![],
            }),
            org: None,
            email: None,
            jar: Default::default(),
        };
        let claim = like.request(What::Claim).expect("agy claim synthesized");
        assert_eq!((claim.method.as_str(), claim.target.as_str()), ("POST", "/v1internal:claimBankedQuotaReset"));
        assert!(claim.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("user-agent") && v.contains("Antigravity/1.5.18")));
    }
}
