//! An upgrade of the pool, against a fake provider: handed over to a new
//! copy, what is under way finishes in the old one while the new answers
//! what comes at once; the old way, what comes meanwhile waits and is
//! answered by the new version on the socket handed over; two copies at
//! once renew a sign-in only once; and a client turned off mid-way has
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

/// Held by the tests that hand sockets or a predecessor over through the
/// environment (one process's, shared by the tests running at once).
static ENV: Mutex<()> = Mutex::new(());

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
    let _env = ENV.lock().unwrap();
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

/// Handed over to a new copy (`ui::hand_over`): the request under way
/// finishes whole in the old copy, while one that comes meanwhile is
/// answered by the new copy at once, long before the old has finished.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_copy_answers_at_once_while_the_old_finishes() {
    let _env = ENV.lock().unwrap();
    let seen: Seen = Default::default();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    let app = fake(seen.clone(), Duration::from_millis(3000));
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let old = shared_with_account(up);
    let authority = Arc::new(Authority::ephemeral());
    let s = old.clone();
    let proxy = intercept::serve(old.clone(), authority.clone(), 0, Arc::new(move |p| s.routes(p)), intercept::Who::Anyone).await.unwrap();
    let first = tokio::spawn({
        let c = client(proxy, &authority);
        async move { ask(&c).await }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(old.drain.busy(), "the request isn't under way");

    // The listening socket for a copy (in this test, the same process: a
    // descriptor of its own, as the copy inherits one), and the old retired.
    let named = old.drain.for_a_copy().expect("nothing to hand over");
    old.drain.sealed();
    let (port, fd) = named.split(',').find_map(|p| p.split_once('=')).unwrap();
    // SAFETY: dup of a descriptor the old copy owns.
    let copy = unsafe { libc::dup(fd.parse().unwrap()) };
    // SAFETY: this test alone uses the variable.
    unsafe { std::env::set_var(olo_token_pool::drain::LISTENERS_ENV, format!("{port}={copy}")) };
    old.drain.retire();
    let new = shared_with_account(up);
    let s = new.clone();
    let again = intercept::serve(new.clone(), authority.clone(), proxy.port(), Arc::new(move |p| s.routes(p)), intercept::Who::Anyone).await.unwrap();
    assert_eq!(again, proxy);

    // A new client now: the new copy takes it at once (the provider has
    // it), while the old still streams its own.
    let second = tokio::spawn({
        let c = client(proxy, &authority);
        async move { ask(&c).await }
    });
    let t = std::time::Instant::now();
    while seen.lock().unwrap().len() < 2 && t.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(seen.lock().unwrap().len(), 2, "the new copy didn't take it at once");
    assert!(!first.is_finished(), "the old copy's request ended before the new one began");
    assert_eq!((new.drain.under_way().0, old.drain.under_way().0), (1, 1), "each copy on its own request");
    let text = tokio::time::timeout(Duration::from_secs(10), second).await.expect("the new copy's request hung").unwrap().expect("the new copy didn't answer");
    assert!(text.contains("part 2"), "{text}");
    // The old copy's runs to its end, whole.
    let text = tokio::time::timeout(Duration::from_secs(10), first).await.expect("the old copy's request hung").unwrap().expect("the old copy's request was cut");
    assert!(text.contains("part 0") && text.contains("part 2"), "{text}");
    for _ in 0..50 {
        if !old.drain.busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!old.drain.busy(), "the old copy still thinks it's busy");
}

/// Two copies at once (the old finishing, the new answering) and a sign-in
/// due for both: renewed once, under the store's lock, the second reading
/// the first's renewal; the provider rotates refresh tokens and refuses
/// one used twice, as Claude's does, so renewing twice would sign it out.
#[tokio::test(flavor = "multi_thread")]
async fn two_copies_renew_a_sign_in_once() {
    let _env = ENV.lock().unwrap();
    use olo_token_pool::store::Store;
    let renewals: Arc<Mutex<Vec<String>>> = Default::default();
    let used: Arc<Mutex<std::collections::HashSet<String>>> = Default::default();
    let app = Router::new().route(
        "/oauth",
        post({
            let (renewals, used) = (renewals.clone(), used.clone());
            move |body: axum::Json<serde_json::Value>| {
                let (renewals, used) = (renewals.clone(), used.clone());
                async move {
                    let token = body["refresh_token"].as_str().unwrap_or("").to_owned();
                    renewals.lock().unwrap().push(token.clone());
                    // (Slow enough for the other copy to be waiting.)
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    if !used.lock().unwrap().insert(token.clone()) {
                        return Response::builder().status(400).body(Body::from(r#"{"error":"invalid_grant"}"#)).unwrap();
                    }
                    let n = renewals.lock().unwrap().len();
                    Response::builder().body(Body::from(json!({"access_token": format!("fresh-{n}"), "refresh_token": format!("r{}", n + 1), "expires_in": 3600}).to_string())).unwrap()
                }
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let dir = std::path::PathBuf::from(format!("/var/tmp/olo-pool-two-copies-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: this test alone uses the variable.
    unsafe { std::env::set_var("OLO_POOL_KEYFILE", dir.join("key")) };
    let path = dir.join("pool.vault");
    let (store, mut saved) = Store::open_at(path.clone()).unwrap();
    let mut a = Account::new("claude-a".into(), "claude-a".into(), Provider::Claude);
    a.created = 1;
    saved.accounts.push(a);
    saved.credentials.insert("claude-a".into(), Credential { access: "old".into(), refresh: Some("r1".into()), expires_at: Some(now() + 30), account_id: None });
    store.save(&saved).unwrap();
    let up = Upstreams::fake(&format!("http://{up}"));

    let old = Shared::new(saved.clone(), Some(Store::open_at(path.clone()).unwrap().0), up.clone(), || {});
    old.drain.retire();
    // The new copy, told the old (here, this process) may still be at work.
    // SAFETY: this test alone uses the variable.
    unsafe { std::env::set_var(olo_token_pool::drain::PREDECESSOR_ENV, std::process::id().to_string()) };
    let new = Shared::new(saved, Some(Store::open_at(path.clone()).unwrap().0), up, || {});
    assert!(new.drain.overlapping() && old.drain.overlapping());

    // (Each its own task, as two processes' requests are.)
    let (o, n) = (old.clone(), new.clone());
    let a = tokio::spawn(async move { o.renewed("claude-a", Provider::Claude).await });
    let b = tokio::spawn(async move { n.renewed("claude-a", Provider::Claude).await });
    let both = tokio::time::timeout(Duration::from_secs(20), async { (a.await.unwrap(), b.await.unwrap()) }).await.expect("the renewals hung");
    let (a, b) = (both.0.expect("the old copy's renewal failed"), both.1.expect("the new copy's renewal failed"));
    assert_eq!(renewals.lock().unwrap().as_slice(), ["r1"], "renewed more than once");
    assert_eq!((a.access.as_str(), b.access.as_str()), ("fresh-1", "fresh-1"));
    // On disk, the renewal (with its rotated refresh token), whichever wrote it.
    let disk = store.read().unwrap();
    let c = &disk.credentials["claude-a"];
    assert_eq!((c.access.as_str(), c.refresh.as_deref()), ("fresh-1", Some("r2")));
    // The new copy's next save keeps it.
    new.touch();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(store.read().unwrap().credentials["claude-a"].access, "fresh-1");
    let _ = std::fs::remove_dir_all(&dir);
}
