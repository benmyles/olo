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
use serde_json::json;
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

    // Codex's Responses WebSocket: 426, so it uses HTTPS (and the pool
    // never sends it on, nor counts it).
    let r = client
        .get("https://chatgpt.com/backend-api/codex/responses")
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 426);
    assert!(seen.lock().unwrap().iter().all(|(p, _, _)| p != "/backend-api/codex/responses"));
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
    text[..text.find("\r\n\r\n").unwrap()].to_owned()
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
