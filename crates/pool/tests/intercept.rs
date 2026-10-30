//! The proxy against a fake provider standing in for every host: never a
//! real account or service. A client trusting the pool's certificate
//! reaches "api.anthropic.com" and "chatgpt.com" through it, as Claude
//! Code and Codex do.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use olo_token_pool::{
    authority::Authority,
    gateway::Shared,
    intercept,
    model::{Account, Credential, Provider, now},
    store::Saved,
    upstream::Upstreams,
};

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
        .route("/v1/messages", post(move |h: HeaderMap| { let f = m(h); async move { f.await; sse("hello") } }))
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
        .route("/backend-api/codex/responses", post(move |h: HeaderMap| { let f = r(h); async move { f.await; sse("codex says hi") } }))
        .route("/backend-api/wham/usage", get(move |h: HeaderMap| { let f = u(h); async move { f.await; axum::Json(json!({"plan_type": "pro"})) } }))
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
    let r = client.post("https://chatgpt.com/backend-api/codex/responses").header("authorization", "Bearer theirs").header("content-type", "application/json").header("content-encoding", "zstd").body(body.clone()).send().await.unwrap();
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
    use std::io::{Read, Write};
    use rustls::pki_types::{CertificateDer, pem::PemObject};
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
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider())).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth();
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
