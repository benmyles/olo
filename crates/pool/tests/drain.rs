//! An upgrade of the pool, against a fake provider: what is under way
//! finishes, what comes meanwhile waits and is answered by the new
//! version on the socket handed over; and a client turned off mid-way has
//! its next request passed through, with nothing restarted. Never a real
//! account or service.

use olo_sync::Mutex;
use std::{sync::Arc, time::Duration};

use axum::{Router, body::Body, http::HeaderMap, response::Response, routing::post};
use olo_token_pool::{
    authority::Authority,
    gateway::Shared,
    intercept,
    model::{Account, Credential, Provider, now},
    store::Saved,
    upstream::Upstreams,
};
use serde_json::json;

/// The sign-ins the fake saw, in order.
type Seen = Arc<Mutex<Vec<String>>>;

/// A provider whose answers stream for `slow`.
fn fake(seen: Seen, slow: Duration) -> Router {
    Router::new().route(
        "/v1/messages",
        post(move |h: HeaderMap| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ").to_owned());
                let stream = async_stream(slow);
                Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(stream)).unwrap()
            }
        }),
    )
}

/// Three events, `slow` apart in all.
fn async_stream(slow: Duration) -> impl futures_util::Stream<Item = Result<String, std::io::Error>> {
    futures_util::stream::unfold(0, move |i| async move {
        if i == 3 {
            return None;
        }
        tokio::time::sleep(slow / 3).await;
        Some((Ok(format!("data: {{\"type\":\"content_block_delta\",\"delta\":{{\"text\":\"part {i}\"}}}}\n\n")), i + 1))
    })
}

fn shared_with_account(up: std::net::SocketAddr) -> Arc<Shared> {
    let mut saved = Saved::default();
    let mut a = Account::new("claude-a".into(), "claude-a".into(), Provider::Claude);
    a.created = 1;
    saved.accounts.push(a);
    saved.credentials.insert("claude-a".into(), Credential { access: "pooled".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {})
}

fn client(proxy: std::net::SocketAddr, authority: &Authority) -> reqwest::Client {
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::https(format!("http://{proxy}")).unwrap())
        .add_root_certificate(reqwest::Certificate::from_pem(authority.cert_pem.as_bytes()).unwrap())
        .build()
        .unwrap()
}

async fn ask(c: &reqwest::Client) -> Result<String, String> {
    let r = c
        .post("https://api.anthropic.com/v1/messages")
        .header("authorization", "Bearer theirs")
        .json(&json!({"model": "claude-opus-5-5", "stream": true, "messages": []}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    r.text().await.map_err(|e| e.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_finishes_what_is_under_way_and_hands_the_rest_over() {
    let seen: Seen = Default::default();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    let app = fake(seen.clone(), Duration::from_millis(1500));
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let old = shared_with_account(up);
    let authority = Arc::new(Authority::ephemeral());
    let s = old.clone();
    let proxy = intercept::serve(old.clone(), authority.clone(), 0, Arc::new(move |p| s.routes(p)), intercept::Who::Anyone).await.unwrap();

    // One under way when the upgrade comes.
    let first = tokio::spawn({
        let c = client(proxy, &authority);
        async move { ask(&c).await }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(old.drain.busy(), "the request isn't under way");
    old.drain.begin();
    // One that comes meanwhile: taken by nobody yet.
    let second = tokio::spawn({
        let c = client(proxy, &authority);
        async move { ask(&c).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!second.is_finished(), "answered before the new version took over");
    // The first runs to its end, whole.
    let text = first.await.unwrap().expect("the request under way was cut");
    assert!(text.contains("part 0") && text.contains("part 2"), "{text}");
    for _ in 0..50 {
        if !old.drain.busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!old.drain.busy());
    assert!(!second.is_finished());

    // The new version takes the socket over (here, in this process) and
    // answers what waited.
    old.drain.hand_over();
    // (Across the exec the old process's copy is gone; here, still in
    // this one, the new version gets one of its own.)
    let named = std::env::var(olo_token_pool::drain::LISTENERS_ENV).expect("nothing handed over");
    let (port, fd) = named.split(',').find_map(|p| p.split_once('=')).unwrap();
    // SAFETY: dup of a descriptor the old version still owns.
    let copy = unsafe { libc::dup(fd.parse().unwrap()) };
    // SAFETY: this test alone uses the variable.
    unsafe { std::env::set_var(olo_token_pool::drain::LISTENERS_ENV, format!("{port}={copy}")) };
    let new = shared_with_account(up);
    let s = new.clone();
    let again = intercept::serve(new.clone(), authority.clone(), proxy.port(), Arc::new(move |p| s.routes(p)), intercept::Who::Anyone).await.unwrap();
    assert_eq!(again, proxy);
    let text = tokio::time::timeout(Duration::from_secs(10), second).await.expect("what waited wasn't answered").unwrap().expect("what waited failed");
    assert!(text.contains("part 2"), "{text}");
    assert_eq!(seen.lock().unwrap().as_slice(), ["pooled", "pooled"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_turned_off_passes_through_from_its_next_request() {
    let seen: Seen = Default::default();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    let app = fake(seen.clone(), Duration::from_millis(30));
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let shared = shared_with_account(up);
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.routes(p)), intercept::Who::Anyone).await.unwrap();
    // One client, one connection kept open between its requests.
    let c = client(proxy, &authority);
    ask(&c).await.unwrap();
    shared.set_paused(Provider::Claude, true);
    ask(&c).await.unwrap();
    shared.set_paused(Provider::Claude, false);
    ask(&c).await.unwrap();
    assert_eq!(seen.lock().unwrap().as_slice(), ["pooled", "theirs", "pooled"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_that_never_comes_takes_connections_again() {
    let seen: Seen = Default::default();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    let app = fake(seen.clone(), Duration::from_millis(30));
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let shared = shared_with_account(up);
    let authority = Arc::new(Authority::ephemeral());
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority.clone(), 0, Arc::new(move |p| s.routes(p)), intercept::Who::Anyone).await.unwrap();
    shared.drain.begin();
    let waiting = tokio::spawn({
        let c = client(proxy, &authority);
        async move { ask(&c).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!waiting.is_finished());
    // The new version wouldn't start: the old takes them again.
    shared.drain.end();
    let text = tokio::time::timeout(Duration::from_secs(10), waiting).await.expect("not taken again").unwrap().unwrap();
    assert!(text.contains("part 2"));
}
