//! The proxy against a fake provider standing in for every host: never a
//! real account or service. A client trusting the pool's certificate
//! reaches "api.anthropic.com" and "chatgpt.com" through it, as Claude
//! Code and Codex do.

use olo_sync::Mutex;
use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use olo_token_pool::{
    authority::Authority,
    gateway::Shared,
    intercept,
    model::{Account, Credential, Provider, now},
    store::Saved,
    upstream::Upstreams,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What the fake saw: (path, its sign-in, every header).
type Seen = Arc<Mutex<Vec<(String, String, HeaderMap)>>>;

fn bearer(h: &HeaderMap) -> String {
    h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ").to_owned()
}

fn sse(text: &str) -> Response {
    Response::builder()
        .header("content-type", "text/event-stream")
        .header("anthropic-ratelimit-unified-5h-utilization", "0.25")
        .body(Body::from(format!(
            "data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":7}}}}}}\n\ndata: {{\"type\":\"content_block_delta\",\"delta\":{{\"text\":\"{text}\"}}}}\n\ndata: {{\"type\":\"message_delta\",\"usage\":{{\"output_tokens\":3}}}}\n\n"
        )))
        .unwrap()
}

fn fake(seen: Seen) -> Router {
    let note = move |path: &'static str| {
        let seen = seen.clone();
        move |h: HeaderMap| {
            seen.lock().unwrap().push((path.to_owned(), bearer(&h), h.clone()));
            async move {}
        }
    };
    let (m, p, r, u) = (note("/v1/messages"), note("/api/oauth/profile"), note("/backend-api/codex/responses"), note("/backend-api/wham/usage"));
    Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let f = m(h);
                async move {
                    f.await;
                    sse("hello")
                }
            }),
        )
        .route(
            "/api/oauth/profile",
            get(move |h: HeaderMap| {
                let f = p(h);
                async move {
                    f.await;
                    Response::builder().header("content-encoding", "identity").header("x-from", "profile").body(Body::from(r#"{"account":"theirs"}"#)).unwrap()
                }
            }),
        )
        .route(
            "/backend-api/codex/responses",
            post(move |h: HeaderMap| {
                let f = r(h);
                async move {
                    f.await;
                    sse("codex says hi")
                }
            }),
        )
        .route(
            "/backend-api/wham/usage",
            get(move |h: HeaderMap| {
                let f = u(h);
                async move {
                    f.await;
                    axum::Json(json!({"plan_type": "pro"}))
                }
            }),
        )
}

async fn serve(app: Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

fn account(id: &str, provider: Provider) -> Account {
    let mut a = Account::new(id.into(), id.into(), provider);
    a.created = 1;
    a
}

#[tokio::test(flavor = "multi_thread")]
async fn opens_the_providers_hosts_and_tunnels_the_rest() {
    let seen: Seen = Default::default();
    let up = serve(fake(seen.clone())).await;
    let mut saved = Saved::default();
    saved.accounts.push(account("claude-a", Provider::Claude));
    saved.credentials.insert("claude-a".into(), Credential { access: "pooled".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    saved.accounts.push(account("codex-a", Provider::Codex));
    saved.credentials.insert("codex-a".into(), Credential { access: "pooled-codex".into(), refresh: None, expires_at: Some(now() + 3600), account_id: Some("acct-1".into()) });
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.serves(p)), intercept::Who::Anyone).await.unwrap();

    // A client as Claude Code and Codex are: through the proxy, trusting
    // the pool's certificate besides the usual ones.
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::https(format!("http://{proxy}")).unwrap())
        .add_root_certificate(reqwest::Certificate::from_pem(authority.cert_pem.as_bytes()).unwrap())
        .build()
        .unwrap();

    // A model request: the pool's account's sign-in, not the client's.
    let r = client
        .post("https://api.anthropic.com/v1/messages?beta=true")
        .header("authorization", "Bearer theirs")
        .header("anthropic-beta", "oauth-2025-04-20,claude-code-20250219")
        .header("x-organization-uuid", "their-org")
        .json(&json!({"model": "claude-opus-5-5", "stream": true, "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["x-olo-pool-account"], "claude-a");
    assert!(r.text().await.unwrap().contains("hello"));

    // Anything else on the host: the client's own, untouched.
    let r = client.get("https://api.anthropic.com/api/oauth/profile").header("authorization", "Bearer theirs").header("accept-encoding", "gzip").send().await.unwrap();
    assert_eq!((r.status().as_u16(), r.headers()["x-from"].to_str().unwrap()), (200, "profile"));
    assert_eq!(r.text().await.unwrap(), r#"{"account":"theirs"}"#);

    // Codex's host likewise, its body compressed as Codex sends it there
    // (and its model still seen).
    let body = zstd::encode_all(json!({"model": "gpt-5-codex", "stream": true}).to_string().as_bytes(), 3).unwrap();
    let r = client
        .post("https://chatgpt.com/backend-api/codex/responses")
        .header("authorization", "Bearer theirs")
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["x-olo-pool-account"], "codex-a");
    assert!(r.text().await.unwrap().contains("codex says hi"));
    for _ in 0..50 {
        if !shared.pool.lock().unwrap().served.is_empty() && shared.pool.lock().unwrap().open.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(shared.pool.lock().unwrap().served.iter().any(|s| s.provider == Provider::Codex && s.request.model == "gpt-5-codex"), "the model wasn't read from the compressed body");
    let r = client.get("https://chatgpt.com/backend-api/wham/usage").header("authorization", "Bearer theirs").send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), r#"{"plan_type":"pro"}"#);

    let seen = seen.lock().unwrap().clone();
    let at = |path: &str| seen.iter().find(|(p, _, _)| p == path).unwrap_or_else(|| panic!("{path} not reached: {seen:?}")).clone();
    let (_, token, h) = at("/v1/messages");
    assert_eq!(token, "pooled");
    assert!(h.get("x-organization-uuid").is_none(), "the client's own account's header went on");
    assert!(h["anthropic-beta"].to_str().unwrap().contains("claude-code-20250219"));
    let (_, token, h) = at("/api/oauth/profile");
    assert_eq!(token, "theirs");
    assert_eq!(h["accept-encoding"], "gzip");
    let (_, token, h) = at("/backend-api/codex/responses");
    assert_eq!((token.as_str(), h["chatgpt-account-id"].to_str().unwrap()), ("pooled-codex", "acct-1"));
    assert_eq!(h["content-encoding"], "zstd", "the body went on otherwise than it came");
    assert_eq!(at("/backend-api/wham/usage").1, "theirs");

    // Any other host: a tunnel the pool never looks into.
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut s, _) = echo.accept().await.unwrap();
        let mut b = [0u8; 64];
        let n = s.read(&mut b).await.unwrap();
        s.write_all(&b[..n]).await.unwrap();
    });
    let mut c = tokio::net::TcpStream::connect(proxy).await.unwrap();
    c.write_all(format!("CONNECT {echo_addr} HTTP/1.1\r\nHost: {echo_addr}\r\n\r\n").as_bytes()).await.unwrap();
    let mut b = vec![0u8; 256];
    let n = c.read(&mut b).await.unwrap();
    assert!(String::from_utf8_lossy(&b[..n]).starts_with("HTTP/1.1 200"), "{}", String::from_utf8_lossy(&b[..n]));
    c.write_all(b"ping through").await.unwrap();
    let n = c.read(&mut b).await.unwrap();
    assert_eq!(&b[..n], b"ping through");

    // Not a CONNECT: refused.
    let mut c = tokio::net::TcpStream::connect(proxy).await.unwrap();
    c.write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n").await.unwrap();
    let n = c.read(&mut b).await.unwrap();
    assert!(String::from_utf8_lossy(&b[..n]).starts_with("HTTP/1.1 405"));
}

#[test]
fn only_the_model_requests_are_the_pools() {
    use intercept::pooled;
    assert_eq!(pooled("api.anthropic.com", "/v1/messages"), Some((Provider::Claude, "messages".into())));
    assert_eq!(pooled("api.anthropic.com", "/v1/messages/count_tokens"), Some((Provider::Claude, "messages/count_tokens".into())));
    assert_eq!(pooled("api.anthropic.com", "/api/oauth/usage"), None);
    assert_eq!(pooled("api.anthropic.com", "/v1/models"), None);
    assert_eq!(pooled("chatgpt.com", "/backend-api/codex/responses"), Some((Provider::Codex, "responses".into())));
    assert_eq!(pooled("chatgpt.com", "/backend-api/wham/usage"), None);
    assert_eq!(pooled("example.com", "/v1/messages"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn switches_protocols_where_it_passes_through_not_where_it_pools() {
    let seen: Seen = Default::default();
    // Answers a switch to "echo" and then sends back what it gets.
    let echo = Router::new().route(
        "/backend-api/echo",
        get(|mut req: axum::extract::Request| async move {
            let on = hyper::upgrade::on(&mut req);
            tokio::spawn(async move {
                let mut io = hyper_util::rt::TokioIo::new(on.await.unwrap());
                let mut b = [0u8; 64];
                let n = io.read(&mut b).await.unwrap();
                io.write_all(&b[..n]).await.unwrap();
            });
            Response::builder().status(101).header("connection", "upgrade").header("upgrade", "echo").body(Body::empty()).unwrap()
        }),
    );
    let up = serve(fake(seen.clone()).merge(echo)).await;
    let mut saved = Saved::default();
    saved.accounts.push(account("codex-a", Provider::Codex));
    saved.credentials.insert("codex-a".into(), Credential { access: "pooled-codex".into(), refresh: None, expires_at: Some(now() + 3600), account_id: Some("acct-1".into()) });
    saved.accounts.push(account("claude-a", Provider::Claude));
    saved.credentials.insert("claude-a".into(), Credential { access: "pooled".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.serves(p)), intercept::Who::Anyone).await.unwrap();
    let client = reqwest::Client::builder()
        .http1_only()
        .proxy(reqwest::Proxy::https(format!("http://{proxy}")).unwrap())
        .add_root_certificate(reqwest::Certificate::from_pem(authority.cert_pem.as_bytes()).unwrap())
        .build()
        .unwrap();

    // A Claude model request asking to switch: 426 (Claude Code never
    // asks; the pool answers it over HTTPS), never sent on, nor counted.
    let r = client
        .get("https://api.anthropic.com/v1/messages")
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 426);
    assert!(seen.lock().unwrap().iter().all(|(p, _, _)| p != "/v1/messages"));
    assert!(shared.pool.lock().unwrap().open.is_empty());

    // Anywhere else: through, and then byte for byte.
    let r = client.get("https://chatgpt.com/backend-api/echo").header("connection", "upgrade").header("upgrade", "echo").send().await.unwrap();
    assert_eq!(r.status(), 101);
    assert_eq!(r.headers()["upgrade"], "echo");
    let mut io = r.upgrade().await.unwrap();
    io.write_all(b"through the pool").await.unwrap();
    let mut b = [0u8; 64];
    let n = io.read(&mut b).await.unwrap();
    assert_eq!(&b[..n], b"through the pool");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_doesnt_wait_for_the_connect_answer() {
    use rustls::pki_types::{CertificateDer, pem::PemObject};
    use std::io::{Read, Write};
    let up = serve(fake(Default::default())).await;
    let mut saved = Saved::default();
    saved.accounts.push(account("claude-a", Provider::Claude));
    saved.credentials.insert("claude-a".into(), Credential { access: "pooled".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.serves(p)), intercept::Who::Anyone).await.unwrap();

    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from_pem_slice(authority.cert_pem.as_bytes()).unwrap()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut tls = rustls::ClientConnection::new(Arc::new(config), "api.anthropic.com".try_into().unwrap()).unwrap();
    // The CONNECT and the TLS hello in one write.
    let mut out = b"CONNECT api.anthropic.com:443 HTTP/1.1\r\nHost: api.anthropic.com:443\r\n\r\n".to_vec();
    tls.write_tls(&mut out).unwrap();
    let mut tcp = tokio::net::TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(&out).await.unwrap();
    let mut got = Vec::new();
    let mut b = [0u8; 4096];
    let end = loop {
        let n = tcp.read(&mut b).await.unwrap();
        assert!(n > 0, "the proxy hung up");
        got.extend_from_slice(&b[..n]);
        if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    assert!(got.starts_with(b"HTTP/1.1 200"));
    let mut pending = got.split_off(end);
    tls.writer().write_all(b"GET /api/oauth/profile HTTP/1.1\r\nhost: api.anthropic.com\r\nauthorization: Bearer theirs\r\nconnection: close\r\n\r\n").unwrap();
    let mut answer = Vec::new();
    while !String::from_utf8_lossy(&answer).contains(r#"{"account":"theirs"}"#) {
        while !pending.is_empty() {
            let n = tls.read_tls(&mut &pending[..]).unwrap();
            pending.drain(..n);
            tls.process_new_packets().unwrap();
        }
        let mut plain = [0u8; 4096];
        while let Ok(n @ 1..) = tls.reader().read(&mut plain) {
            answer.extend_from_slice(&plain[..n]);
        }
        let mut w = Vec::new();
        while tls.wants_write() {
            tls.write_tls(&mut w).unwrap();
        }
        tcp.write_all(&w).await.unwrap();
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), tcp.read(&mut b)).await.expect("TLS went nowhere").unwrap();
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&b[..n]);
    }
    assert!(String::from_utf8_lossy(&answer).contains(r#"{"account":"theirs"}"#), "{}", String::from_utf8_lossy(&answer));
}

/// A Claude model request goes on as Claude Code sent it, byte for byte
/// (each header's name in its own case, in its order, its value), but for
/// the account's sign-in in the client's place (Ben, 2026-11-19: "all
/// requests from the pool need to be exactly the same as if they came from
/// claude code / codex"); over HTTP/1.1, kept alive for the next.
#[tokio::test(flavor = "multi_thread")]
async fn passes_claude_requests_on_exactly() {
    // A host that writes down each head as it came.
    let heads: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let host = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = host.local_addr().unwrap();
    let kept = heads.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = host.accept().await else { return };
            let kept = kept.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end + 4]).into_owned();
                    let length: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                    while buf.len() < end + 4 + length {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    buf.drain(..end + 4 + length);
                    kept.lock().unwrap().push(head);
                    let answer = r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#;
                    let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{answer}", answer.len());
                    if s.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    let mut saved = Saved::default();
    saved.accounts.push(account("claude-a", Provider::Claude));
    saved.credentials.insert("claude-a".into(), Credential { access: "pooled".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.serves(p)), intercept::Who::Anyone).await.unwrap();

    // Claude Code: a CONNECT, its TLS, then its head as its SDK writes it.
    let mut tcp = tokio::net::TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(b"CONNECT api.anthropic.com:443 HTTP/1.1\r\nHost: api.anthropic.com:443\r\n\r\n").await.unwrap();
    let mut reply = [0u8; 39];
    tcp.read_exact(&mut reply).await.unwrap();
    assert!(reply.starts_with(b"HTTP/1.1 200"));
    let mut roots = rustls::RootCertStore::empty();
    use rustls::pki_types::pem::PemObject;
    roots.add(rustls::pki_types::CertificateDer::from_pem_slice(authority.cert_pem.as_bytes()).unwrap()).unwrap();
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(rustls::pki_types::ServerName::try_from("api.anthropic.com").unwrap(), tcp).await.unwrap();
    let body = json!({"model": "claude-opus-5-5", "messages": []}).to_string();
    let head = |auth: &str| {
        format!(
            "POST /v1/messages?beta=true HTTP/1.1\r\nAccept: application/json\r\nAuthorization: Bearer {auth}\r\nContent-Type: application/json\r\nUser-Agent: claude-cli/2.1.329 (external, cli)\r\nX-Claude-Code-Session-Id: s-1\r\nX-Stainless-Lang: js\r\nanthropic-beta: claude-code-20250219,oauth-2025-04-20\r\nanthropic-version: 2023-06-01\r\nx-app: cli\r\nConnection: keep-alive\r\nHost: api.anthropic.com\r\nAccept-Encoding: gzip, deflate, br, zstd\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
    };
    for _ in 0..2 {
        tls.write_all(head("theirs").as_bytes()).await.unwrap();
        tls.write_all(body.as_bytes()).await.unwrap();
        // (Its answer read to its end before the next.)
        let mut got = Vec::new();
        let mut chunk = [0u8; 4096];
        while !String::from_utf8_lossy(&got).contains("output_tokens\":4}}") {
            let n = tls.read(&mut chunk).await.unwrap();
            assert!(n > 0, "the proxy closed: {}", String::from_utf8_lossy(&got));
            got.extend_from_slice(&chunk[..n]);
        }
    }
    let heads = heads.lock().unwrap().clone();
    assert_eq!(heads.len(), 2);
    for h in &heads {
        assert_eq!(*h, head("pooled"), "not as the client sent it");
    }
}

/// What passing a request on costs, each way: Claude's exact relay (its
/// own HTTP/1.1 connection, kept alive) and the reqwest path (Codex's, as
/// both went before): the time a request takes through the proxy and the
/// processor time for it (the proxy, a host answering at once and the
/// client, all in this process). Run on a release build:
/// `cargo test --release -p olo-token-pool --test intercept pass_on_cost -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn pass_on_cost() {
    const N: usize = 300;
    // A host answering each request at once, keeping its connection.
    let host = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = host.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = host.accept().await else { return };
            // (As a host's: its answers not held back for the last ones'
            // acknowledgements, Nagle's.)
            let _ = s.set_nodelay(true);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end + 4]).to_ascii_lowercase();
                    let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                    while buf.len() < end + 4 + length {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    buf.drain(..end + 4 + length);
                    let answer = r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#;
                    let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{answer}", answer.len());
                    if s.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    let mut saved = Saved::default();
    saved.accounts.push(account("claude-a", Provider::Claude));
    saved.credentials.insert("claude-a".into(), Credential { access: "pooled".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    saved.accounts.push(account("codex-a", Provider::Codex));
    saved.credentials.insert("codex-a".into(), Credential { access: "pooled-codex".into(), refresh: None, expires_at: Some(now() + 3600), account_id: Some("acct-1".into()) });
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.serves(p)), intercept::Who::Anyone).await.unwrap();
    let cpu = || {
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        (u.ru_utime.tv_sec + u.ru_stime.tv_sec) as f64 + (u.ru_utime.tv_usec + u.ru_stime.tv_usec) as f64 / 1e6
    };
    // Claude's: one client connection, as Claude Code keeps it.
    let mut tcp = tokio::net::TcpStream::connect(proxy).await.unwrap();
    tcp.set_nodelay(true).unwrap();
    tcp.write_all(b"CONNECT api.anthropic.com:443 HTTP/1.1\r\nHost: api.anthropic.com:443\r\n\r\n").await.unwrap();
    let mut reply = [0u8; 39];
    tcp.read_exact(&mut reply).await.unwrap();
    let mut roots = rustls::RootCertStore::empty();
    use rustls::pki_types::pem::PemObject;
    roots.add(rustls::pki_types::CertificateDer::from_pem_slice(authority.cert_pem.as_bytes()).unwrap()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots.clone())
        .with_no_client_auth();
    let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(rustls::pki_types::ServerName::try_from("api.anthropic.com").unwrap(), tcp).await.unwrap();
    // (A body the size of Claude Code's in a session some way in: its
    // system prompt, tools and history, about 400 KB.)
    let turn = |n: usize| json!({"role": if n % 2 == 0 { "user" } else { "assistant" }, "content": [{"type": "text", "text": format!("Turn {n}: {}", "Some words of a conversation, with \"quotes\" and a\nnew line. ".repeat(40))}]});
    let body = json!({
        "model": "claude-opus-5-5",
        "max_tokens": 32000,
        "system": [{"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."}, {"type": "text", "text": "Instructions. ".repeat(1500)}],
        "tools": (0..40).map(|i| json!({"name": format!("tool{i}"), "description": "A tool. ".repeat(100), "input_schema": {"type": "object", "properties": {"a": {"type": "string"}}}})).collect::<Vec<_>>(),
        "messages": (0..120).map(turn).collect::<Vec<_>>(),
        "metadata": {"user_id": "{\"device_id\":\"d\",\"account_uuid\":\"11111111-2222-3333-4444-555555555555\",\"session_id\":\"s\"}"},
        "stream": true,
    })
    .to_string();
    println!("a body of {} KB", body.len() / 1024);
    let head = format!(
        "POST /v1/messages?beta=true HTTP/1.1\r\nAccept: application/json\r\nAuthorization: Bearer theirs\r\nContent-Type: application/json\r\nanthropic-version: 2023-06-01\r\nHost: api.anthropic.com\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    async fn one(tls: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>, head: &str, body: &str) {
        tls.write_all(head.as_bytes()).await.unwrap();
        tls.write_all(body.as_bytes()).await.unwrap();
        // (Its answer may come chunked, its last chunk after: the JSON in
        // it is enough.)
        let mut got = Vec::new();
        let mut chunk = [0u8; 4096];
        while !got.windows(19).any(|w| w == b"\"output_tokens\":4}}") {
            let n = tls.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            got.extend_from_slice(&chunk[..n]);
        }
    }
    for _ in 0..20 {
        one(&mut tls, &head, &body).await;
    }
    let (t0, c0) = (std::time::Instant::now(), cpu());
    for _ in 0..N {
        one(&mut tls, &head, &body).await;
    }
    let (claude_t, claude_c) = (t0.elapsed().as_secs_f64() / N as f64, (cpu() - c0) / N as f64);
    // The reqwest path (Codex's).
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::https(format!("http://{proxy}")).unwrap())
        .add_root_certificate(reqwest::Certificate::from_pem(authority.cert_pem.as_bytes()).unwrap())
        .build()
        .unwrap();
    let codex = || {
        client
            .post("https://chatgpt.com/backend-api/codex/responses")
            .header("authorization", "Bearer theirs")
            .header("content-type", "application/json")
            .body(json!({"model": "gpt-5-codex"}).to_string())
            .send()
    };
    for _ in 0..20 {
        codex().await.unwrap().bytes().await.unwrap();
    }
    let (t0, c0) = (std::time::Instant::now(), cpu());
    for _ in 0..N {
        codex().await.unwrap().bytes().await.unwrap();
    }
    let (codex_t, codex_c) = (t0.elapsed().as_secs_f64() / N as f64, (cpu() - c0) / N as f64);
    println!("exact relay (Claude): {:.0} µs a request, {:.0} µs of processor", claude_t * 1e6, claude_c * 1e6);
    println!("reqwest path (Codex): {:.0} µs a request, {:.0} µs of processor", codex_t * 1e6, codex_c * 1e6);
}

/// A host that answers every request on a connection kept alive, writing
/// down which of its connections each head came on, and counting the
/// connections closed by the other end.
async fn raw_host() -> (SocketAddr, Arc<std::sync::Mutex<Vec<(usize, String)>>>, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let heads: Arc<std::sync::Mutex<Vec<(usize, String)>>> = Default::default();
    let closed = Arc::new(AtomicUsize::new(0));
    let host = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = host.local_addr().unwrap();
    let (kept, ended) = (heads.clone(), closed.clone());
    tokio::spawn(async move {
        for n in 0.. {
            let Ok((mut s, _)) = host.accept().await else { return };
            let (kept, ended) = (kept.clone(), ended.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            ended.fetch_add(1, SeqCst);
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end + 4]).into_owned();
                    let length: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                    while buf.len() < end + 4 + length {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            ended.fetch_add(1, SeqCst);
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    buf.drain(..end + 4 + length);
                    kept.lock().unwrap().push((n, head));
                    let answer = r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#;
                    let reply = format!("HTTP/1.1 200 Fine\r\nContent-Type: application/json\r\nX-Answer: yes\r\nContent-Length: {}\r\n\r\n{answer}", answer.len());
                    if s.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (up, heads, closed)
}

/// A client's connection to Anthropic through the proxy: its CONNECT and
/// TLS (offering HTTP/1.1, as Claude Code's SDK does).
async fn claude_conn(proxy: SocketAddr, authority: &Authority) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
    let mut tcp = tokio::net::TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(b"CONNECT api.anthropic.com:443 HTTP/1.1\r\nHost: api.anthropic.com:443\r\n\r\n").await.unwrap();
    let mut reply = [0u8; 39];
    tcp.read_exact(&mut reply).await.unwrap();
    let mut roots = rustls::RootCertStore::empty();
    use rustls::pki_types::pem::PemObject;
    roots.add(rustls::pki_types::CertificateDer::from_pem_slice(authority.cert_pem.as_bytes()).unwrap()).unwrap();
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    tokio_rustls::TlsConnector::from(Arc::new(config)).connect(rustls::pki_types::ServerName::try_from("api.anthropic.com").unwrap(), tcp).await.unwrap()
}

/// One request on a client's connection, and its answer's head.
async fn exchange(tls: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>, method: &str, path: &str, auth: &str) -> String {
    let body = if method == "POST" { json!({"model": "claude-opus-5-5", "messages": []}).to_string() } else { String::new() };
    let head = format!(
        "{method} {path} HTTP/1.1\r\nAccept: application/json\r\nAuthorization: Bearer {auth}\r\nContent-Type: application/json\r\nanthropic-version: 2023-06-01\r\nHost: api.anthropic.com\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    tls.write_all(head.as_bytes()).await.unwrap();
    tls.write_all(body.as_bytes()).await.unwrap();
    let mut got = Vec::new();
    let mut chunk = [0u8; 4096];
    while !String::from_utf8_lossy(&got).contains("output_tokens\":4}}") {
        let n = tls.read(&mut chunk).await.unwrap();
        assert!(n > 0, "the proxy closed: {}", String::from_utf8_lossy(&got));
        got.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&got).into_owned();
    text[..text.find("\r\n\r\n").unwrap() + 2].to_owned()
}

/// The pool's connections follow the client's (Codex's review: every
/// client's own requests shared one connection, two sign-ins on it): for
/// each client connection, one of the pool's for its own sign-in's
/// requests and one for each account its model requests went on for, kept
/// between its requests, closed with it. The answer comes back as the host
/// wrote it: its reason, each name's case, its length.
#[tokio::test(flavor = "multi_thread")]
async fn connections_follow_the_clients() {
    use std::sync::atomic::Ordering::SeqCst;
    let (up, heads, closed) = raw_host().await;
    let mut saved = Saved::default();
    saved.accounts.push(account("claude-a", Provider::Claude));
    saved.credentials.insert("claude-a".into(), Credential { access: "pooled".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.serves(p)), intercept::Who::Anyone).await.unwrap();

    let mut one = claude_conn(proxy, &authority).await;
    let answer = exchange(&mut one, "GET", "/api/oauth/profile", "one").await;
    assert!(answer.starts_with("HTTP/1.1 200 Fine\r\n") && answer.contains("\r\nX-Answer: yes\r\n") && answer.contains("\r\nContent-Length: 46\r\n"), "{answer}");
    exchange(&mut one, "POST", "/v1/messages", "one").await;
    let mut two = claude_conn(proxy, &authority).await;
    exchange(&mut two, "GET", "/api/oauth/profile", "two").await;
    exchange(&mut one, "GET", "/api/oauth/profile", "one").await;
    exchange(&mut one, "POST", "/v1/messages", "one").await;
    let on: Vec<(usize, String)> = heads.lock().unwrap().iter().map(|(n, h)| (*n, h.lines().find(|l| l.starts_with("Authorization")).unwrap().to_owned())).collect();
    let conn = |i: usize| on[i].0;
    assert_eq!(
        on.iter().map(|(_, a)| a.as_str()).collect::<Vec<_>>(),
        ["Authorization: Bearer one", "Authorization: Bearer pooled", "Authorization: Bearer two", "Authorization: Bearer one", "Authorization: Bearer pooled"]
    );
    // Client one's own requests on one connection, its pooled ones on
    // another, kept; client two's own on a third.
    assert_eq!((conn(0), conn(1)), (conn(3), conn(4)));
    assert!(conn(0) != conn(1) && conn(2) != conn(0) && conn(2) != conn(1));
    assert_eq!(closed.load(SeqCst), 0);
    // Client one leaves: both of its go with it; two's stays.
    drop(one);
    let t = std::time::Instant::now();
    while closed.load(SeqCst) < 2 {
        assert!(t.elapsed().as_secs() < 5, "the pool's connections for a client that left are still open");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(closed.load(SeqCst), 2);
    assert_eq!(shared.exact().idle(), 1);
    exchange(&mut two, "GET", "/api/oauth/profile", "two").await;
    assert_eq!(heads.lock().unwrap().last().unwrap().0, conn(2));
}

/// A ChatGPT that speaks Codex's Responses over a WebSocket: each
/// handshake written down (its head as it came), then each turn answered
/// as ChatGPT does (`response.created`, a delta, `response.completed` with
/// its usage). The sign-in `runs-out` runs out inside its socket (a 429
/// event, as ChatGPT sends one); `refused` is refused at its handshake.
async fn chatgpt_sockets() -> (SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
    let heads: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = l.local_addr().unwrap();
    let kept = heads.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            let kept = kept.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let end = loop {
                    if let Some(e) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break e + 4;
                    }
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                };
                let head = String::from_utf8_lossy(&buf[..end]).into_owned();
                kept.lock().unwrap().push(head.clone());
                let field = |name: &str| head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_owned())).unwrap_or_default();
                let token = field("authorization");
                if token == "Bearer refused" {
                    let body = r#"{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached"}}"#;
                    let reply = format!(
                        "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nx-codex-primary-used-percent: 100\r\nx-codex-primary-reset-after-seconds: 3600\r\ncontent-length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.write_all(reply.as_bytes()).await;
                    return;
                }
                let accept = tungstenite::handshake::derive_accept_key(field("sec-websocket-key").as_bytes());
                let reply = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\nX-Codex-Turn-State: sticky-1\r\n\r\n");
                s.write_all(reply.as_bytes()).await.unwrap();
                let mut rest = buf[end..].to_vec();
                loop {
                    // A client's frame: masked, whole in what's come.
                    while rest.len() < 2 || rest.len() < frame_len(&rest) {
                        let n = s.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        rest.extend_from_slice(&chunk[..n]);
                    }
                    let len = frame_len(&rest);
                    let (opcode, payload) = unmask(&rest[..len]);
                    rest.drain(..len);
                    if opcode == 0x8 {
                        let _ = s.write_all(&server_frame(0x8, &payload)).await;
                        return;
                    }
                    let asked: Value = serde_json::from_slice(&payload).unwrap();
                    let model = asked["model"].as_str().unwrap_or("gpt-5").to_owned();
                    let mut said = server_frame(0x1, json!({"type": "codex.rate_limits", "plan_type": "pro", "rate_limits": {"allowed": true, "limit_reached": false, "primary": {"used_percent": 42.0, "window_minutes": 300, "reset_after_seconds": 3600, "reset_at": now() + 3600}, "secondary": null}}).to_string().as_bytes());
                    said.extend(server_frame(0x1, json!({"type": "response.created", "response": {"model": model}}).to_string().as_bytes()));
                    if token == "Bearer runs-out" {
                        said.extend(server_frame(0x1, json!({"type": "error", "status": 429, "error": {"type": "usage_limit_reached", "resets_at": now() + 7200}, "headers": {"x-codex-primary-used-percent": "100.0", "x-codex-primary-window-minutes": 300}}).to_string().as_bytes()));
                    } else {
                        said.extend(server_frame(0x1, br#"{"type":"response.output_text.delta","delta":"hi"}"#));
                        said.extend(server_frame(0x1, json!({"type": "response.completed", "response": {"usage": {"input_tokens": 11, "output_tokens": 4}}}).to_string().as_bytes()));
                    }
                    if s.write_all(&said).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (at, heads)
}

/// How long the frame at the start of `b` is, all told (a bound larger
/// than `b` while its head hasn't all come).
fn frame_len(b: &[u8]) -> usize {
    if b.len() < 2 {
        return usize::MAX;
    }
    let mask = if b[1] & 0x80 != 0 { 4 } else { 0 };
    match b[1] & 0x7f {
        126 if b.len() >= 4 => 4 + mask + u16::from_be_bytes([b[2], b[3]]) as usize,
        126 => usize::MAX,
        127 if b.len() >= 10 => 10 + mask + u64::from_be_bytes(b[2..10].try_into().unwrap()) as usize,
        127 => usize::MAX,
        n => 2 + mask + n as usize,
    }
}

/// A frame's opcode and its payload, unmasked.
fn unmask(f: &[u8]) -> (u8, Vec<u8>) {
    let (len7, mut at) = (f[1] & 0x7f, 2);
    at += match len7 {
        126 => 2,
        127 => 8,
        _ => 0,
    };
    let key = [f[at], f[at + 1], f[at + 2], f[at + 3]];
    at += 4;
    (f[0] & 0x0f, f[at..].iter().enumerate().map(|(i, b)| b ^ key[i % 4]).collect())
}

/// A server's frame, whole, unmasked.
fn server_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0x80 | opcode];
    if payload.len() < 126 {
        f.push(payload.len() as u8);
    } else {
        f.push(126);
        f.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    f.extend_from_slice(payload);
    f
}

/// A client's frame, masked as a client masks it.
fn client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let key = [0x37u8, 0xfa, 0x21, 0x3d];
    let mut f = vec![0x80 | opcode];
    if payload.len() < 126 {
        f.push(0x80 | payload.len() as u8);
    } else {
        f.push(0x80 | 126);
        f.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    f.extend_from_slice(&key);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
    f
}

/// Codex's socket through the proxy: its CONNECT, its TLS (no ALPN, as
/// Codex's socket offers none), its handshake as Codex writes it; the
/// answer's head, and the stream.
async fn codex_socket(proxy: SocketAddr, authority: &Authority, key: &str) -> (String, tokio_rustls::client::TlsStream<tokio::net::TcpStream>) {
    let mut tcp = tokio::net::TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(b"CONNECT chatgpt.com:443 HTTP/1.1\r\nHost: chatgpt.com:443\r\n\r\n").await.unwrap();
    let mut reply = [0u8; 39];
    tcp.read_exact(&mut reply).await.unwrap();
    let mut roots = rustls::RootCertStore::empty();
    use rustls::pki_types::pem::PemObject;
    roots.add(rustls::pki_types::CertificateDer::from_pem_slice(authority.cert_pem.as_bytes()).unwrap()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(rustls::pki_types::ServerName::try_from("chatgpt.com").unwrap(), tcp).await.unwrap();
    let head = format!(
        "GET /backend-api/codex/responses HTTP/1.1\r\nHost: chatgpt.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\ncookie: client=own\r\nchatgpt-account-id: theirs-acct\r\nauthorization: Bearer theirs\r\nuser-agent: codex-tui/0.170.0\r\noriginator: codex-tui\r\nx-codex-turn-metadata: {{\"turn\":1}}\r\nopenai-beta: responses_websockets=2026-02-06\r\n\r\n"
    );
    tls.write_all(head.as_bytes()).await.unwrap();
    let mut got = Vec::new();
    let mut b = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") {
        assert_eq!(tls.read(&mut b).await.unwrap(), 1, "closed before its answer: {}", String::from_utf8_lossy(&got));
        got.push(b[0]);
    }
    (String::from_utf8(got).unwrap(), tls)
}

/// Read what the server says on `tls` until `until` comes (or the socket
/// closes): each message, and the bytes as they came.
async fn heard(tls: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>, until: &str) -> (Vec<String>, Vec<u8>, bool) {
    let mut reader = olo_token_pool::socket::Reader::new(None);
    let (mut messages, mut bytes) = (Vec::new(), Vec::new());
    let mut chunk = [0u8; 4096];
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), tls.read(&mut chunk)).await.expect("silent").unwrap_or(0);
        if n == 0 {
            return (messages, bytes, true);
        }
        bytes.extend_from_slice(&chunk[..n]);
        let p = reader.push(&chunk[..n]);
        messages.extend(p.messages);
        if messages.iter().any(|m| m.contains(until)) {
            return (messages, bytes, false);
        }
    }
}

/// Codex's Responses over its WebSocket are the pool's (Ben, 2026-11-19:
/// "all requests from the pool need to be exactly the same as if they came
/// from claude code / codex"; it was answered 426 and Codex spoke HTTP):
/// the handshake goes on as Codex wrote it but for the account's own (its
/// sign-in, ChatGPT account and cookies), its answer comes back as it came
/// (Codex's key accepted, each name in its case), the socket passes both
/// ways byte for byte, and each turn is counted with its usage. An account
/// refused at the handshake gives way to the next before Codex hears of
/// it; one that runs out inside its socket is marked, and the socket
/// closed with that unsaid, so Codex asks again, on another account.
#[tokio::test(flavor = "multi_thread")]
async fn codex_sockets_are_the_pools() {
    let (up, heads) = chatgpt_sockets().await;
    let mut saved = Saved::default();
    for (id, token) in [("a-refused", "refused"), ("b-runs-out", "runs-out"), ("c", "pooled-c")] {
        let mut a = account(id, Provider::Codex);
        a.created = match id {
            "a-refused" => 1,
            "b-runs-out" => 2,
            _ => 3,
        };
        saved.accounts.push(a);
        saved.credentials.insert(id.into(), Credential { access: token.into(), refresh: None, expires_at: Some(now() + 3600), account_id: Some(format!("acct-{id}")) });
    }
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.serves(p)), intercept::Who::Anyone).await.unwrap();
    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let order = |heads: &[String]| heads.iter().map(|h| h.lines().find(|l| l.starts_with("authorization")).unwrap().to_owned()).collect::<Vec<_>>();

    // The first refused at its handshake, the next taken: one 101.
    let (answer, mut tls) = codex_socket(proxy, &authority, key).await;
    assert!(answer.starts_with("HTTP/1.1 101 Switching Protocols\r\n"), "{answer}");
    assert!(answer.contains(&format!("\r\nSec-WebSocket-Accept: {}\r\n", tungstenite::handshake::derive_accept_key(key.as_bytes()))), "{answer}");
    assert!(answer.contains("\r\nX-Codex-Turn-State: sticky-1\r\n"), "{answer}");
    {
        let heads = heads.lock().unwrap();
        assert_eq!(order(&heads), ["authorization: Bearer refused", "authorization: Bearer runs-out"]);
        // As Codex wrote it, the account's own in Codex's places.
        let sent = &heads[1];
        let names: Vec<&str> = sent.lines().skip(1).filter_map(|l| l.split_once(':').map(|(k, _)| k)).collect();
        assert_eq!(
            names,
            ["Host", "Connection", "Upgrade", "Sec-WebSocket-Version", "Sec-WebSocket-Key", "chatgpt-account-id", "authorization", "user-agent", "originator", "x-codex-turn-metadata", "openai-beta"]
        );
        assert!(sent.contains("\r\nchatgpt-account-id: acct-b-runs-out\r\n") && !sent.contains("theirs") && !sent.contains("client=own"), "{sent}");
        assert!(sent.contains(&format!("\r\nSec-WebSocket-Key: {key}\r\n")));
    }
    // It runs out inside: the socket closes, that never said.
    tls.write_all(&client_frame(0x1, br#"{"type":"response.create","model":"gpt-5"}"#)).await.unwrap();
    let (said, _, closed) = heard(&mut tls, "response.completed").await;
    assert!(closed, "the socket stayed open: {said:?}");
    assert!(said.iter().all(|m| !m.contains("usage_limit_reached")), "{said:?}");
    let p = shared.snapshot();
    assert!(p.account("b-runs-out").unwrap().cooldown_until.is_some_and(|t| t > now() + 3600), "not marked out");
    assert!(p.account("a-refused").unwrap().cooldown_until.is_some_and(|t| t > now()), "not marked out at its handshake");
    assert!(p.open.is_empty() && p.in_flight.values().all(|&n| n == 0));

    // Codex asks again: the next account, the turn served, counted.
    let (answer, mut tls) = codex_socket(proxy, &authority, key).await;
    assert!(answer.starts_with("HTTP/1.1 101"), "{answer}");
    assert_eq!(order(&heads.lock().unwrap())[2..], ["authorization: Bearer pooled-c"]);
    tls.write_all(&client_frame(0x1, br#"{"type":"response.create","model":"gpt-5"}"#)).await.unwrap();
    let (said, bytes, closed) = heard(&mut tls, "response.completed").await;
    assert!(!closed);
    assert_eq!(said.len(), 4);
    let limits = said[0].clone();
    let mut wire = server_frame(0x1, limits.as_bytes());
    wire.extend(server_frame(0x1, json!({"type": "response.created", "response": {"model": "gpt-5"}}).to_string().as_bytes()));
    wire.extend(server_frame(0x1, br#"{"type":"response.output_text.delta","delta":"hi"}"#));
    wire.extend(server_frame(0x1, json!({"type": "response.completed", "response": {"usage": {"input_tokens": 11, "output_tokens": 4}}}).to_string().as_bytes()));
    assert_eq!(bytes, wire, "not as it came");
    tls.write_all(&client_frame(0x8, &[0x03, 0xe8])).await.unwrap();
    let (_, _, closed) = heard(&mut tls, "never").await;
    assert!(closed);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let p = shared.snapshot();
    let c = p.account("c").unwrap();
    assert_eq!((c.total.requests, c.total.input_tokens, c.total.output_tokens), (1, 11, 4));
    // (Its meters, as the socket said them.)
    assert!(c.windows.iter().any(|w| (w.used - 42.0).abs() < 0.01), "{:?}", c.windows);
    assert!(p.open.is_empty() && p.in_flight.values().all(|&n| n == 0));
    assert!(p.served.iter().any(|s| s.account_id == "b-runs-out" && s.request.outcome == olo_token_pool::model::Outcome::RateLimited));
}

/// One Codex account ("test") pooled through the proxy, its ChatGPT a
/// host the test speaks for itself: the handshake answered 101, then
/// whatever the test writes, frame by frame, and reads.
async fn one_socket() -> (Arc<Shared>, tokio_rustls::client::TlsStream<tokio::net::TcpStream>, tokio::net::TcpStream) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let shared = one_account(listener.local_addr().unwrap()).await;
    let (proxy, authority) = proxy_for(&shared).await;
    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let host = async {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(tcp.read_u8().await.unwrap());
        }
        let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
        tcp.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).await.unwrap();
        tcp
    };
    let ((answer, client), host) = tokio::join!(codex_socket(proxy, &authority, key), host);
    assert!(answer.starts_with("HTTP/1.1 101"), "{answer}");
    (shared, client, host)
}

/// The pool with one Codex account, its provider at `up`.
async fn one_account(up: SocketAddr) -> Arc<Shared> {
    let mut saved = Saved::default();
    saved.accounts.push(account("test", Provider::Codex));
    saved.credentials.insert("test".into(), Credential { access: "pooled".into(), refresh: Some("renewal".into()), expires_at: Some(now() + 3600), account_id: Some("acct-test".into()) });
    Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {})
}

async fn proxy_for(shared: &Arc<Shared>) -> (SocketAddr, Arc<Authority>) {
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.serves(p)), intercept::Who::Anyone).await.unwrap();
    (proxy, authority)
}

fn event(v: Value) -> Vec<u8> {
    server_frame(0x1, v.to_string().as_bytes())
}

/// The host's end shows the socket closed (or not, within `within`).
async fn host_closed(host: &mut tokio::net::TcpStream, within: std::time::Duration) -> bool {
    let mut b = [0u8; 64];
    loop {
        match tokio::time::timeout(within, host.read(&mut b)).await {
            Ok(Ok(0) | Err(_)) => return true,
            Ok(Ok(_)) => continue,
            Err(_) => return false,
        }
    }
}

/// A refusal read in one piece with what came before it (Codex's fourth
/// review: the frames before it were dropped with it): what came before
/// reaches Codex, the refusal doesn't, and the socket is closed both
/// ways, ChatGPT's end too, not left open.
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_read_with_what_came_before() {
    for apart in [true, false] {
        let (shared, mut client, mut host) = one_socket().await;
        let before = event(json!({"type": "response.created", "response": {"model": "gpt-5"}}));
        let refusal = event(json!({"type": "error", "status": 429, "error": {"type": "usage_limit_reached", "resets_at": now() + 100}}));
        if apart {
            host.write_all(&before).await.unwrap();
            let (_, got, _) = heard(&mut client, "response.created").await;
            assert_eq!(got, before);
            host.write_all(&refusal).await.unwrap();
            let (said, got, closed) = heard(&mut client, "never").await;
            assert!(closed && got.is_empty(), "{said:?}");
        } else {
            host.write_all(&[&before[..], &refusal[..]].concat()).await.unwrap();
            let (said, got, closed) = heard(&mut client, "never").await;
            assert!(closed, "{said:?}");
            assert_eq!(got, before, "what came before the refusal, all of it, and nothing else");
        }
        assert!(host_closed(&mut host, std::time::Duration::from_secs(2)).await, "ChatGPT's end left open");
        assert!(shared.snapshot().account("test").unwrap().cooldown_until.is_some_and(|t| t > now() + 50));
    }
}

/// A turn under way but quiet (a long think before anything is said)
/// keeps a handover waiting past the ten seconds a quiet tunnel counts
/// for (Codex's fourth review: the retiring copy saw nothing busy and
/// went, cutting the turn); and a turn that ends incomplete is counted
/// with its usage, as not passed (it was counted with none, passed).
#[tokio::test(flavor = "multi_thread")]
async fn a_quiet_turn_holds_a_handover_and_an_incomplete_one_counts() {
    let (shared, mut client, mut host) = one_socket().await;
    let asked = client_frame(0x1, br#"{"type":"response.create","model":"gpt-5"}"#);
    client.write_all(&asked).await.unwrap();
    let mut got = vec![0u8; asked.len()];
    host.read_exact(&mut got).await.unwrap();
    assert_eq!(got, asked);
    let created = event(json!({"type": "response.created", "response": {"model": "gpt-5"}}));
    host.write_all(&created).await.unwrap();
    heard(&mut client, "response.created").await;
    shared.drain.retire();
    tokio::time::sleep(std::time::Duration::from_millis(10_300)).await;
    assert!(shared.drain.busy(), "a quiet turn let the handover go: {:?}", shared.drain.under_way());
    let end = event(json!({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "max_output_tokens"}, "usage": {"input_tokens": 11, "output_tokens": 4}}}));
    host.write_all(&end).await.unwrap();
    heard(&mut client, "response.incomplete").await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let p = shared.snapshot();
    let r = &p.served.back().unwrap().request;
    assert_eq!((r.usage.input_tokens, r.usage.output_tokens), (11, 4));
    assert_eq!(r.outcome, olo_token_pool::model::Outcome::Failed);
    // (The turn over, it holds the handover no more: the socket still
    // does, as a tunnel, while anything crosses it.)
    assert_eq!(shared.drain.under_way().0, 0, "the turn over, it still holds the handover");
}

/// An open socket's next turn is the pool's to give again: with Codex
/// paused in the pool (or the account out, taken away, reset), it isn't
/// passed on and the socket closes, so Codex asks again (Codex's fourth
/// review: a paused pool's open socket passed the next turn on).
#[tokio::test(flavor = "multi_thread")]
async fn a_paused_pool_takes_no_next_turn() {
    let (shared, mut client, mut host) = one_socket().await;
    shared.set_paused(Provider::Codex, true);
    client.write_all(&client_frame(0x1, br#"{"type":"response.create","model":"gpt-5"}"#)).await.unwrap();
    assert!(host_closed(&mut host, std::time::Duration::from_secs(2)).await, "the turn went on");
    let (_, _, closed) = heard(&mut client, "never").await;
    assert!(closed);
}

/// Only the account's quota closes a socket: a burst's 429 (another kind
/// of error, its retry advice with it) goes to Codex as it came, and the
/// account isn't marked out for it; a meter of another name (a model's
/// own) is kept as its own, not taken for the account's.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_accounts_quota_closes_a_socket() {
    let (shared, mut client, mut host) = one_socket().await;
    let meter =
        event(json!({"type": "codex.rate_limits", "metered_limit_name": "codex_other_model", "rate_limits": {"primary": {"used_percent": 100, "window_minutes": 300, "reset_at": now() + 600}}}));
    let burst = event(json!({"type": "error", "status": 429, "error": {"type": "rate_limit_exceeded", "message": "Slow down"}, "headers": {"retry-after": "17"}}));
    host.write_all(&[&meter[..], &burst[..]].concat()).await.unwrap();
    let (said, got, closed) = heard(&mut client, "rate_limit_exceeded").await;
    assert!(!closed, "{said:?}");
    assert_eq!(got, [&meter[..], &burst[..]].concat(), "not as it came");
    let p = shared.snapshot();
    let a = p.account("test").unwrap();
    assert_eq!(a.cooldown_until, None, "marked out for a burst");
    assert!(olo_token_pool::schedule::blocked_until(a, "gpt-5", now()).is_none(), "another meter taken for the account's: {:?}", a.windows);
}

/// A refusal at the handshake keeps the reset its body says (it was a
/// minute, the body unread), and a renewal that can't be had isn't told
/// to Codex as the pool out of quota (Codex gives up on that for good):
/// a 502, which Codex tries again.
#[tokio::test(flavor = "multi_thread")]
async fn a_handshake_refusal_keeps_its_reset_and_an_outage_isnt_quota() {
    let reset = now() + 7200;
    let app = Router::new().route(
        "/backend-api/codex/responses",
        get(move || async move { (axum::http::StatusCode::TOO_MANY_REQUESTS, axum::Json(json!({"error": {"type": "usage_limit_reached", "resets_at": reset}}))) }),
    );
    let shared = one_account(serve(app).await).await;
    let (proxy, authority) = proxy_for(&shared).await;
    let (answer, _client) = codex_socket(proxy, &authority, "dGhlIHNhbXBsZSBub25jZQ==").await;
    assert!(answer.starts_with("HTTP/1.1 429"), "{answer}");
    let until = shared.snapshot().account("test").unwrap().cooldown_until.unwrap();
    assert!((until - reset).abs() <= 1, "kept {} s, not the {} s it said", until - now(), reset - now());

    let app =
        Router::new().route("/backend-api/codex/responses", get(|| async { axum::http::StatusCode::UNAUTHORIZED })).route("/oauth", post(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }));
    let shared = one_account(serve(app).await).await;
    let (proxy, authority) = proxy_for(&shared).await;
    let (answer, _client) = codex_socket(proxy, &authority, "dGhlIHNhbXBsZSBub25jZQ==").await;
    assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");
    let p = shared.snapshot();
    assert_eq!(p.account("test").unwrap().cooldown_until, None);
}

/// Codex done sending (its half closed): what ChatGPT still says reaches
/// it, a frame ChatGPT broke off at its end too, before the socket ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_half_closed_socket_hears_the_rest() {
    let (_shared, mut client, mut host) = one_socket().await;
    client.shutdown().await.unwrap();
    let mut b = [0u8; 1];
    assert_eq!(host.read(&mut b).await.unwrap(), 0);
    let whole = server_frame(0x1, b"done");
    let broken = server_frame(0x1, b"unfinished");
    host.write_all(&[&whole[..], &broken[..5]].concat()).await.unwrap();
    host.shutdown().await.unwrap();
    let mut got = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut chunk)).await.expect("silent").unwrap_or(0);
        if n == 0 {
            break;
        }
        got.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(got, [&whole[..], &broken[..5]].concat());
}

/// A turn that finishes in a copy that handed over (an upgrade) is booked
/// in the copy now at work, which saves it: told over the pipe the old copy
/// is known to be finishing by (Codex's fourth review: it was counted in
/// the old copy's memory only, gone with it); a meter it hears then too.
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_that_ends_after_a_handover_is_booked() {
    let (old, mut client, mut host) = one_socket().await;
    let new = one_account("127.0.0.1:9".parse().unwrap()).await;
    let pipe = old.drain.finishing().expect("a pipe");
    new.drain.took_over_from(pipe);
    new.follow_predecessor();

    client.write_all(&client_frame(0x1, br#"{"type":"response.create","model":"gpt-5"}"#)).await.unwrap();
    let mut asked = vec![0u8; 64];
    let _ = host.read(&mut asked).await.unwrap();
    host.write_all(&event(json!({"type": "response.created", "response": {"model": "gpt-5"}}))).await.unwrap();
    heard(&mut client, "response.created").await;
    old.drain.retire();
    let limits = event(json!({"type": "codex.rate_limits", "rate_limits": {"primary": {"used_percent": 77.0, "window_minutes": 300, "reset_at": now() + 3600}}}));
    let done = event(json!({"type": "response.completed", "response": {"usage": {"input_tokens": 11, "output_tokens": 4}}}));
    host.write_all(&[&limits[..], &done[..]].concat()).await.unwrap();
    heard(&mut client, "response.completed").await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let p = new.snapshot();
        let a = p.account("test").unwrap();
        if a.total.input_tokens == 11 && a.windows.iter().any(|w| (w.used - 77.0).abs() < 0.01) {
            assert_eq!((a.total.requests, a.total.output_tokens), (1, 4));
            assert!(p.served.iter().any(|s| s.account_id == "test" && s.request.usage.input_tokens == 11));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "not booked in the copy at work: {:?} {:?}", a.total, a.windows);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // (The old copy's own books aren't the ones saved.)
    old.drain.finished();
}

/// What a client is told of its usage (Ben, 2026-11-26: "always reports
/// to the clients full usage when there remain additional accounts or
/// resets to draw from (for both claude and codex)"). A stand-in Claude
/// and ChatGPT whose answers say an account is nearly out: 93% of its
/// 5 hours with a warning, 81% of Codex's week; and whose usage reports
/// say the same, Claude's compressed as the real one's is.
///
/// With another account behind the one that answers, the client is told
/// nothing is used, in the answer's headers and in the report, each
/// reset left as it came. With the answering account the last (the other
/// turned off), it is told how that one really stands. And the pool's
/// own reading is the provider's either way.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_is_told_nothing_is_used_while_the_pool_has_more() {
    use std::io::Write as _;
    let limits = |r: axum::http::response::Builder| {
        r.header("anthropic-ratelimit-unified-status", "allowed_warning")
            .header("anthropic-ratelimit-unified-5h-utilization", "0.93")
            .header("anthropic-ratelimit-unified-5h-reset", (now() + 1800).to_string())
            .header("anthropic-ratelimit-unified-representative-claim", "five_hour")
    };
    let fake = Router::new()
        .route("/v1/messages", post(move || async move { limits(Response::builder()).header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#)).unwrap() }))
        .route(
            "/backend-api/codex/responses",
            post(|| async {
                Response::builder()
                    .header("content-type", "application/json")
                    .header("x-codex-primary-used-percent", "81")
                    .header("x-codex-primary-window-minutes", "10080")
                    .header("x-codex-primary-reset-after-seconds", "3600")
                    .body(Body::from(r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#))
                    .unwrap()
            }),
        )
        .route(
            "/api/oauth/usage",
            get(|| async {
                let report = json!({"five_hour": {"utilization": 93.0, "resets_at": "2026-11-27T02:00:00Z"}, "limits": [{"kind": "session", "group": "session", "percent": 93, "resets_at": "2026-11-27T02:00:00Z"}]});
                let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                gz.write_all(report.to_string().as_bytes()).unwrap();
                Response::builder().header("content-type", "application/json").header("content-encoding", "gzip").body(Body::from(gz.finish().unwrap())).unwrap()
            }),
        )
        .route("/backend-api/wham/usage", get(|| async { axum::Json(json!({"plan_type": "pro", "rate_limit": {"allowed": true, "limit_reached": false, "primary_window": {"used_percent": 81, "limit_window_seconds": 604800, "reset_after_seconds": 3600}}})) }));
    let up = serve(fake).await;
    let mut saved = Saved::default();
    for (id, provider, created) in [("claude-a", Provider::Claude, 1), ("claude-b", Provider::Claude, 2), ("codex-a", Provider::Codex, 1), ("codex-b", Provider::Codex, 2)] {
        let mut a = account(id, provider);
        a.created = created;
        saved.accounts.push(a);
        saved.credentials.insert(id.into(), Credential { access: id.into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    }
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.serves(p)), intercept::Who::Anyone).await.unwrap();
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::https(format!("http://{proxy}")).unwrap())
        .add_root_certificate(reqwest::Certificate::from_pem(authority.cert_pem.as_bytes()).unwrap())
        .build()
        .unwrap();
    let told = |client: reqwest::Client| async move {
        let h = |r: &reqwest::Response, k: &str| r.headers().get(k).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let claude = client.post("https://api.anthropic.com/v1/messages").header("authorization", "Bearer theirs").json(&json!({"model": "claude-opus-5-5", "messages": []})).send().await.unwrap();
        let claude = (
            claude.status().as_u16(),
            h(&claude, "x-olo-pool-account"),
            h(&claude, "anthropic-ratelimit-unified-5h-utilization"),
            h(&claude, "anthropic-ratelimit-unified-status"),
            h(&claude, "anthropic-ratelimit-unified-5h-reset").is_some(),
        );
        let codex = client.post("https://chatgpt.com/backend-api/codex/responses").header("authorization", "Bearer theirs").json(&json!({"model": "gpt-5-codex"})).send().await.unwrap();
        let codex = (codex.status().as_u16(), h(&codex, "x-olo-pool-account"), h(&codex, "x-codex-primary-used-percent"), h(&codex, "x-codex-primary-reset-after-seconds"));
        let report = client.get("https://api.anthropic.com/api/oauth/usage").header("authorization", "Bearer theirs").header("accept-encoding", "gzip").send().await.unwrap();
        let coding = h(&report, "content-encoding");
        let bytes = report.bytes().await.unwrap();
        // (As the client reads it: undone if it is said to be compressed.)
        let text = match coding.as_deref() {
            Some("gzip") => {
                let mut out = String::new();
                std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&bytes[..]), &mut out).unwrap();
                out
            }
            _ => String::from_utf8(bytes.to_vec()).unwrap(),
        };
        let report: Value = serde_json::from_str(&text).unwrap();
        let chatgpt: Value = client.get("https://chatgpt.com/backend-api/wham/usage").header("authorization", "Bearer theirs").send().await.unwrap().json().await.unwrap();
        (claude, codex, report, chatgpt)
    };
    // Another account behind each: nothing used, told four ways.
    let (claude, codex, report, chatgpt) = told(client.clone()).await;
    assert_eq!(claude, (200, Some("claude-a".into()), Some("0".into()), Some("allowed".into()), true));
    assert_eq!(codex, (200, Some("codex-a".into()), Some("0".into()), Some("3600".into())));
    assert_eq!((report["five_hour"]["utilization"].as_f64(), report["limits"][0]["percent"].as_f64(), report["five_hour"]["resets_at"].as_str()), (Some(0.0), Some(0.0), Some("2026-11-27T02:00:00Z")));
    assert_eq!(
        (chatgpt["rate_limit"]["primary_window"]["used_percent"].as_f64(), chatgpt["rate_limit"]["primary_window"]["reset_after_seconds"].as_i64(), chatgpt["plan_type"].as_str()),
        (Some(0.0), Some(3600), Some("pro"))
    );
    // (The pool's own reading: the provider's.)
    let p = shared.snapshot();
    assert_eq!(p.account("claude-a").unwrap().windows.iter().find(|w| w.name == "five_hour").map(|w| w.used), Some(93.0));
    assert_eq!(p.account("codex-a").unwrap().windows.iter().find(|w| w.name == "primary_window").map(|w| w.used), Some(81.0));
    // The answering account the last (the other turned off): as it is.
    {
        let mut p = shared.pool.lock().unwrap();
        for id in ["claude-b", "codex-b"] {
            p.account_mut(id).unwrap().enabled = false;
        }
    }
    let (claude, codex, report, chatgpt) = told(client.clone()).await;
    assert_eq!(claude, (200, Some("claude-a".into()), Some("0.93".into()), Some("allowed_warning".into()), true));
    assert_eq!(codex, (200, Some("codex-a".into()), Some("81".into()), Some("3600".into())));
    assert_eq!((report["five_hour"]["utilization"].as_f64(), report["limits"][0]["percent"].as_f64()), (Some(93.0), Some(93.0)));
    assert_eq!(chatgpt["rate_limit"]["primary_window"]["used_percent"].as_f64(), Some(81.0));
    // A reset to claim on the last one: more to draw from again.
    shared.pool.lock().unwrap().account_mut("claude-a").unwrap().credits =
        vec![olo_token_pool::model::ResetCredit { id: "g".into(), remaining: 1, usable: true, expires_at: Some(now() + 86400), ..Default::default() }];
    let (claude, _, report, _) = told(client).await;
    assert_eq!((claude.2.as_deref(), claude.3.as_deref(), report["five_hour"]["utilization"].as_f64()), (Some("0"), Some("allowed"), Some(0.0)));
}
